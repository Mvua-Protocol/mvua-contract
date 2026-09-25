#![no_std]
//! risk-pool: custody of premiums and tranche capital.
//!
//! Owns pool configuration, tranche balances and receipts, season state, the
//! treasury, and the solvency record. It is the single enforcement point for
//! the solvency invariant `reserves >= committed` (FR-POOL-5). Sprint 1.2 added
//! pool creation (FR-POOL-1), tranche deposits with pro rata receipts
//! (FR-POOL-2), and the withdraw path guarded by solvency (FR-POOL-3,
//! FR-POOL-5). Sprint 1.3 adds the season lifecycle (FR-POOL-6), premium intake
//! with capped protocol and publisher fees (FR-POOL-7, FR-ACC-1), and the
//! settlement waterfall that credits surplus to junior and absorbs losses
//! junior first then senior. Receipts are held as an internal ledger, not a
//! separate token contract (DR-0020).
//!
//! Requirements: FR-POOL-1 to 7, FR-ACC-1, FR-ACC-3.

use common::{apply_bps, extend_instance_ttl, extend_persistent_ttl, BPS_DENOMINATOR};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env,
    Symbol, Vec,
};

/// One whole unit of coverage in the settlement token's smallest denomination
/// (1 USDC at 7 decimals). The premium curve's slope is applied per whole unit
/// of coverage, so a pool prices in cents without fractional rounding surprises.
const COVERAGE_UNIT: i128 = 10_000_000;

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

/// Fee schedule applied to each premium (FR-POOL-7). Rates are basis points of
/// the gross premium; a cap of zero means the accrued fee of that kind is
/// uncapped. Protocol and publisher rates together must not exceed 100 percent.
#[contracttype]
#[derive(Clone)]
pub struct FeeConfig {
    /// Protocol fee rate in basis points of each premium.
    pub protocol_bps: u32,
    /// Publisher fee rate in basis points of each premium.
    pub publisher_bps: u32,
    /// Absolute cap on total accrued protocol fees (zero means uncapped).
    pub protocol_cap: i128,
    /// Absolute cap on total accrued publisher fees (zero means uncapped).
    pub publisher_cap: i128,
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
    /// Protocol and publisher fee schedule applied to premiums.
    pub fee: FeeConfig,
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

/// Season lifecycle state (FR-POOL-6). Transitions run strictly forward:
/// `Open -> Active -> Closed -> Settled`. Any other transition is rejected.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeasonState {
    /// Enrollment: premiums are collected and coverage has not started.
    Open,
    /// Coverage window is live; premiums are still accepted and payouts commit.
    Active,
    /// Coverage window ended; awaiting settlement. No premiums, no new payouts.
    Closed,
    /// Surplus distributed and fees swept; terminal.
    Settled,
}

/// One underwriting season for a pool: its lifecycle state, window, and the
/// running premium and payout totals that settlement reconciles (FR-POOL-6,
/// FR-ACC-1). `premiums_in` is net of fees; `payouts_paid` is the underlying
/// already released to triggered policies during the season.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Season {
    pub state: SeasonState,
    pub opened_at: u64,
    pub closes_at: u64,
    pub premiums_in: i128,
    pub payouts_committed: i128,
    pub payouts_paid: i128,
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
    /// Policy contract permitted to request premium refunds. Instance durability.
    PolicyContract,
    /// Monotonic counter for allocating pool ids. Instance durability.
    PoolCount,
    /// Monotonic counter for allocating season ids within a pool.
    SeasonCount(u64),
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
    InvalidSeasonState = 110,
    SeasonNotFound = 111,
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

/// Emitted when a season opens. Topic `season_opened`; data is the detail.
#[contractevent(topics = ["season_opened"], data_format = "map")]
pub struct SeasonOpened {
    pub pool_id: u64,
    pub season_id: u64,
    pub opened_at: u64,
    pub closes_at: u64,
}

/// Emitted when a season changes state (activate, close). Topic
/// `season_state`; data is the pool, season, and the new state.
#[contractevent(topics = ["season_state"], data_format = "map")]
pub struct SeasonStateChanged {
    pub pool_id: u64,
    pub season_id: u64,
    pub state: SeasonState,
}

