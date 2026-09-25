#![no_std]
//! policy: the user facing policy certificate and its lifecycle.
//!
//! Mints policy certificates, holds per policy state, and mediates between
//! purchase (against risk-pool) and payout (via payout-vault). It holds no
//! value itself: premiums flow directly into risk-pool and payouts flow out of
//! payout-vault. Sprint 1.4 adds quoting against the pool curve (FR-POL-2),
//! minting with premium collection (FR-POL-1), the lifecycle state machine
//! (FR-POL-1), owner cancellation with a premium refund before the coverage
//! window opens (FR-POL-3), and an opaque off chain metadata pointer that keeps
//! PII off chain (FR-POL-4, NFR-PRIV-1).
//!
//! Requirements: FR-POL-1 to 6.

use common::{extend_instance_ttl, extend_persistent_ttl};
use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, Address,
    BytesN, Env, Symbol,
};

/// Cross contract client for risk-pool (the subset policy calls). Declared as a
/// local interface so policy needs no build dependency on risk-pool; the real
/// contract is wired by address at construction. Signatures mirror risk-pool's
/// exported functions, and a sub-call error traps the enclosing policy call.
#[contractclient(name = "RiskPoolClient")]
pub trait RiskPoolInterface {
    fn premium_for(env: Env, pool_id: u64, coverage: i128) -> i128;
    fn collect_premium(env: Env, pool_id: u64, season_id: u64, from: Address, amount: i128)
        -> i128;
    fn refund_premium(env: Env, pool_id: u64, season_id: u64, to: Address, net: i128) -> i128;
}

/// Lifecycle state of a policy certificate (`docs/ARCHITECTURE.md` section 5.2).
/// Transitions run `Active -> Triggered -> Paid`, `Active -> Expired` after the
/// coverage window, and `Active -> Cancelled` before it opens. Any other move
/// is rejected with `InvalidState` (201).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyState {
    Active,
    Triggered,
    Paid,
    Expired,
    Cancelled,
}

/// A policy certificate record (`docs/ARCHITECTURE.md` section 5.2). Holds no
/// value: `premium_paid` is the gross the buyer paid and `net_reserved` is the
/// portion risk-pool credited to reserves (gross minus fees), which is exactly
/// what a cancellation refunds.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyRecord {
    pub owner: Address,
    pub pool_id: u64,
    pub season_id: u64,
    pub region: Symbol,
    pub coverage: i128,
    pub index_ref: u64,
    pub window_start: u64,
    pub window_end: u64,
    pub severity_curve: u32,
    pub premium_paid: i128,
    pub net_reserved: i128,
    pub state: PolicyState,
}

/// The terms a buyer specifies when minting a policy, grouped into one struct so
/// `mint` stays within Soroban's ten parameter limit on exported functions.
/// These map one to one onto the matching `PolicyRecord` fields.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyTerms {
    pub region: Symbol,
    pub coverage: i128,
    pub index_ref: u64,
    pub window_start: u64,
    pub window_end: u64,
    pub severity_curve: u32,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.2). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// The risk-pool contract policies purchase against. Instance durability.
    RiskPool,
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
/// to 999 are the shared range (`common::error_codes`). Codes are never reused
/// (`docs/ARCHITECTURE.md` section 6).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    PolicyNotFound = 200,
    InvalidState = 201,
    WindowStarted = 202,
    NotTransferable = 203,
    QuoteMismatch = 204,
    InvalidCoverage = 205,
    InvalidWindow = 206,
    NotExpired = 207,
    Unauthorized = 900,
    NotInitialized = 902,
    Overflow = 903,
}

/// Emitted when a policy is minted. Topic `policy_minted`; data is the detail.
#[contractevent(topics = ["policy_minted"], data_format = "map")]
pub struct PolicyMinted {
    pub policy_id: u64,
    pub owner: Address,
    pub pool_id: u64,
    pub season_id: u64,
    pub coverage: i128,
    pub premium_paid: i128,
}

/// Emitted when a policy changes state (triggered, paid, expired). Topic
/// `policy_state`; data is the policy id and the new state.
#[contractevent(topics = ["policy_state"], data_format = "map")]
pub struct PolicyStateChanged {
    pub policy_id: u64,
    pub state: PolicyState,
}

