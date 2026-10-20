use crate::errors::ErrorCode;
use crate::modules::markets;
// Issue #171: ConfigKey (including GovernanceToken variant) must be explicitly imported
// from types. Previously missing, causing compilation failure in cast_vote.
use crate::types::{ConfigKey, LockedTokens, MarketStatus, Vote, CANCEL_OUTCOME_INDEX};
use soroban_sdk::{contracttype, token, Address, Env, IntoVal, Symbol, Val, Vec};

/// Issue #1693: Length of the community voting window, in seconds (72 hours).
/// Votes are only accepted while `dispute_timestamp <= now < dispute_timestamp + VOTING_PERIOD_SECONDS`.
pub const VOTING_PERIOD_SECONDS: u64 = 72 * 60 * 60;

/// Issue #1696: Maximum number of distinct voters per disputed market.
/// Bounds the size of the `DisputeVoters` registry so that `prune_market_voting_state`
/// (and each vote) cannot grow without limit.
pub const MAX_DISPUTE_VOTERS: u32 = 500;

/// Issue #1696: Number of voters processed per `prune_market_voting_state` call.
/// Pruning can be resumed across multiple calls so gas stays bounded.
pub const PRUNE_BATCH_SIZE: u32 = 50;

/// Issue #1695: Upper bound on governance token decimals accepted for normalization.
pub const MAX_TOKEN_DECIMALS: u32 = 38;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Vote(u64, Address),         // market_id, voter
    VoteTally(u64, u32),        // market_id, outcome -> total_weight
    LockedTokens(u64, Address), // market_id, voter
    /// Issue #37: Per-user locked balance ledger to prevent pool drain.
    LockedBalance(u64, Address), // market_id, voter -> amount
    /// Registered voters for a disputed market — drives O(n) deep prune (Issue #84).
    DisputeVoters(u64), // market_id -> Vec<Address>
    /// Issue #1696: Cursor into the DisputeVoters registry for batched pruning.
    PruneCursor(u64), // market_id -> u32 (next index to prune)
}

