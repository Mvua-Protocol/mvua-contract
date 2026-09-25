#![no_std]
//! risk-pool: custody of premiums and tranche capital.
//!
//! Owns pool configuration, tranche balances and receipts, season state, the
//! treasury, and the solvency record. It is the single enforcement point for
//! the solvency invariant `reserves >= committed` (FR-POOL-5). Sprint 1.2 adds
//! pool creation (FR-POOL-1), tranche deposits with pro rata receipts
//! (FR-POOL-2), and the withdraw path guarded by solvency (FR-POOL-3,
//! FR-POOL-5). Receipts are held as an internal ledger, not a separate token
//! contract (DR-0020). Premium accounting and season settlement land later.
//!
//! Requirements: FR-POOL-1 to 7, FR-ACC-1, FR-ACC-3.

use common::{extend_instance_ttl, extend_persistent_ttl};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env,
    Symbol, Vec,
};

/// Capital tranche seniority. Junior absorbs losses first and earns more; the
/// senior tranche is paid down first on withdrawal.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    Junior,
    Senior,
}

/// Premium curve parameters, stored at creation and evaluated by the pricing
/// path in a later sprint. Kept as documented inputs so a pool's price basis is
/// fixed when it opens.
#[contracttype]
#[derive(Clone)]
pub struct PremiumParams {
    /// Flat base premium in basis points of coverage.
    pub base_bps: u32,
    /// Additional basis points applied per unit of coverage (curve slope).
    pub coverage_slope_bps: u32,
}

/// Per tranche configuration. A cap of zero means the tranche is uncapped.
#[contracttype]
#[derive(Clone)]
pub struct TrancheConfig {
    pub junior_cap: i128,
    pub senior_cap: i128,
}

/// Immutable and parameterized pool configuration. Immutable fields are fixed
/// at creation; parameter fields change only via timelock (a later sprint).
#[contracttype]
#[derive(Clone)]
pub struct PoolConfig {
    /// Settlement token (the USDC Stellar Asset Contract address).
    pub token: Address,
    /// Region codes this pool underwrites. Must be non empty.
    pub regions: Vec<Symbol>,
    /// Season length in ledger seconds.
    pub season_length: u64,
    /// Premium curve parameters (evaluated by the pricing sprint).
    pub premium: PremiumParams,
    /// Per tranche deposit caps.
    pub tranche: TrancheConfig,
    /// Default transferability of receipts for this pool.
    pub transferable: bool,
}

/// Deposited underlying and receipt supply for one pool tranche.
#[contracttype]
#[derive(Clone)]
pub struct TrancheState {
    pub deposited: i128,
    pub supply: i128,
}

/// Solvency record for a pool. The invariant `reserves >= committed` must hold
/// at the end of every mutating call (FR-POOL-5).
#[contracttype]
#[derive(Clone)]
pub struct SolvencyState {
    pub reserves: i128,
    pub committed: i128,
}

/// Accrued protocol and publisher fees for a pool, capped by `PoolConfig`.
#[contracttype]
#[derive(Clone)]
pub struct TreasuryState {
    pub protocol_fees: i128,
    pub publisher_fees: i128,
}

/// Authoritative storage layout (`docs/ARCHITECTURE.md` section 5.1). Adding or
/// changing a key updates that table in the same pull request.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Guardian multisig configuration reference. Instance durability.
    Admin,
    /// Monotonic counter for allocating pool ids. Instance durability.
    PoolCount,
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
    InvalidConfig = 106,
    InvalidAmount = 107,
    InsufficientReceipts = 108,
    TrancheCapExceeded = 109,
    Unauthorized = 900,
    TimelockPending = 901,
    NotInitialized = 902,
    Overflow = 903,
}

/// Emitted when a pool is created. Topic `pool_created`; data is the new id.
#[contractevent(topics = ["pool_created"], data_format = "single-value")]
pub struct PoolCreated {
    pub pool_id: u64,
}

/// Emitted on a tranche deposit. Topic `deposit`; data is the deposit detail.
#[contractevent(topics = ["deposit"], data_format = "map")]
pub struct Deposited {
    pub pool_id: u64,
    pub tier: Tier,
    pub from: Address,
    pub amount: i128,
    pub minted: i128,
}

