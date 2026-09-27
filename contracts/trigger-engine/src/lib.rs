#![no_std]
//! trigger-engine: deterministic index evaluation and the trigger lifecycle.
//!
//! Holds immutable index definitions, reduces a coverage window's daily median
//! series into a bounded per-window accumulator, evaluates the rainfall
//! shortfall and consecutive dry day formulas as pure integer functions, and
//! drives each window through the trigger state machine to an irreversible
//! finalization with a severity in basis points.
//!
//! Daily values are ingested one of two ways. The guardian may feed a median
//! directly (`record_day`), the Sprint 1.7 path retained as a fallback. In the
//! Sprint 1.8 integrated flow (DR-0025) `record_day_from_oracle` reads the
//! trusted median from `oracle-adapter` on chain instead: the guardian asserts
//! only which window day it is, and the value comes from the corroborated
//! oracle. A stale or thin oracle traps the read, so no day is folded.
//! Evaluation is fail safe: an incomplete window is `StaleIndex`, never a payout.
//!
//! Requirements: FR-TRG-1 to 5.

use common::{apply_bps, extend_instance_ttl, extend_persistent_ttl, Bps, BPS_DENOMINATOR};
use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, Address,
    BytesN, Env, Symbol,
};

/// Challenge window between a trigger being observed and it becoming finalizable,
/// so a guardian can dispute a bad evaluation first. Provisional 24 hours
/// (DR-0025), governance tunable.
const CHALLENGE_WINDOW_SECS: u64 = 24 * 60 * 60;

/// The index formula a definition evaluates (INDEX-SPEC sections 4 and 5).
/// Extensible: consecutive heat days and NDVI drop are held for P1.7.5.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexKind {
    /// Cumulative rainfall over the window falls below a baseline ratio.
    RainfallShortfall,
    /// Longest run of consecutive dry days reaches a threshold.
    ConsecutiveDryDays,
}

/// Trigger lifecycle for one window instance. Moves Healthy to Triggered to
/// Finalized only; Finalized never changes (FR-TRG-4, invariant 3).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriggerStatus {
    /// No breach observed (or the window is not yet complete).
    Healthy,
    /// Breach observed at the given ledger timestamp; challenge window open.
    Triggered(u64),
    /// Finalized at the given timestamp with a severity in basis points.
    Finalized(u64, Bps),
}

/// Parameters for a new index definition, passed to `create_index` as one
/// struct so the entry point stays within Soroban's parameter limit. The
/// stored `IndexDefinition` adds the assigned `index_id`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexParams {
    pub kind: IndexKind,
    pub region: Symbol,
    pub metric: Symbol,
    /// Coverage window length in days; the accumulator fills over this many days.
    pub window_len: u32,
    /// Historical seasonal cumulative baseline (RainfallShortfall).
    pub baseline: i128,
    /// Shortfall trigger threshold in bps (RainfallShortfall).
    pub trigger_ratio_bps: Bps,
    /// Full payout threshold in bps, below trigger (RainfallShortfall).
    pub exhaustion_ratio_bps: Bps,
    /// Run length that fires the index (ConsecutiveDryDays).
    pub trigger_days: u32,
    /// Run length at full payout, above trigger (ConsecutiveDryDays).
    pub exhaustion_days: u32,
    /// Daily rainfall at or below which a day is dry (ConsecutiveDryDays).
    pub dry_floor: i128,
    /// Opaque pointer to the off chain methodology entry (no PII on chain).
    pub data_source_ref: BytesN<32>,
}

/// A stored, immutable index definition (INDEX-SPEC section 7). Once created it
/// never changes; a new definition gets a new `index_id` so policies keep the
/// terms they were sold under.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexDefinition {
    pub index_id: u64,
    pub params: IndexParams,
}

