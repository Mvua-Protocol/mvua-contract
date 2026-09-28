//! Full season integration test (P1.8.4).
//!
//! Registers all five real contracts on one Soroban test host, wires them as a
//! testnet deployment would, and drives a single rainfall season from capital
//! and premium intake through signed observations, the on chain oracle median
//! feed, evaluation, finalization, batch payout, and claim. Ledger time is
//! advanced to clear the publisher registry timelock and the trigger challenge
//! window, which a real testnet run waits out in wall clock time.

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{
    symbol_short, testutils::Address as _, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol,
    Vec,
};
use std::vec::Vec as StdVec;

use oracle_adapter::{ObservationPayload, OracleAdapter, OracleAdapterClient};
use payout_vault::{PayoutEntry, PayoutVault, PayoutVaultClient};
use policy::{Policy, PolicyClient, PolicyState, PolicyTerms};
use risk_pool::{
    FeeConfig, PoolConfig, PremiumParams, RiskPool, RiskPoolClient, Tier, TrancheConfig,
};
use test_utils::advance_time;
use trigger_engine::{IndexKind, IndexParams, TriggerEngine, TriggerEngineClient, TriggerStatus};

// Registry timelock and challenge window, mirrored from the contracts so the
// test advances past exactly the delays a testnet deployment enforces.
const TIMELOCK: u64 = 7 * 24 * 60 * 60;
const CHALLENGE: u64 = 24 * 60 * 60;

// One coverage unit at the settlement token's seven decimals.
const COVERAGE: i128 = 10_000_000;
// Junior capital seeded into the pool, comfortably above the largest payout.
const RESERVES: i128 = 40_000_000;

// The index region and metric. Observations must be submitted under the same
// pair for the trigger engine's `record_day_from_oracle` to read them.
fn region() -> Symbol {
    symbol_short!("KE_NAK")
}
fn metric() -> Symbol {
    symbol_short!("rain_mm")
}

// A deterministic ed25519 key from one seed byte, so publishers are reproducible
// and distinct, exactly as oracle-adapter's own tests build them.
fn signer(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey(env: &Env, sk: &SigningKey) -> BytesN<32> {
    BytesN::from_array(env, &sk.verifying_key().to_bytes())
}

// Sign the XDR of the exact payload the oracle reconstructs in `submit`.
fn sign(env: &Env, sk: &SigningKey, timestamp: u64, value: i128) -> BytesN<64> {
    let payload = ObservationPayload {
        region: region(),
        metric: metric(),
        timestamp,
        value,
    };
    let message: Bytes = payload.to_xdr(env);
    let bytes: StdVec<u8> = message.iter().collect();
    let sig = sk.sign(&bytes);
    BytesN::from_array(env, &sig.to_bytes())
}

// A rainfall index matching the golden-vector baseline (INDEX-SPEC section 8):
// baseline 800 over a four day window, trigger 7500 bps, exhaustion 4000 bps.
fn rainfall_params(env: &Env) -> IndexParams {
    IndexParams {
        kind: IndexKind::RainfallShortfall,
        region: region(),
        metric: metric(),
        window_len: 4,
        baseline: 800,
        trigger_ratio_bps: 7500,
        exhaustion_ratio_bps: 4000,
        trigger_days: 0,
        exhaustion_days: 0,
        dry_floor: 0,
        data_source_ref: BytesN::from_array(env, &[7u8; 32]),
    }
}

// A no-fee pool over the index region, denominated in the given token.
fn pool_config(env: &Env, token: &Address) -> PoolConfig {
    let mut regions = Vec::new(env);
    regions.push_back(region());
    PoolConfig {
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
    }
}

// The fully wired protocol on one test host, plus the identifiers a season
// needs. Clients are built on demand so each borrows the shared env.
struct World {
    env: Env,
    rp_id: Address,
    policy_id: Address,
    oracle_id: Address,
    te_id: Address,
    vault_id: Address,
    token: Address,
    pool_id: u64,
    season_id: u64,
}

impl World {
    fn rp(&self) -> RiskPoolClient<'_> {
        RiskPoolClient::new(&self.env, &self.rp_id)
    }
    fn policy(&self) -> PolicyClient<'_> {
        PolicyClient::new(&self.env, &self.policy_id)
    }
    fn oracle(&self) -> OracleAdapterClient<'_> {
        OracleAdapterClient::new(&self.env, &self.oracle_id)
    }
    fn te(&self) -> TriggerEngineClient<'_> {
        TriggerEngineClient::new(&self.env, &self.te_id)
    }
    fn vault(&self) -> PayoutVaultClient<'_> {
        PayoutVaultClient::new(&self.env, &self.vault_id)
    }
}

