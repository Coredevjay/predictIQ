use crate::errors::ErrorCode;
use crate::modules::{admin, markets};
use crate::types::{MarketStatus, CANCEL_OUTCOME_INDEX};
use soroban_sdk::{Address, Env};

const FAILED_MARKET_THRESHOLD_BPS: i128 = 7500; // 75% vote required to cancel
/// Voting window length, matching the 72h rule used by `finalize_resolution`.
const VOTING_PERIOD_SECONDS: u64 = 72 * 60 * 60;
/// Minimum number of votes cast (summed across all outcomes) required before a
/// vote-driven cancellation can be evaluated.
const CANCEL_QUORUM: i128 = 1;

/// Admin override to cancel a market
pub fn cancel_market_admin(e: &Env, market_id: u64) -> Result<(), ErrorCode> {
    admin::require_admin(e)?;

    let mut market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    if market.status == MarketStatus::Resolved || market.status == MarketStatus::Cancelled {
        return Err(ErrorCode::CannotChangeOutcome);
    }

    market.status = MarketStatus::Cancelled;
    markets::update_market(e, market);

    // admin::require_admin already verified and authenticated the admin; fetch
    // it to pass the acting address to the standardized event emitter so
    // indexers can identify who triggered the cancellation.
    let admin_address = admin::get_admin(e).expect("admin must be set after require_admin");
    crate::modules::events::emit_market_cancelled(e, market_id, admin_address);

    Ok(())
}

/// Community vote to cancel a market (requires 75% threshold)
pub fn cancel_market_vote(e: &Env, market_id: u64) -> Result<(), ErrorCode> {
    let mut market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    if market.status != MarketStatus::Disputed {
        return Err(ErrorCode::MarketNotDisputed);
    }

    // Issue #1692: the voting window must have closed before a vote-driven
    // cancellation can be evaluated, using the same 72h rule as
    // `finalize_resolution`. Otherwise a single early voter holding 100% of a
    // tiny tally could cancel the market immediately.
    let voting_start = market
        .dispute_timestamp
        .ok_or(ErrorCode::VotingNotStarted)?;
    let voting_end = voting_start
        .checked_add(VOTING_PERIOD_SECONDS)
        .ok_or(ErrorCode::ArithmeticOverflow)?;
    if e.ledger().timestamp() < voting_end {
        return Err(ErrorCode::VotingNotEnded);
    }

    // Calculate if cancellation threshold is met
    let cancel_votes = crate::modules::voting::get_tally(e, market_id, CANCEL_OUTCOME_INDEX);
    let mut total_votes = cancel_votes;

    for outcome in 0..market.options.len() {
        let tally = crate::modules::voting::get_tally(e, market_id, outcome);
        total_votes = total_votes
            .checked_add(tally)
            .ok_or(ErrorCode::ArithmeticOverflow)?;
    }

    // Issue #1692: require a quorum of votes cast before evaluating the
    // cancellation threshold.
    if total_votes < CANCEL_QUORUM {
        return Err(ErrorCode::InsufficientVotingWeight);
    }

    // Issue #52: Use checked_mul to prevent overflow with large voting weights.
    let cancel_pct = cancel_votes
        .checked_mul(10000)
        .and_then(|n| n.checked_div(total_votes))
        .ok_or(ErrorCode::InsufficientVotingWeight)?;
    if cancel_pct < FAILED_MARKET_THRESHOLD_BPS {
        return Err(ErrorCode::InsufficientVotingWeight);
    }

    market.status = MarketStatus::Cancelled;
    markets::update_market(e, market);

    crate::modules::events::emit_market_cancelled_vote(
        e,
        market_id,
        e.current_contract_address(),
    );

    Ok(())
}

// Issue #1189: withdraw_refund used to be duplicated here, but this module's
// copy was never reachable from any public entrypoint (see #83). The real,
// reachable implementation now lives in `bets::withdraw_refund`.

