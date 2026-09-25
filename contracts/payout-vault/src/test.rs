#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn constructor_sets_admin_and_starts_unpaused() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let id = env.register(PayoutVault, (admin.clone(), pool));
    let client = PayoutVaultClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
    assert!(!client.is_paused());
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::Paused as u32, 500);
    assert_eq!(Error::BatchComplete as u32, 501);
    assert_eq!(Error::NoFinalizedTrigger as u32, 502);
    assert_eq!(Error::PayoutExists as u32, 503);
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