/// Emitted on premium intake. Topic `premium`; data is the gross, the net
/// credited to the season, and the protocol and publisher fees accrued.
#[contractevent(topics = ["premium"], data_format = "map")]
pub struct PremiumCollected {
    pub pool_id: u64,
    pub season_id: u64,
    pub from: Address,
    pub gross: i128,
    pub net: i128,
    pub protocol_fee: i128,
    pub publisher_fee: i128,
}

/// Emitted when a season settles. Topic `season_settled`; data is the season
/// totals and the signed net result distributed by the tranche waterfall.
#[contractevent(topics = ["season_settled"], data_format = "map")]
pub struct SeasonSettled {
    pub pool_id: u64,
    pub season_id: u64,
    pub premiums_in: i128,
    pub payouts_paid: i128,
    pub net: i128,
}

/// Emitted when a premium is refunded to a cancelled policy. Topic
/// `premium_refunded`; data is the pool, season, recipient, and net returned.
#[contractevent(topics = ["premium_refunded"], data_format = "map")]
pub struct PremiumRefunded {
    pub pool_id: u64,
    pub season_id: u64,
    pub to: Address,
    pub net: i128,
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

    /// Configure the policy contract permitted to request premium refunds
    /// (cancellation path, ARCHITECTURE section 7). Guardian only. Set once the
    /// policy contract is deployed; `refund_premium` authorizes no other caller.
    pub fn set_policy_contract(env: Env, policy: Address) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::PolicyContract, &policy);
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Read the configured policy contract address, or `NotInitialized` (902)
    /// if none has been set.
    pub fn policy_contract(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::PolicyContract)
            .ok_or(Error::NotInitialized)
    }

    /// Create a pool (FR-POOL-1). Admin only, since it mutates config
    /// (authorization model, ARCHITECTURE section 7). Validates parameters,
    /// allocates a pool id, and initializes both tranches, the solvency
    /// record, and the treasury to zero. Returns the new pool id.
    pub fn create_pool(env: Env, config: PoolConfig) -> Result<u64, Error> {
        require_admin(&env)?;

        if config.regions.is_empty() || config.season_length == 0 {
            return Err(Error::InvalidConfig);
        }
        if config.premium.base_bps > 10_000 || config.premium.coverage_slope_bps > 10_000 {
            return Err(Error::InvalidConfig);
        }
        if config.tranche.junior_cap < 0 || config.tranche.senior_cap < 0 {
            return Err(Error::InvalidConfig);
        }
        if config.fee.protocol_cap < 0 || config.fee.publisher_cap < 0 {
            return Err(Error::InvalidConfig);
        }
        // Each rate is a valid fraction and, combined, cannot claim more than
        // the whole premium. Bounding each first keeps the sum from overflowing.
        if config.fee.protocol_bps > 10_000 || config.fee.publisher_bps > 10_000 {
            return Err(Error::InvalidConfig);
        }
        if config.fee.protocol_bps + config.fee.publisher_bps > 10_000 {
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

    /// Quote the gross premium for a coverage amount under a pool's curve
    /// (pricing path; ARCHITECTURE section 4.2 sequence). This is a pure read:
    /// it derives the price from the pool's stored `PremiumParams` and touches
    /// no season or solvency state, so the policy contract can call it cross
    /// contract before it mints a policy. Returns the gross premium in the pool
    /// token's smallest unit. Rejects a missing pool with `PoolNotFound` (100)
    /// and non positive coverage with `InvalidAmount` (107). The premium model
    /// is provisional (DR-0022).
    pub fn premium_for(env: Env, pool_id: u64, coverage: i128) -> Result<i128, Error> {
        let config = read_config(&env, pool_id)?;
        compute_premium(&config.premium, coverage)
    }

    /// Open a new season for a pool (FR-POOL-6). Guardian only. Allocates a
    /// season id, derives the coverage window from the pool's season length,
    /// and starts the season in `Open`. Returns the new season id.
    pub fn open_season(env: Env, pool_id: u64) -> Result<u64, Error> {
        require_admin(&env)?;
        let config = read_config(&env, pool_id)?;

        let season_id = env
            .storage()
            .persistent()
            .get(&DataKey::SeasonCount(pool_id))
            .unwrap_or(0u64)
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::SeasonCount(pool_id), &season_id);

        let opened_at = env.ledger().timestamp();
        let closes_at = opened_at
            .checked_add(config.season_length)
            .ok_or(Error::Overflow)?;
        let season = Season {
            state: SeasonState::Open,
            opened_at,
            closes_at,
            premiums_in: 0,
            payouts_committed: 0,
            payouts_paid: 0,
        };
        write_persistent(&env, &DataKey::Season(pool_id, season_id), &season);

        extend_instance_ttl(&env);
        SeasonOpened {
            pool_id,
            season_id,
            opened_at,
            closes_at,
        }
        .publish(&env);
        Ok(season_id)
    }

    /// Move a season from `Open` to `Active` (FR-POOL-6). Guardian only. Any
    /// other starting state is rejected with `InvalidSeasonState` (110).
    pub fn activate_season(env: Env, pool_id: u64, season_id: u64) -> Result<(), Error> {
        transition_season(
            &env,
            pool_id,
            season_id,
            SeasonState::Open,
            SeasonState::Active,
        )
    }

    /// Move a season from `Active` to `Closed` (FR-POOL-6). Guardian only. Any
    /// other starting state is rejected with `InvalidSeasonState` (110).
    pub fn close_season(env: Env, pool_id: u64, season_id: u64) -> Result<(), Error> {
        transition_season(
            &env,
            pool_id,
            season_id,
            SeasonState::Active,
            SeasonState::Closed,
        )
    }

    /// Collect a premium into a season (FR-POOL-4 custody, FR-POOL-7 fees).
    /// Requires the payer's authorization. Splits the capped protocol and
    /// publisher fees to the treasury and credits the net premium to the season
    /// and to reserves. Accepted only while the season is `Open` or `Active`
    /// (else `SeasonNotOpen`, 103). Returns the net premium credited.
    pub fn collect_premium(
        env: Env,
        pool_id: u64,
        season_id: u64,
        from: Address,
        amount: i128,
    ) -> Result<i128, Error> {
        from.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = read_config(&env, pool_id)?;
        let mut season = read_season(&env, pool_id, season_id)?;
        if season.state != SeasonState::Open && season.state != SeasonState::Active {
            return Err(Error::SeasonNotOpen);
        }

        let mut treasury = read_treasury(&env, pool_id)?;
        let protocol_fee = accrue_fee(
            amount,
            config.fee.protocol_bps,
            treasury.protocol_fees,
            config.fee.protocol_cap,
        )?;
        let publisher_fee = accrue_fee(
            amount,
            config.fee.publisher_bps,
            treasury.publisher_fees,
            config.fee.publisher_cap,
        )?;
        let net = amount
            .checked_sub(protocol_fee)
            .and_then(|v| v.checked_sub(publisher_fee))
            .ok_or(Error::Overflow)?;

        token::TokenClient::new(&env, &config.token).transfer(
            &from,
            env.current_contract_address(),
            &amount,
        );

        treasury.protocol_fees = treasury
            .protocol_fees
            .checked_add(protocol_fee)
            .ok_or(Error::Overflow)?;
        treasury.publisher_fees = treasury
            .publisher_fees
            .checked_add(publisher_fee)
            .ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Treasury(pool_id), &treasury);

        season.premiums_in = season.premiums_in.checked_add(net).ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Season(pool_id, season_id), &season);

        let mut solvency = read_solvency(&env, pool_id)?;
        solvency.reserves = solvency.reserves.checked_add(net).ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Solvency(pool_id), &solvency);

        extend_instance_ttl(&env);
        PremiumCollected {
            pool_id,
            season_id,
            from,
            gross: amount,
            net,
            protocol_fee,
            publisher_fee,
        }
        .publish(&env);
        Ok(net)
    }

    /// Refund the net premium of a cancelled policy (cancellation path). Callable
    /// only by the configured policy contract (`set_policy_contract`), which
    /// authorizes itself in the cross contract call. Reverses `net` from the
    /// season's `premiums_in` and from solvency reserves, then returns the
    /// underlying to `to`. Fees already swept to the treasury are not reversed:
    /// only the reserve portion a policy contributed is refundable. Accepted
    /// only while the season is `Open` or `Active` (else `SeasonNotOpen`, 103),
    /// and rejected if it would break the solvency invariant `reserves >=
    /// committed` (`SolvencyViolated`, 102) or exceed available reserves
    /// (`InsufficientReserves`, 101). Returns the refunded amount.
    pub fn refund_premium(
        env: Env,
        pool_id: u64,
        season_id: u64,
        to: Address,
        net: i128,
    ) -> Result<i128, Error> {
        require_policy_contract(&env)?;
        if net <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config = read_config(&env, pool_id)?;
        let mut season = read_season(&env, pool_id, season_id)?;
        if season.state != SeasonState::Open && season.state != SeasonState::Active {
            return Err(Error::SeasonNotOpen);
        }
        if net > season.premiums_in {
            return Err(Error::InvalidAmount);
        }

        let mut solvency = read_solvency(&env, pool_id)?;
        if net > solvency.reserves {
            return Err(Error::InsufficientReserves);
        }
        let new_reserves = solvency.reserves.checked_sub(net).ok_or(Error::Overflow)?;
        if new_reserves < solvency.committed {
            return Err(Error::SolvencyViolated);
        }

        season.premiums_in = season.premiums_in.checked_sub(net).ok_or(Error::Overflow)?;
        write_persistent(&env, &DataKey::Season(pool_id, season_id), &season);

        solvency.reserves = new_reserves;
        write_persistent(&env, &DataKey::Solvency(pool_id), &solvency);

        token::TokenClient::new(&env, &config.token).transfer(
            &env.current_contract_address(),
            &to,
            &net,
        );

        extend_instance_ttl(&env);
        PremiumRefunded {
            pool_id,
            season_id,
            to,
            net,
        }
        .publish(&env);
        Ok(net)
    }

    /// Settle a `Closed` season (FR-POOL-6, settlement waterfall). Guardian
    /// only. A surplus (net premiums over payouts) is credited to the junior
    /// tranche as residual yield; a loss is absorbed by junior capital first
    /// and then senior. Moves the season to `Settled` and returns the signed
    /// net result. Rejects any state other than `Closed` (`InvalidSeasonState`,
    /// 110).
    pub fn settle_season(env: Env, pool_id: u64, season_id: u64) -> Result<i128, Error> {
        require_admin(&env)?;
        let mut season = read_season(&env, pool_id, season_id)?;
        if season.state != SeasonState::Closed {
            return Err(Error::InvalidSeasonState);
        }
        let net = season
            .premiums_in
            .checked_sub(season.payouts_paid)
            .ok_or(Error::Overflow)?;

        if net > 0 {
            credit_junior_surplus(&env, pool_id, net)?;
        } else if net < 0 {
            absorb_loss(&env, pool_id, net.checked_neg().ok_or(Error::Overflow)?)?;
        }

        season.state = SeasonState::Settled;
        write_persistent(&env, &DataKey::Season(pool_id, season_id), &season);

        extend_instance_ttl(&env);
        SeasonSettled {
            pool_id,
            season_id,
            premiums_in: season.premiums_in,
            payouts_paid: season.payouts_paid,
            net,
        }
        .publish(&env);
        Ok(net)
    }

    /// Read a season's record, or `SeasonNotFound` (111).
    pub fn season(env: Env, pool_id: u64, season_id: u64) -> Result<Season, Error> {
        read_season(&env, pool_id, season_id)
    }

    /// Read a pool's accrued treasury fees, or `PoolNotFound` (100).
    pub fn treasury(env: Env, pool_id: u64) -> Result<TreasuryState, Error> {
        read_treasury(&env, pool_id)
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

fn read_season(env: &Env, pool_id: u64, season_id: u64) -> Result<Season, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Season(pool_id, season_id))
        .ok_or(Error::SeasonNotFound)
}

