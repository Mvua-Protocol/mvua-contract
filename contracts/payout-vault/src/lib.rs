#![no_std]
//! payout-vault: settled payouts to a per holder claim ledger.
//!
//! When a trigger is finalized, the guardian assembles bounded batches of
//! payout entries; the vault independently confirms each trigger through
//! `trigger-engine.payout_for` (which traps unless the window is finalized),
//! pulls the underlying from risk-pool with the guarded `pay_out`, and credits
//! each holder's claim ledger. A farmer then receives funds with `claim`, which
//! pushes the accrued amount to their address and needs no XLM or signature
//! from them, the Soroban equivalent of a classic claimable balance (DR-0026).
//!
//! Requirements: FR-PAY-1 to FR-PAY-5.

use common::{extend_instance_ttl, extend_persistent_ttl};
use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, token,
    Address, Env, Vec,
};

/// Maximum payout entries in a single `pay_batch` call. A larger region is paid
/// by chunking on the client side, keeping instruction cost bounded (DR-0026,
/// mirrors the mint batch cap of DR-0023).
const MAX_BATCH: u32 = 20;

/// Progress of a payout batch for an index window (resumable, FR-PAY-2).
#[contracttype]
#[derive(Clone)]
pub enum BatchState {
    /// Batch created, no payouts made yet.
    Open,
    /// Batch partially paid; more chunks may follow.
    InProgress,
    /// Guardian has closed the batch; no further payouts accepted.
    Complete,
}

/// One payout instruction in a batch. The guardian supplies `owner` and
/// `coverage` from the authoritative policy record; the vault gates the payout
/// on chain through `trigger-engine.payout_for` (DR-0026).
#[contracttype]
#[derive(Clone)]
pub struct PayoutEntry {
    pub policy_id: u64,
    pub owner: Address,
    pub coverage: i128,
}

/// A single policy's payout record: idempotency key and audit trail (FR-PAY-4).
#[contracttype]
#[derive(Clone)]
pub struct PayoutRecord {
    pub owner: Address,
    pub amount: i128,
    pub paid_at: u64,
}

/// Running checkpoint for an index window batch.
#[contracttype]
#[derive(Clone)]
pub struct BatchProgress {
    pub policies_paid: u32,
    pub total_disbursed: i128,
    pub state: BatchState,
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
    /// Resumable batch checkpoint for an index window.
    Batch(u64, u64),
    /// A single policy's payout record (one per finalized trigger, FR-PAY-4).
    Payout(u64),
    /// The risk-pool address, source of committed funds. Instance durability.
    PoolRef,
    /// The policy contract address (reserved for on chain record reads in a
    /// future sprint, DR-0026). Instance durability.
    PolicyRef,
    /// The trigger-engine address, queried for finalized severity. Instance
    /// durability.
    TriggerRef,
    /// The settlement asset (one SAC per deployment). Instance durability.
    Token,
    /// A holder's accrued claimable balance.
    Claim(Address),
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
    NothingToClaim = 504,
    InvalidBatch = 505,
    InvalidEntry = 506,
    Unauthorized = 900,
    NotInitialized = 902,
    Overflow = 903,
}

/// Emitted when a policy's payout is recorded and its holder's claim credited.
/// Topic `payout_recorded`; data is the policy, owner, and amount.
#[contractevent(topics = ["payout_recorded"], data_format = "map")]
pub struct PayoutRecorded {
    pub policy_id: u64,
    pub owner: Address,
    pub amount: i128,
}

/// Emitted when a holder claims their accrued balance. Topic `claimed`; data is
/// the owner and amount pushed.
#[contractevent(topics = ["claimed"], data_format = "map")]
pub struct Claimed {
    pub owner: Address,
    pub amount: i128,
}

/// Emitted when the pause flag changes. Topic `pause_set`; data is the flag.
#[contractevent(topics = ["pause_set"], data_format = "single-value")]
pub struct PauseSet {
    pub paused: bool,
}

