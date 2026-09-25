#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn constructor_sets_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(OracleAdapter, (admin.clone(),));
    let client = OracleAdapterClient::new(&env, &id);
    assert_eq!(client.admin(), admin);
}

#[test]
fn error_codes_match_contract_range() {
    assert_eq!(Error::UnknownPublisher as u32, 300);
    assert_eq!(Error::BadSignature as u32, 301);
    assert_eq!(Error::ObservationStale as u32, 302);
    assert_eq!(Error::Challenged as u32, 303);
    assert_eq!(Error::RegistryTimelock as u32, 304);
    assert_eq!(
        Error::Unauthorized as u32,
        common::error_codes::UNAUTHORIZED
    );
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