fn read_treasury(env: &Env, pool_id: u64) -> Result<TreasuryState, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Treasury(pool_id))
        .ok_or(Error::PoolNotFound)
}

/// Read the guardian admin and require its authorization, or fail with
/// `NotInitialized` (902) if the contract was never constructed.
fn require_admin(env: &Env) -> Result<(), Error> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

/// Read the configured policy contract and require its authorization. Fails
/// with `Unauthorized` (900) if no policy contract has been configured, so a
/// refund can never be authorized before governance wires the two contracts.
fn require_policy_contract(env: &Env) -> Result<Address, Error> {
    let policy: Address = env
        .storage()
        .instance()
        .get(&DataKey::PolicyContract)
        .ok_or(Error::Unauthorized)?;
    policy.require_auth();
    Ok(policy)
}

/// Apply a forward season transition. Guardian only. Rejects a season whose
/// current state is not `from` with `InvalidSeasonState` (110).
fn transition_season(
    env: &Env,
    pool_id: u64,
    season_id: u64,
    from: SeasonState,
    to: SeasonState,
) -> Result<(), Error> {
    require_admin(env)?;
    let mut season = read_season(env, pool_id, season_id)?;
    if season.state != from {
        return Err(Error::InvalidSeasonState);
    }
    season.state = to;
    write_persistent(env, &DataKey::Season(pool_id, season_id), &season);
    extend_instance_ttl(env);
    SeasonStateChanged {
        pool_id,
        season_id,
        state: to,
    }
    .publish(env);
    Ok(())
}

