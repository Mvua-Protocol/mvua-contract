//! Shared test fixtures and helpers for the Mvua Protocol contracts.
//!
//! Dev dependency only: this crate uses the soroban-sdk `testutils` feature
//! and is never linked into a deployed contract. It is the Sprint 1.1 base
//! (P1.1.3); the richer fixtures (observation builders, season builder, golden
//! vector loader) are decomposed as public issues and land as they are needed.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};

/// A fresh test environment with all authorizations mocked, the common default
/// for unit tests that are not specifically exercising auth.
pub fn new_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

/// Generate a fresh random address in the given environment.
pub fn random_address(env: &Env) -> Address {
    Address::generate(env)
}

/// Advance the ledger timestamp by `delta_secs` seconds. Saturates rather than
/// wrapping so tests never silently roll the clock backward.
pub fn advance_time(env: &Env, delta_secs: u64) {
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp.saturating_add(delta_secs);
    });
}

/// Advance the ledger sequence number by `delta` ledgers.
pub fn advance_sequence(env: &Env, delta: u32) {
    env.ledger().with_mut(|li| {
        li.sequence_number = li.sequence_number.saturating_add(delta);
    });
}

/// Assert a contract error code equals the expected value. All contract
/// `#[contracterror]` enums lower to a `u32`, so tests compare codes directly.
pub fn assert_error_code(actual: u32, expected: u32) {
    assert_eq!(actual, expected, "unexpected contract error code");
}

/// Compute the expected result of applying a basis point fraction, using the
/// shared math in `common`. Panics on overflow, which is acceptable in tests.
pub fn expected_bps(amount: i128, bps: u32) -> i128 {
    common::apply_bps(amount, bps).expect("bps math overflow in test fixture")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn time_and_sequence_advance() {
        let env = new_env();
        let t0 = env.ledger().timestamp();
        let s0 = env.ledger().sequence();
        advance_time(&env, 3_600);
        advance_sequence(&env, 10);
        assert_eq!(env.ledger().timestamp(), t0 + 3_600);
        assert_eq!(env.ledger().sequence(), s0 + 10);
    }

    #[test]
    fn addresses_are_distinct() {
        let env = new_env();
        assert_ne!(random_address(&env), random_address(&env));
    }

    #[test]
    fn expected_bps_matches_common() {
        assert_eq!(expected_bps(1_000, 5_000), 500);
    }
}