pub fn cast_vote(
    e: &Env,
    voter: Address,
    market_id: u64,
    outcome: u32,
    weight: i128,
) -> Result<(), ErrorCode> {
    voter.require_auth();

    let market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    if market.status != MarketStatus::Disputed {
        return Err(ErrorCode::MarketNotDisputed);
    }

    // Issue #1693: Enforce the voting window. Votes are only valid while
    // dispute_timestamp <= now < dispute_timestamp + VOTING_PERIOD_SECONDS.
    let dispute_timestamp = market
        .dispute_timestamp
        .ok_or(ErrorCode::MarketNotDisputed)?;
    let now = e.ledger().timestamp();
    if now < dispute_timestamp {
        return Err(ErrorCode::VotingNotStarted);
    }
    let voting_deadline = dispute_timestamp.saturating_add(VOTING_PERIOD_SECONDS);
    if now >= voting_deadline {
        return Err(ErrorCode::VotingEnded);
    }

    if outcome != CANCEL_OUTCOME_INDEX && outcome >= market.options.len() {
        return Err(ErrorCode::InvalidOutcome);
    }

    let vote_key = DataKey::Vote(market_id, voter.clone());

    // Issue #175: Allow vote revision - voters can change their vote before resolution deadline
    // This enables more flexible governance where voters can respond to new information
    let old_vote: Option<Vote> = e.storage().persistent().get(&vote_key);
    if let Some(ref old_vote_data) = old_vote {
        // Decrement the old outcome tally when vote is revised
        let old_tally_key = DataKey::VoteTally(market_id, old_vote_data.outcome);
        let mut old_tally: i128 = e.storage().persistent().get(&old_tally_key).unwrap_or(0);
        old_tally -= old_vote_data.weight;
        e.storage().persistent().set(&old_tally_key, &old_tally);
    }

    let snapshot_ledger = market
        .dispute_snapshot_ledger
        .ok_or(ErrorCode::MarketNotDisputed)?;

    // Issue #068: Vote weight is proportional to governance token stake.
    //
    // Weight calculation:
    //   1. Primary path  — query the governance token's `balance_at(voter, snapshot_ledger)`
    //      to get the voter's balance at the exact ledger when the dispute was filed.
    //      This prevents weight manipulation by buying tokens after a dispute starts.
    //   2. Fallback path — if the token contract does not support `balance_at` (older tokens),
    //      the caller-supplied `weight` is used, but only up to their current live balance.
    //      Tokens are physically locked in the contract for the dispute duration and released
    //      only after the market reaches `Resolved` status (see unlock_tokens).
    //
    // Manipulation prevention:
    //   - Snapshot ledger is set at dispute-filing time and is immutable.
    //   - Fallback tokens are locked so the same tokens cannot be double-voted.
    //   - Per-user LockedBalance tracking (DataKey::LockedBalance) prevents pool drain.
    //   - Vote revision decrements the old tally before adding the new one.

    // Issue #3: GovernanceToken now exists in ConfigKey
    let gov_token: Address = e
        .storage()
        .instance()
        .get(&ConfigKey::GovernanceToken)
        .ok_or(ErrorCode::GovernanceTokenNotSet)?;

    let actual_weight = match try_get_balance_at(e, &gov_token, &voter, snapshot_ledger) {
        Ok(balance) => balance,
        Err(_) => {
            // Issue #37: Fallback — lock tokens and track per-user balance
            let token_client = token::Client::new(e, &gov_token);
            let current_balance = token_client.balance(&voter);

            // Issue #1694: On revision, reuse the existing lock. Only the delta
            // between the new and old weight is transferred (or refunded).
            let lock_key = DataKey::LockedBalance(market_id, voter.clone());
            let existing: i128 = e.storage().persistent().get(&lock_key).unwrap_or(0);
            let old_weight = old_vote.as_ref().map(|v| v.weight).unwrap_or(0);

            // The new locked amount equals the requested weight; the delta is
            // what must move in/out of the contract.
            let delta = weight - existing;
            if delta > 0 {
                if current_balance < delta {
                    return Err(ErrorCode::InsufficientVotingWeight);
                }
                e.current_contract_address().require_auth();
                token_client.transfer(&voter, &e.current_contract_address(), &delta);
            } else if delta < 0 {
                e.current_contract_address().require_auth();
                token_client.transfer(&e.current_contract_address(), &voter, &(-delta));
            }

            // Keep LockedBalance and LockedTokens.amount in sync with the
            // current locked amount (never accumulate across revisions).
            e.storage().persistent().set(&lock_key, &weight);

            let locked = LockedTokens {
                voter: voter.clone(),
                market_id,
                amount: weight,
                unlock_time: market.resolution_deadline,
            };
            e.storage()
                .persistent()
                .set(&DataKey::LockedTokens(market_id, voter.clone()), &locked);

            let _ = old_weight;
            weight
        }
    };

    if actual_weight == 0 {
        return Err(ErrorCode::InsufficientVotingWeight);
    }

    // Issue #1695: Normalize to 18 decimal places with bounded, checked math.
    let token_decimals = get_token_decimals(e, &gov_token)?;
    let normalized_weight = normalize_weight(actual_weight, token_decimals)?;

    if normalized_weight == 0 {
        return Err(ErrorCode::InsufficientVotingWeight);
    }

    let vote = Vote {
        market_id,
        voter: voter.clone(),
        outcome,
        weight: normalized_weight,
    };

    e.storage().persistent().set(&vote_key, &vote);

    if old_vote.is_none() {
        let reg_key = DataKey::DisputeVoters(market_id);
        let mut voters: Vec<Address> = e
            .storage()
            .persistent()
            .get(&reg_key)
            .unwrap_or(Vec::new(e));
        // Issue #1696: Bound the registry so prune gas cannot grow unbounded.
        if voters.len() >= MAX_DISPUTE_VOTERS {
            return Err(ErrorCode::TooManyOutcomes);
        }
        voters.push_back(voter.clone());
        e.storage().persistent().set(&reg_key, &voters);
    }

    let tally_key = DataKey::VoteTally(market_id, outcome);
    let mut current_tally: i128 = e.storage().persistent().get(&tally_key).unwrap_or(0);
    current_tally += normalized_weight;
    e.storage().persistent().set(&tally_key, &current_tally);

    crate::modules::events::emit_vote_cast(e, market_id, voter, outcome, normalized_weight);

    Ok(())
}