/// Compute the fee to accrue on `amount` at `bps`, clamped so the running
/// `accrued` total never exceeds `cap` (a cap of zero means uncapped). Returns
/// `Overflow` (903) on checked arithmetic failure.
fn accrue_fee(amount: i128, bps: u32, accrued: i128, cap: i128) -> Result<i128, Error> {
    let desired = apply_bps(amount, bps).ok_or(Error::Overflow)?;
    if cap == 0 {
        return Ok(desired);
    }
    let room = cap.checked_sub(accrued).unwrap_or(0).max(0);
    Ok(desired.min(room))
}

/// Credit a settlement surplus to the junior tranche as residual yield, raising
/// the junior receipt share price.
fn credit_junior_surplus(env: &Env, pool_id: u64, surplus: i128) -> Result<(), Error> {
    let mut junior = read_tranche(env, pool_id, Tier::Junior);
    junior.deposited = junior
        .deposited
        .checked_add(surplus)
        .ok_or(Error::Overflow)?;
    write_persistent(env, &DataKey::Tranche(pool_id, Tier::Junior), &junior);
    Ok(())
}

/// Absorb a settlement loss against tranche capital, junior first then senior.
/// Any residual beyond both tranches' capital is left unabsorbed; the solvency
/// invariant on the payout path prevents committing more than reserves cover.
fn absorb_loss(env: &Env, pool_id: u64, loss: i128) -> Result<(), Error> {
    let mut junior = read_tranche(env, pool_id, Tier::Junior);
    let from_junior = loss.min(junior.deposited);
    junior.deposited = junior
        .deposited
        .checked_sub(from_junior)
        .ok_or(Error::Overflow)?;
    write_persistent(env, &DataKey::Tranche(pool_id, Tier::Junior), &junior);

    let remainder = loss.checked_sub(from_junior).ok_or(Error::Overflow)?;
    if remainder > 0 {
        let mut senior = read_tranche(env, pool_id, Tier::Senior);
        let from_senior = remainder.min(senior.deposited);
        senior.deposited = senior
            .deposited
            .checked_sub(from_senior)
            .ok_or(Error::Overflow)?;
        write_persistent(env, &DataKey::Tranche(pool_id, Tier::Senior), &senior);
    }
    Ok(())
}

