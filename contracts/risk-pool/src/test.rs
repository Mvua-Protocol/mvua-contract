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
