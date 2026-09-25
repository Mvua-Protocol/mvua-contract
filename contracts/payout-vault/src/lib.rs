#![no_std]
//! payout-vault: resumable payouts to claimable balances.
//!
//! Computes the payout list for a finalized trigger and creates one claimable
//! balance per policy holder in resumable batches, so a farmer with no XLM can
//! still receive funds. Holds the pause flag and the payout ledger. This
//! module is the Sprint 1.1 scaffold: storage layout and error model only;
//! payout computation and batch execution land in Sprint 1.8.
//!
//! Requirements: FR-PAY-1 to 5.

use common::extend_instance_ttl;
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

/// Progress of a payout batch for a region and window (resumable, FR-PAY-2).
#[contracttype]
#[derive(Clone)]
pub enum BatchState {
    /// Batch created, no payouts made yet.
    Open,
    /// Batch partially paid; `cursor` marks progress.
    InProgress,
    /// Every eligible policy has a payout entry.
    Complete,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.5). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// Emergency pause flag (FR-PAY-5). Instance durability.
    Paused,
    /// Resumable batch checkpoint for a region and window.
    Batch(Symbol, u64),
    /// A single policy's payout record (one per finalized trigger, FR-PAY-4).
    Payout(u64),
    /// The risk-pool address, source of committed funds. Instance durability.
    PoolRef,
}

/// Errors for payout-vault. Codes 500 to 599 are owned by this contract;
/// codes 900 to 999 are the shared range (`common::error_codes`).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    Paused = 500,
    BatchComplete = 501,
    NoFinalizedTrigger = 502,
    PayoutExists = 503,
    Unauthorized = 900,
    NotInitialized = 902,
    Overflow = 903,
}

#[contract]
pub struct PayoutVault;

#[contractimpl]
impl PayoutVault {
    /// Initialize the vault with its guardian admin and the risk-pool address.
    /// Runs once at deployment (FR-GOV-1). Starts unpaused.
    pub fn __constructor(env: Env, admin: Address, pool: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::PoolRef, &pool);
        env.storage().instance().set(&DataKey::Paused, &false);
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

    /// Whether payouts are currently paused (FR-PAY-5).
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }
}

mod test;
