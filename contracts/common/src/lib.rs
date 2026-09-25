#![no_std]
//! Shared building blocks for the Mvua Protocol contracts.
//!
//! This crate is a plain library (no contract of its own). It holds the
//! conventions that every contract must follow so they stay consistent:
//! the shared error code range, common value types, and the storage
//! time to live (TTL) helpers. See `docs/ARCHITECTURE.md` sections 5 and 6.

use soroban_sdk::{Env, IntoVal, Val};

/// Shared error codes, reserved in the 900 to 999 range for cross cutting
/// concerns (`docs/ARCHITECTURE.md` section 6). Each contract defines its own
/// `#[contracterror]` enum and includes these variants with these exact
/// numeric values so a code reads the same from any contract.
pub mod error_codes {
    /// Caller lacks the required authorization.
    pub const UNAUTHORIZED: u32 = 900;
    /// A timelocked change has been proposed but its delay has not elapsed.
    pub const TIMELOCK_PENDING: u32 = 901;
    /// The contract has not been initialized.
    pub const NOT_INITIALIZED: u32 = 902;
    /// A checked arithmetic operation overflowed.
    pub const OVERFLOW: u32 = 903;
}

/// Basis points, one hundredth of a percent. 10000 bps == 100 percent.
/// Used for premium curves, fee caps, and severity fractions.
pub type Bps = u32;

/// Denominator for basis point math: `value * bps / BPS_DENOMINATOR`.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Approximate number of ledgers in a day at a five second close time.
pub const DAY_IN_LEDGERS: u32 = 17_280;

/// How far to extend instance storage when it is touched (about 30 days).
pub const INSTANCE_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
/// Extend instance storage once its remaining life drops below this (about 29 days).
pub const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - DAY_IN_LEDGERS;

/// How far to extend a persistent entry when it is touched (about 90 days).
pub const PERSISTENT_BUMP_AMOUNT: u32 = 90 * DAY_IN_LEDGERS;
/// Extend a persistent entry once its remaining life drops below this (about 89 days).
pub const PERSISTENT_LIFETIME_THRESHOLD: u32 = PERSISTENT_BUMP_AMOUNT - DAY_IN_LEDGERS;

/// Extend the contract instance storage TTL. Call this in every mutating
/// entry point so a live contract's instance never expires.
pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}

/// Extend the TTL of a single persistent entry. Call this after reading or
/// writing a persistent key that must outlive the default archival window.
pub fn extend_persistent_ttl<K>(env: &Env, key: &K)
where
    K: IntoVal<Env, Val>,
{
    env.storage().persistent().extend_ttl(
        key,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );
}

/// Multiply an amount by a basis point fraction with checked arithmetic.
/// Returns `None` on overflow so the caller can map it to its own
/// `Overflow` error (903) rather than panicking (NFR-SEC-1).
pub fn apply_bps(amount: i128, bps: Bps) -> Option<i128> {
    amount
        .checked_mul(i128::from(bps))
        .and_then(|scaled| scaled.checked_div(BPS_DENOMINATOR))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn bps_denominator_is_one_hundred_percent() {
        assert_eq!(apply_bps(1_000, 10_000), Some(1_000));
    }

    #[test]
    fn bps_half_of_amount() {
        assert_eq!(apply_bps(1_000, 5_000), Some(500));
    }

    #[test]
    fn bps_overflow_is_none() {
        assert_eq!(apply_bps(i128::MAX, 10_000), None);
    }

    #[test]
    fn shared_error_codes_are_stable() {
        assert_eq!(error_codes::UNAUTHORIZED, 900);
        assert_eq!(error_codes::OVERFLOW, 903);
    }
}
