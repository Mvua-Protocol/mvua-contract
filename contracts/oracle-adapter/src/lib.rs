#![no_std]
//! oracle-adapter: the trust boundary for external data.
//!
//! Registers publishers, ingests and verifies ed25519 signed observations,
//! aggregates a median series per region and metric, enforces staleness, and
//! runs the challenge window. The trigger-engine consumes the already trusted
//! median series from here. This module is the Sprint 1.1 scaffold: storage
//! layout and error model only; registration, submission, verification, and
//! aggregation land in Sprint 1.3 and 1.6.
//!
//! Requirements: FR-ORC-1 to 6.

use common::extend_instance_ttl;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol,
};

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.3). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// A registered publisher keyed by its ed25519 public key.
    Publisher(BytesN<32>),
    /// A single observation in the append only ring, keyed by region, metric, seq.
    Obs(Symbol, Symbol, u32),
    /// Ring head: latest seq and count for a region and metric.
    ObsHead(Symbol, Symbol),
    /// A guardian challenge flag on an observation.
    Challenge(Symbol, Symbol, u32),
    /// Accrued fees owed to a publisher.
    PublisherFee(BytesN<32>),
    /// Staleness threshold in hours. Instance durability (fail safe, FR-ORC-4).
    StalenessBound,
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
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
}

#[contract]
pub struct OracleAdapter;

#[contractimpl]
impl OracleAdapter {
    /// Initialize the oracle contract with its guardian admin. Runs once at
    /// deployment (FR-GOV-1). Registry changes are timelocked (FR-ORC-1).
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
