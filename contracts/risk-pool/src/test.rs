#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn constructor_sets_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(RiskPool, (admin.clone(),));
    let client = RiskPoolClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::PoolNotFound as u32, 100);
    assert_eq!(Error::InsufficientReserves as u32, 101);
    assert_eq!(Error::SolvencyViolated as u32, 102);
    assert_eq!(Error::FeeCapExceeded as u32, 105);
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
