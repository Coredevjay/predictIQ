use crate::errors::ErrorCode;
use crate::modules::{markets, sac};
use crate::types::{ConfigKey, MarketStatus};
use soroban_sdk::{contracttype, Address, Env};

#[contracttype]
#[derive(Clone)]
pub struct ResolutionMetrics {
    pub winner_count: u32,
    pub total_winning_stake: i128,
    pub gas_estimate: u64,
}

pub fn file_dispute(e: &Env, disciplinarian: Address, market_id: u64) -> Result<(), ErrorCode> {
    disciplinarian.require_auth();

    let mut market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    if market.status != MarketStatus::PendingResolution {
        return Err(ErrorCode::MarketNotPendingResolution);
    }

    let pending_ts = market
        .pending_resolution_timestamp
        .ok_or(ErrorCode::ResolutionNotReady)?;
    let dispute_window = markets::get_market_dispute_window(e, market_id);
    if e.ledger().timestamp() >= pending_ts + dispute_window {
        return Err(ErrorCode::DisputeWindowClosed);
    }

    // Issue #1697: filing a dispute must cost something so that finalisation
    // cannot be frozen for free. The bond is refundable if the dispute
    // succeeds and slashed (sent to the market pool) if it fails.
    let bond_amount: i128 = e
        .storage()
        .persistent()
        .get(&ConfigKey::DisputeBond)
        .unwrap_or(crate::types::DEFAULT_DISPUTE_BOND);
    if bond_amount <= 0 {
        return Err(ErrorCode::InvalidAmount);
    }
    let bond_token = markets::get_market_bond_token(e, market_id);
    sac::safe_transfer(e, &bond_token, &disciplinarian, &e.current_contract_address(), bond_amount)?;
    markets::set_dispute_bond(e, market_id, &disciplinarian, bond_amount);

    market.status = MarketStatus::Disputed;
    market.dispute_timestamp = Some(e.ledger().timestamp());
    // Extend resolution deadline by the full dispute window duration
    market.resolution_deadline += dispute_window;
    let new_deadline = market.resolution_deadline;

    markets::update_market(e, market);

    crate::modules::events::emit_dispute_filed(
        e,
        market_id,
        disciplinarian,
        new_deadline,
        bond_amount,
    );

    Ok(())
}

/// Issue #23: payout_mode is immutable after creation — never mutated here.
/// Issue #24: Use actual winner_counts instead of heuristic.
/// Issue #35: Calculate and emit actual total payout.
pub fn resolve_market(e: &Env, market_id: u64, winning_outcome: u32) -> Result<(), ErrorCode> {
    let mut market = markets::get_market(e, market_id).ok_or(ErrorCode::MarketNotFound)?;

    // Issue #1186: Resolved/Cancelled are terminal states (INVARIANTS.md §3) and
    // must never be re-entered. Without this guard an admin could call this path
    // twice with different outcomes after bettors already claimed under the
    // first outcome, allowing overpayment against unchanged total_staked.
    if market.status == MarketStatus::Resolved || market.status == MarketStatus::Cancelled {
        return Err(ErrorCode::CannotChangeOutcome);
    }

    if winning_outcome >= market.options.len() {
        return Err(ErrorCode::InvalidOutcome);
    }

    // Issue #1698: resolving to an outcome with zero stake would leave the
    // whole pool permanently locked (claim_winnings returns NoWinnings to
    // everyone and Resolved is terminal). Reject such resolutions.
    let winning_stake = market.outcome_stakes.get(winning_outcome).unwrap_or(0);
    if winning_stake <= 0 {
        return Err(ErrorCode::ZeroWinningStake);
    }

    // payout_mode is intentionally NOT mutated here — it is fixed at creation
    // time and must remain stable throughout PendingResolution and Disputed
    // phases so that gas and distribution path calculations are consistent.

    market.status = MarketStatus::Resolved;
    market.winning_outcome = Some(winning_outcome);
    market.resolved_at = Some(e.ledger().timestamp());

    // Issue #35: Calculate actual total payout for the event
    let total_payout = market.total_staked;

    markets::update_market(e, market);

    // Issue #1697: settle the dispute bond now that the market is finalised.
    // If the disputed outcome matches the final outcome the dispute succeeded
    // and the bond is refunded; otherwise it is slashed into the market pool.
    settle_dispute_bond(e, market_id, winning_outcome);

    let admin = crate::modules::admin::get_admin(e).unwrap_or(e.current_contract_address());
    crate::modules::events::emit_resolution_finalized(
        e,
        market_id,
        admin,
        winning_outcome,
        total_payout, // Issue #35: actual payout, not hardcoded 0
    );

    Ok(())
}

/// Issue #1697: refund or slash the dispute bond recorded when the dispute was
/// filed. A successful dispute (the final outcome differs from the outcome
/// that was pending when the dispute was filed) refunds the bond to the
/// disputer; a failed dispute transfers it into the market pool.
fn settle_dispute_bond(e: &Env, market_id: u64, winning_outcome: u32) {
    let record = match markets::get_dispute_bond(e, market_id) {
        Some(r) => r,
        None => return,
    };
    markets::clear_dispute_bond(e, market_id);

    let bond_token = markets::get_market_bond_token(e, market_id);
    let contract = e.current_contract_address();
    if record.disputed_outcome != winning_outcome {
        // Dispute succeeded: refund the bond to the disputer.
        let _ = sac::safe_transfer(e, &bond_token, &contract, &record.disputer, record.amount);
    } else {
        // Dispute failed: slash the bond into the market pool.
        let _ = sac::safe_transfer(e, &bond_token, &contract, &markets::get_market_pool(e, market_id), record.amount);
    }
}

pub fn set_max_push_payout_winners(e: &Env, threshold: u32) -> Result<(), ErrorCode> {
    crate::modules::admin::require_admin(e)?;
    e.storage()
        .persistent()
        .set(&ConfigKey::MaxPushPayoutWinners, &threshold);
    e.storage().persistent().extend_ttl(
        &ConfigKey::MaxPushPayoutWinners,
        crate::types::GOV_TTL_LOW_THRESHOLD,
        crate::types::GOV_TTL_HIGH_THRESHOLD,
    );
    Ok(())
}

pub fn get_max_push_payout_winners(e: &Env) -> u32 {
    e.storage()
        .persistent()
        .get(&ConfigKey::MaxPushPayoutWinners)
        .unwrap_or(crate::types::MAX_PUSH_PAYOUT_WINNERS)
}

// Batch resolution metrics for monitoring
pub fn get_resolution_metrics(e: &Env, market_id: u64, outcome: u32) -> ResolutionMetrics {
    let winner_count = markets::count_bets_for_outcome(e, market_id, outcome);
    let total_stake = match markets::get_market(e, market_id) {
        Some(m) => m.outcome_stakes.get(outcome).unwrap_or(0),
        None => 0,
    };

    let gas_estimate = 100_000 + (winner_count as u64 * 50_000);

    ResolutionMetrics {
        winner_count,
        total_winning_stake: total_stake,
        gas_estimate,
    }
}