/// Emitted on a withdrawal. Topic `withdraw`; data is the withdrawal detail.
#[contractevent(topics = ["withdraw"], data_format = "map")]
pub struct Withdrawn {
    pub pool_id: u64,
    pub tier: Tier,
    pub holder: Address,
    pub receipt_amount: i128,
    pub underlying: i128,
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

    /// Create a pool (FR-POOL-1). Admin only, since it mutates config
    /// (authorization model, ARCHITECTURE section 7). Validates parameters,
    /// allocates a pool id, and initializes both tranches, the solvency
    /// record, and the treasury to zero. Returns the new pool id.
    pub fn create_pool(env: Env, config: PoolConfig) -> Result<u64, Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if config.regions.is_empty() || config.season_length == 0 {
            return Err(Error::InvalidConfig);
        }
        if config.premium.base_bps > 10_000 || config.premium.coverage_slope_bps > 10_000 {
            return Err(Error::InvalidConfig);
        }
        if config.tranche.junior_cap < 0 || config.tranche.senior_cap < 0 {
            return Err(Error::InvalidConfig);
        }

        let id = env
            .storage()
            .instance()
            .get(&DataKey::PoolCount)
            .unwrap_or(0u64)
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        env.storage().instance().set(&DataKey::PoolCount, &id);

        write_persistent(&env, &DataKey::PoolConfig(id), &config);
        for tier in [Tier::Junior, Tier::Senior] {
            write_persistent(
                &env,
                &DataKey::Tranche(id, tier),
                &TrancheState {
                    deposited: 0,
                    supply: 0,
                },
            );
        }
        write_persistent(
            &env,
            &DataKey::Solvency(id),
            &SolvencyState {
                reserves: 0,
                committed: 0,
            },
        );
        write_persistent(
            &env,
            &DataKey::Treasury(id),
            &TreasuryState {
                protocol_fees: 0,
                publisher_fees: 0,
            },
        );

