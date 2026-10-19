#![cfg(test)]
use super::admin::*;
use crate::errors::ErrorCode;
use crate::{PredictIQ, PredictIQClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup() -> (Env, Address) {
    let e = Env::default();
    e.mock_all_auths();
    let contract_id = e.register_contract(None, PredictIQ);
    (e, contract_id)
}

#[test]
fn test_set_and_get_admin() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        let stored_admin = get_admin(&e).unwrap();
        assert_eq!(stored_admin, admin);
    });
}

#[test]
fn test_require_admin_success() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        let result = require_admin(&e);
        assert!(result.is_ok());
    });
}

#[test]
fn test_require_admin_not_set() {
    let (e, contract_id) = setup();

    e.as_contract(&contract_id, || {
        let result = require_admin(&e);
        assert_eq!(result, Err(ErrorCode::NotAuthorized));
    });
}

#[test]
fn test_set_and_get_guardian() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();
        let stored_guardian = get_guardian(&e).unwrap();
        assert_eq!(stored_guardian, guardian);
    });
}

#[test]
fn test_require_guardian_success() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();
        let result = require_guardian(&e);
        assert!(result.is_ok());
    });
}

#[test]
fn test_require_guardian_not_set() {
    let (e, contract_id) = setup();

    e.as_contract(&contract_id, || {
        let result = require_guardian(&e);
        assert_eq!(result, Err(ErrorCode::NotAuthorized));
    });
}

#[test]
fn test_admin_can_change_guardian() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian1 = Address::generate(&e);
    let guardian2 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian1.clone()).unwrap();
        assert_eq!(get_guardian(&e).unwrap(), guardian1);
        set_guardian(&e, guardian2.clone()).unwrap();
        assert_eq!(get_guardian(&e).unwrap(), guardian2);
    });
}

#[test]
fn test_admin_and_guardian_are_independent() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();
        assert_eq!(get_admin(&e).unwrap(), admin);
        assert_eq!(get_guardian(&e).unwrap(), guardian);
        assert_ne!(admin, guardian);
    });
}

// --- Separation of powers: admin-transfer path (issue #1538) ---
//
// `governance::initialize_guardians` and `add_guardian` reject a guardian
// address that equals the current Admin. The admin-transfer path
// (`propose_admin` / `accept_admin`) must enforce the symmetric invariant:
// an incoming admin that is already in the guardian set must be rejected so
// that no address can hold both roles simultaneously.

#[test]
fn test_propose_admin_rejects_existing_guardian() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        // The guardian is already in the guardian set, so proposing it as the
        // incoming admin must be rejected to preserve separation of powers.
        let result = propose_admin(&e, guardian.clone());
        assert_eq!(result, Err(ErrorCode::NotAuthorized));
    });
}

#[test]
fn test_accept_admin_rejects_existing_guardian() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        // Even if a pending admin was somehow set, accepting it while the
        // address is a guardian must be rejected.
        let result = accept_admin(&e, guardian.clone());
        assert_eq!(result, Err(ErrorCode::NotAuthorized));
    });
}

#[test]
fn test_accept_admin_allows_non_guardian() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);
    let new_admin = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        // A non-guardian incoming admin is allowed to complete the transfer.
        propose_admin(&e, new_admin.clone()).unwrap();
        accept_admin(&e, new_admin.clone()).unwrap();
        assert_eq!(get_admin(&e).unwrap(), new_admin);
        assert_eq!(get_guardian(&e).unwrap(), guardian);
    });
}

// --- Governance: power-weighted guardian removal (issue #1699) ---
//
// `vote_on_guardian_removal` must use the same power-weighted majority rule as
// `is_majority_met` for upgrades. A head-count majority would let a single
// low-power guardian outvote a high-power guardian; the tests below pin the
// power-weighted outcome.

