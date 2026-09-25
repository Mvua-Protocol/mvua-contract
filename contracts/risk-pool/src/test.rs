#![cfg(test)]
use super::*;
use soroban_sdk::{symbol_short, token, Address, Vec};
use test_utils::{new_env, random_address};

// Register a risk-pool with a fresh USDC style SAC, create one pool, and fund a
// depositor. Returns the env, the contract id, the token id, the pool id, and a
// depositor holding a large balance. `junior_cap` of zero means uncapped.
fn setup_pool(junior_cap: i128) -> (Env, Address, Address, u64, Address) {
    let env = new_env();
    let admin = random_address(&env);
    let issuer = random_address(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();
    let contract_id = env.register(RiskPool, (admin,));
    let client = RiskPoolClient::new(&env, &contract_id);

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
            junior_cap,
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
    let pool_id = client.create_pool(&config);

    let depositor = random_address(&env);
    token::StellarAssetClient::new(&env, &token).mint(&depositor, &1_000_000);
    (env, contract_id, token, pool_id, depositor)
}

#[test]
fn constructor_sets_admin() {
    let env = new_env();
    let admin = random_address(&env);
    let contract_id = env.register(RiskPool, (admin.clone(),));
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(client.admin(), admin);
}

#[test]
fn error_codes_match_contract_range() {
    // Contract owned codes live in 100 to 199; shared codes in 900 to 999.
    assert_eq!(Error::PoolNotFound as u32, 100);
    assert_eq!(Error::TrancheCapExceeded as u32, 109);
    assert_eq!(
        Error::Unauthorized as u32,
        common::error_codes::UNAUTHORIZED
    );
    assert_eq!(Error::Overflow as u32, common::error_codes::OVERFLOW);
}

#[test]
fn create_pool_allocates_ids_and_zeroes_state() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(pool_id, 1);

    let config = client.pool_config(&pool_id);
    assert_eq!(config.season_length, 100);

    let solvency = client.solvency(&pool_id);
    assert_eq!(solvency.reserves, 0);
    assert_eq!(solvency.committed, 0);

    let junior = client.tranche(&pool_id, &Tier::Junior);
    assert_eq!(junior.deposited, 0);
    assert_eq!(junior.supply, 0);
}

#[test]
fn create_pool_rejects_empty_regions() {
    let env = new_env();
    let admin = random_address(&env);
    let issuer = random_address(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();
    let contract_id = env.register(RiskPool, (admin,));
    let client = RiskPoolClient::new(&env, &contract_id);

    let config = PoolConfig {
        token,
        regions: Vec::new(&env),
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
    assert_eq!(
        client.try_create_pool(&config),
        Err(Ok(Error::InvalidConfig))
    );
}

#[test]
fn deposit_first_time_mints_one_to_one() {
    let (env, contract_id, token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let token_client = token::TokenClient::new(&env, &token);

    let minted = client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);
    assert_eq!(minted, 1_000);
    assert_eq!(client.receipt(&pool_id, &Tier::Junior, &depositor), 1_000);

    let tranche = client.tranche(&pool_id, &Tier::Junior);
    assert_eq!(tranche.deposited, 1_000);
    assert_eq!(tranche.supply, 1_000);
    assert_eq!(client.solvency(&pool_id).reserves, 1_000);

    // The underlying moved from the depositor to the pool contract.
    assert_eq!(token_client.balance(&contract_id), 1_000);
    assert_eq!(token_client.balance(&depositor), 999_000);
}

#[test]
fn deposit_mints_pro_rata_to_existing_supply() {
    let (env, contract_id, token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);

    client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);

    // Simulate the tranche gaining value (supply 1000, deposited 2000) so the
    // next deposit is priced against a non trivial share price.
    env.as_contract(&contract_id, || {
        let key = DataKey::Tranche(pool_id, Tier::Junior);
        let mut tranche: TrancheState = env.storage().persistent().get(&key).unwrap();
        tranche.deposited = 2_000;
        env.storage().persistent().set(&key, &tranche);
    });

    let second = random_address(&env);
    token::StellarAssetClient::new(&env, &token).mint(&second, &1_000);
    // 1000 * supply(1000) / deposited(2000) = 500 receipts.
    let minted = client.deposit(&pool_id, &Tier::Junior, &second, &1_000);
    assert_eq!(minted, 500);
    assert_eq!(client.receipt(&pool_id, &Tier::Junior, &second), 500);
}

#[test]
fn deposit_into_senior_tranche_is_isolated() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);

    client.deposit(&pool_id, &Tier::Senior, &depositor, &2_000);
    assert_eq!(client.receipt(&pool_id, &Tier::Senior, &depositor), 2_000);
    // Junior is untouched by a senior deposit.
    assert_eq!(client.tranche(&pool_id, &Tier::Junior).supply, 0);
    assert_eq!(client.tranche(&pool_id, &Tier::Senior).deposited, 2_000);
}