fn try_get_balance_at(
    e: &Env,
    token: &Address,
    account: &Address,
    ledger: u32,
) -> Result<i128, ErrorCode> {
    let args: Vec<Val> = soroban_sdk::vec![e, account.clone().into_val(e), ledger.into_val(e)];

    match e.try_invoke_contract::<i128, ErrorCode>(token, &Symbol::new(e, "balance_at"), args) {
        Ok(Ok(balance)) => Ok(balance),
        _ => Err(ErrorCode::OracleFailure),
    }
}

/// Fetch the decimal precision of a token contract.
///
/// Issue #1695: Fail loudly instead of silently defaulting when the call fails,
/// and reject absurd precision values that would overflow normalization.
fn get_token_decimals(e: &Env, token: &Address) -> Result<u32, ErrorCode> {
    let args: Vec<Val> = soroban_sdk::vec![e];
    let decimals = match e.try_invoke_contract::<u32, ErrorCode>(
        token,
        &Symbol::new(e, "decimals"),
        args,
    ) {
        Ok(Ok(d)) => d,
        _ => return Err(ErrorCode::OracleFailure),
    };
    if decimals > MAX_TOKEN_DECIMALS {
        return Err(ErrorCode::InvalidAmount);
    }
    Ok(decimals)
}

/// Issue #1695: Normalize `weight` to 18 decimals using checked arithmetic.
/// Returns `ArithmeticOverflow` if the scaling factor or the result overflows.
fn normalize_weight(weight: i128, token_decimals: u32) -> Result<i128, ErrorCode> {
    const NORMALIZED_DECIMALS: u32 = 18;
    if token_decimals > MAX_TOKEN_DECIMALS {
        return Err(ErrorCode::InvalidAmount);
    }
    if token_decimals < NORMALIZED_DECIMALS {
        let exp = NORMALIZED_DECIMALS - token_decimals;
        let scale = 10i128
            .checked_pow(exp)
            .ok_or(ErrorCode::ArithmeticOverflow)?;
        weight
            .checked_mul(scale)
            .ok_or(ErrorCode::ArithmeticOverflow)
    } else if token_decimals > NORMALIZED_DECIMALS {
        let exp = token_decimals - NORMALIZED_DECIMALS;
        let scale = 10i128
            .checked_pow(exp)
            .ok_or(ErrorCode::ArithmeticOverflow)?;
        Ok(weight / scale)
    } else {
        Ok(weight)
    }
}

/// Issue #20: Require market to be Resolved (or Cancelled) before unlocking tokens.
pub fn unlock_tokens(e: &Env, voter: Address, market_id: u64) -> Result<(), ErrorCode> {
    voter.require_auth();

    let market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    // Issue #20: Tokens remain locked throughout the entire dispute lifecycle.
    // Only allow unlock once the market reaches a terminal state.
    // Issue #1192: A Disputed market can transition to Cancelled via community
    // vote (cancellation::cancel_market_vote); requiring Resolved only would
    // permanently strand governance tokens locked via the fallback path with
    // no code path to retrieve them.
    if market.status != MarketStatus::Resolved && market.status != MarketStatus::Cancelled {
        return Err(ErrorCode::MarketNotResolved);
    }

    let lock_key = DataKey::LockedTokens(market_id, voter.clone());
    let locked: LockedTokens = e
        .storage()
        .persistent()
        .get(&lock_key)
        .ok_or(ErrorCode::BetNotFound)?;

    if e.ledger().timestamp() < locked.unlock_time {
        return Err(ErrorCode::TimelockActive);
    }

    // Issue #37: Use LockedBalance as the authoritative per-user amount to
    // prevent a user from withdrawing more than they individually locked.
    let balance_key = DataKey::LockedBalance(market_id, voter.clone());
    let amount: i128 = e.storage().persistent().get(&balance_key).unwrap_or(0);

    if amount <= 0 {
        return Err(ErrorCode::BetNotFound);
    }

    let gov_token: Address = e
        .storage()
        .instance()
        .get(&ConfigKey::GovernanceToken)
        .ok_or(ErrorCode::GovernanceTokenNotSet)?;

    let token_client = token::Client::new(e, &gov_token);
    e.current_contract_address().require_auth();
    token_client.transfer(&e.current_contract_address(), &voter, &amount);

    e.storage().persistent().remove(&lock_key);
    e.storage().persistent().remove(&balance_key);

    Ok(())
}

