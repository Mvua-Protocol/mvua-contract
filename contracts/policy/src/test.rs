#![cfg(test)]
use super::*;
use soroban_sdk::{symbol_short, token, Address, BytesN, Vec};
use test_utils::{advance_time, new_env, random_address};

use risk_pool::{
    FeeConfig, PoolConfig, PremiumParams, RiskPool, RiskPoolClient as PoolClient, TrancheConfig,
};

// One whole coverage unit priced under the pool curve below: base 100 bps plus
// 10 bps of slope for the single unit gives 110 bps, so 10_000_000 * 110 /
// 10_000 = 110_000.
const COVERAGE: i128 = 10_000_000;
const EXPECTED_PREMIUM: i128 = 110_000;
const WINDOW_START: u64 = 1_000;
const WINDOW_END: u64 = 2_000;

// Wire a risk-pool and a policy contract together: create a no-fee pool, deploy
// the policy pointed at it, authorize policy to request refunds, open and
// activate a season, and fund a buyer. Returns env, pool contract id, policy
// contract id, token id, pool id, season id, and the buyer.
fn setup() -> (Env, Address, Address, Address, u64, u64, Address) {
    let env = new_env();
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

    let pol_id = env.register(Policy, (admin.clone(), rp_id.clone()));
    rp.set_policy_contract(&pol_id);

    let season_id = rp.open_season(&pool_id);
    rp.activate_season(&pool_id, &season_id);

    let buyer = random_address(&env);
    token::StellarAssetClient::new(&env, &token).mint(&buyer, &1_000_000);
    (env, rp_id, pol_id, token, pool_id, season_id, buyer)
}

// Mint a standard policy for `buyer` over the active season, paying exactly the
// quoted premium. Returns the new policy id.
fn mint_policy(env: &Env, pol_id: &Address, buyer: &Address, pool_id: u64, season_id: u64) -> u64 {
    let pol = PolicyClient::new(env, pol_id);
    let metadata = BytesN::from_array(env, &[7u8; 32]);
    pol.mint(buyer, &pool_id, &season_id, &standard_terms(), &EXPECTED_PREMIUM, &metadata)
}

// The reference set of policy terms used across the tests: one coverage unit in
// region KE_NAK over the `[WINDOW_START, WINDOW_END)` window.
fn standard_terms() -> PolicyTerms {
    PolicyTerms {
        region: symbol_short!("KE_NAK"),
        coverage: COVERAGE,
        index_ref: 1,
        window_start: WINDOW_START,
        window_end: WINDOW_END,
        severity_curve: 5_000,
    }
}

#[test]
fn constructor_sets_admin_and_risk_pool() {
    let (env, rp_id, pol_id, _token, _pool_id, _season_id, _buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    assert_eq!(pol.risk_pool(), rp_id);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::PolicyNotFound as u32, 200);
    assert_eq!(Error::InvalidState as u32, 201);
    assert_eq!(Error::WindowStarted as u32, 202);
    assert_eq!(Error::QuoteMismatch as u32, 204);
    assert_eq!(Error::InvalidCoverage as u32, 205);
    assert_eq!(Error::InvalidWindow as u32, 206);
    assert_eq!(Error::NotExpired as u32, 207);
    assert_eq!(
        Error::Unauthorized as u32,
        common::error_codes::UNAUTHORIZED
    );
    assert_eq!(Error::Overflow as u32, common::error_codes::OVERFLOW);
}

#[test]
fn quote_proxies_the_pool_curve() {
    let (env, _rp_id, pol_id, _token, pool_id, _season_id, _buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    assert_eq!(pol.quote(&pool_id, &COVERAGE), EXPECTED_PREMIUM);
}
#[test]
fn mint_creates_active_policy_and_collects_premium() {
    let (env, rp_id, pol_id, token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let rp = PoolClient::new(&env, &rp_id);
    let tok = token::TokenClient::new(&env, &token);

    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);
    assert_eq!(policy_id, 1);

    let record = pol.policy(&policy_id);
    assert_eq!(record.owner, buyer);
    assert_eq!(record.pool_id, pool_id);
    assert_eq!(record.season_id, season_id);
    assert_eq!(record.coverage, COVERAGE);
    assert_eq!(record.premium_paid, EXPECTED_PREMIUM);
    // No fees on this pool, so the whole gross is reserved and refundable.
    assert_eq!(record.net_reserved, EXPECTED_PREMIUM);
    assert_eq!(record.state, PolicyState::Active);

    // Premium moved from buyer into the pool's season and reserves.
    assert_eq!(tok.balance(&buyer), 1_000_000 - EXPECTED_PREMIUM);
    assert_eq!(
        rp.season(&pool_id, &season_id).premiums_in,
        EXPECTED_PREMIUM
    );
    assert_eq!(rp.solvency(&pool_id).reserves, EXPECTED_PREMIUM);

    // Metadata pointer is stored opaquely and read back verbatim.
    assert_eq!(
        pol.metadata(&policy_id),
        BytesN::from_array(&env, &[7u8; 32])
    );
}

#[test]
fn mint_rejects_when_quote_exceeds_max_premium() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let metadata = BytesN::from_array(&env, &[7u8; 32]);
    let result = pol.try_mint(
        &buyer,
        &pool_id,
        &season_id,
        &standard_terms(),
        &(EXPECTED_PREMIUM - 1),
        &metadata,
    );
    assert_eq!(result, Err(Ok(Error::QuoteMismatch)));
}

