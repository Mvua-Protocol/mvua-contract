#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN};
use test_utils::{advance_time, new_env};

const CHALLENGE: u64 = 24 * 60 * 60;

fn setup() -> (Env, Address, Address) {
    let env = new_env();
    let admin = Address::generate(&env);
    let id = env.register(TriggerEngine, (admin.clone(),));
    (env, admin, id)
}

fn source_ref(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

// A rainfall index matching the golden-vector baseline (INDEX-SPEC section 8):
// baseline 800, trigger 7500 bps, exhaustion 4000 bps. `window_len` days must be
// recorded before it evaluates.
fn rainfall_params(env: &Env, window_len: u32) -> IndexParams {
    IndexParams {
        kind: IndexKind::RainfallShortfall,
        region: symbol_short!("nairobi"),
        metric: symbol_short!("rain_mm"),
        window_len,
        baseline: 800,
        trigger_ratio_bps: 7500,
        exhaustion_ratio_bps: 4000,
        trigger_days: 0,
        exhaustion_days: 0,
        dry_floor: 0,
        data_source_ref: source_ref(env),
    }
}

// A consecutive-dry-days index: a day at or below `dry_floor` (10) is dry.
fn dry_params(env: &Env, window_len: u32, trigger_days: u32, exhaustion_days: u32) -> IndexParams {
    IndexParams {
        kind: IndexKind::ConsecutiveDryDays,
        region: symbol_short!("nairobi"),
        metric: symbol_short!("rain_mm"),
        window_len,
        baseline: 0,
        trigger_ratio_bps: 0,
        exhaustion_ratio_bps: 0,
        trigger_days,
        exhaustion_days,
        dry_floor: 10,
        data_source_ref: source_ref(env),
    }
}

// A window accumulator carrying a given rainfall sum, for the pure formula.
fn rain_acc(rain_sum: i128) -> WindowAccumulator {
    WindowAccumulator {
        days_recorded: 0,
        rain_sum,
        current_run: 0,
        max_run: 0,
    }
}

// A window accumulator carrying a given longest dry run, for the pure formula.
fn run_acc(max_run: u32) -> WindowAccumulator {
    WindowAccumulator {
        days_recorded: 0,
        rain_sum: 0,
        current_run: 0,
        max_run,
    }
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::IndexNotFound as u32, 400);
    assert_eq!(Error::StaleIndex as u32, 401);
    assert_eq!(Error::AlreadyFinalized as u32, 402);
    assert_eq!(Error::NotTriggered as u32, 403);
    assert_eq!(Error::NonDeterministicInput as u32, 404);
    assert_eq!(Error::InvalidIndexDef as u32, 405);
    assert_eq!(
        Error::Unauthorized as u32,
        common::error_codes::UNAUTHORIZED
    );
    assert_eq!(
        Error::TimelockPending as u32,
        common::error_codes::TIMELOCK_PENDING
    );
    assert_eq!(
        Error::NotInitialized as u32,
        common::error_codes::NOT_INITIALIZED
    );
    assert_eq!(Error::Overflow as u32, common::error_codes::OVERFLOW);
}

#[test]
fn constructor_sets_admin() {
    let (env, admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
}

// Golden vectors G1 to G4 (INDEX-SPEC section 8), pinned against the pure
// rainfall formula so the ratios and severities are exact.
#[test]
fn golden_rainfall_vectors() {
    let env = Env::default();
    let p = rainfall_params(&env, 1);
    // G1: actual 800 -> ratio 10000, at baseline, not triggered.
    assert_eq!(eval_rainfall(&p, &rain_acc(800)).unwrap(), (false, 0));
    // G2: actual 600 -> ratio 7500, exactly at the trigger, NOT triggered (<).
    assert_eq!(eval_rainfall(&p, &rain_acc(600)).unwrap(), (false, 0));
    // G3: actual 520 -> ratio 6500, triggered, severity 2857 (rounds down).
    assert_eq!(eval_rainfall(&p, &rain_acc(520)).unwrap(), (true, 2857));
    // G4: actual 320 -> ratio 4000, at exhaustion, full severity.
    assert_eq!(eval_rainfall(&p, &rain_acc(320)).unwrap(), (true, 10000));
}

// Golden vectors G5 to G8 (INDEX-SPEC section 8), pinned against the pure
// consecutive-dry-days formula.
#[test]
fn golden_dry_day_vectors() {
    let env = Env::default();
    let p = dry_params(&env, 40, 21, 35);
    // G5: max_run 20 -> below trigger, not triggered.
    assert_eq!(eval_dry_days(&p, &run_acc(20)).unwrap(), (false, 0));
    // G6: max_run 21 -> at trigger, triggered with zero severity.
    assert_eq!(eval_dry_days(&p, &run_acc(21)).unwrap(), (true, 0));
    // G7: max_run 28 -> midway, severity 5000.
    assert_eq!(eval_dry_days(&p, &run_acc(28)).unwrap(), (true, 5000));
    // G8: max_run 35 -> at exhaustion, full severity.
    assert_eq!(eval_dry_days(&p, &run_acc(35)).unwrap(), (true, 10000));
}

#[test]
fn create_index_assigns_monotonic_ids() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    assert_eq!(client.index_count(), 0);
    let a = client.create_index(&rainfall_params(&env, 3));
    let b = client.create_index(&dry_params(&env, 5, 2, 4));
    assert_eq!(a, 0);
    assert_eq!(b, 1);
    assert_eq!(client.index_count(), 2);
    assert_eq!(
        client.index_def(&a).params.kind,
        IndexKind::RainfallShortfall
    );
}

