#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn constructor_sets_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(TriggerEngine, (admin.clone(),));
    let client = TriggerEngineClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::IndexNotFound as u32, 400);
    assert_eq!(Error::StaleIndex as u32, 401);
    assert_eq!(Error::AlreadyFinalized as u32, 402);
    assert_eq!(Error::NotTriggered as u32, 403);
    assert_eq!(Error::NonDeterministicInput as u32, 404);
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