#[test]
fn mint_rejects_bad_coverage_and_window() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let metadata = BytesN::from_array(&env, &[7u8; 32]);

    let zero_coverage = PolicyTerms {
        coverage: 0,
        ..standard_terms()
    };
    let bad_coverage = pol.try_mint(
        &buyer,
        &pool_id,
        &season_id,
        &zero_coverage,
        &EXPECTED_PREMIUM,
        &metadata,
    );
    assert_eq!(bad_coverage, Err(Ok(Error::InvalidCoverage)));

    let inverted_window = PolicyTerms {
        window_start: WINDOW_END,
        window_end: WINDOW_START,
        ..standard_terms()
    };
    let bad_window = pol.try_mint(
        &buyer,
        &pool_id,
        &season_id,
        &inverted_window,
        &EXPECTED_PREMIUM,
        &metadata,
    );
    assert_eq!(bad_window, Err(Ok(Error::InvalidWindow)));
}
#[test]
fn lifecycle_runs_active_triggered_paid() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    pol.mark_triggered(&policy_id);
    assert_eq!(pol.policy(&policy_id).state, PolicyState::Triggered);
    pol.mark_paid(&policy_id);
    assert_eq!(pol.policy(&policy_id).state, PolicyState::Paid);
}

#[test]
fn illegal_transition_is_rejected() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);
    // Cannot mark an Active policy Paid without first triggering it.
    assert_eq!(pol.try_mark_paid(&policy_id), Err(Ok(Error::InvalidState)));
}

#[test]
fn expire_requires_the_window_to_have_ended() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    // Before window_end the policy cannot expire.
    assert_eq!(pol.try_expire(&policy_id), Err(Ok(Error::NotExpired)));

    advance_time(&env, WINDOW_END);
    pol.expire(&policy_id);
    assert_eq!(pol.policy(&policy_id).state, PolicyState::Expired);
}

#[test]
fn cancel_refunds_net_and_marks_cancelled() {
    let (env, rp_id, pol_id, token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let rp = PoolClient::new(&env, &rp_id);
    let tok = token::TokenClient::new(&env, &token);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    let refunded = pol.cancel(&policy_id);
    assert_eq!(refunded, EXPECTED_PREMIUM);
    assert_eq!(pol.policy(&policy_id).state, PolicyState::Cancelled);

    // The buyer is made whole and the pool accounting is reversed.
    assert_eq!(tok.balance(&buyer), 1_000_000);
    assert_eq!(rp.season(&pool_id, &season_id).premiums_in, 0);
    assert_eq!(rp.solvency(&pool_id).reserves, 0);
}

#[test]
fn cancel_is_blocked_once_the_window_opens() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    advance_time(&env, WINDOW_START);
    assert_eq!(pol.try_cancel(&policy_id), Err(Ok(Error::WindowStarted)));
}

#[test]
fn cancel_rejects_a_non_active_policy() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    pol.cancel(&policy_id);
    // A second cancel finds the policy already Cancelled.
    assert_eq!(pol.try_cancel(&policy_id), Err(Ok(Error::InvalidState)));
}

#[test]
fn set_metadata_updates_the_pointer() {
    let (env, _rp_id, pol_id, _token, pool_id, season_id, buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    let policy_id = mint_policy(&env, &pol_id, &buyer, pool_id, season_id);

    let updated = BytesN::from_array(&env, &[9u8; 32]);
    pol.set_metadata(&policy_id, &updated);
    assert_eq!(pol.metadata(&policy_id), updated);
}

#[test]
fn policy_and_metadata_report_missing_ids() {
    let (env, _rp_id, pol_id, _token, _pool_id, _season_id, _buyer) = setup();
    let pol = PolicyClient::new(&env, &pol_id);
    assert_eq!(pol.try_policy(&999), Err(Ok(Error::PolicyNotFound)));
    assert_eq!(pol.try_metadata(&999), Err(Ok(Error::PolicyNotFound)));
}
