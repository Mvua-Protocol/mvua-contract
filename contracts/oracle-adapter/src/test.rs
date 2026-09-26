#![cfg(test)]
extern crate std;

use super::*;
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{symbol_short, testutils::Address as _, xdr::ToXdr, Address, Bytes, Env};
use std::vec::Vec as StdVec;
use test_utils::{advance_time, new_env};

const TIMELOCK: u64 = 7 * 24 * 60 * 60;

// A deterministic signing key from a single seed byte, so tests are
// reproducible and each seed yields a distinct publisher.
fn signer(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey(env: &Env, sk: &SigningKey) -> BytesN<32> {
    BytesN::from_array(env, &sk.verifying_key().to_bytes())
}

// Sign the XDR of the exact payload the contract reconstructs in `submit`.
fn sign(
    env: &Env,
    sk: &SigningKey,
    region: &Symbol,
    metric: &Symbol,
    timestamp: u64,
    value: i128,
) -> BytesN<64> {
    let payload = ObservationPayload {
        region: region.clone(),
        metric: metric.clone(),
        timestamp,
        value,
    };
    let message: Bytes = payload.to_xdr(env);
    let bytes: StdVec<u8> = message.iter().collect();
    let sig = sk.sign(&bytes);
    BytesN::from_array(env, &sig.to_bytes())
}

fn setup() -> (Env, Address, Address) {
    let env = new_env();
    let admin = Address::generate(&env);
    let id = env.register(OracleAdapter, (admin.clone(),));
    (env, admin, id)
}

// Drive a publisher addition through the full timelock so it is active.
fn register(env: &Env, client: &OracleAdapterClient, pk: &BytesN<32>) {
    client.propose_publisher(pk, &true);
    advance_time(env, TIMELOCK + 1);
    client.execute_publisher(pk);
}

#[test]
fn constructor_sets_admin_and_default_staleness() {
    let (env, admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
    assert_eq!(client.staleness_bound(), 48);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::UnknownPublisher as u32, 300);
    assert_eq!(Error::BadSignature as u32, 301);
    assert_eq!(Error::ObservationStale as u32, 302);
    assert_eq!(Error::Challenged as u32, 303);
    assert_eq!(Error::RegistryTimelock as u32, 304);
    assert_eq!(Error::PublisherExists as u32, 305);
    assert_eq!(Error::NoPendingChange as u32, 306);
    assert_eq!(Error::PendingExists as u32, 307);
    assert_eq!(Error::InvalidObservation as u32, 308);
    assert_eq!(Error::Unauthorized as u32, common::error_codes::UNAUTHORIZED);
    assert_eq!(
        Error::TimelockPending as u32,
        common::error_codes::TIMELOCK_PENDING
    );
    assert_eq!(
        Error::NotInitialized as u32,
        common::error_codes::NOT_INITIALIZED
    );
    assert_eq!(Error::Overflow as u32, common::error_codes::OVERFLOW);
}

#[test]
fn timelock_gates_registration() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(1));

    let eta = client.propose_publisher(&pk, &true);
    assert_eq!(eta, env.ledger().timestamp() + TIMELOCK);
    // Executing before the eta is rejected.
    assert_eq!(
        client.try_execute_publisher(&pk),
        Err(Ok(Error::RegistryTimelock))
    );
    assert!(!client.is_publisher(&pk));

    advance_time(&env, TIMELOCK + 1);
    client.execute_publisher(&pk);
    assert!(client.is_publisher(&pk));
    // The pending record is consumed.
    assert_eq!(
        client.try_pending_publisher(&pk),
        Err(Ok(Error::NoPendingChange))
    );
}

#[test]
fn propose_add_existing_is_rejected() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(2));
    register(&env, &client, &pk);
    assert_eq!(
        client.try_propose_publisher(&pk, &true),
        Err(Ok(Error::PublisherExists))
    );
}

#[test]
fn propose_remove_unknown_is_rejected() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(3));
    assert_eq!(
        client.try_propose_publisher(&pk, &false),
        Err(Ok(Error::UnknownPublisher))
    );
}