/// The daily median series reduced to exactly what each index needs, so the
/// contract never stores an unbounded per day series. Updated once per day by
/// `record_day` and read at evaluation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowAccumulator {
    /// Days recorded so far; evaluation requires this to reach `window_len`.
    pub days_recorded: u32,
    /// Running sum of daily medians (RainfallShortfall `actual`).
    pub rain_sum: i128,
    /// Current run of consecutive dry days.
    pub current_run: u32,
    /// Longest dry run seen in the window (ConsecutiveDryDays `max_run`).
    pub max_run: u32,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.4). Adding or
/// changing a key updates that table in the same pull request. A window
/// instance is keyed by `(index_id, window_id)`, so one definition serves many
/// seasons and each season finalizes independently.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// Monotonic counter for the next index id. Instance durability.
    IndexCount,
    /// The `oracle-adapter` contract address for the integrated median feed
    /// (DR-0025). Instance durability. Set by the guardian via `set_oracle`.
    OracleRef,
    /// An index definition keyed by its id (first class object, FR-TRG-1).
    IndexDef(u64),
    /// Window accumulator for an `(index_id, window_id)` instance.
    Accumulator(u64, u64),
    /// Trigger state for an `(index_id, window_id)` instance.
    TriggerState(u64, u64),
    /// Last recorded day for an `(index_id, window_id)` instance (replay safety).
    EvalCursor(u64, u64),
}

/// Errors for trigger-engine. Codes 400 to 499 are owned by this contract;
/// codes 900 to 999 are the shared range (`common::error_codes`).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    IndexNotFound = 400,
    StaleIndex = 401,
    AlreadyFinalized = 402,
    NotTriggered = 403,
    NonDeterministicInput = 404,
    InvalidIndexDef = 405,
    OracleNotConfigured = 406,
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
}

/// Emitted when an index definition is created. Topic `index_created`.
#[contractevent(topics = ["index_created"], data_format = "single-value")]
pub struct IndexCreated {
    pub index_id: u64,
}

/// Emitted when a day is recorded into a window accumulator. Topic `day`.
#[contractevent(topics = ["day"], data_format = "map")]
pub struct DayRecorded {
    pub index_id: u64,
    pub window_id: u64,
    pub day: u64,
}

/// Emitted when a window transitions to Triggered. Topic `triggered`.
#[contractevent(topics = ["triggered"], data_format = "map")]
pub struct Triggered {
    pub index_id: u64,
    pub window_id: u64,
    pub at: u64,
}

/// Emitted when a window is finalized. Topic `finalized`; data carries the
/// severity in basis points.
#[contractevent(topics = ["finalized"], data_format = "map")]
pub struct Finalized {
    pub index_id: u64,
    pub window_id: u64,
    pub at: u64,
    pub severity: Bps,
}

/// Emitted when the oracle-adapter address is configured. Topic `oracle_set`.
#[contractevent(topics = ["oracle_set"], data_format = "single-value")]
pub struct OracleSet {
    pub oracle: Address,
}

/// Cross contract view of `oracle-adapter`'s trusted median (DR-0025 integrated
/// flow). The method returns the median as a primitive; the real function
/// returns `Result<i128, Error>`, so a stale or under-sourced index (its
/// `ObservationStale`) traps this call and reverts the day recording rather
/// than folding an untrusted value. The fail safe holds across the boundary.
/// Declared locally (the DR-0022 primitive-only pattern) so trigger-engine
/// depends on `oracle-adapter` only as a dev-dependency.
#[contractclient(name = "OracleAdapterClient")]
pub trait OracleAdapterInterface {
    fn median(env: Env, region: Symbol, metric: Symbol) -> i128;
}

#[contract]
pub struct TriggerEngine;