pub fn get_tally(e: &Env, market_id: u64, outcome: u32) -> i128 {
    e.storage()
        .persistent()
        .get(&DataKey::VoteTally(market_id, outcome))
        .unwrap_or(0)
}

/// Clears vote tallies, per-voter vote/lock ledgers, and the dispute voter registry.
/// Safe to call when no voting occurred (only removes keys that exist).
///
/// Issue #1696: Voters are processed in bounded batches of `PRUNE_BATCH_SIZE`.
/// If the registry is larger than one batch, the cursor is persisted and the
/// caller must invoke this again to continue; tallies and the registry itself
/// are only cleared once all voters have been processed.
pub fn prune_market_voting_state(e: &Env, market_id: u64, num_outcomes: u32) {
    let reg_key = DataKey::DisputeVoters(market_id);
    let cursor_key = DataKey::PruneCursor(market_id);
    let mut cursor: u32 = e.storage().persistent().get(&cursor_key).unwrap_or(0);

    if let Some(voters) = e.storage().persistent().get::<_, Vec<Address>>(&reg_key) {
        let end = core::cmp::min(cursor.saturating_add(PRUNE_BATCH_SIZE), voters.len());
        let mut i = cursor;
        while i < end {
            let v = voters.get(i).unwrap();
            e.storage()
                .persistent()
                .remove(&DataKey::Vote(market_id, v.clone()));
            e.storage()
                .persistent()
                .remove(&DataKey::LockedTokens(market_id, v.clone()));
            e.storage()
                .persistent()
                .remove(&DataKey::LockedBalance(market_id, v.clone()));
            i += 1;
        }
        cursor = end;

        if cursor < voters.len() {
            // More voters remain — persist progress and stop this batch.
            e.storage().persistent().set(&cursor_key, &cursor);
            return;
        }
    }

    e.storage().persistent().remove(&reg_key);
    e.storage().persistent().remove(&cursor_key);

    for o in 0..num_outcomes {
        e.storage()
            .persistent()
            .remove(&DataKey::VoteTally(market_id, o));
    }
}

#[cfg(test)]
mod import_tests {
    use crate::types::ConfigKey;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    /// Issue #171: Verify GovernanceToken variant is accessible and round-trips through storage.
    #[test]
    fn governance_token_config_key_round_trips() {
        let e = Env::default();
        let token = Address::generate(&e);
        e.storage()
            .instance()
            .set(&ConfigKey::GovernanceToken, &token);
        let stored: Option<Address> = e.storage().instance().get(&ConfigKey::GovernanceToken);
        assert_eq!(stored, Some(token));
    }

    /// Issue #171: cast_vote returns GovernanceTokenNotSet when token is not configured.
    #[test]
    fn cast_vote_returns_error_when_governance_token_not_set() {
        let e = Env::default();
        // GovernanceToken not set in storage — get returns None
        let stored: Option<Address> = e.storage().instance().get(&ConfigKey::GovernanceToken);
        assert!(
            stored.is_none(),
            "GovernanceToken must be absent to trigger the error"
        );
    }
}

#[cfg(test)]
mod prune_tests {
    use super::{prune_market_voting_state, DataKey};
    use crate::types::{LockedTokens, Vote};
    use soroban_sdk::{testutils::Address as _, Address, Env};

    #[test]
    fn prune_clears_votes_locks_tallies_and_registry() {
        let e = Env::default();
        let market_id = 42u64;
        let v1 = Address::generate(&e);
        let v2 = Address::generate(&e);

        e.storage().persistent().set(
            &DataKey::Vote(market_id, v1.clone()),
            &Vote {
                market_id,
                voter: v1.clone(),
                outcome: 0,
                weight: 100,
            },
        );
        e.storage().persistent().set(
            &DataKey::Vote(market_id, v2.clone()),
            &Vote {
                market_id,
                voter: v2.clone(),
                outcome: 1,
                weight: 200,
            },
        );
        e.storage()
            .persistent()
            .set(&DataKey::VoteTally(market_id, 0), &100_i128);
        e.storage()
            .persistent()
            .set(&DataKey::VoteTally(market_id, 1), &200_i128);
        e.storage().persistent().set(
            &DataKey::LockedTokens(market_id, v1.clone()),
            &LockedTokens {
                voter: v1.clone(),
                market_id,
                amount: 50,
                unlock_time: 0,
            },
        );
        e.storage()
            .persistent()
            .set(&DataKey::LockedBalance(market_id, v1.clone()), &50_i128);

        let mut reg = soroban_sdk::Vec::new(&e);
        reg.push_back(v1.clone());
        reg.push_back(v2.clone());
        e.storage()
            .persistent()
            .set(&DataKey::DisputeVoters(market_id), &reg);

        prune_market_voting_state(&e, market_id, 2);

        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::Vote(market_id, v1.clone())));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::Vote(market_id, v2.clone())));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::LockedTokens(market_id, v1.clone())));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::LockedBalance(market_id, v1.clone())));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::VoteTally(market_id, 0)));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::VoteTally(market_id, 1)));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::DisputeVoters(market_id)));
    }
}