/// Emitted when a policy is cancelled and its net premium refunded. Topic
/// `policy_cancelled`; data is the policy id, owner, and refunded amount.
#[contractevent(topics = ["policy_cancelled"], data_format = "map")]
pub struct PolicyCancelled {
    pub policy_id: u64,
    pub owner: Address,
    pub refunded: i128,
}

#[contract]
pub struct Policy;

#[contractimpl]
impl Policy {
    /// Initialize the policy contract with its guardian admin and the risk-pool
    /// it purchases against. Runs once at deployment (FR-GOV-1).
    pub fn __constructor(env: Env, admin: Address, risk_pool: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::RiskPool, &risk_pool);
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

    /// Read the configured risk-pool contract address, or `NotInitialized`
    /// (902) if the contract was never constructed.
    pub fn risk_pool(env: Env) -> Result<Address, Error> {
        read_risk_pool(&env)
    }

    /// Quote the gross premium for a coverage amount under a pool's curve
    /// (FR-POL-2). Proxies to risk-pool `premium_for`; a missing pool or non
    /// positive coverage traps the call from the pool side.
    pub fn quote(env: Env, pool_id: u64, coverage: i128) -> Result<i128, Error> {
        let rp = read_risk_pool(&env)?;
        Ok(RiskPoolClient::new(&env, &rp).premium_for(&pool_id, &coverage))
    }

    /// Mint a policy certificate (FR-POL-1). The buyer authorizes the purchase;
    /// the premium is quoted against the pool curve, checked against
    /// `max_premium` (else `QuoteMismatch`, 204), and collected into risk-pool
    /// for the given season, which credits the net to reserves. The new policy
    /// starts `Active` and records the gross paid and the net reserved (what a
    /// cancellation refunds). `metadata` is an opaque off chain pointer with no
    /// PII on chain. Returns the new policy id.
    pub fn mint(
        env: Env,
        buyer: Address,
        pool_id: u64,
        season_id: u64,
        terms: PolicyTerms,
        max_premium: i128,
        metadata: BytesN<32>,
    ) -> Result<u64, Error> {
        buyer.require_auth();
        let PolicyTerms {
            region,
            coverage,
            index_ref,
            window_start,
            window_end,
            severity_curve,
        } = terms;
        if coverage <= 0 {
            return Err(Error::InvalidCoverage);
        }
        if window_end <= window_start {
            return Err(Error::InvalidWindow);
        }

        let rp = read_risk_pool(&env)?;
        let client = RiskPoolClient::new(&env, &rp);
        let premium = client.premium_for(&pool_id, &coverage);
        if premium > max_premium {
            return Err(Error::QuoteMismatch);
        }
        let net = client.collect_premium(&pool_id, &season_id, &buyer, &premium);

        let policy_id = env
            .storage()
            .instance()
            .get(&DataKey::PolicyCounter)
            .unwrap_or(0u64)
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        env.storage()
            .instance()
            .set(&DataKey::PolicyCounter, &policy_id);

        let record = PolicyRecord {
            owner: buyer.clone(),
            pool_id,
            season_id,
            region,
            coverage,
            index_ref,
            window_start,
            window_end,
            severity_curve,
            premium_paid: premium,
            net_reserved: net,
            state: PolicyState::Active,
        };
        write_persistent(&env, &DataKey::Policy(policy_id), &record);
        write_persistent(&env, &DataKey::MetaPointer(policy_id), &metadata);

        extend_instance_ttl(&env);
        PolicyMinted {
            policy_id,
            owner: buyer,
            pool_id,
            season_id,
            coverage,
            premium_paid: premium,
        }
        .publish(&env);
        Ok(policy_id)
    }

    /// Mark an `Active` policy as `Triggered` (FR-POL-1). Guardian only; the
    /// trigger-engine drives this once an index breach is confirmed. Rejects
    /// any other starting state with `InvalidState` (201).
    pub fn mark_triggered(env: Env, policy_id: u64) -> Result<(), Error> {
        require_admin(&env)?;
        transition(&env, policy_id, PolicyState::Active, PolicyState::Triggered)
    }

