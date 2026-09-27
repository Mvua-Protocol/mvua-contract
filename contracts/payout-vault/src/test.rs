#![cfg(test)]
use super::*;
use risk_pool::{
    FeeConfig, PoolConfig, PremiumParams, RiskPool, RiskPoolClient as PoolClient, Tier,
    TrancheConfig,
};
use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, Env, Vec};
use test_utils::{new_env, random_address};

const COVERAGE: i128 = 10_000_000;
// The payable mock finalizes at a 50% severity, so a full coverage unit pays
// half. Reserves are seeded well above the largest test batch.
const EXPECTED_PAYOUT: i128 = COVERAGE / 2;
const RESERVES: i128 = 40_000_000;

// A stand in trigger-engine whose window is finalized at 50% severity: it pays
// half of any positive coverage, matching `payout_for`'s success contract.
#[contract]
pub struct MockTriggerHalf;

#[contractimpl]
impl MockTriggerHalf {
    pub fn payout_for(_env: Env, _index_id: u64, _window_id: u64, coverage: i128) -> i128 {
        coverage / 2
    }
}

// A stand in trigger-engine whose window is finalized but not breached: it pays
// zero, exercising the vault's skip path.
#[contract]
pub struct MockTriggerZero;

#[contractimpl]
impl MockTriggerZero {
    pub fn payout_for(_env: Env, _index_id: u64, _window_id: u64, _coverage: i128) -> i128 {
        0
    }
}

// Handles returned by the setup helpers.
struct Ctx {
    env: Env,
    rp_id: Address,
    vault_id: Address,
    token: Address,
    pool_id: u64,
    season_id: u64,
}

// Wire a funded risk-pool, the given trigger stand in, and a payout vault on one
// env. Creates a no-fee pool, funds its reserves through a junior deposit, opens
// and activates a season, deploys the vault pointed at all three contracts, and
// authorizes the vault to draw payouts.
fn setup_core(env: Env, trigger: Address) -> Ctx {
    let admin = random_address(&env);
    let issuer = random_address(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();

    let rp_id = env.register(RiskPool, (admin.clone(),));
    let rp = PoolClient::new(&env, &rp_id);

    let mut regions = Vec::new(&env);
    regions.push_back(symbol_short!("KE_NAK"));
    let config = PoolConfig {
        token: token.clone(),
        regions,
        season_length: 100,
        premium: PremiumParams {
            base_bps: 100,
            coverage_slope_bps: 10,
        },
        tranche: TrancheConfig {
            junior_cap: 0,
            senior_cap: 0,
        },
        fee: FeeConfig {
            protocol_bps: 0,
            publisher_bps: 0,
            protocol_cap: 0,
            publisher_cap: 0,
        },
        transferable: false,
    };
    let pool_id = rp.create_pool(&config);

    // Fund reserves: a junior depositor supplies the capital payouts draw from.
    let depositor = random_address(&env);
    token::StellarAssetClient::new(&env, &token).mint(&depositor, &RESERVES);
    rp.deposit(&pool_id, &Tier::Junior, &depositor, &RESERVES);

    let season_id = rp.open_season(&pool_id);
    rp.activate_season(&pool_id, &season_id);

    let policy_ref = random_address(&env);
    let vault_id = env.register(
        PayoutVault,
        (
            admin.clone(),
            rp_id.clone(),
            policy_ref,
            trigger,
            token.clone(),
        ),
    );
    rp.set_payout_vault(&vault_id);

    Ctx {
        env,
        rp_id,
        vault_id,
        token,
        pool_id,
        season_id,
    }
}

// The common fixture: a trigger stand in that finalizes at 50% severity.
fn setup() -> Ctx {
    let env = new_env();
    let trigger = env.register(MockTriggerHalf, ());
    setup_core(env, trigger)
}

// A fixture whose trigger reports a finalized but unbreached window (zero pay).
fn setup_zero() -> Ctx {
    let env = new_env();
    let trigger = env.register(MockTriggerZero, ());
    setup_core(env, trigger)
}

// A single payout entry for `owner` over one coverage unit.
fn one_entry(env: &Env, owner: &Address, policy_id: u64) -> Vec<PayoutEntry> {
    let mut entries = Vec::new(env);
    entries.push_back(PayoutEntry {
        policy_id,
        owner: owner.clone(),
        coverage: COVERAGE,
    });
    entries
}

#[test]
fn constructor_starts_unpaused_and_sets_admin() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    assert!(!vault.is_paused());
    // admin() returns the configured guardian without trapping.
    let _ = vault.admin();
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::Paused as u32, 500);
    assert_eq!(Error::BatchComplete as u32, 501);
    assert_eq!(Error::NoFinalizedTrigger as u32, 502);
    assert_eq!(Error::PayoutExists as u32, 503);
    assert_eq!(Error::NothingToClaim as u32, 504);
    assert_eq!(Error::InvalidBatch as u32, 505);
    assert_eq!(Error::InvalidEntry as u32, 506);
    assert_eq!(
        Error::Unauthorized as u32,
        common::error_codes::UNAUTHORIZED
    );
    assert_eq!(
        Error::NotInitialized as u32,
        common::error_codes::NOT_INITIALIZED
    );
    assert_eq!(Error::Overflow as u32, common::error_codes::OVERFLOW);
}