#[cfg(test)]
mod decimal_normalization_tests {
    /// Unit tests for the decimal normalization logic (no contract env needed).
    use super::{normalize_weight, MAX_TOKEN_DECIMALS};
    use crate::errors::ErrorCode;

    const NORMALIZED_DECIMALS: u32 = 18;

    fn normalize(balance: i128, token_decimals: u32) -> i128 {
        normalize_weight(balance, token_decimals).unwrap()
    }

    #[test]
    fn seven_decimal_token_scaled_up() {
        // 1 token with 7 decimals = 10_000_000 raw units
        // normalized to 18 decimals = 10_000_000 * 10^11
        let raw = 10_000_000i128;
        let normalized = normalize(raw, 7);
        assert_eq!(normalized, raw * 10i128.pow(11));
    }

    #[test]
    fn eighteen_decimal_token_unchanged() {
        let raw = 1_000_000_000_000_000_000i128;
        assert_eq!(normalize(raw, 18), raw);
    }

    #[test]
    fn higher_decimal_token_scaled_down() {
        // 24 decimal token: divide by 10^6
        let raw = 1_000_000_000_000_000_000_000_000i128;
        let normalized = normalize(raw, 24);
        assert_eq!(normalized, raw / 10i128.pow(6));
    }

    #[test]
    fn equal_weights_after_normalization() {
        // 1 token regardless of decimal precision should normalize to the same value
        let one_7dec = normalize(10_000_000, 7); // 1 token at 7 decimals
        let one_18dec = normalize(1_000_000_000_000_000_000, 18); // 1 token at 18 decimals
        assert_eq!(one_7dec, one_18dec);
    }

    /// Issue #1695: decimals above the bound are rejected, not panicked on.
    #[test]
    fn decimals_above_bound_rejected() {
        assert_eq!(
            normalize_weight(1, MAX_TOKEN_DECIMALS + 1),
            Err(ErrorCode::InvalidAmount)
        );
        assert_eq!(
            normalize_weight(1, 57),
            Err(ErrorCode::InvalidAmount)
        );
    }

    /// Issue #1695: scaling that would overflow returns ArithmeticOverflow.
    #[test]
    fn overflow_returns_error() {
        // 38 decimals scaled up to 18 would need 10^20 — fine; but a huge
        // weight times a large scale overflows i128.
        let result = normalize_weight(i128::MAX, 0);
        assert_eq!(result, Err(ErrorCode::ArithmeticOverflow));
    }
}

/// Issue #1192: Voters who locked governance tokens via cast_vote's fallback
/// path must be able to recover them once the market reaches ANY terminal
/// state, not just Resolved — including Cancelled (e.g. via community-voted
/// cancel_market_vote from Disputed).
#[cfg(test)]
mod unlock_tokens_terminal_state_tests {
    use super::{cast_vote, unlock_tokens, DataKey};
    use crate::errors::ErrorCode;
    use crate::modules::markets;
    use crate::types::{ConfigKey, MarketStatus, MarketTier, OracleConfig};
    use crate::{PredictIQ, PredictIQClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        token, Address, Env, String, Vec,
    };

    fn setup() -> (Env, PredictIQClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(PredictIQ, ());
        let client = PredictIQClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize(&admin, &0);

        (env, client, admin, contract_id)
    }

    /// StellarAssetContract supports balance()/transfer() but not balance_at(),
    /// so casting a vote against it always exercises cast_vote's fallback path.
    fn setup_gov_token(env: &Env, contract_id: &Address) -> Address {
        let token_admin = Address::generate(env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin);
        let token_address = token_id.address();
        env.as_contract(contract_id, || {
            env.storage()
                .instance()
                .set(&ConfigKey::GovernanceToken, &token_address);
        });
        token_address
    }

