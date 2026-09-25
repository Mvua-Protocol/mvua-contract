#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn constructor_sets_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(Policy, (admin.clone(),));
    let client = PolicyClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::PolicyNotFound as u32, 200);
    assert_eq!(Error::InvalidState as u32, 201);
    assert_eq!(Error::WindowStarted as u32, 202);
    assert_eq!(Error::NotTransferable as u32, 203);
    assert_eq!(Error::QuoteMismatch as u32, 204);
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