#[test]
fn second_proposal_is_rejected_while_pending() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(4));
    client.propose_publisher(&pk, &true);
    assert_eq!(
        client.try_propose_publisher(&pk, &true),
        Err(Ok(Error::PendingExists))
    );
}

#[test]
fn cancel_clears_pending() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(5));
    client.propose_publisher(&pk, &true);
    client.cancel_publisher(&pk);
    assert_eq!(
        client.try_pending_publisher(&pk),
        Err(Ok(Error::NoPendingChange))
    );
    // Cancelling with nothing pending is rejected.
    assert_eq!(
        client.try_cancel_publisher(&pk),
        Err(Ok(Error::NoPendingChange))
    );
    // Executing with nothing pending is rejected.
    assert_eq!(
        client.try_execute_publisher(&pk),
        Err(Ok(Error::NoPendingChange))
    );
}

#[test]
fn removal_deactivates_but_keeps_history() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let pk = pubkey(&env, &signer(6));
    register(&env, &client, &pk);

    client.propose_publisher(&pk, &false);
    advance_time(&env, TIMELOCK + 1);
    client.execute_publisher(&pk);

    assert!(!client.is_publisher(&pk));
    // The record is retained so past observations keep a resolvable author.
    let info = client.publisher_info(&pk);
    assert!(!info.active);
}

#[test]
fn submit_by_unknown_publisher_is_rejected() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk = signer(7);
    let pk = pubkey(&env, &sk);
    let ts = env.ledger().timestamp();
    let sig = sign(&env, &sk, &region, &metric, ts, 100);
    assert_eq!(
        client.try_submit(&region, &metric, &ts, &100, &pk, &sig),
        Err(Ok(Error::UnknownPublisher))
    );
}

#[test]
fn submit_stores_observation_and_advances_ring() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk = signer(8);
    let pk = pubkey(&env, &sk);
    register(&env, &client, &pk);

    let ts = env.ledger().timestamp();
    let sig = sign(&env, &sk, &region, &metric, ts, 125);
    let slot = client.submit(&region, &metric, &ts, &125, &pk, &sig);
    assert_eq!(slot, 0);

    let head = client.head(&region, &metric);
    assert_eq!(head.next_seq, 1);
    assert_eq!(head.count, 1);

    let obs = client.observation(&region, &metric, &0);
    assert_eq!(obs.value, 125);
    assert_eq!(obs.publisher, pk);
    assert!(obs.accepted);
}

#[test]
fn submit_rejects_invalid_value_and_future_timestamp() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk = signer(9);
    let pk = pubkey(&env, &sk);
    register(&env, &client, &pk);

    let ts = env.ledger().timestamp();
    let neg = sign(&env, &sk, &region, &metric, ts, -1);
    assert_eq!(
        client.try_submit(&region, &metric, &ts, &-1, &pk, &neg),
        Err(Ok(Error::InvalidObservation))
    );

    let future = ts + 10;
    let sig = sign(&env, &sk, &region, &metric, future, 10);
    assert_eq!(
        client.try_submit(&region, &metric, &future, &10, &pk, &sig),
        Err(Ok(Error::InvalidObservation))
    );
}

#[test]
fn forged_signature_traps() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk = signer(10);
    let pk = pubkey(&env, &sk);
    register(&env, &client, &pk);

    let ts = env.ledger().timestamp();
    // Sign with a different key than the claimed publisher.
    let forged = sign(&env, &signer(11), &region, &metric, ts, 50);
    // A forged signature traps in ed25519_verify (host error, not a contract
    // error), so try_submit returns the outer Err.
    assert!(client.try_submit(&region, &metric, &ts, &50, &pk, &forged).is_err());
}

// Sign and submit a value from `sk` at the current ledger time. Returns slot.
fn submit_from(
    env: &Env,
    client: &OracleAdapterClient,
    sk: &SigningKey,
    region: &Symbol,
    metric: &Symbol,
    value: i128,
) -> u32 {
    let pk = pubkey(env, sk);
    let ts = env.ledger().timestamp();
    let sig = sign(env, sk, region, metric, ts, value);
    client.submit(region, metric, &ts, &value, &pk, &sig)
}