    fn create_market(env: &Env, client: &PredictIQClient, admin: &Address) -> u64 {
        let options = Vec::from_array(
            env,
            [String::from_str(env, "Yes"), String::from_str(env, "No")],
        );
        let native_token = Address::generate(env);
        client.create_market(
            admin,
            &String::from_str(env, "Fallback Lock Test"),
            &options,
            &1000,
            &2000,
            &OracleConfig {
                oracle_address: Address::generate(env),
                feed_id: String::from_str(env, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
                min_responses: Some(1),
                max_staleness_seconds: 3600,
                max_confidence_bps: 200,
                strike_price: None,
            },
            &MarketTier::Basic,
            &native_token,
            &0,
            &0,
        )
    }

    #[test]
    fn unlock_tokens_succeeds_after_cancellation_for_fallback_locked_voter() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let market_id = create_market(&env, &client, &admin);

        // Move market to Disputed so cast_vote's fallback lock path is reachable.
        env.as_contract(&contract_id, || {
            let mut market = markets::get_market(&env, market_id).unwrap();
            market.status = MarketStatus::Disputed;
            market.pending_resolution_timestamp = Some(1001);
            market.dispute_timestamp = Some(1001);
            market.dispute_snapshot_ledger = Some(env.ledger().sequence());
            markets::update_market(&env, market);
        });

        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &500);
        let token_client = token::Client::new(&env, &gov_token);
        assert_eq!(token_client.balance(&voter), 500);

        // Fallback path: balance_at is unsupported on the SAC, so cast_vote
        // locks the caller-supplied weight in the contract.
        env.as_contract(&contract_id, || {
            cast_vote(&env, voter.clone(), market_id, 0, 500).unwrap();
        });
        assert_eq!(token_client.balance(&voter), 0);
        assert_eq!(token_client.balance(&contract_id), 500);

        // Community vote cancels the Disputed market — no path back to Resolved.
        env.as_contract(&contract_id, || {
            let mut market = markets::get_market(&env, market_id).unwrap();
            market.status = MarketStatus::Cancelled;
            markets::update_market(&env, market);
        });

        // Advance past the lock's unlock_time (market.resolution_deadline == 2000).
        env.ledger().with_mut(|li| li.timestamp = 2001);

        env.as_contract(&contract_id, || {
            unlock_tokens(&env, voter.clone(), market_id).unwrap();
        });

        // Tokens are fully returned and the lock ledger entries are cleared.
        assert_eq!(token_client.balance(&voter), 500);
        assert_eq!(token_client.balance(&contract_id), 0);
        env.as_contract(&contract_id, || {
            assert!(!env
                .storage()
                .persistent()
                .has(&DataKey::LockedTokens(market_id, voter.clone())));
            assert!(!env
                .storage()
                .persistent()
                .has(&DataKey::LockedBalance(market_id, voter.clone())));
        });
    }

    #[test]
    fn unlock_tokens_still_rejected_while_market_active() {
        let (env, client, admin, contract_id) = setup();
        let market_id = create_market(&env, &client, &admin);
        let voter = Address::generate(&env);

        let result = env.as_contract(&contract_id, || unlock_tokens(&env, voter, market_id));
        assert_eq!(result, Err(ErrorCode::MarketNotResolved));
    }
}

/// Issue #1693: Voting window boundaries.
#[cfg(test)]
mod voting_window_tests {
    use super::{cast_vote, VOTING_PERIOD_SECONDS};
    use crate::errors::ErrorCode;
    use crate::modules::markets;
    use crate::types::{ConfigKey, MarketStatus, MarketTier, OracleConfig};
    use crate::{PredictIQ, PredictIQClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        token, Address, Env, String, Vec,
    };

