#![no_std]
//! oracle-adapter: the trust boundary for external data.
//!
//! Registers publishers behind a timelock, ingests ed25519 signed weather
//! observations into a per region and metric ring, aggregates a median over the
//! valid, unchallenged, fresh observations, and enforces two fail safe gates: a
//! staleness bound and a minimum number of distinct publishers. The
//! trigger-engine consumes only the already trusted median from `median`; it
//! never sees a raw observation.
//!
//! Trust model: a submission is authenticated by an ed25519 signature over the
//! XDR of an `ObservationPayload`, not by `require_auth`, so an off-chain
//! publisher never needs a Stellar account or to co-sign the ingesting
//! transaction. A forged signature traps in `env.crypto().ed25519_verify`,
//! which surfaces to a `try_` caller as a host error, so `BadSignature` (301)
//! is reserved for the malformed input path.
//!
//! Sprint 1.6 scope (P1.6.1 to P1.6.4). The publisher fee ledger (P1.6.5,
//! FR-ORC-6) is deferred; `DataKey::PublisherFee` stays reserved but unused.
//!
//! Requirements: FR-ORC-1 to 5.

use common::{extend_instance_ttl, extend_persistent_ttl};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, xdr::ToXdr, Address, Bytes,
    BytesN, Env, Symbol, Vec,
};

/// Observations retained per region and metric. The ring overwrites its oldest
/// slot once full; `median` reads at most this many observations. Provisional
/// (INDEX-SPEC), tunable by governance in a later sprint.
const RING_SIZE: u32 = 16;

/// Minimum distinct publishers among the valid observations for `median` to
/// return a value. Below this the index is stale (fail safe): one publisher can
/// never set the index alone. Provisional M (INDEX-SPEC), governance tunable.
const MIN_PUBLISHERS: u32 = 2;

/// Default staleness bound in hours, stored at construction and readable via
/// `staleness_bound`. An observation older than this is ignored by `median`.
/// Provisional X (INDEX-SPEC), governance tunable.
const DEFAULT_STALENESS_HOURS: u64 = 48;

/// Registry changes (add or remove a publisher) wait behind this timelock
/// before they can be executed (FR-ORC-1, trust set changes). Provisional 7
/// days, governance tunable.
const REGISTRY_TIMELOCK_SECS: u64 = 7 * 24 * 60 * 60;

/// Seconds per hour, for converting the staleness bound to a duration.
const SECONDS_PER_HOUR: u64 = 3_600;

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.3). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// A registered publisher keyed by its ed25519 public key.
    Publisher(BytesN<32>),
    /// A pending, timelocked registry change for a publisher key.
    PendingPublisher(BytesN<32>),
    /// A single observation in the ring, keyed by region, metric, ring slot.
    Obs(Symbol, Symbol, u32),
    /// Ring cursor: next sequence and current count for a region and metric.
    ObsHead(Symbol, Symbol),
    /// A guardian challenge flag on a ring slot; the value is the reason code.
    Challenge(Symbol, Symbol, u32),
    /// Accrued fees owed to a publisher. Reserved for P1.6.5, unused in 1.6.
    PublisherFee(BytesN<32>),
    /// Staleness threshold in hours. Instance durability (fail safe, FR-ORC-4).
    StalenessBound,
}

/// A registered publisher. `active` is retained (never deleted) after a removal
/// so the observation history keeps a resolvable author.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublisherInfo {
    pub active: bool,
    pub added_at: u64,
}

/// A pending registry change awaiting its timelock. `add` distinguishes an
/// addition from a removal; `eta` is the earliest ledger time it can execute.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRegistry {
    pub add: bool,
    pub eta: u64,
}

/// A stored observation. `accepted` is always true once written (a rejected
/// submission never reaches storage); the field is kept for forward
/// compatibility with a soft reject path.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub value: i128,
    pub timestamp: u64,
    pub publisher: BytesN<32>,
    pub accepted: bool,
}

/// Ring cursor for a region and metric. `next_seq` increments on every write;
/// the slot is `next_seq % RING_SIZE`. `count` caps at `RING_SIZE`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Head {
    pub next_seq: u32,
    pub count: u32,
}