#[test]
fn test_guardian_removal_majority_is_power_weighted() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let g1 = Address::generate(&e);
    let g2 = Address::generate(&e);
    let g3 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, g1.clone()).unwrap();

        // g1 holds 1 power, g2 holds 10 power, g3 holds 10 power.
        set_guardian_voting_power(&e, g1.clone(), 1).unwrap();
        set_guardian_voting_power(&e, g2.clone(), 10).unwrap();
        set_guardian_voting_power(&e, g3.clone(), 10).unwrap();

        // g1 alone (1 of 21 power) must not meet the majority threshold.
        vote_on_guardian_removal(&e, g1.clone(), true).unwrap();
        assert!(!is_guardian_removal_majority_met(&e));

        // g2 (10 power) plus g1 (1 power) = 11 of 21 power, which is a
        // majority. A head-count rule would have required 2 of 3 heads and
        // would also pass here, so the decisive case is the one above.
        vote_on_guardian_removal(&e, g2.clone(), true).unwrap();
        assert!(is_guardian_removal_majority_met(&e));
    });
}

#[test]
fn test_guardian_removal_head_count_would_differ_from_power_weight() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let g1 = Address::generate(&e);
    let g2 = Address::generate(&e);
    let g3 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, g1.clone()).unwrap();

        // Two guardians hold a head-count majority (2 of 3) but only 2 of 21
        // total power, so a power-weighted rule must reject the removal.
        set_guardian_voting_power(&e, g1.clone(), 1).unwrap();
        set_guardian_voting_power(&e, g2.clone(), 1).unwrap();
        set_guardian_voting_power(&e, g3.clone(), 19).unwrap();

        vote_on_guardian_removal(&e, g1.clone(), true).unwrap();
        vote_on_guardian_removal(&e, g2.clone(), true).unwrap();
        assert!(!is_guardian_removal_majority_met(&e));
    });
}

// --- Governance: pending removal proposal is not silently replaced (issue #1700) ---

#[test]
fn test_remove_guardian_rejects_second_pending_proposal() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);
    let target1 = Address::generate(&e);
    let target2 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        remove_guardian(&e, target1.clone()).unwrap();
        let result = remove_guardian(&e, target2.clone());
        assert_eq!(result, Err(ErrorCode::RemovalAlreadyPending));
    });
}

#[test]
fn test_remove_guardian_allowed_after_previous_rejected() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);
    let target1 = Address::generate(&e);
    let target2 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        remove_guardian(&e, target1.clone()).unwrap();
        reject_guardian_removal(&e).unwrap();

        // Once the previous proposal is rejected a new one may be opened.
        remove_guardian(&e, target2.clone()).unwrap();
    });
}

#[test]
fn test_remove_guardian_allowed_after_previous_expired() {
    let (e, contract_id) = setup();
    let admin = Address::generate(&e);
    let guardian = Address::generate(&e);
    let target1 = Address::generate(&e);
    let target2 = Address::generate(&e);

    e.as_contract(&contract_id, || {
        set_admin(&e, admin.clone());
        set_guardian(&e, guardian.clone()).unwrap();

        remove_guardian(&e, target1.clone()).unwrap();

        // Advance the ledger past the proposal's expiry window.
        e.ledger().set_sequence_number(
            e.ledger().sequence() + GUARDIAN_REMOVAL_PROPOSAL_TTL + 1,
        );

        // An expired proposal must not block a fresh one.
        remove_guardian(&e, target2.clone()).unwrap();
    });
}

// --- Initialization guards (issue #1680) ---
//
// `initialize` must require authorization from the admin address being
// installed (so a front-runner cannot seize the admin role) and must reject
// out-of-range `base_fee` values.

#[test]
fn test_initialize_requires_admin_auth() {
    let e = Env::default();
    // Deliberately do NOT mock auths: the admin has not authorized the call.
    let contract_id = e.register_contract(None, PredictIQ);
    let client = PredictIQClient::new(&e, &contract_id);
    let admin = Address::generate(&e);

    let result = client.try_initialize(&admin, &100);
    assert!(result.is_err());
}

#[test]
fn test_initialize_rejects_base_fee_above_max() {
    let (e, contract_id) = setup();
    let client = PredictIQClient::new(&e, &contract_id);
    let admin = Address::generate(&e);

    let result = client.try_initialize(&admin, &10_001);
    assert_eq!(result, Err(Ok(ErrorCode::FeeTooHigh)));
}

#[test]
fn test_initialize_accepts_max_base_fee() {
    let (e, contract_id) = setup();
    let client = PredictIQClient::new(&e, &contract_id);
    let admin = Address::generate(&e);

    let result = client.try_initialize(&admin, &10_000);
    assert!(result.is_ok());
}