        extend_instance_ttl(&env);
        PoolCreated { pool_id: id }.publish(&env);
        Ok(id)
    }

    /// Deposit `amount` of the pool token into a tranche and mint pro rata
    /// receipts (FR-POOL-2). Requires the depositor's authorization. The first
    /// deposit into an empty tranche mints receipts one to one; later deposits
    /// mint pro rata to the existing supply. Returns the minted receipts.
    pub fn deposit(
        env: Env,
        pool_id: u64,
        tier: Tier,
        from: Address,
        amount: i128,
    ) -> Result<i128, Error> {
        from.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = read_config(&env, pool_id)?;

        let cap = match tier {
            Tier::Junior => config.tranche.junior_cap,
            Tier::Senior => config.tranche.senior_cap,
        };
        let mut tranche = read_tranche(&env, pool_id, tier);
        let new_deposited = tranche
            .deposited
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        if cap > 0 && new_deposited > cap {
            return Err(Error::TrancheCapExceeded);
        }

        let minted = if tranche.supply == 0 {
            amount
        } else {
            mul_div(amount, tranche.supply, tranche.deposited).ok_or(Error::Overflow)?
        };
        if minted <= 0 {
            return Err(Error::InvalidAmount);
        }

        token::TokenClient::new(&env, &config.token).transfer(
            &from,
            env.current_contract_address(),
            &amount,
        );
        tranche.deposited = new_deposited;
        tranche.supply = tranche.supply.checked_add(minted).ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Tranche(pool_id, tier), &tranche);

        let receipt_key = DataKey::Receipt(pool_id, tier, from.clone());
        let new_balance = read_receipt(&env, &receipt_key)
            .checked_add(minted)
            .ok_or(Error::Overflow)?;
        write_persistent(&env, &receipt_key, &new_balance);

        let mut solvency = read_solvency(&env, pool_id)?;
        solvency.reserves = solvency
            .reserves
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Solvency(pool_id), &solvency);

        extend_instance_ttl(&env);
        Deposited {
            pool_id,
            tier,
            from,
            amount,
            minted,
        }
        .publish(&env);
        Ok(minted)
    }

    /// Withdraw underlying from a tranche by burning receipts (FR-POOL-3),
    /// rejected if it would break the solvency invariant `reserves >=
    /// committed` (FR-POOL-5). Senior withdrawals are blocked while any payout
    /// is committed. Requires the holder's authorization. Returns the
    /// underlying released.
    pub fn withdraw(
        env: Env,
        pool_id: u64,
        tier: Tier,
        holder: Address,
        receipt_amount: i128,
    ) -> Result<i128, Error> {
        holder.require_auth();
        if receipt_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = read_config(&env, pool_id)?;

        let receipt_key = DataKey::Receipt(pool_id, tier, holder.clone());
        let balance = read_receipt(&env, &receipt_key);
        if receipt_amount > balance {
            return Err(Error::InsufficientReceipts);
        }

        let mut tranche = read_tranche(&env, pool_id, tier);
        if tranche.supply <= 0 {
            return Err(Error::InsufficientReceipts);
        }
        let underlying =
            mul_div(receipt_amount, tranche.deposited, tranche.supply).ok_or(Error::Overflow)?;
        if underlying <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut solvency = read_solvency(&env, pool_id)?;
        if tier == Tier::Senior && solvency.committed > 0 {
            return Err(Error::TrancheWithdrawBlocked);
        }
        if underlying > solvency.reserves {
            return Err(Error::InsufficientReserves);
        }
        let new_reserves = solvency
            .reserves
            .checked_sub(underlying)
            .ok_or(Error::Overflow)?;
        if new_reserves < solvency.committed {
            return Err(Error::SolvencyViolated);
        }

        tranche.deposited = tranche
            .deposited
            .checked_sub(underlying)
            .ok_or(Error::Overflow)?;
        tranche.supply = tranche
            .supply
            .checked_sub(receipt_amount)
            .ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Tranche(pool_id, tier), &tranche);

        let new_balance = balance.checked_sub(receipt_amount).ok_or(Error::Overflow)?;
        write_persistent(&env, &receipt_key, &new_balance);

        solvency.reserves = new_reserves;
        write_persistent(&env, &DataKey::Solvency(pool_id), &solvency);

        token::TokenClient::new(&env, &config.token).transfer(
            &env.current_contract_address(),
            &holder,
            &underlying,
        );

        extend_instance_ttl(&env);
        Withdrawn {
            pool_id,
            tier,
            holder,
            receipt_amount,
            underlying,
        }
        .publish(&env);
        Ok(underlying)
    }

    /// Read a pool's configuration, or `PoolNotFound` (100).
    pub fn pool_config(env: Env, pool_id: u64) -> Result<PoolConfig, Error> {
        read_config(&env, pool_id)
    }

    /// Read a tranche's deposited underlying and receipt supply (zero if the
    /// tranche has never been touched).
    pub fn tranche(env: Env, pool_id: u64, tier: Tier) -> TrancheState {
        read_tranche(&env, pool_id, tier)
    }

    /// Read a holder's receipt balance in a tranche (zero if none).
    pub fn receipt(env: Env, pool_id: u64, tier: Tier, holder: Address) -> i128 {
        read_receipt(&env, &DataKey::Receipt(pool_id, tier, holder))
    }

    /// Read a pool's solvency record, or `PoolNotFound` (100).
    pub fn solvency(env: Env, pool_id: u64) -> Result<SolvencyState, Error> {
        read_solvency(&env, pool_id)
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

fn read_config(env: &Env, pool_id: u64) -> Result<PoolConfig, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::PoolConfig(pool_id))
        .ok_or(Error::PoolNotFound)
}

fn read_tranche(env: &Env, pool_id: u64, tier: Tier) -> TrancheState {
    env.storage()
        .persistent()
        .get(&DataKey::Tranche(pool_id, tier))
        .unwrap_or(TrancheState {
            deposited: 0,
            supply: 0,
        })
}

fn read_receipt(env: &Env, key: &DataKey) -> i128 {
    env.storage().persistent().get(key).unwrap_or(0)
}

fn read_solvency(env: &Env, pool_id: u64) -> Result<SolvencyState, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Solvency(pool_id))
        .ok_or(Error::PoolNotFound)
}

/// Multiply then divide with i128 checked arithmetic. Returns `None` on
/// overflow or division by zero so callers map it to `Overflow` (903).
fn mul_div(a: i128, b: i128, denom: i128) -> Option<i128> {
    a.checked_mul(b).and_then(|p| p.checked_div(denom))
}

mod test;