#[contractimpl]
impl TriggerEngine {
    /// Initialize the trigger engine with its guardian admin. Runs once at
    /// deployment (FR-GOV-1). Index definition changes are timelocked.
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        extend_instance_ttl(&env);
    }

    /// Read the configured admin (guardian multisig) address. Returns
    /// `NotInitialized` (902) if the contract was never constructed.
    pub fn admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)
    }

    /// Create an immutable index definition (FR-TRG-1). Guardian only. Validates
    /// the parameters per kind and assigns a monotonic `index_id`, returned to
    /// the caller. There is no update path: a change is a new definition.
    pub fn create_index(env: Env, params: IndexParams) -> Result<u64, Error> {
        require_admin(&env)?;
        validate_params(&params)?;
        let index_id = env
            .storage()
            .instance()
            .get(&DataKey::IndexCount)
            .unwrap_or(0u64);
        let next = index_id.checked_add(1).ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::IndexCount, &next);
        let def = IndexDefinition { index_id, params };
        write_persistent(&env, &DataKey::IndexDef(index_id), &def);
        extend_instance_ttl(&env);
        IndexCreated { index_id }.publish(&env);
        Ok(index_id)
    }

    /// Record one day's median value into a window accumulator. Guardian only.
    /// This is the fallback path (and the local-testing path): the guardian
    /// asserts the value directly. The trustless path is `record_day_from_oracle`
    /// below, which reads the value from `oracle-adapter` instead (DR-0025). Both
    /// share `fold_day`. Days must arrive in strictly increasing order and stop at
    /// `window_len`; a negative value, a replay, or an overrun is
    /// `NonDeterministicInput` (404).
    pub fn record_day(
        env: Env,
        index_id: u64,
        window_id: u64,
        day: u64,
        median_value: i128,
    ) -> Result<(), Error> {
        require_admin(&env)?;
        let def = read_def(&env, index_id)?;
        fold_day(&env, &def, index_id, window_id, day, median_value)
    }

    /// Record one day by reading the value from the configured `oracle-adapter`
    /// (DR-0025 integrated flow). Guardian only: the guardian still asserts the
    /// day-to-calendar mapping (which `window_id`/`day` a reading belongs to), but
    /// the value itself is now the trusted on chain median rather than a guardian
    /// assertion. Requires `set_oracle` first (`OracleNotConfigured` 406). A stale
    /// or under-sourced oracle traps the cross call and reverts the whole record,
    /// so a window can never be advanced on untrusted data. Returns the folded
    /// median value.
    pub fn record_day_from_oracle(
        env: Env,
        index_id: u64,
        window_id: u64,
        day: u64,
    ) -> Result<i128, Error> {
        require_admin(&env)?;
        let def = read_def(&env, index_id)?;
        let oracle: Address = env
            .storage()
            .instance()
            .get(&DataKey::OracleRef)
            .ok_or(Error::OracleNotConfigured)?;
        // Traps if the oracle reports `ObservationStale`; the day is not folded.
        let value =
            OracleAdapterClient::new(&env, &oracle).median(&def.params.region, &def.params.metric);
        fold_day(&env, &def, index_id, window_id, day, value)?;
        Ok(value)
    }

    /// Configure the `oracle-adapter` address for the integrated median feed
    /// (DR-0025). Guardian only. Mirrors `risk-pool::set_payout_vault`: a setter
    /// rather than a constructor argument, so the existing constructor and its
    /// tests are unchanged. Re-callable to rotate the oracle.
    pub fn set_oracle(env: Env, oracle: Address) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::OracleRef, &oracle);
        extend_instance_ttl(&env);
        OracleSet { oracle }.publish(&env);
        Ok(())
    }

    /// Read the configured `oracle-adapter` address, or `OracleNotConfigured`
    /// (406) if `set_oracle` was never called.
    pub fn oracle(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::OracleRef)
            .ok_or(Error::OracleNotConfigured)
    }

    /// Evaluate a window against its index (FR-TRG-2). Permissionless and
    /// deterministic. An incomplete window (`days_recorded < window_len`) is
    /// `StaleIndex` (401), so a payout can never fire on partial data. On a
    /// breach a Healthy window moves to Triggered; an already Triggered or
    /// Finalized window is returned unchanged (idempotent).
    pub fn evaluate(env: Env, index_id: u64, window_id: u64) -> Result<TriggerStatus, Error> {
        let def = read_def(&env, index_id)?;
        let acc = read_accumulator(&env, index_id, window_id);
        if acc.days_recorded < def.params.window_len {
            return Err(Error::StaleIndex);
        }
        let state = read_state(&env, index_id, window_id);
        match state {
            TriggerStatus::Healthy => {
                let (triggered, _severity) = eval_index(&def.params, &acc)?;
                if triggered {
                    let now = env.ledger().timestamp();
                    let new_state = TriggerStatus::Triggered(now);
                    write_persistent(
                        &env,
                        &DataKey::TriggerState(index_id, window_id),
                        &new_state,
                    );
                    extend_instance_ttl(&env);
                    Triggered {
                        index_id,
                        window_id,
                        at: now,
                    }
                    .publish(&env);
                    Ok(new_state)
                } else {
                    Ok(TriggerStatus::Healthy)
                }
            }
            other => Ok(other),
        }
    }

    /// Finalize a triggered window (FR-TRG-4). Guardian only. Permitted only once
    /// the challenge window has elapsed since the trigger (`TimelockPending` 901
    /// before then). Computes the severity from the accumulator and writes the
    /// irreversible `Finalized` state; a second call is `AlreadyFinalized` (402)
    /// and finalizing an untriggered window is `NotTriggered` (403).
    pub fn finalize(env: Env, index_id: u64, window_id: u64) -> Result<TriggerStatus, Error> {
        require_admin(&env)?;
        let state = read_state(&env, index_id, window_id);
        let triggered_at = match state {
            TriggerStatus::Finalized(_, _) => return Err(Error::AlreadyFinalized),
            TriggerStatus::Healthy => return Err(Error::NotTriggered),
            TriggerStatus::Triggered(at) => at,
        };
        let now = env.ledger().timestamp();
        let ready = triggered_at
            .checked_add(CHALLENGE_WINDOW_SECS)
            .ok_or(Error::Overflow)?;
        if now < ready {
            return Err(Error::TimelockPending);
        }
        let def = read_def(&env, index_id)?;
        let acc = read_accumulator(&env, index_id, window_id);
        let severity = severity_of(&def.params, &acc)?;
        let new_state = TriggerStatus::Finalized(now, severity);
        write_persistent(
            &env,
            &DataKey::TriggerState(index_id, window_id),
            &new_state,
        );
        extend_instance_ttl(&env);
        Finalized {
            index_id,
            window_id,
            at: now,
            severity,
        }
        .publish(&env);
        Ok(new_state)
    }

    /// Payout for a coverage amount against a finalized window (FR-TRG-5):
    /// `min(coverage, coverage * severity / BPS)`, rounding down to favor the
    /// pool. `NotTriggered` (403) until the window is finalized.
    pub fn payout_for(
        env: Env,
        index_id: u64,
        window_id: u64,
        coverage: i128,
    ) -> Result<i128, Error> {
        match read_state(&env, index_id, window_id) {
            TriggerStatus::Finalized(_, severity) => {
                let scaled = apply_bps(coverage, severity).ok_or(Error::Overflow)?;
                Ok(scaled.min(coverage))
            }
            _ => Err(Error::NotTriggered),
        }
    }

    /// Read an index definition, or `IndexNotFound` (400).
    pub fn index_def(env: Env, index_id: u64) -> Result<IndexDefinition, Error> {
        read_def(&env, index_id)
    }

    /// Read the trigger state for a window instance (defaults to Healthy).
    pub fn trigger_state(env: Env, index_id: u64, window_id: u64) -> TriggerStatus {
        read_state(&env, index_id, window_id)
    }

    /// Read the window accumulator (defaults to an empty accumulator).
    pub fn accumulator(env: Env, index_id: u64, window_id: u64) -> WindowAccumulator {
        read_accumulator(&env, index_id, window_id)
    }

    /// Read the number of index definitions created so far.
    pub fn index_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::IndexCount)
            .unwrap_or(0u64)
    }
}