#[cfg(test)]
mod cancellation_event_tests {
    use super::*;
    use crate::modules::{admin as admin_mod, events::EVENT_VERSION};
    use crate::PredictIQ;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events as _},
        Address, Env, IntoVal,
    };

    fn setup_contract_with_admin(e: &Env) -> (Address, Address) {
        let contract_id = e.register(PredictIQ, ());
        let admin = Address::generate(e);
        e.as_contract(&contract_id, || {
            admin_mod::set_admin(e, admin.clone());
        });
        (contract_id, admin)
    }

    /// cancel_market_admin must emit the standardized `mkt_cncl` event that
    /// includes EVENT_VERSION in the data payload and the admin address as
    /// topic 2, so indexers can identify who triggered the cancellation.
    #[test]
    fn cancel_market_admin_emits_standardized_event_with_version_and_admin() {
        let e = Env::default();
        e.mock_all_auths();

        let (contract_id, admin) = setup_contract_with_admin(&e);
        let market_id: u64 = 42;

        // Seed a minimal market in storage so cancel_market_admin finds it.
        e.as_contract(&contract_id, || {
            use crate::types::{Market, MarketStatus, OracleConfig};
            use soroban_sdk::String;
            let market = Market {
                id: market_id,
                creator: Address::generate(&e),
                status: MarketStatus::Active,
                winning_outcome: None,
                resolved_at: None,
                dispute_timestamp: None,
                pending_resolution_timestamp: None,
                resolution_deadline: 9_999_999,
                oracle_config: OracleConfig::default(),
            };
            crate::modules::markets::update_market(&e, market);

            cancel_market_admin(&e, market_id).expect("cancel_market_admin failed");
        });

        let events = e.events().all();
        let events_debug = format!("{:?}", events);

        // Topic 0: "mkt_cncl"
        assert!(
            events_debug.contains("mkt_cncl"),
            "event topic 'mkt_cncl' not found in: {events_debug}"
        );

        // Topic 2: admin address
        let admin_debug = format!("{:?}", admin);
        assert!(
            events_debug.contains(&admin_debug),
            "admin address not found in event topics: {events_debug}"
        );

        // Data field 0: EVENT_VERSION
        let version_debug = format!("{EVENT_VERSION}");
        assert!(
            events_debug.contains(&version_debug),
            "EVENT_VERSION ({version_debug}) not found in event data: {events_debug}"
        );
    }

    /// cancel_market_vote must emit the standardized `mk_cn_vt` event that
    /// includes EVENT_VERSION in the data payload and the contract address as
    /// topic 2, matching the documented Indexer Integration Guide schema.
    #[test]
    fn cancel_market_vote_emits_standardized_event_with_version_and_contract_address() {
        let e = Env::default();
        e.mock_all_auths();

        let (contract_id, _admin) = setup_contract_with_admin(&e);
        let market_id: u64 = 7;

        e.as_contract(&contract_id, || {
            use crate::types::{Market, MarketStatus, OracleConfig};
            use soroban_sdk::String;

            // Seed market in Disputed status (required by cancel_market_vote).
            // The dispute timestamp is set far enough in the past that the 72h
            // voting window has already closed.
            let market = Market {
                id: market_id,
                creator: Address::generate(&e),
                status: MarketStatus::Disputed,
                winning_outcome: None,
                resolved_at: None,
                dispute_timestamp: Some(0),
                pending_resolution_timestamp: None,
                resolution_deadline: 9_999_999,
                oracle_config: OracleConfig::default(),
            };
            crate::modules::markets::update_market(&e, market);

            // Seed enough cancel votes to exceed the 75% threshold.
            // total_votes = cancel_votes only (no other outcome votes seeded),
            // so cancel_pct = 10000 bps = 100% ≥ 7500 bps threshold.
            crate::modules::voting::set_tally_for_test(
                &e,
                market_id,
                crate::types::CANCEL_OUTCOME_INDEX,
                10_000,
            );

            cancel_market_vote(&e, market_id).expect("cancel_market_vote failed");
        });

        let events = e.events().all();
        let events_debug = format!("{:?}", events);

        // Topic 0: "mk_cn_vt"
        assert!(
            events_debug.contains("mk_cn_vt"),
            "event topic 'mk_cn_vt' not found in: {events_debug}"
        );

        // Topic 2: contract address (the resolver for a vote-driven cancellation)
        let contract_debug = format!("{:?}", contract_id);
        assert!(
            events_debug.contains(&contract_debug),
            "contract address not found in event topics: {events_debug}"
        );

        // Data field 0: EVENT_VERSION
        let version_debug = format!("{EVENT_VERSION}");
        assert!(
            events_debug.contains(&version_debug),
            "EVENT_VERSION ({version_debug}) not found in event data: {events_debug}"
        );
    }

    /// Issue #1692: a vote-driven cancellation must not succeed while the
    /// voting window is still open, even if the cancel tally already exceeds
    /// the 75% threshold.
    #[test]
    fn cancel_market_vote_rejected_before_voting_window_closes() {
        let e = Env::default();
        e.mock_all_auths();

        let (contract_id, _admin) = setup_contract_with_admin(&e);
        let market_id: u64 = 8;

        e.as_contract(&contract_id, || {
            use crate::types::{Market, MarketStatus, OracleConfig};

            let now = e.ledger().timestamp();
            let market = Market {
                id: market_id,
                creator: Address::generate(&e),
                status: MarketStatus::Disputed,
                winning_outcome: None,
                resolved_at: None,
                dispute_timestamp: Some(now),
                pending_resolution_timestamp: None,
                resolution_deadline: 9_999_999,
                oracle_config: OracleConfig::default(),
            };
            crate::modules::markets::update_market(&e, market);

            crate::modules::voting::set_tally_for_test(
                &e,
                market_id,
                crate::types::CANCEL_OUTCOME_INDEX,
                10_000,
            );

            let result = cancel_market_vote(&e, market_id);
            assert_eq!(result, Err(ErrorCode::VotingNotEnded));
        });
    }

    /// Issue #1692: a vote-driven cancellation must be rejected when no votes
    /// have been cast (quorum not met), even after the window has closed.
    #[test]
    fn cancel_market_vote_rejected_without_quorum() {
        let e = Env::default();
        e.mock_all_auths();

        let (contract_id, _admin) = setup_contract_with_admin(&e);
        let market_id: u64 = 9;

        e.as_contract(&contract_id, || {
            use crate::types::{Market, MarketStatus, OracleConfig};

            let market = Market {
                id: market_id,
                creator: Address::generate(&e),
                status: MarketStatus::Disputed,
                winning_outcome: None,
                resolved_at: None,
                dispute_timestamp: Some(0),
                pending_resolution_timestamp: None,
                resolution_deadline: 9_999_999,
                oracle_config: OracleConfig::default(),
            };
            crate::modules::markets::update_market(&e, market);

            let result = cancel_market_vote(&e, market_id);
            assert_eq!(result, Err(ErrorCode::InsufficientVotingWeight));
        });
    }
}