#[test]
fn deposit_rejects_non_positive_amount() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(
        client.try_deposit(&pool_id, &Tier::Junior, &depositor, &0),
        Err(Ok(Error::InvalidAmount))
    );
}

#[test]
fn deposit_rejects_unknown_pool() {
    let (env, contract_id, _token, _pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(
        client.try_deposit(&999, &Tier::Junior, &depositor, &100),
        Err(Ok(Error::PoolNotFound))
    );
}

#[test]
fn deposit_rejects_when_over_tranche_cap() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(500);
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(
        client.try_deposit(&pool_id, &Tier::Junior, &depositor, &1_000),
        Err(Ok(Error::TrancheCapExceeded))
    );
    // A deposit up to the cap is accepted.
    assert_eq!(
        client.deposit(&pool_id, &Tier::Junior, &depositor, &500),
        500
    );
}

#[test]
fn withdraw_burns_receipts_and_returns_underlying() {
    let (env, contract_id, token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let token_client = token::TokenClient::new(&env, &token);

    client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);
    let released = client.withdraw(&pool_id, &Tier::Junior, &depositor, &400);
    assert_eq!(released, 400);

    assert_eq!(client.receipt(&pool_id, &Tier::Junior, &depositor), 600);
    assert_eq!(client.tranche(&pool_id, &Tier::Junior).deposited, 600);
    assert_eq!(client.solvency(&pool_id).reserves, 600);
    assert_eq!(token_client.balance(&depositor), 999_400);
}

#[test]
fn withdraw_rejects_more_receipts_than_held() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);
    assert_eq!(
        client.try_withdraw(&pool_id, &Tier::Junior, &depositor, &1_500),
        Err(Ok(Error::InsufficientReceipts))
    );
}

// Seed a pool's committed payout figure directly, standing in for the
// payout-vault path that will set it in a later sprint.
fn set_committed(env: &Env, contract_id: &Address, pool_id: u64, committed: i128) {
    env.as_contract(contract_id, || {
        let key = DataKey::Solvency(pool_id);
        let mut solvency: SolvencyState = env.storage().persistent().get(&key).unwrap();
        solvency.committed = committed;
        env.storage().persistent().set(&key, &solvency);
    });
}

#[test]
fn withdraw_rejected_when_it_would_underfund_committed() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);

    client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);
    set_committed(&env, &contract_id, pool_id, 800);

    // Releasing 400 would drop reserves to 600, below the 800 committed.
    assert_eq!(
        client.try_withdraw(&pool_id, &Tier::Junior, &depositor, &400),
        Err(Ok(Error::SolvencyViolated))
    );
    // Releasing 200 leaves reserves exactly at the committed floor, so it holds.
    assert_eq!(
        client.withdraw(&pool_id, &Tier::Junior, &depositor, &200),
        200
    );
    assert_eq!(client.solvency(&pool_id).reserves, 800);
}

#[test]
fn senior_withdraw_blocked_while_any_payout_committed() {
    let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);

    client.deposit(&pool_id, &Tier::Senior, &depositor, &1_000);
    set_committed(&env, &contract_id, pool_id, 1);

    // Even a tiny withdrawal that keeps reserves far above committed is blocked
    // for the senior tranche while a payout is outstanding.
    assert_eq!(
        client.try_withdraw(&pool_id, &Tier::Senior, &depositor, &100),
        Err(Ok(Error::TrancheWithdrawBlocked))
    );
}

#[test]
fn solvency_invariant_holds_across_committed_levels() {
    // Property: for a fixed 1000 in reserves, a junior withdrawal of `w`
    // succeeds exactly when reserves after release stay at or above committed,
    // and is rejected otherwise. The end state never violates the invariant.
    for committed in [0i128, 250, 500, 750, 1_000] {
        let (env, contract_id, _token, pool_id, depositor) = setup_pool(0);
        let client = RiskPoolClient::new(&env, &contract_id);
        client.deposit(&pool_id, &Tier::Junior, &depositor, &1_000);
        set_committed(&env, &contract_id, pool_id, committed);

        let want = 400i128;
        let result = client.try_withdraw(&pool_id, &Tier::Junior, &depositor, &want);
        if 1_000 - want >= committed {
            assert_eq!(result, Ok(Ok(want)));
        } else {
            assert_eq!(result, Err(Ok(Error::SolvencyViolated)));
        }
        let solvency = client.solvency(&pool_id);
        assert!(solvency.reserves >= solvency.committed);
    }
}