#[test]
fn pay_batch_moves_funds_and_credits_claim() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let rp = PoolClient::new(&ctx.env, &ctx.rp_id);
    let tok = token::TokenClient::new(&ctx.env, &ctx.token);
    let owner = random_address(&ctx.env);

    let paid = vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );

    assert_eq!(paid, 1);
    assert_eq!(vault.claimable(&owner), EXPECTED_PAYOUT);
    // Funds left the pool for the vault, and reserves fell by the payout.
    assert_eq!(tok.balance(&ctx.vault_id), EXPECTED_PAYOUT);
    assert_eq!(
        rp.solvency(&ctx.pool_id).reserves,
        RESERVES - EXPECTED_PAYOUT
    );
    // The season records the payout so settlement reconciles it.
    assert_eq!(
        rp.season(&ctx.pool_id, &ctx.season_id).payouts_paid,
        EXPECTED_PAYOUT
    );
    // The payout record and batch checkpoint are written.
    assert_eq!(vault.payout(&42).amount, EXPECTED_PAYOUT);
    let batch = vault.batch(&1, &7);
    assert_eq!(batch.policies_paid, 1);
    assert_eq!(batch.total_disbursed, EXPECTED_PAYOUT);
    assert!(matches!(batch.state, BatchState::InProgress));
}

#[test]
fn claim_pushes_to_owner_and_zeroes_ledger() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let tok = token::TokenClient::new(&ctx.env, &ctx.token);
    let owner = random_address(&ctx.env);

    vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );
    let claimed = vault.claim(&owner);

    assert_eq!(claimed, EXPECTED_PAYOUT);
    assert_eq!(tok.balance(&owner), EXPECTED_PAYOUT);
    assert_eq!(vault.claimable(&owner), 0);
    assert_eq!(tok.balance(&ctx.vault_id), 0);
}

#[test]
fn claim_with_no_balance_is_rejected() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let owner = random_address(&ctx.env);
    let err = vault.try_claim(&owner).err().unwrap().unwrap();
    assert_eq!(err, Error::NothingToClaim);
}

#[test]
fn pay_batch_is_idempotent_on_retry() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let rp = PoolClient::new(&ctx.env, &ctx.rp_id);
    let owner = random_address(&ctx.env);
    let entries = one_entry(&ctx.env, &owner, 42);

    let first = vault.pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries);
    let second = vault.pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries);

    assert_eq!(first, 1);
    assert_eq!(second, 0);
    // Nothing double paid.
    assert_eq!(vault.claimable(&owner), EXPECTED_PAYOUT);
    assert_eq!(
        rp.solvency(&ctx.pool_id).reserves,
        RESERVES - EXPECTED_PAYOUT
    );
}