/// Cross contract view of risk-pool's guarded payout release (DR-0026). The
/// generated client drops the leading `env`; a sub-call error traps this
/// contract, so the return is the success type.
#[contractclient(name = "RiskPoolClient")]
pub trait RiskPoolInterface {
    fn pay_out(env: Env, pool_id: u64, season_id: u64, to: Address, amount: i128) -> i128;
}

/// Cross contract view of trigger-engine's finalized payout amount (DR-0026).
/// `payout_for` traps unless the window is finalized, which is how the vault
/// enforces "no payout without finalization" without decoding a foreign type.
#[contractclient(name = "TriggerEngineClient")]
pub trait TriggerEngineInterface {
    fn payout_for(env: Env, index_id: u64, window_id: u64, coverage: i128) -> i128;
}

#[contract]
pub struct PayoutVault;

#[contractimpl]
impl PayoutVault {
    /// Initialize the vault with its guardian admin, the risk-pool it draws
    /// from, the policy and trigger-engine contracts it references, and the
    /// settlement asset. Runs once at deployment (FR-GOV-1). Starts unpaused.
    pub fn __constructor(
        env: Env,
        admin: Address,
        pool: Address,
        policy: Address,
        trigger: Address,
        token: Address,
    ) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::PoolRef, &pool);
        env.storage().instance().set(&DataKey::PolicyRef, &policy);
        env.storage().instance().set(&DataKey::TriggerRef, &trigger);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Paused, &false);
        extend_instance_ttl(&env);
    }

    /// Pause payouts (FR-PAY-5). Guardian only. Blocks `pay_batch`; never blocks
    /// `claim`, so an already recorded payout can always be collected.
    pub fn pause(env: Env) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        extend_instance_ttl(&env);
        PauseSet { paused: true }.publish(&env);
        Ok(())
    }

    /// Resume payouts (FR-PAY-5). Guardian only.
    pub fn unpause(env: Env) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        extend_instance_ttl(&env);
        PauseSet { paused: false }.publish(&env);
        Ok(())
    }

    /// Pay a bounded batch of finalized policies (FR-PAY-1 to FR-PAY-3).
    /// Guardian only, rejected while paused (`Paused`, 500). Each entry is
    /// idempotent: a policy that already has a payout record is skipped, so an
    /// overlapping retry is safe. For each new entry the vault confirms the
    /// trigger through `payout_for` (which traps unless finalized), pulls the
    /// amount from risk-pool with `pay_out`, credits the owner's claim ledger,
    /// and records the payout. Returns the number of policies paid this call.
    /// Rejects an empty or oversized batch (`InvalidBatch`, 505) and a closed
    /// window (`BatchComplete`, 501).
    pub fn pay_batch(
        env: Env,
        index_id: u64,
        window_id: u64,
        pool_id: u64,
        season_id: u64,
        entries: Vec<PayoutEntry>,
    ) -> Result<u32, Error> {
        require_admin(&env)?;
        if is_paused_inner(&env) {
            return Err(Error::Paused);
        }
        let count = entries.len();
        if count == 0 || count > MAX_BATCH {
            return Err(Error::InvalidBatch);
        }

        let mut progress = read_batch(&env, index_id, window_id);
        if let BatchState::Complete = progress.state {
            return Err(Error::BatchComplete);
        }

        let pool: Address = read_ref(&env, &DataKey::PoolRef)?;
        let trigger: Address = read_ref(&env, &DataKey::TriggerRef)?;
        let rp = RiskPoolClient::new(&env, &pool);
        let te = TriggerEngineClient::new(&env, &trigger);
        let vault = env.current_contract_address();

        let mut paid: u32 = 0;
        for entry in entries.iter() {
            if env
                .storage()
                .persistent()
                .has(&DataKey::Payout(entry.policy_id))
            {
                continue;
            }
            if entry.coverage <= 0 {
                return Err(Error::InvalidEntry);
            }

            let amount = te.payout_for(&index_id, &window_id, &entry.coverage);
            if amount <= 0 {
                continue;
            }

            rp.pay_out(&pool_id, &season_id, &vault, &amount);

            let credited = read_claim(&env, &entry.owner)
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
            write_persistent(&env, &DataKey::Claim(entry.owner.clone()), &credited);

            let record = PayoutRecord {
                owner: entry.owner.clone(),
                amount,
                paid_at: env.ledger().timestamp(),
            };
            write_persistent(&env, &DataKey::Payout(entry.policy_id), &record);

            progress.total_disbursed = progress
                .total_disbursed
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
            progress.policies_paid += 1;
            paid += 1;
            PayoutRecorded {
                policy_id: entry.policy_id,
                owner: entry.owner.clone(),
                amount,
            }
            .publish(&env);
        }

        if paid > 0 {
            progress.state = BatchState::InProgress;
        }
        write_persistent(&env, &DataKey::Batch(index_id, window_id), &progress);
        extend_instance_ttl(&env);
        Ok(paid)
    }

    /// Close an index window batch (FR-PAY-2). Guardian only. Marks the batch
    /// `Complete` so no further `pay_batch` is accepted for the window; call it
    /// once the client has finished chunking the region.
    pub fn close_batch(env: Env, index_id: u64, window_id: u64) -> Result<(), Error> {
        require_admin(&env)?;
        let mut progress = read_batch(&env, index_id, window_id);
        progress.state = BatchState::Complete;
        write_persistent(&env, &DataKey::Batch(index_id, window_id), &progress);
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Push a holder's accrued balance to their address (FR-PAY-1). Permissionless
    /// and never blocked by the pause flag: anyone may trigger a claim and the
    /// funds always land at `owner`, who needs no XLM or signature. Zeroes the
    /// ledger before the transfer. Returns the amount, or `NothingToClaim` (504)
    /// if the balance is zero.
    pub fn claim(env: Env, owner: Address) -> Result<i128, Error> {
        let amount = read_claim(&env, &owner);
        if amount <= 0 {
            return Err(Error::NothingToClaim);
        }
        let token_addr: Address = read_ref(&env, &DataKey::Token)?;
        write_persistent(&env, &DataKey::Claim(owner.clone()), &0i128);
        token::TokenClient::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &owner,
            &amount,
        );
        extend_instance_ttl(&env);
        Claimed {
            owner: owner.clone(),
            amount,
        }
        .publish(&env);
        Ok(amount)
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
        is_paused_inner(&env)
    }

    /// A holder's accrued claimable balance (zero if none).
    pub fn claimable(env: Env, owner: Address) -> i128 {
        read_claim(&env, &owner)
    }

    /// Read a policy's payout record, or `NoFinalizedTrigger` (502) if the
    /// policy has not been paid.
    pub fn payout(env: Env, policy_id: u64) -> Result<PayoutRecord, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Payout(policy_id))
            .ok_or(Error::NoFinalizedTrigger)
    }

    /// Read the batch checkpoint for an index window (defaults to `Open`).
    pub fn batch(env: Env, index_id: u64, window_id: u64) -> BatchProgress {
        read_batch(&env, index_id, window_id)
    }
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

fn is_paused_inner(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn read_ref(env: &Env, key: &DataKey) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(Error::NotInitialized)
}

fn read_claim(env: &Env, owner: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Claim(owner.clone()))
        .unwrap_or(0)
}

fn read_batch(env: &Env, index_id: u64, window_id: u64) -> BatchProgress {
    env.storage()
        .persistent()
        .get(&DataKey::Batch(index_id, window_id))
        .unwrap_or(BatchProgress {
            policies_paid: 0,
            total_disbursed: 0,
            state: BatchState::Open,
        })
}

// Persist a value under `key` and refresh its TTL, mirroring the storage helper
// used across the other contracts (DR-0018).
fn write_persistent<V>(env: &Env, key: &DataKey, value: &V)
where
    V: soroban_sdk::IntoVal<Env, soroban_sdk::Val>,
{
    env.storage().persistent().set(key, value);
    extend_persistent_ttl(env, key);
}

mod test;
