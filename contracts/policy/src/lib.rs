#![no_std]
//! policy: the user facing policy certificate and its lifecycle.
//!
//! Mints policy certificates, holds per policy state, and mediates between
//! purchase (against risk-pool) and payout (via payout-vault). It holds no
//! value itself. This module is the Sprint 1.1 scaffold: storage layout and
//! error model only; quoting, minting, batch purchase, and cancellation land
//! in Sprint 1.2 onward.
//!
//! Requirements: FR-POL-1 to 6.

use common::extend_instance_ttl;
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

/// Lifecycle state of a policy certificate (`docs/ARCHITECTURE.md` section 5.2).
#[contracttype]
#[derive(Clone)]
pub enum PolicyState {
    Active,
    Triggered,
    Paid,
    Expired,
    Cancelled,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.2). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// Monotonic source of policy ids. Instance durability.
    PolicyCounter,
    /// A policy record keyed by its id.
    Policy(u64),
    /// Opaque off chain reference for a policy (no PII on chain, NFR-PRIV-1).
    MetaPointer(u64),
    /// Whether policies of a pool may be transferred (default false, FR-POL-6).
    Transferable(u64),
}

/// Errors for policy. Codes 200 to 299 are owned by this contract; codes 900
/// to 999 are the shared range (`common::error_codes`).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    PolicyNotFound = 200,
    InvalidState = 201,
    WindowStarted = 202,
    NotTransferable = 203,
    QuoteMismatch = 204,
    Unauthorized = 900,
    NotInitialized = 902,
    Overflow = 903,
}

#[contract]
pub struct Policy;

#[contractimpl]
impl Policy {
    /// Initialize the policy contract with its guardian admin. Runs once at
    /// deployment (FR-GOV-1).
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