/// The signed payload. A publisher signs the XDR of this struct; `submit`
/// reconstructs the identical bytes and verifies the signature over them. Field
/// order and types are consensus critical: changing them breaks every signer.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationPayload {
    pub region: Symbol,
    pub metric: Symbol,
    pub timestamp: u64,
    pub value: i128,
}

/// Errors for oracle-adapter. Codes 300 to 399 are owned by this contract;
/// codes 900 to 999 are the shared range (`common::error_codes`).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    UnknownPublisher = 300,
    BadSignature = 301,
    ObservationStale = 302,
    Challenged = 303,
    RegistryTimelock = 304,
    PublisherExists = 305,
    NoPendingChange = 306,
    PendingExists = 307,
    InvalidObservation = 308,
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
}

/// Emitted when a registry change is proposed. Topic `publisher_proposed`.
#[contractevent(topics = ["publisher_proposed"], data_format = "map")]
pub struct PublisherProposed {
    pub publisher: BytesN<32>,
    pub add: bool,
    pub eta: u64,
}

/// Emitted when a publisher addition executes. Topic `publisher_registered`.
#[contractevent(topics = ["publisher_registered"], data_format = "single-value")]
pub struct PublisherRegistered {
    pub publisher: BytesN<32>,
}

/// Emitted when a publisher removal executes. Topic `publisher_removed`.
#[contractevent(topics = ["publisher_removed"], data_format = "single-value")]
pub struct PublisherRemoved {
    pub publisher: BytesN<32>,
}

/// Emitted when a pending registry change is cancelled. Topic
/// `publisher_cancelled`.
#[contractevent(topics = ["publisher_cancelled"], data_format = "single-value")]
pub struct PublisherProposalCancelled {
    pub publisher: BytesN<32>,
}

/// Emitted when an observation is accepted. Topic `observation`; data locates
/// the ring slot and names the publisher.
#[contractevent(topics = ["observation"], data_format = "map")]
pub struct ObservationSubmitted {
    pub region: Symbol,
    pub metric: Symbol,
    pub seq: u32,
    pub publisher: BytesN<32>,
}

/// Emitted when a guardian challenges a slot. Topic `challenge`.
#[contractevent(topics = ["challenge"], data_format = "map")]
pub struct ObservationChallenged {
    pub region: Symbol,
    pub metric: Symbol,
    pub seq: u32,
    pub reason: u32,
}

/// Emitted when a guardian clears a challenge. Topic `unchallenge`.
#[contractevent(topics = ["unchallenge"], data_format = "map")]
pub struct ObservationUnchallenged {
    pub region: Symbol,
    pub metric: Symbol,
    pub seq: u32,
}

#[contract]
pub struct OracleAdapter;

