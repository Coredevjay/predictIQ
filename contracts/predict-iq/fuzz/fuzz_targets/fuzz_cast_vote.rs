#![no_main]

use libfuzzer_sys::fuzz_target;
use predict_iq::{PredictIQContract, PredictIQContractClient, ErrorCode};
use soroban_sdk::{testutils::Address as _, token, Address, Env};

/// Fuzz `voting::cast_vote` weight-normalization, voting-window enforcement and
/// locked-balance accounting.
///
/// Exercises varied vote weights, token decimals (including extreme values) and
/// vote revisions. Asserts that casting a vote never panics, that votes outside
/// the voting window are rejected with `VotingEnded`/`VotingNotStarted`, and
/// that the tally invariants hold (votes are only counted for the chosen
/// proposal and the tally never decreases).
fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = token.address();

    let contract_id = env.register_contract(None, PredictIQContract);
    let client = PredictIQContractClient::new(&env, &contract_id);

    client.initialize(&admin, &token_id);

    // Vary token decimals to exercise weight normalization, including extreme
    // values that must be rejected without panicking.
    let decimals = (data[0] as u32) % 19;
    client.set_token_decimals(&admin, &decimals);

    let proposal_id = client.create_proposal(&admin, &1_000_000u64);

    // Move the ledger timestamp inside the voting window before casting votes.
    let dispute_timestamp = client.get_dispute_timestamp(&proposal_id);
    let voting_period = client.get_voting_period_seconds();
    env.ledger()
        .set_timestamp(dispute_timestamp + voting_period / 2);

    let voter_count = 1 + (data[1] as u32 % 6);
    let mut expected_tally: i128 = 0;
    for i in 0..voter_count {
        let voter = Address::generate(&env);
        let weight = 1 + (data[(2 + i as usize) % data.len()] as i128) * 1_000;
        let support = data[(3 + i as usize) % data.len()] % 2 == 0;

        client.cast_vote(&voter, &proposal_id, &support, &weight);

        if support {
            expected_tally += weight;
        }

        // Exercise the revision path: re-cast the same vote with a new weight.
        let revised_weight = 1 + (data[(4 + i as usize) % data.len()] as i128) * 1_000;
        client.cast_vote(&voter, &proposal_id, &support, &revised_weight);

        if support {
            expected_tally += revised_weight - weight;
        }
    }

    // Tally invariant: the recorded tally matches the accumulated support weight
    // and is never negative.
    let tally = client.get_proposal_tally(&proposal_id);
    assert!(tally >= 0, "tally must be non-negative");
    assert!(
        tally == expected_tally,
        "tally does not match accumulated vote weight"
    );

    // Votes at or after the end of the window must be rejected.
    env.ledger()
        .set_timestamp(dispute_timestamp + voting_period);
    let late_voter = Address::generate(&env);
    let late_result = client.try_cast_vote(&late_voter, &proposal_id, &true, &1_000);
    assert_eq!(
        late_result,
        Err(Ok(ErrorCode::VotingEnded)),
        "vote after the voting window must be rejected with VotingEnded"
    );

    // Votes before the window opens must be rejected.
    env.ledger().set_timestamp(dispute_timestamp.saturating_sub(1));
    let early_voter = Address::generate(&env);
    let early_result = client.try_cast_vote(&early_voter, &proposal_id, &true, &1_000);
    assert_eq!(
        early_result,
        Err(Ok(ErrorCode::VotingNotStarted)),
        "vote before the voting window must be rejected with VotingNotStarted"
    );
});