// Persist a value under `key` and refresh its TTL, mirroring the storage helper
// used across the other contracts (DR-0018).
fn write_persistent<V>(env: &Env, key: &DataKey, value: &V)
where
    V: soroban_sdk::IntoVal<Env, soroban_sdk::Val>,
{
    env.storage().persistent().set(key, value);
    extend_persistent_ttl(env, key);
}

// Require the configured guardian admin to authorize the call.
fn require_admin(env: &Env) -> Result<(), Error> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

// Read an index definition, or `IndexNotFound` (400).
fn read_def(env: &Env, index_id: u64) -> Result<IndexDefinition, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::IndexDef(index_id))
        .ok_or(Error::IndexNotFound)
}

// Fold one day's value into a window accumulator. Shared by `record_day` (the
// guardian-asserted value) and `record_day_from_oracle` (the oracle-read value)
// so both ingestion paths apply the identical determinism guards and update. A
// negative value, a replay or out-of-order day, or an overrun past `window_len`
// is `NonDeterministicInput` (404).
fn fold_day(
    env: &Env,
    def: &IndexDefinition,
    index_id: u64,
    window_id: u64,
    day: u64,
    value: i128,
) -> Result<(), Error> {
    if value < 0 {
        return Err(Error::NonDeterministicInput);
    }
    if let Some(last) = read_cursor(env, index_id, window_id) {
        if day <= last {
            return Err(Error::NonDeterministicInput);
        }
    }
    let mut acc = read_accumulator(env, index_id, window_id);
    if acc.days_recorded >= def.params.window_len {
        return Err(Error::NonDeterministicInput);
    }
    acc.rain_sum = acc.rain_sum.checked_add(value).ok_or(Error::Overflow)?;
    if value <= def.params.dry_floor {
        acc.current_run += 1;
        if acc.current_run > acc.max_run {
            acc.max_run = acc.current_run;
        }
    } else {
        acc.current_run = 0;
    }
    acc.days_recorded += 1;
    write_persistent(env, &DataKey::Accumulator(index_id, window_id), &acc);
    write_persistent(env, &DataKey::EvalCursor(index_id, window_id), &day);
    extend_instance_ttl(env);
    DayRecorded {
        index_id,
        window_id,
        day,
    }
    .publish(env);
    Ok(())
}