#[test]
fn index_def_unknown_is_rejected() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    assert_eq!(client.try_index_def(&0), Err(Ok(Error::IndexNotFound)));
}

#[test]
fn create_index_rejects_invalid_params() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);

    // Zero window never completes.
    let mut p = rainfall_params(&env, 0);
    assert_eq!(client.try_create_index(&p), Err(Ok(Error::InvalidIndexDef)));

    // Non-positive rainfall baseline (the ratio denominator).
    p = rainfall_params(&env, 3);
    p.baseline = 0;
    assert_eq!(client.try_create_index(&p), Err(Ok(Error::InvalidIndexDef)));

    // Exhaustion ratio not strictly below the trigger ratio.
    p = rainfall_params(&env, 3);
    p.exhaustion_ratio_bps = 7500;
    assert_eq!(client.try_create_index(&p), Err(Ok(Error::InvalidIndexDef)));

    // Dry-day trigger run unreachable within the window.
    let mut d = dry_params(&env, 5, 6, 8);
    assert_eq!(client.try_create_index(&d), Err(Ok(Error::InvalidIndexDef)));

    // Dry-day exhaustion not above trigger.
    d = dry_params(&env, 30, 21, 21);
    assert_eq!(client.try_create_index(&d), Err(Ok(Error::InvalidIndexDef)));
}

#[test]
fn record_day_rejects_bad_input() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 3));

    // Negative median is non-deterministic input.
    assert_eq!(
        client.try_record_day(&idx, &0, &1, &-1),
        Err(Ok(Error::NonDeterministicInput))
    );

    client.record_day(&idx, &0, &1, &100);
    // Replaying or going backwards is rejected.
    assert_eq!(
        client.try_record_day(&idx, &0, &1, &100),
        Err(Ok(Error::NonDeterministicInput))
    );
    assert_eq!(
        client.try_record_day(&idx, &0, &0, &100),
        Err(Ok(Error::NonDeterministicInput))
    );
}

#[test]
fn record_day_rejects_overrun_and_unknown_index() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    // Unknown index.
    assert_eq!(
        client.try_record_day(&0, &0, &1, &100),
        Err(Ok(Error::IndexNotFound))
    );

    let idx = client.create_index(&rainfall_params(&env, 2));
    client.record_day(&idx, &0, &1, &100);
    client.record_day(&idx, &0, &2, &100);
    // The window is full; a further day overruns it.
    assert_eq!(
        client.try_record_day(&idx, &0, &3, &100),
        Err(Ok(Error::NonDeterministicInput))
    );
    let acc = client.accumulator(&idx, &0);
    assert_eq!(acc.days_recorded, 2);
    assert_eq!(acc.rain_sum, 200);
}

// Fill a rainfall window with `days` equal daily values, so the sum lands on a
// chosen ratio. Days are recorded in strictly increasing order from 1.
fn fill_window(client: &TriggerEngineClient, idx: u64, window: u64, days: u32, each: i128) {
    for day in 1..=days as u64 {
        client.record_day(&idx, &window, &day, &each);
    }
}

#[test]
fn evaluate_incomplete_window_is_stale() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    client.record_day(&idx, &0, &1, &130);
    // Only one of four days recorded: a payout can never fire on partial data.
    assert_eq!(client.try_evaluate(&idx, &0), Err(Ok(Error::StaleIndex)));
}