// ----- Sprint 1.3: season lifecycle, premiums and fees, settlement -----

// Register a pool with the given fee schedule and fund a premium payer with a
// large balance. Tranche and premium params mirror `setup_pool`.
fn setup_pool_with_fees(
    protocol_bps: u32,
    publisher_bps: u32,
    protocol_cap: i128,
    publisher_cap: i128,
) -> (Env, Address, Address, u64, Address) {
    let env = new_env();
    let admin = random_address(&env);
    let issuer = random_address(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();
    let contract_id = env.register(RiskPool, (admin,));
    let client = RiskPoolClient::new(&env, &contract_id);

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
            protocol_bps,
            publisher_bps,
            protocol_cap,
            publisher_cap,
        },
        transferable: false,
    };
    let pool_id = client.create_pool(&config);
    let payer = random_address(&env);
    token::StellarAssetClient::new(&env, &token).mint(&payer, &1_000_000);
    (env, contract_id, token, pool_id, payer)
}

// Seed a season's realized payouts directly, standing in for the payout-vault
// path that will set it in a later sprint.
fn set_payouts_paid(env: &Env, contract_id: &Address, pool_id: u64, season_id: u64, paid: i128) {
    env.as_contract(contract_id, || {
        let key = DataKey::Season(pool_id, season_id);
        let mut season: Season = env.storage().persistent().get(&key).unwrap();
        season.payouts_paid = paid;
        env.storage().persistent().set(&key, &season);
    });
}

#[test]
fn open_season_allocates_ids_and_starts_open() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);

    let first = client.open_season(&pool_id);
    let second = client.open_season(&pool_id);
    assert_eq!(first, 1);
    assert_eq!(second, 2);

    let season = client.season(&pool_id, &first);
    assert_eq!(season.state, SeasonState::Open);
    assert_eq!(season.premiums_in, 0);
    // Window is opened_at plus the pool's season length (100).
    assert_eq!(season.closes_at - season.opened_at, 100);
}

#[test]
fn season_transitions_open_active_closed() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);

    client.activate_season(&pool_id, &season_id);
    assert_eq!(
        client.season(&pool_id, &season_id).state,
        SeasonState::Active
    );
    client.close_season(&pool_id, &season_id);
    assert_eq!(
        client.season(&pool_id, &season_id).state,
        SeasonState::Closed
    );
}

#[test]
fn illegal_season_transitions_are_rejected() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);

    // Cannot close a season that is still Open (must be Active first).
    assert_eq!(
        client.try_close_season(&pool_id, &season_id),
        Err(Ok(Error::InvalidSeasonState))
    );
    // Cannot activate twice.
    client.activate_season(&pool_id, &season_id);
    assert_eq!(
        client.try_activate_season(&pool_id, &season_id),
        Err(Ok(Error::InvalidSeasonState))
    );
}

#[test]
fn season_read_unknown_is_not_found() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    assert_eq!(
        client.try_season(&pool_id, &999),
        Err(Ok(Error::SeasonNotFound))
    );
}

#[test]
fn settle_rejected_unless_closed() {
    let (env, contract_id, _token, pool_id, _depositor) = setup_pool(0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);
    // Open, not Closed.
    assert_eq!(
        client.try_settle_season(&pool_id, &season_id),
        Err(Ok(Error::InvalidSeasonState))
    );
}