// Read the last recorded day for a window instance, if any.
fn read_cursor(env: &Env, index_id: u64, window_id: u64) -> Option<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::EvalCursor(index_id, window_id))
}

// Read a window accumulator, defaulting to an empty one.
fn read_accumulator(env: &Env, index_id: u64, window_id: u64) -> WindowAccumulator {
    env.storage()
        .persistent()
        .get(&DataKey::Accumulator(index_id, window_id))
        .unwrap_or(WindowAccumulator {
            days_recorded: 0,
            rain_sum: 0,
            current_run: 0,
            max_run: 0,
        })
}

// Read a trigger state, defaulting to Healthy.
fn read_state(env: &Env, index_id: u64, window_id: u64) -> TriggerStatus {
    env.storage()
        .persistent()
        .get(&DataKey::TriggerState(index_id, window_id))
        .unwrap_or(TriggerStatus::Healthy)
}

// Validate index parameters per kind (INDEX-SPEC section 7). Rejects a
// definition that could never evaluate deterministically or that inverts the
// trigger/exhaustion ordering the severity ramp depends on.
fn validate_params(params: &IndexParams) -> Result<(), Error> {
    if params.window_len == 0 {
        return Err(Error::InvalidIndexDef);
    }
    match params.kind {
        IndexKind::RainfallShortfall => {
            // Baseline must be positive (it is the ratio denominator), and the
            // exhaustion threshold sits strictly below the trigger threshold so
            // the severity ramp has a positive span.
            if params.baseline <= 0 {
                return Err(Error::InvalidIndexDef);
            }
            if params.trigger_ratio_bps > BPS_DENOMINATOR as Bps {
                return Err(Error::InvalidIndexDef);
            }
            if params.exhaustion_ratio_bps >= params.trigger_ratio_bps {
                return Err(Error::InvalidIndexDef);
            }
        }
        IndexKind::ConsecutiveDryDays => {
            // Trigger run must be reachable within the window, and exhaustion
            // must exceed trigger so the severity ramp has a positive span.
            if params.trigger_days == 0 || params.trigger_days > params.window_len {
                return Err(Error::InvalidIndexDef);
            }
            if params.exhaustion_days <= params.trigger_days {
                return Err(Error::InvalidIndexDef);
            }
        }
    }
    Ok(())
}