#[contractimpl]
impl OracleAdapter {
    /// Initialize the oracle contract with its guardian admin and the default
    /// staleness bound. Runs once at deployment (FR-GOV-1). Registry changes
    /// are timelocked (FR-ORC-1).
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::StalenessBound, &DEFAULT_STALENESS_HOURS);
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

    /// Propose a timelocked registry change for `publisher` (FR-ORC-1). Guardian
    /// only. `add == true` proposes registration; `add == false` proposes
    /// removal. Rejects a no-op (`PublisherExists` 305 to add an active key,
    /// `UnknownPublisher` 300 to remove an unknown one) and a second concurrent
    /// proposal for the same key (`PendingExists` 307). Returns the eta.
    pub fn propose_publisher(
        env: Env,
        publisher: BytesN<32>,
        add: bool,
    ) -> Result<u64, Error> {
        require_admin(&env)?;
        if read_pending(&env, &publisher).is_some() {
            return Err(Error::PendingExists);
        }
        let existing = read_publisher(&env, &publisher);
        let active = existing.map(|p| p.active).unwrap_or(false);
        if add && active {
            return Err(Error::PublisherExists);
        }
        if !add && !active {
            return Err(Error::UnknownPublisher);
        }
        let eta = env
            .ledger()
            .timestamp()
            .checked_add(REGISTRY_TIMELOCK_SECS)
            .ok_or(Error::Overflow)?;
        write_persistent(
            &env,
            &DataKey::PendingPublisher(publisher.clone()),
            &PendingRegistry { add, eta },
        );
        extend_instance_ttl(&env);
        PublisherProposed {
            publisher,
            add,
            eta,
        }
        .publish(&env);
        Ok(eta)
    }

    /// Execute a pending registry change once its timelock has elapsed
    /// (FR-ORC-1). Guardian only. `NoPendingChange` (306) if none is pending;
    /// `RegistryTimelock` (304) if the eta has not been reached. A removal sets
    /// `active = false` rather than deleting, so historical observations keep a
    /// resolvable author.
    pub fn execute_publisher(env: Env, publisher: BytesN<32>) -> Result<(), Error> {
        require_admin(&env)?;
        let pending = read_pending(&env, &publisher).ok_or(Error::NoPendingChange)?;
        if env.ledger().timestamp() < pending.eta {
            return Err(Error::RegistryTimelock);
        }
        if pending.add {
            write_persistent(
                &env,
                &DataKey::Publisher(publisher.clone()),
                &PublisherInfo {
                    active: true,
                    added_at: env.ledger().timestamp(),
                },
            );
        } else {
            let mut info = read_publisher(&env, &publisher).ok_or(Error::UnknownPublisher)?;
            info.active = false;
            write_persistent(&env, &DataKey::Publisher(publisher.clone()), &info);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPublisher(publisher.clone()));
        extend_instance_ttl(&env);
        if pending.add {
            PublisherRegistered { publisher }.publish(&env);
        } else {
            PublisherRemoved { publisher }.publish(&env);
        }
        Ok(())
    }

    /// Cancel a pending registry change before it executes (FR-ORC-1). Guardian
    /// only. `NoPendingChange` (306) if nothing is pending for the key.
    pub fn cancel_publisher(env: Env, publisher: BytesN<32>) -> Result<(), Error> {
        require_admin(&env)?;
        if read_pending(&env, &publisher).is_none() {
            return Err(Error::NoPendingChange);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPublisher(publisher.clone()));
        extend_instance_ttl(&env);
        PublisherProposalCancelled { publisher }.publish(&env);
        Ok(())
    }

    /// Ingest a signed observation (FR-ORC-2). No `require_auth`: the ed25519
    /// signature over the XDR of `ObservationPayload { region, metric,
    /// timestamp, value }` authenticates the submission, so a publisher needs
    /// no Stellar account. `UnknownPublisher` (300) if the key is not active;
    /// `InvalidObservation` (308) for a negative value or a future timestamp. A
    /// forged signature traps in `ed25519_verify` (surfaces to a `try_` caller
    /// as a host error). On success the observation is written into the ring
    /// slot `next_seq % RING_SIZE`, any stale challenge on the reused slot is
    /// cleared, and the ring cursor advances. Returns the written slot.
    pub fn submit(
        env: Env,
        region: Symbol,
        metric: Symbol,
        timestamp: u64,
        value: i128,
        publisher: BytesN<32>,
        signature: BytesN<64>,
    ) -> Result<u32, Error> {
        let info = read_publisher(&env, &publisher).ok_or(Error::UnknownPublisher)?;
        if !info.active {
            return Err(Error::UnknownPublisher);
        }
        let now = env.ledger().timestamp();
        if value < 0 || timestamp > now {
            return Err(Error::InvalidObservation);
        }
        let payload = ObservationPayload {
            region: region.clone(),
            metric: metric.clone(),
            timestamp,
            value,
        };
        let message: Bytes = payload.to_xdr(&env);
        env.crypto()
            .ed25519_verify(&publisher, &message, &signature);

        let mut head = read_head(&env, &region, &metric);
        let slot = head.next_seq % RING_SIZE;
        write_persistent(
            &env,
            &DataKey::Obs(region.clone(), metric.clone(), slot),
            &Observation {
                value,
                timestamp,
                publisher: publisher.clone(),
                accepted: true,
            },
        );
        env.storage()
            .persistent()
            .remove(&DataKey::Challenge(region.clone(), metric.clone(), slot));
        head.next_seq = head.next_seq.checked_add(1).ok_or(Error::Overflow)?;
        if head.count < RING_SIZE {
            head.count += 1;
        }
        write_persistent(&env, &DataKey::ObsHead(region.clone(), metric.clone()), &head);
        extend_instance_ttl(&env);
        ObservationSubmitted {
            region,
            metric,
            seq: slot,
            publisher,
        }
        .publish(&env);
        Ok(slot)
    }

    /// Flag a ring slot so `median` excludes it (FR-ORC-5). Guardian only.
    /// `InvalidObservation` (308) if the slot holds no observation; `Challenged`
    /// (303) if it is already flagged.
    pub fn challenge(
        env: Env,
        region: Symbol,
        metric: Symbol,
        seq: u32,
        reason: u32,
    ) -> Result<(), Error> {
        require_admin(&env)?;
        if read_obs(&env, &region, &metric, seq).is_none() {
            return Err(Error::InvalidObservation);
        }
        if is_challenged(&env, &region, &metric, seq) {
            return Err(Error::Challenged);
        }
        write_persistent(
            &env,
            &DataKey::Challenge(region.clone(), metric.clone(), seq),
            &reason,
        );
        extend_instance_ttl(&env);
        ObservationChallenged {
            region,
            metric,
            seq,
            reason,
        }
        .publish(&env);
        Ok(())
    }

    /// Clear a challenge flag (FR-ORC-5). Guardian only. `NoPendingChange` is
    /// not reused here: clearing an unflagged slot is a no-op that still emits,
    /// so a guardian retry is idempotent.
    pub fn unchallenge(
        env: Env,
        region: Symbol,
        metric: Symbol,
        seq: u32,
    ) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage()
            .persistent()
            .remove(&DataKey::Challenge(region.clone(), metric.clone(), seq));
        extend_instance_ttl(&env);
        ObservationUnchallenged {
            region,
            metric,
            seq,
        }
        .publish(&env);
        Ok(())
    }

    /// The trusted median over the valid observations for a region and metric
    /// (FR-ORC-3, FR-ORC-4). An observation counts only if it is accepted, not
    /// challenged, and no older than the staleness bound. Returns
    /// `ObservationStale` (302) when no observation qualifies or fewer than
    /// `MIN_PUBLISHERS` distinct publishers are represented, so a stale or
    /// under-sourced index blocks a trigger rather than paying on thin data.
    pub fn median(env: Env, region: Symbol, metric: Symbol) -> Result<i128, Error> {
        let head = read_head(&env, &region, &metric);
        if head.count == 0 {
            return Err(Error::ObservationStale);
        }
        let now = env.ledger().timestamp();
        let bound_secs = read_staleness_hours(&env)
            .checked_mul(SECONDS_PER_HOUR)
            .ok_or(Error::Overflow)?;

        let mut values = [0i128; RING_SIZE as usize];
        let mut len: usize = 0;
        let mut publishers: Vec<BytesN<32>> = Vec::new(&env);
        let mut slot: u32 = 0;
        while slot < head.count {
            if let Some(obs) = read_obs(&env, &region, &metric, slot) {
                let fresh = now.saturating_sub(obs.timestamp) <= bound_secs;
                if obs.accepted && fresh && !is_challenged(&env, &region, &metric, slot) {
                    values[len] = obs.value;
                    len += 1;
                    if !contains_publisher(&publishers, &obs.publisher) {
                        publishers.push_back(obs.publisher);
                    }
                }
            }
            slot += 1;
        }
        if len == 0 || publishers.len() < MIN_PUBLISHERS {
            return Err(Error::ObservationStale);
        }
        sort_ascending(&mut values, len);
        let mid = len / 2;
        if len % 2 == 1 {
            Ok(values[mid])
        } else {
            let sum = values[mid - 1].checked_add(values[mid]).ok_or(Error::Overflow)?;
            Ok(sum / 2)
        }
    }

    /// True if the key is a currently active publisher.
    pub fn is_publisher(env: Env, publisher: BytesN<32>) -> bool {
        read_publisher(&env, &publisher)
            .map(|p| p.active)
            .unwrap_or(false)
    }

    /// Read a publisher record (active or retired), or `UnknownPublisher` (300).
    pub fn publisher_info(env: Env, publisher: BytesN<32>) -> Result<PublisherInfo, Error> {
        read_publisher(&env, &publisher).ok_or(Error::UnknownPublisher)
    }

    /// Read a pending registry change, or `NoPendingChange` (306).
    pub fn pending_publisher(
        env: Env,
        publisher: BytesN<32>,
    ) -> Result<PendingRegistry, Error> {
        read_pending(&env, &publisher).ok_or(Error::NoPendingChange)
    }

    /// Read an observation at a ring slot, or `InvalidObservation` (308).
    pub fn observation(
        env: Env,
        region: Symbol,
        metric: Symbol,
        seq: u32,
    ) -> Result<Observation, Error> {
        read_obs(&env, &region, &metric, seq).ok_or(Error::InvalidObservation)
    }

    /// Read the ring cursor for a region and metric (zeroed if none yet).
    pub fn head(env: Env, region: Symbol, metric: Symbol) -> Head {
        read_head(&env, &region, &metric)
    }

    /// True if the ring slot carries a challenge flag.
    pub fn is_challenged(env: Env, region: Symbol, metric: Symbol, seq: u32) -> bool {
        is_challenged(&env, &region, &metric, seq)
    }

    /// The configured staleness bound in hours.
    pub fn staleness_bound(env: Env) -> u64 {
        read_staleness_hours(&env)
    }

}