#[test]
fn collect_premium_accrues_fees_and_credits_net() {
    // 1 percent protocol, 0.5 percent publisher, uncapped.
    let (env, contract_id, token, pool_id, payer) = setup_pool_with_fees(100, 50, 0, 0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let token_client = token::TokenClient::new(&env, &token);
    let season_id = client.open_season(&pool_id);
    client.activate_season(&pool_id, &season_id);

    // 1000 premium: 10 protocol, 5 publisher, 985 net.
    let net = client.collect_premium(&pool_id, &season_id, &payer, &1_000);
    assert_eq!(net, 985);

    let season = client.season(&pool_id, &season_id);
    assert_eq!(season.premiums_in, 985);
    let treasury = client.treasury(&pool_id);
    assert_eq!(treasury.protocol_fees, 10);
    assert_eq!(treasury.publisher_fees, 5);
    // Net premium backs coverage; fees are held but not counted as reserves.
    assert_eq!(client.solvency(&pool_id).reserves, 985);
    // The full gross moved into the contract.
    assert_eq!(token_client.balance(&contract_id), 1_000);
    assert_eq!(token_client.balance(&payer), 999_000);
}

#[test]
fn collect_premium_respects_fee_caps() {
    // 10 percent protocol fee, capped at 15 in absolute terms.
    let (env, contract_id, _token, pool_id, payer) = setup_pool_with_fees(1_000, 0, 15, 0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);
    client.activate_season(&pool_id, &season_id);

    // First 100 premium: desired fee 10, room 15, accrues 10.
    client.collect_premium(&pool_id, &season_id, &payer, &100);
    assert_eq!(client.treasury(&pool_id).protocol_fees, 10);
    // Second 100: desired 10 but only 5 of cap remains, so fee is clamped to 5.
    let net = client.collect_premium(&pool_id, &season_id, &payer, &100);
    assert_eq!(net, 95);
    assert_eq!(client.treasury(&pool_id).protocol_fees, 15);
    // A third premium accrues no further protocol fee (cap reached).
    let net3 = client.collect_premium(&pool_id, &season_id, &payer, &100);
    assert_eq!(net3, 100);
    assert_eq!(client.treasury(&pool_id).protocol_fees, 15);
}

#[test]
fn collect_premium_rejected_when_season_not_open() {
    let (env, contract_id, _token, pool_id, payer) = setup_pool_with_fees(0, 0, 0, 0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);
    client.activate_season(&pool_id, &season_id);
    client.close_season(&pool_id, &season_id);
    // Closed seasons no longer accept premiums.
    assert_eq!(
        client.try_collect_premium(&pool_id, &season_id, &payer, &100),
        Err(Ok(Error::SeasonNotOpen))
    );
}

#[test]
fn collect_premium_rejects_bad_inputs() {
    let (env, contract_id, _token, pool_id, payer) = setup_pool_with_fees(0, 0, 0, 0);
    let client = RiskPoolClient::new(&env, &contract_id);
    let season_id = client.open_season(&pool_id);
    assert_eq!(
        client.try_collect_premium(&pool_id, &season_id, &payer, &0),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        client.try_collect_premium(&pool_id, &999, &payer, &100),
        Err(Ok(Error::SeasonNotFound))
    );
}

#[test]
fn settle_credits_surplus_to_junior_and_reaches_holders() {
    // No fees, so the whole premium is surplus at settlement.
    let (env, contract_id, _token, pool_id, payer) = setup_pool_with_fees(0, 0, 0, 0);
    let client = RiskPoolClient::new(&env, &contract_id);

    // Junior provides 1000 of capital; a 300 premium comes in over the season.
    client.deposit(&pool_id, &Tier::Junior, &payer, &1_000);
    let season_id = client.open_season(&pool_id);
    client.activate_season(&pool_id, &season_id);
    client.collect_premium(&pool_id, &season_id, &payer, &300);
    client.close_season(&pool_id, &season_id);

    let net = client.settle_season(&pool_id, &season_id);
    assert_eq!(net, 300);
    assert_eq!(
        client.season(&pool_id, &season_id).state,
        SeasonState::Settled
    );
    // Surplus lifts junior deposited to 1300 against an unchanged 1000 supply.
    let junior = client.tranche(&pool_id, &Tier::Junior);
    assert_eq!(junior.deposited, 1_300);
    assert_eq!(junior.supply, 1_000);
    // The holder's receipts are now worth the premium yield: 1000 -> 1300.
    let released = client.withdraw(&pool_id, &Tier::Junior, &payer, &1_000);
    assert_eq!(released, 1_300);
}

#[test]
fn settle_absorbs_loss_junior_first_then_senior() {
    // Property: a season loss reduces junior capital first, then senior, and
    // never drives either tranche's deposited below zero.
    for loss in [0i128, 500, 1_000, 1_500, 2_000, 2_500] {
        let (env, contract_id, token, pool_id, payer) = setup_pool_with_fees(0, 0, 0, 0);
        let client = RiskPoolClient::new(&env, &contract_id);
        let second = random_address(&env);
        token::StellarAssetClient::new(&env, &token).mint(&second, &1_000_000);

        client.deposit(&pool_id, &Tier::Junior, &payer, &1_000);
        client.deposit(&pool_id, &Tier::Senior, &second, &1_000);
        let season_id = client.open_season(&pool_id);
        client.activate_season(&pool_id, &season_id);
        client.close_season(&pool_id, &season_id);
        // premiums_in is 0, so payouts_paid becomes a pure loss.
        set_payouts_paid(&env, &contract_id, pool_id, season_id, loss);

        let net = client.settle_season(&pool_id, &season_id);
        assert_eq!(net, -loss);

        let expected_junior = (1_000 - loss).max(0);
        let expected_senior = (2_000 - loss).clamp(0, 1_000);
        assert_eq!(
            client.tranche(&pool_id, &Tier::Junior).deposited,
            expected_junior
        );
        assert_eq!(
            client.tranche(&pool_id, &Tier::Senior).deposited,
            expected_senior
        );
    }
}