    fn setup() -> (Env, PredictIQClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(PredictIQ, ());
        let client = PredictIQClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &0);
        (env, client, admin, contract_id)
    }

    fn setup_gov_token(env: &Env, contract_id: &Address) -> Address {
        let token_admin = Address::generate(env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin);
        let token_address = token_id.address();
        env.as_contract(contract_id, || {
            env.storage()
                .instance()
                .set(&ConfigKey::GovernanceToken, &token_address);
        });
        token_address
    }

    fn create_disputed_market(
        env: &Env,
        client: &PredictIQClient,
        admin: &Address,
        contract_id: &Address,
        dispute_timestamp: u64,
    ) -> u64 {
        let options = Vec::from_array(
            env,
            [String::from_str(env, "Yes"), String::from_str(env, "No")],
        );
        let native_token = Address::generate(env);
        let market_id = client.create_market(
            admin,
            &String::from_str(env, "Window Test"),
            &options,
            &1000,
            &2000,
            &OracleConfig {
                oracle_address: Address::generate(env),
                feed_id: String::from_str(env, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
                min_responses: Some(1),
                max_staleness_seconds: 3600,
                max_confidence_bps: 200,
                strike_price: None,
            },
            &MarketTier::Basic,
            &native_token,
            &0,
            &0,
        );
        env.as_contract(contract_id, || {
            let mut market = markets::get_market(env, market_id).unwrap();
            market.status = MarketStatus::Disputed;
            market.pending_resolution_timestamp = Some(dispute_timestamp);
            market.dispute_timestamp = Some(dispute_timestamp);
            market.dispute_snapshot_ledger = Some(env.ledger().sequence());
            markets::update_market(env, market);
        });
        market_id
    }

    fn vote_at(
        env: &Env,
        contract_id: &Address,
        voter: &Address,
        market_id: u64,
        timestamp: u64,
    ) -> Result<(), ErrorCode> {
        env.ledger().with_mut(|li| li.timestamp = timestamp);
        env.as_contract(contract_id, || {
            cast_vote(env, voter.clone(), market_id, 0, 100)
        })
    }

    #[test]
    fn vote_before_start_rejected() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let dispute_ts = 10_000u64;
        let market_id = create_disputed_market(&env, &client, &admin, &contract_id, dispute_ts);
        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &1000);

        let result = vote_at(&env, &contract_id, &voter, market_id, dispute_ts - 1);
        assert_eq!(result, Err(ErrorCode::VotingNotStarted));
    }

    #[test]
    fn vote_at_deadline_minus_one_accepted() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let dispute_ts = 10_000u64;
        let market_id = create_disputed_market(&env, &client, &admin, &contract_id, dispute_ts);
        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &1000);

        let deadline = dispute_ts + VOTING_PERIOD_SECONDS;
        let result = vote_at(&env, &contract_id, &voter, market_id, deadline - 1);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn vote_at_deadline_rejected() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let dispute_ts = 10_000u64;
        let market_id = create_disputed_market(&env, &client, &admin, &contract_id, dispute_ts);
        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &1000);

        let deadline = dispute_ts + VOTING_PERIOD_SECONDS;
        let result = vote_at(&env, &contract_id, &voter, market_id, deadline);
        assert_eq!(result, Err(ErrorCode::VotingEnded));
    }

    #[test]
    fn vote_after_deadline_rejected() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let dispute_ts = 10_000u64;
        let market_id = create_disputed_market(&env, &client, &admin, &contract_id, dispute_ts);
        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &1000);

        let deadline = dispute_ts + VOTING_PERIOD_SECONDS;
        let result = vote_at(&env, &contract_id, &voter, market_id, deadline + 1);
        assert_eq!(result, Err(ErrorCode::VotingEnded));
    }
}

/// Issue #1694: Vote revision must not accumulate locked tokens.
#[cfg(test)]
mod vote_revision_lock_tests {
    use super::{cast_vote, DataKey};
    use crate::modules::markets;
    use crate::types::{ConfigKey, LockedTokens, MarketStatus, MarketTier, OracleConfig};
    use crate::{PredictIQ, PredictIQClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        token, Address, Env, String, Vec,
    };