// Deploy and wire all five contracts, create a no-fee pool over the index
// region, fund its reserves with a junior deposit, and open and activate one
// season. Mirrors the wiring a testnet deploy script performs:
// `set_policy_contract` and `set_payout_vault` on risk-pool, `set_oracle` on
// trigger-engine, all with a shared guardian admin.
fn deploy_world() -> World {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();

    let rp_id = env.register(RiskPool, (admin.clone(),));
    let policy_id = env.register(Policy, (admin.clone(), rp_id.clone()));
    let oracle_id = env.register(OracleAdapter, (admin.clone(),));
    let te_id = env.register(TriggerEngine, (admin.clone(),));
    let vault_id = env.register(
        PayoutVault,
        (
            admin.clone(),
            rp_id.clone(),
            policy_id.clone(),
            te_id.clone(),
            token.clone(),
        ),
    );

    let rp = RiskPoolClient::new(&env, &rp_id);
    rp.set_policy_contract(&policy_id);
    rp.set_payout_vault(&vault_id);
    TriggerEngineClient::new(&env, &te_id).set_oracle(&oracle_id);

    let pool_id = rp.create_pool(&pool_config(&env, &token));

    // Fund reserves: a junior depositor supplies the capital payouts draw from.
    let depositor = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token).mint(&depositor, &RESERVES);
    rp.deposit(&pool_id, &Tier::Junior, &depositor, &RESERVES);

    let season_id = rp.open_season(&pool_id);
    rp.activate_season(&pool_id, &season_id);

    World {
        env,
        rp_id,
        policy_id,
        oracle_id,
        te_id,
        vault_id,
        token,
        pool_id,
        season_id,
    }
}

// Drive two publisher keys through one shared registry timelock so both are
// active. The oracle median needs at least two distinct publishers, so a single
// season always registers a pair. Proposing both before the single time advance
// keeps the ledger clock at one timelock rather than two.
fn register_publishers(w: &World, a: &BytesN<32>, b: &BytesN<32>) {
    let oracle = w.oracle();
    oracle.propose_publisher(a, &true);
    oracle.propose_publisher(b, &true);
    advance_time(&w.env, TIMELOCK + 1);
    oracle.execute_publisher(a);
    oracle.execute_publisher(b);
}

// Submit one observation of `value` from each of two publishers at the current
// ledger time, signed over the payload the oracle reconstructs. The median over
// two equal values is that value, satisfying the two-publisher quorum.
fn report(w: &World, sk_a: &SigningKey, sk_b: &SigningKey, value: i128) {
    let pk_a = pubkey(&w.env, sk_a);
    let pk_b = pubkey(&w.env, sk_b);
    let ts = w.env.ledger().timestamp();
    let oracle = w.oracle();
    oracle.submit(
        &region(),
        &metric(),
        &ts,
        &value,
        &pk_a,
        &sign(&w.env, sk_a, ts, value),
    );
    oracle.submit(
        &region(),
        &metric(),
        &ts,
        &value,
        &pk_b,
        &sign(&w.env, sk_b, ts, value),
    );
}

// A farmer buys one coverage unit against `index_id` for the active season. The
// premium is quoted, funded, and collected through the real policy path.
fn buy_policy(w: &World, farmer: &Address, index_id: u64) -> u64 {
    let premium = w.policy().quote(&w.pool_id, &COVERAGE);
    token::StellarAssetClient::new(&w.env, &w.token).mint(farmer, &premium);
    let now = w.env.ledger().timestamp();
    let terms = PolicyTerms {
        region: region(),
        coverage: COVERAGE,
        index_ref: index_id,
        window_start: now,
        window_end: now + 100,
        severity_curve: 0,
    };
    w.policy().mint(
        farmer,
        &w.pool_id,
        &w.season_id,
        &terms,
        &premium,
        &BytesN::from_array(&w.env, &[0u8; 32]),
    )
}

