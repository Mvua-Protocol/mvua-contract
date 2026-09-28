//! End to end integration tests for the Mvua Protocol contracts.
//!
//! This crate ships no contract of its own. Its only purpose is the integration
//! test target in `tests/`, which registers all five real contracts (risk-pool,
//! policy, oracle-adapter, trigger-engine, payout-vault) on a single Soroban
//! test host, wires them exactly as a testnet deployment would, and drives one
//! underwriting season end to end through the genuine cross contract calls:
//! deposit, premium intake, signed observations, the on chain oracle median
//! feed (DR-0025 then DR-0027), deterministic evaluation, finalization after the
//! challenge window, batch payout, and claim (P1.8.4).
//!
//! The library itself is intentionally empty.