/// Persist a value under `key` and extend its TTL in one step.
fn write_persistent<V>(env: &Env, key: &DataKey, value: &V)
where
    V: soroban_sdk::IntoVal<Env, soroban_sdk::Val>,
{
    env.storage().persistent().set(key, value);
    extend_persistent_ttl(env, key);
}

fn read_publisher(env: &Env, publisher: &BytesN<32>) -> Option<PublisherInfo> {
    env.storage()
        .persistent()
        .get(&DataKey::Publisher(publisher.clone()))
}

fn read_pending(env: &Env, publisher: &BytesN<32>) -> Option<PendingRegistry> {
    env.storage()
        .persistent()
        .get(&DataKey::PendingPublisher(publisher.clone()))
}

fn read_obs(env: &Env, region: &Symbol, metric: &Symbol, seq: u32) -> Option<Observation> {
    env.storage()
        .persistent()
        .get(&DataKey::Obs(region.clone(), metric.clone(), seq))
}

fn read_head(env: &Env, region: &Symbol, metric: &Symbol) -> Head {
    env.storage()
        .persistent()
        .get(&DataKey::ObsHead(region.clone(), metric.clone()))
        .unwrap_or(Head {
            next_seq: 0,
            count: 0,
        })
}

fn is_challenged(env: &Env, region: &Symbol, metric: &Symbol, seq: u32) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::Challenge(region.clone(), metric.clone(), seq))
}

fn read_staleness_hours(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::StalenessBound)
        .unwrap_or(DEFAULT_STALENESS_HOURS)
}

/// Read the guardian admin and require its authorization, or fail with
/// `NotInitialized` (902) if the contract was never constructed.
fn require_admin(env: &Env) -> Result<(), Error> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

/// Membership test over the distinct-publisher accumulator. A linear scan is
/// bounded by `RING_SIZE`, so it stays cheap.
fn contains_publisher(seen: &Vec<BytesN<32>>, publisher: &BytesN<32>) -> bool {
    for existing in seen.iter() {
        if existing == *publisher {
            return true;
        }
    }
    false
}

/// In-place insertion sort of `values[0..len]` ascending. `len` never exceeds
/// `RING_SIZE`, so the quadratic cost is bounded by a small constant.
fn sort_ascending(values: &mut [i128; RING_SIZE as usize], len: usize) {
    let mut i = 1;
    while i < len {
        let key = values[i];
        let mut j = i;
        while j > 0 && values[j - 1] > key {
            values[j] = values[j - 1];
            j -= 1;
        }
        values[j] = key;
        i += 1;
    }
}

mod test;