/// Multiply then divide with i128 checked arithmetic. Returns `None` on
/// overflow or division by zero so callers map it to `Overflow` (903).
fn mul_div(a: i128, b: i128, denom: i128) -> Option<i128> {
    a.checked_mul(b).and_then(|p| p.checked_div(denom))
}

/// Evaluate the provisional premium curve for a coverage amount (DR-0022). The
/// marginal rate rises with coverage:
/// `rate_bps = base_bps + coverage_slope_bps * floor(coverage / COVERAGE_UNIT)`,
/// and the gross premium is `ceil(coverage * rate_bps / BPS_DENOMINATOR)`.
/// Rounding is always up so the pool never underprices a policy by a
/// sub-unit remainder. All arithmetic is i128 checked; failures map to
/// `Overflow` (903). Non positive coverage is rejected with `InvalidAmount`
/// (107).
fn compute_premium(premium: &PremiumParams, coverage: i128) -> Result<i128, Error> {
    if coverage <= 0 {
        return Err(Error::InvalidAmount);
    }
    let units = coverage / COVERAGE_UNIT;
    let slope_component = i128::from(premium.coverage_slope_bps)
        .checked_mul(units)
        .ok_or(Error::Overflow)?;
    let rate_bps = i128::from(premium.base_bps)
        .checked_add(slope_component)
        .ok_or(Error::Overflow)?;
    coverage
        .checked_mul(rate_bps)
        .and_then(|n| n.checked_add(BPS_DENOMINATOR - 1))
        .and_then(|n| n.checked_div(BPS_DENOMINATOR))
        .ok_or(Error::Overflow)
}

mod test;