#[test]
fn median_requires_min_distinct_publishers() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk1 = signer(20);
    let sk2 = signer(21);
    // Register both up front so neither observation ages past staleness while
    // the other publisher clears its own registration timelock.
    register(&env, &client, &pubkey(&env, &sk1));
    register(&env, &client, &pubkey(&env, &sk2));

    // One publisher is not enough: the index stays stale.
    submit_from(&env, &client, &sk1, &region, &metric, 100);
    assert_eq!(
        client.try_median(&region, &metric),
        Err(Ok(Error::ObservationStale))
    );

    // A second distinct publisher lifts the gate.
    submit_from(&env, &client, &sk2, &region, &metric, 200);
    assert_eq!(client.median(&region, &metric), 150);
}

#[test]
fn median_of_odd_count_is_the_middle() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk1 = signer(30);
    let sk2 = signer(31);
    let sk3 = signer(32);
    register(&env, &client, &pubkey(&env, &sk1));
    register(&env, &client, &pubkey(&env, &sk2));
    register(&env, &client, &pubkey(&env, &sk3));

    submit_from(&env, &client, &sk1, &region, &metric, 10);
    submit_from(&env, &client, &sk2, &region, &metric, 30);
    submit_from(&env, &client, &sk3, &region, &metric, 20);
    assert_eq!(client.median(&region, &metric), 20);
}

#[test]
fn challenge_excludes_slot_from_median() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk1 = signer(40);
    let sk2 = signer(41);
    let sk3 = signer(42);
    register(&env, &client, &pubkey(&env, &sk1));
    register(&env, &client, &pubkey(&env, &sk2));
    register(&env, &client, &pubkey(&env, &sk3));

    submit_from(&env, &client, &sk1, &region, &metric, 10);
    submit_from(&env, &client, &sk2, &region, &metric, 20);
    let outlier = submit_from(&env, &client, &sk3, &region, &metric, 30);
    assert_eq!(client.median(&region, &metric), 20);

    // Flagging the outlier drops it: [10, 20] over two publishers averages 15.
    client.challenge(&region, &metric, &outlier, &1);
    assert!(client.is_challenged(&region, &metric, &outlier));
    assert_eq!(client.median(&region, &metric), 15);

    // Re-challenging is rejected; clearing restores the value.
    assert_eq!(
        client.try_challenge(&region, &metric, &outlier, &1),
        Err(Ok(Error::Challenged))
    );
    client.unchallenge(&region, &metric, &outlier);
    assert_eq!(client.median(&region, &metric), 20);
}

#[test]
fn challenge_unknown_slot_is_rejected() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    assert_eq!(
        client.try_challenge(&region, &metric, &0, &1),
        Err(Ok(Error::InvalidObservation))
    );
}

#[test]
fn stale_observations_are_excluded() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    let sk1 = signer(50);
    let sk2 = signer(51);
    register(&env, &client, &pubkey(&env, &sk1));
    register(&env, &client, &pubkey(&env, &sk2));

    submit_from(&env, &client, &sk1, &region, &metric, 10);
    submit_from(&env, &client, &sk2, &region, &metric, 20);
    assert_eq!(client.median(&region, &metric), 15);

    // Past the 48h staleness bound, both drop out and the index is stale.
    advance_time(&env, 49 * 3_600);
    assert_eq!(
        client.try_median(&region, &metric),
        Err(Ok(Error::ObservationStale))
    );
}

#[test]
fn median_with_no_observations_is_stale() {
    let (env, _admin, id) = setup();
    let client = OracleAdapterClient::new(&env, &id);
    let region = symbol_short!("nairobi");
    let metric = symbol_short!("rain_mm");
    assert_eq!(
        client.try_median(&region, &metric),
        Err(Ok(Error::ObservationStale))
    );
}





