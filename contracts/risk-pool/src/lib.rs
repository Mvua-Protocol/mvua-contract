#![no_std]
//! risk-pool: custody of premiums and tranche capital.
//!
//! Owns pool configuration, tranche balances and receipts, season state, the
//! treasury, and the solvency record. It is the single enforcement point for
//! the solvency invariant `reserves >= committed` (FR-POOL-5). This module is
//! the Sprint 1.1 scaffold: storage layout and error model only; deposits,
//! premium accounting, and season settlement land in Sprint 1.2 onward.
//!
//! Requirements: FR-POOL-1 to 7, FR-ACC-1, FR-ACC-3.

use common::extend_instance_ttl;
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

/// Capital tranche seniority. Junior absorbs losses first and earns more; the
/// senior tranche is paid down first on withdrawal.
#[contracttype]
#[derive(Clone)]
pub enum Tier {
    Junior,
    Senior,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.1). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// Per pool configuration: token, regions, season length, curve, tranches.
    PoolConfig(u64),
    /// Deposited amount and receipt supply for a pool tranche.
    Tranche(u64, Tier),
    /// A holder's receipt balance in a pool tranche.
    Receipt(u64, Tier, Address),
    /// Season state and running premium and payout totals.
    Season(u64, u64),
    /// Accrued protocol and publisher fees for a pool.
    Treasury(u64),
    /// Solvency record: reserves and committed payouts for a pool.
    Solvency(u64),
}

/// Errors for risk-pool. Codes 100 to 199 are owned by this contract; codes
/// 900 to 999 are the shared range (`common::error_codes`). Codes are never
/// reused (`docs/ARCHITECTURE.md` section 6).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    PoolNotFound = 100,
    InsufficientReserves = 101,
    SolvencyViolated = 102,
    SeasonNotOpen = 103,
    TrancheWithdrawBlocked = 104,
    FeeCapExceeded = 105,
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
}

#[contract]
pub struct RiskPool;

#[contractimpl]
impl RiskPool {
    /// Initialize the pool contract with its guardian admin. Runs once at
    /// deployment (Soroban constructor). Admin resolves to the guardian
    /// multisig (FR-GOV-1).
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
}

mod test;