#[test]
fn evaluate_healthy_when_not_breached() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    // Sum 800 -> ratio 10000, at baseline, no breach.
    fill_window(&client, idx, 0, 4, 200);
    assert_eq!(client.evaluate(&idx, &0), TriggerStatus::Healthy);
}

#[test]
fn evaluate_transitions_to_triggered_and_is_idempotent() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    // Sum 520 -> ratio 6500, triggered.
    fill_window(&client, idx, 0, 4, 130);
    let now = env.ledger().timestamp();
    assert_eq!(client.evaluate(&idx, &0), TriggerStatus::Triggered(now));
    // A second evaluation returns the same state without re-triggering.
    assert_eq!(client.evaluate(&idx, &0), TriggerStatus::Triggered(now));
}

#[test]
fn finalize_respects_challenge_window() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    fill_window(&client, idx, 0, 4, 130);
    client.evaluate(&idx, &0);

    // Finalizing inside the challenge window is rejected.
    assert_eq!(
        client.try_finalize(&idx, &0),
        Err(Ok(Error::TimelockPending))
    );

    advance_time(&env, CHALLENGE + 1);
    let at = env.ledger().timestamp();
    // Severity 2857 for a ratio of 6500 (G3).
    assert_eq!(
        client.finalize(&idx, &0),
        TriggerStatus::Finalized(at, 2857)
    );
}

#[test]
fn finalize_untriggered_is_rejected() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    fill_window(&client, idx, 0, 4, 200);
    client.evaluate(&idx, &0);
    assert_eq!(client.try_finalize(&idx, &0), Err(Ok(Error::NotTriggered)));
}

#[test]
fn finalize_is_once_only_and_irreversible() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    fill_window(&client, idx, 0, 4, 130);
    client.evaluate(&idx, &0);
    advance_time(&env, CHALLENGE + 1);
    let finalized = client.finalize(&idx, &0);

    // A second finalize is rejected.
    assert_eq!(
        client.try_finalize(&idx, &0),
        Err(Ok(Error::AlreadyFinalized))
    );
    // Re-evaluating a finalized window returns it unchanged.
    assert_eq!(client.evaluate(&idx, &0), finalized);
    assert_eq!(client.trigger_state(&idx, &0), finalized);
}

#[test]
fn payout_scales_by_severity_and_caps_at_coverage() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));

    // Before finalization, no payout is defined.
    assert_eq!(
        client.try_payout_for(&idx, &0, &1_000_000),
        Err(Ok(Error::NotTriggered))
    );

    fill_window(&client, idx, 0, 4, 130);
    client.evaluate(&idx, &0);
    advance_time(&env, CHALLENGE + 1);
    client.finalize(&idx, &0);

    // Severity 2857 bps of 1_000_000 = 285_700 (rounds down, favors the pool).
    assert_eq!(client.payout_for(&idx, &0, &1_000_000), 285_700);
}

#[test]
fn dry_day_flow_triggers_on_consecutive_dry_days() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    // Window 3, trigger 2, exhaustion 3, dry floor 10.
    let idx = client.create_index(&dry_params(&env, 3, 2, 3));
    // A wet day resets the run; then two dry days reach the trigger.
    client.record_day(&idx, &0, &1, &50);
    client.record_day(&idx, &0, &2, &5);
    client.record_day(&idx, &0, &3, &5);
    let acc = client.accumulator(&idx, &0);
    assert_eq!(acc.max_run, 2);

    let now = env.ledger().timestamp();
    // max_run 2 at trigger 2 -> triggered with zero severity.
    assert_eq!(client.evaluate(&idx, &0), TriggerStatus::Triggered(now));
    advance_time(&env, CHALLENGE + 1);
    let at = env.ledger().timestamp();
    assert_eq!(client.finalize(&idx, &0), TriggerStatus::Finalized(at, 0));
}

#[test]
fn windows_finalize_independently() {
    let (env, _admin, id) = setup();
    let client = TriggerEngineClient::new(&env, &id);
    let idx = client.create_index(&rainfall_params(&env, 4));
    // Window 0 breaches, window 1 stays healthy under the same definition.
    fill_window(&client, idx, 0, 4, 130);
    fill_window(&client, idx, 1, 4, 200);
    let breached = client.evaluate(&idx, &0);
    assert!(matches!(breached, TriggerStatus::Triggered(_)));
    assert_eq!(client.evaluate(&idx, &1), TriggerStatus::Healthy);
}