// Evaluate the index for a complete window, returning `(triggered, severity)`.
// Pure and deterministic: no ledger, storage, or env access, so the golden
// vectors (INDEX-SPEC section 8) pin it exactly. Callers must ensure the window
// is complete before trusting the result.
fn eval_index(params: &IndexParams, acc: &WindowAccumulator) -> Result<(bool, Bps), Error> {
    match params.kind {
        IndexKind::RainfallShortfall => eval_rainfall(params, acc),
        IndexKind::ConsecutiveDryDays => eval_dry_days(params, acc),
    }
}

// Severity-only view of `eval_index`, used at finalization.
fn severity_of(params: &IndexParams, acc: &WindowAccumulator) -> Result<Bps, Error> {
    let (_triggered, severity) = eval_index(params, acc)?;
    Ok(severity)
}

// Rainfall shortfall (INDEX-SPEC section 4). `actual_ratio_bps = actual * BPS /
// baseline` (rounds toward zero); triggered when strictly below the trigger
// ratio; severity ramps linearly from 0 at the trigger ratio to BPS at the
// exhaustion ratio, rounding down.
fn eval_rainfall(params: &IndexParams, acc: &WindowAccumulator) -> Result<(bool, Bps), Error> {
    let actual = acc.rain_sum;
    let ratio = actual
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(Error::Overflow)?
        .checked_div(params.baseline)
        .ok_or(Error::Overflow)?;
    let trigger = params.trigger_ratio_bps as i128;
    let exhaustion = params.exhaustion_ratio_bps as i128;
    if ratio >= trigger {
        return Ok((false, 0));
    }
    if ratio <= exhaustion {
        return Ok((true, BPS_DENOMINATOR as Bps));
    }
    // trigger > ratio > exhaustion, and trigger > exhaustion by validation, so
    // the numerator and denominator are both positive.
    let severity = (trigger - ratio)
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(Error::Overflow)?
        .checked_div(trigger - exhaustion)
        .ok_or(Error::Overflow)?;
    Ok((true, severity as Bps))
}

// Consecutive dry days (INDEX-SPEC section 5). The accumulator already carries
// the longest dry run seen in the window (`max_run`); triggered when it reaches
// the trigger run; severity ramps from 0 at the trigger run to BPS at the
// exhaustion run, rounding down.
fn eval_dry_days(params: &IndexParams, acc: &WindowAccumulator) -> Result<(bool, Bps), Error> {
    let max_run = acc.max_run;
    if max_run < params.trigger_days {
        return Ok((false, 0));
    }
    if max_run >= params.exhaustion_days {
        return Ok((true, BPS_DENOMINATOR as Bps));
    }
    // trigger_days <= max_run < exhaustion_days, and exhaustion_days >
    // trigger_days by validation, so the span is positive.
    let numerator = (max_run - params.trigger_days) as i128;
    let denominator = (params.exhaustion_days - params.trigger_days) as i128;
    let severity = numerator
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(Error::Overflow)?
        .checked_div(denominator)
        .ok_or(Error::Overflow)?;
    Ok((true, severity as Bps))
}

#[cfg(test)]
mod test;