// The whole protocol, end to end: capital in, a policy sold, a signed drought
// observed, the index fed from the oracle on chain, a breach finalized after the
// challenge window, the payout drawn from the pool under solvency, and the
// farmer's claim collected, with the season and policy reconciling.
#[test]
fn full_rainfall_season_pays_and_claims() {
    let w = deploy_world();

    let sk_a = signer(1);
    let sk_b = signer(2);
    register_publishers(&w, &pubkey(&w.env, &sk_a), &pubkey(&w.env, &sk_b));

    // Define the rainfall index (id 0) the season underwrites.
    let index_id = w.te().create_index(&rainfall_params(&w.env));

    let farmer = Address::generate(&w.env);
    let policy_id = buy_policy(&w, &farmer, index_id);

    // Both publishers report drought-level daily rainfall (130 mm). Median 130.
    report(&w, &sk_a, &sk_b, 130);

    // Record four days from the oracle: sum 520 over baseline 800 = ratio 6500.
    for day in 1..=4u64 {
        assert_eq!(w.te().record_day_from_oracle(&index_id, &0, &day), 130);
    }

    // Evaluate: the window breaches and enters Triggered.
    let triggered = w.te().evaluate(&index_id, &0);
    assert!(matches!(triggered, TriggerStatus::Triggered(_)));

    // Finalization waits out the challenge window, then locks severity 2857.
    advance_time(&w.env, CHALLENGE + 1);
    let at = w.env.ledger().timestamp();
    assert_eq!(
        w.te().finalize(&index_id, &0),
        TriggerStatus::Finalized(at, 2857)
    );

    // Severity 2857 bps of one coverage unit, rounded down, favors the pool.
    let expected = COVERAGE * 2857 / 10_000;
    assert_eq!(w.te().payout_for(&index_id, &0, &COVERAGE), expected);

    // The guardian records the payout batch. The vault confirms finalization on
    // chain through payout_for, then draws the funds from the pool.
    let mut entries = Vec::new(&w.env);
    entries.push_back(PayoutEntry {
        policy_id,
        owner: farmer.clone(),
        coverage: COVERAGE,
    });
    assert_eq!(
        w.vault()
            .pay_batch(&index_id, &0, &w.pool_id, &w.season_id, &entries),
        1
    );
    w.vault().close_batch(&index_id, &0);

    // The claim is accrued but not yet collected.
    let token_client = token::TokenClient::new(&w.env, &w.token);
    assert_eq!(w.vault().claimable(&farmer), expected);
    assert_eq!(token_client.balance(&farmer), 0);

    // Claiming pushes the funds to the farmer and zeroes the ledger.
    assert_eq!(w.vault().claim(&farmer), expected);
    assert_eq!(token_client.balance(&farmer), expected);
    assert_eq!(w.vault().claimable(&farmer), 0);

    // The season and pool reconcile: the payout is booked and reserves fell by
    // exactly the amount paid (junior capital plus net premium, less payout).
    let season = w.rp().season(&w.pool_id, &w.season_id);
    assert_eq!(season.payouts_paid, expected);
    assert_eq!(
        w.rp().solvency(&w.pool_id).reserves,
        RESERVES + season.premiums_in - expected
    );

    // The policy lifecycle completes: guardian marks it triggered, then paid.
    w.policy().mark_triggered(&policy_id);
    w.policy().mark_paid(&policy_id);
    assert_eq!(w.policy().policy(&policy_id).state, PolicyState::Paid);
}

// A season whose rainfall never falls to the trigger stays healthy, and the
// on chain finalization gate blocks any payout: pay_batch traps because the
// window was never finalized, so no funds can leave the pool.
#[test]
fn healthy_season_blocks_payout() {
    let w = deploy_world();

    let sk_a = signer(3);
    let sk_b = signer(4);
    register_publishers(&w, &pubkey(&w.env, &sk_a), &pubkey(&w.env, &sk_b));

    let index_id = w.te().create_index(&rainfall_params(&w.env));
    let farmer = Address::generate(&w.env);
    let policy_id = buy_policy(&w, &farmer, index_id);

    // Ample rainfall (250 mm/day, sum 1000 over baseline 800): no breach.
    report(&w, &sk_a, &sk_b, 250);
    for day in 1..=4u64 {
        w.te().record_day_from_oracle(&index_id, &0, &day);
    }
    assert_eq!(w.te().evaluate(&index_id, &0), TriggerStatus::Healthy);

    // Attempting a payout on an unfinalized window traps in payout_for and
    // reverts the whole batch, so the pool's reserves are never touched.
    let mut entries = Vec::new(&w.env);
    entries.push_back(PayoutEntry {
        policy_id,
        owner: farmer.clone(),
        coverage: COVERAGE,
    });
    assert!(w
        .vault()
        .try_pay_batch(&index_id, &0, &w.pool_id, &w.season_id, &entries)
        .is_err());
    assert_eq!(w.vault().claimable(&farmer), 0);

    // No payout was booked and reserves hold exactly the junior capital plus the
    // net premium the policy paid in: the failed batch drew nothing from the pool.
    let season = w.rp().season(&w.pool_id, &w.season_id);
    assert_eq!(season.payouts_paid, 0);
    assert_eq!(
        w.rp().solvency(&w.pool_id).reserves,
        RESERVES + season.premiums_in
    );
}