    /// Mark a `Triggered` policy as `Paid` (FR-POL-1). Guardian only; set once
    /// payout-vault has released the payout. Rejects any other starting state
    /// with `InvalidState` (201).
    pub fn mark_paid(env: Env, policy_id: u64) -> Result<(), Error> {
        require_admin(&env)?;
        transition(&env, policy_id, PolicyState::Triggered, PolicyState::Paid)
    }

    /// Expire an `Active` policy whose coverage window has ended (FR-POL-1).
    /// Permissionless housekeeping guarded by the ledger timestamp: rejects
    /// `NotExpired` (207) before `window_end` and `InvalidState` (201) if the
    /// policy is not `Active`.
    pub fn expire(env: Env, policy_id: u64) -> Result<(), Error> {
        let record = read_policy(&env, policy_id)?;
        if env.ledger().timestamp() < record.window_end {
            return Err(Error::NotExpired);
        }
        transition(&env, policy_id, PolicyState::Active, PolicyState::Expired)
    }

    /// Cancel an `Active` policy and refund its net premium (FR-POL-3). Owner
    /// only, and permitted only before the coverage window opens (else
    /// `WindowStarted`, 202). Calls risk-pool `refund_premium` for the reserved
    /// net, which returns the underlying to the owner, then moves the policy to
    /// `Cancelled`. Returns the refunded amount.
    pub fn cancel(env: Env, policy_id: u64) -> Result<i128, Error> {
        let mut record = read_policy(&env, policy_id)?;
        record.owner.require_auth();
        if record.state != PolicyState::Active {
            return Err(Error::InvalidState);
        }
        if env.ledger().timestamp() >= record.window_start {
            return Err(Error::WindowStarted);
        }

        let rp = read_risk_pool(&env)?;
        let refunded = RiskPoolClient::new(&env, &rp).refund_premium(
            &record.pool_id,
            &record.season_id,
            &record.owner,
            &record.net_reserved,
        );

        record.state = PolicyState::Cancelled;
        write_persistent(&env, &DataKey::Policy(policy_id), &record);

        extend_instance_ttl(&env);
        PolicyCancelled {
            policy_id,
            owner: record.owner,
            refunded,
        }
        .publish(&env);
        Ok(refunded)
    }

    /// Update a policy's opaque off chain metadata pointer (FR-POL-4). Owner
    /// only. The pointer is a content hash or reference and carries no PII on
    /// chain (NFR-PRIV-1).
    pub fn set_metadata(env: Env, policy_id: u64, pointer: BytesN<32>) -> Result<(), Error> {
        let record = read_policy(&env, policy_id)?;
        record.owner.require_auth();
        write_persistent(&env, &DataKey::MetaPointer(policy_id), &pointer);
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Read a policy record, or `PolicyNotFound` (200).
    pub fn policy(env: Env, policy_id: u64) -> Result<PolicyRecord, Error> {
        read_policy(&env, policy_id)
    }

    /// Read a policy's opaque metadata pointer, or `PolicyNotFound` (200) if no
    /// policy exists for the id.
    pub fn metadata(env: Env, policy_id: u64) -> Result<BytesN<32>, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::MetaPointer(policy_id))
            .ok_or(Error::PolicyNotFound)
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

fn read_policy(env: &Env, policy_id: u64) -> Result<PolicyRecord, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Policy(policy_id))
        .ok_or(Error::PolicyNotFound)
}

fn read_risk_pool(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&DataKey::RiskPool)
        .ok_or(Error::NotInitialized)
}

/// Read the guardian admin and require its authorization, or `NotInitialized`
/// (902) if the contract was never constructed.
fn require_admin(env: &Env) -> Result<(), Error> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

/// Apply a policy state transition, rejecting a policy whose current state is
/// not `from` with `InvalidState` (201). Emits `PolicyStateChanged`.
fn transition(env: &Env, policy_id: u64, from: PolicyState, to: PolicyState) -> Result<(), Error> {
    let mut record = read_policy(env, policy_id)?;
    if record.state != from {
        return Err(Error::InvalidState);
    }
    record.state = to;
    write_persistent(env, &DataKey::Policy(policy_id), &record);
    extend_instance_ttl(env);
    PolicyStateChanged {
        policy_id,
        state: to,
    }
    .publish(env);
    Ok(())
}

mod test;