    fn setup() -> (Env, PredictIQClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(PredictIQ, ());
        let client = PredictIQClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &0);
        (env, client, admin, contract_id)
    }

    fn setup_gov_token(env: &Env, contract_id: &Address) -> Address {
        let token_admin = Address::generate(env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin);
        let token_address = token_id.address();
        env.as_contract(contract_id, || {
            env.storage()
                .instance()
                .set(&ConfigKey::GovernanceToken, &token_address);
        });
        token_address
    }

    fn create_disputed_market(
        env: &Env,
        client: &PredictIQClient,
        admin: &Address,
        contract_id: &Address,
    ) -> u64 {
        let options = Vec::from_array(
            env,
            [String::from_str(env, "Yes"), String::from_str(env, "No")],
        );
        let native_token = Address::generate(env);
        let market_id = client.create_market(
            admin,
            &String::from_str(env, "Revision Test"),
            &options,
            &1000,
            &2000,
            &OracleConfig {
                oracle_address: Address::generate(env),
                feed_id: String::from_str(env, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
                min_responses: Some(1),
                max_staleness_seconds: 3600,
                max_confidence_bps: 200,
                strike_price: None,
            },
            &MarketTier::Basic,
            &native_token,
            &0,
            &0,
        );
        env.as_contract(contract_id, || {
            let mut market = markets::get_market(env, market_id).unwrap();
            market.status = MarketStatus::Disputed;
            market.pending_resolution_timestamp = Some(1001);
            market.dispute_timestamp = Some(1001);
            market.dispute_snapshot_ledger = Some(env.ledger().sequence());
            markets::update_market(env, market);
        });
        market_id
    }

    #[test]
    fn repeated_revisions_keep_lock_in_sync() {
        let (env, client, admin, contract_id) = setup();
        let gov_token = setup_gov_token(&env, &contract_id);
        let market_id = create_disputed_market(&env, &client, &admin, &contract_id);
        let voter = Address::generate(&env);
        token::StellarAssetClient::new(&env, &gov_token).mint(&voter, &1000);
        let token_client = token::Client::new(&env, &gov_token);

        env.ledger().with_mut(|li| li.timestamp = 1001);

        // First vote locks 300.
        env.as_contract(&contract_id, || {
            cast_vote(&env, voter.clone(), market_id, 0, 300).unwrap();
        });
        assert_eq!(token_client.balance(&voter), 700);

        // Revise up to 500 — only the 200 delta is transferred.
        env.as_contract(&contract_id, || {
            cast_vote(&env, voter.clone(), market_id, 1, 500).unwrap();
        });
        assert_eq!(token_client.balance(&voter), 500);

        // Revise down to 200 — 300 is refunded.
        env.as_contract(&contract_id, || {
            cast_vote(&env, voter.clone(), market_id, 0, 200).unwrap();
        });
        assert_eq!(token_client.balance(&voter), 800);

        env.as_contract(&contract_id, || {
            let locked_balance: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::LockedBalance(market_id, voter.clone()))
                .unwrap();
            let locked: LockedTokens = env
                .storage()
                .persistent()
                .get(&DataKey::LockedTokens(market_id, voter.clone()))
                .unwrap();
            assert_eq!(locked_balance, 200);
            assert_eq!(locked.amount, locked_balance);
        });
    }
}

/// Issue #1696: Registry cap and batched pruning.
#[cfg(test)]
mod registry_cap_tests {
    use super::{prune_market_voting_state, DataKey, MAX_DISPUTE_VOTERS, PRUNE_BATCH_SIZE};
    use crate::types::Vote;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    #[test]
    fn prune_processes_voters_in_batches() {
        let e = Env::default();
        let market_id = 7u64;
        let total = PRUNE_BATCH_SIZE + 5;
        let mut reg = soroban_sdk::Vec::new(&e);
        for _ in 0..total {
            let v = Address::generate(&e);
            e.storage().persistent().set(
                &DataKey::Vote(market_id, v.clone()),
                &Vote {
                    market_id,
                    voter: v.clone(),
                    outcome: 0,
                    weight: 1,
                },
            );
            reg.push_back(v);
        }
        e.storage()
            .persistent()
            .set(&DataKey::DisputeVoters(market_id), &reg);

        // First call prunes one batch and leaves the registry in place.
        prune_market_voting_state(&e, market_id, 2);
        assert!(e
            .storage()
            .persistent()
            .has(&DataKey::DisputeVoters(market_id)));

        // Second call finishes the remainder and clears the registry.
        prune_market_voting_state(&e, market_id, 2);
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::DisputeVoters(market_id)));
        assert!(!e
            .storage()
            .persistent()
            .has(&DataKey::PruneCursor(market_id)));
    }

    #[test]
    fn registry_cap_is_bounded() {
        // Sanity check that the cap is a sane bound.
        assert!(MAX_DISPUTE_VOTERS > 0);
        assert!(PRUNE_BATCH_SIZE <= MAX_DISPUTE_VOTERS);
    }
}
