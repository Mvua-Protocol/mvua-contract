#![no_std]
//! trigger-engine: deterministic index evaluation and the trigger lifecycle.
//!
//! Holds index definitions, evaluates them as pure functions of the stored
//! observation series, and drives each region and window through the trigger
//! state machine to an irreversible finalization. This module is the Sprint
//! 1.1 scaffold: storage layout and error model only; index definitions and
//! evaluation land in Sprint 1.6 and 1.7.
//!
//! Requirements: FR-TRG-1 to 5.

use common::{extend_instance_ttl, Bps};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol};

/// Trigger lifecycle for a region and window. Moves Healthy to Triggered to
/// Finalized only; Finalized never changes (FR-TRG-4, invariant 3).
#[contracttype]
#[derive(Clone)]
pub enum TriggerStatus {
    /// No breach observed.
    Healthy,
    /// Breach observed at the given ledger timestamp; challenge window open.
    Triggered(u64),
    /// Finalized at the given timestamp with a severity in basis points.
    Finalized(u64, Bps),
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.4). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// An index definition keyed by its id (first class object, FR-TRG-1).
    IndexDef(u64),
    /// Trigger state for a region and window.
    TriggerState(Symbol, u64),
    /// Last evaluated observation seq for a region and window (replay safety).
    EvalCursor(Symbol, u64),
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
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
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
}

mod test;