#[test]
fn pay_batch_credits_each_owner_in_the_batch() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let owner_a = random_address(&ctx.env);
    let owner_b = random_address(&ctx.env);
    let mut entries = Vec::new(&ctx.env);
    entries.push_back(PayoutEntry {
        policy_id: 1,
        owner: owner_a.clone(),
        coverage: COVERAGE,
    });
    entries.push_back(PayoutEntry {
        policy_id: 2,
        owner: owner_b.clone(),
        coverage: COVERAGE,
    });

    let paid = vault.pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries);

    assert_eq!(paid, 2);
    assert_eq!(vault.claimable(&owner_a), EXPECTED_PAYOUT);
    assert_eq!(vault.claimable(&owner_b), EXPECTED_PAYOUT);
    assert_eq!(vault.batch(&1, &7).total_disbursed, EXPECTED_PAYOUT * 2);
}

#[test]
fn pay_batch_skips_unbreached_window() {
    let ctx = setup_zero();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let rp = PoolClient::new(&ctx.env, &ctx.rp_id);
    let owner = random_address(&ctx.env);

    let paid = vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );

    assert_eq!(paid, 0);
    assert_eq!(vault.claimable(&owner), 0);
    assert_eq!(rp.solvency(&ctx.pool_id).reserves, RESERVES);
}

#[test]
fn pay_batch_rejects_empty_batch() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let entries: Vec<PayoutEntry> = Vec::new(&ctx.env);
    let err = vault
        .try_pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries)
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, Error::InvalidBatch);
}

#[test]
fn pay_batch_rejects_oversized_batch() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let mut entries = Vec::new(&ctx.env);
    for i in 0..(MAX_BATCH + 1) {
        entries.push_back(PayoutEntry {
            policy_id: i as u64,
            owner: random_address(&ctx.env),
            coverage: COVERAGE,
        });
    }
    let err = vault
        .try_pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries)
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, Error::InvalidBatch);
}

#[test]
fn pay_batch_rejects_nonpositive_coverage() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let mut entries = Vec::new(&ctx.env);
    entries.push_back(PayoutEntry {
        policy_id: 1,
        owner: random_address(&ctx.env),
        coverage: 0,
    });
    let err = vault
        .try_pay_batch(&1, &7, &ctx.pool_id, &ctx.season_id, &entries)
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, Error::InvalidEntry);
}

#[test]
fn pause_blocks_pay_batch_but_not_claim() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let owner = random_address(&ctx.env);

    // Fund a claim, then pause.
    vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );
    vault.pause();
    assert!(vault.is_paused());

    // A new payout is blocked while paused.
    let err = vault
        .try_pay_batch(
            &1,
            &7,
            &ctx.pool_id,
            &ctx.season_id,
            &one_entry(&ctx.env, &owner, 99),
        )
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, Error::Paused);

    // But an already recorded payout can still be claimed.
    assert_eq!(vault.claim(&owner), EXPECTED_PAYOUT);

    vault.unpause();
    assert!(!vault.is_paused());
}

#[test]
fn close_batch_blocks_further_payouts() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let owner = random_address(&ctx.env);

    vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );
    vault.close_batch(&1, &7);
    assert!(matches!(vault.batch(&1, &7).state, BatchState::Complete));

    let err = vault
        .try_pay_batch(
            &1,
            &7,
            &ctx.pool_id,
            &ctx.season_id,
            &one_entry(&ctx.env, &owner, 43),
        )
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, Error::BatchComplete);
}

#[test]
fn pay_out_works_after_season_close() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let rp = PoolClient::new(&ctx.env, &ctx.rp_id);
    let owner = random_address(&ctx.env);

    // A finalized trigger can pay after the covered window closes.
    rp.close_season(&ctx.pool_id, &ctx.season_id);
    let paid = vault.pay_batch(
        &1,
        &7,
        &ctx.pool_id,
        &ctx.season_id,
        &one_entry(&ctx.env, &owner, 42),
    );
    assert_eq!(paid, 1);
    assert_eq!(vault.claimable(&owner), EXPECTED_PAYOUT);
}

#[test]
fn batch_and_claimable_default_to_empty() {
    let ctx = setup();
    let vault = PayoutVaultClient::new(&ctx.env, &ctx.vault_id);
    let stranger = random_address(&ctx.env);
    assert_eq!(vault.claimable(&stranger), 0);
    let batch = vault.batch(&9, &9);
    assert_eq!(batch.policies_paid, 0);
    assert_eq!(batch.total_disbursed, 0);
    assert!(matches!(batch.state, BatchState::Open));
}
