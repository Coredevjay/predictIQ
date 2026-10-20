'use client';

import { useCallback, useEffect, useState } from 'react';
import { useAsync } from '@/lib/hooks/useAsync';
import { useTransaction } from '@/lib/hooks/useTransaction';
import { describeWriteError } from '@/lib/api/contractErrors';
import { api } from '@/lib/api/public-client';
import { isSorobanConfigured, invokeContract } from '@/lib/wallet/soroban';
import { useWallet } from '@/lib/wallet/WalletProvider';

interface DisputedOutcome {
  outcome_index: number;
  label: string;
  vote_count: number;
}

interface DisputeStatusResponse {
  market_id: string;
  is_disputed: boolean;
  /** May contain more than one entry — a dispute can produce multiple winning outcomes. */
  disputed_outcomes: DisputedOutcome[];
}

/**
 * Read the market's dispute state from the typed client.
 *
 * There is no `/api/v1/blockchain/markets/{id}/disputes` route in
 * `services/api/openapi.yaml` (#1755), so the status is derived from the
 * market view the backend does serve. The disputed-outcome list is optional
 * on that view; when it is absent we render "no dispute filed yet" rather
 * than fabricating an endpoint.
 */
async function fetchDisputeStatus(
  marketId: string,
  signal: AbortSignal
): Promise<DisputeStatusResponse> {
  const raw = (await api.getBlockchainMarket(marketId, signal)) as {
    id?: string | number;
    is_disputed?: boolean;
    disputed_outcomes?: DisputedOutcome[];
  };
  return {
    market_id: String(raw.id ?? marketId),
    is_disputed: raw.is_disputed ?? false,
    disputed_outcomes: raw.disputed_outcomes ?? [],
  };
}

interface DisputePageProps {
  params: { marketId: string };
}

/**
 * Lets an eligible party raise a dispute on a market and shows every
 * currently disputed winner outcome — a dispute can resolve to more than
 * one winner (see contracts/predict-iq/src/test_disputes_winner_count.rs),
 * so this must render the full list, not just the first result.
 *
 * Filing goes through the contract's `file_dispute` entry point, signed by
 * the connected wallet (#1755). When the deployment has no contract id / RPC
 * configured, the button is disabled with a clear message instead of posting
 * to a nonexistent backend route.
 */
export default function DisputePage({ params }: DisputePageProps) {
  const { marketId } = params;
  const [reason, setReason] = useState('');
  const wallet = useWallet();
  const tx = useTransaction<{ marketId: string }>();

  const load = useCallback(
    (signal: AbortSignal) => fetchDisputeStatus(marketId, signal),
    [marketId]
  );
  const { data, status, error: loadError, retry: reload } = useAsync(load, { immediate: true });
  const loading = status === 'loading';

  // Clear the draft reason once the dispute lands on-chain.
  useEffect(() => {
    if (tx.status === 'confirmed') setReason('');
  }, [tx.status]);

  const filingError =
    tx.status === 'failed' && tx.error ? describeWriteError(tx.error, 'dispute') : null;
  const onChainReady = isSorobanConfigured();

  async function handleFileDispute() {
    if (!wallet.address) {
      await wallet.connect();
      return;
    }
    if (!onChainReady) return;

    await tx.run({
      sign: async () => ({ marketId }),
      submit: async () =>
        invokeContract(
          wallet.address,
          { method: 'file_dispute', args: [{ type: 'u64', value: marketId }] },
          wallet.signAndSubmit
        ),
    });
    // Re-read the status regardless of the outcome: on success it reflects the
    // new dispute, and on failure/rejection it simply confirms nothing changed.
    // (`tx.status` here is the value captured before `run` resolved, so it must
    // not be used to decide whether to reload.)
    void reload();
  }

  const disputedOutcomes = data?.disputed_outcomes ?? [];
  const busy = tx.status === 'pending' || tx.status === 'signed' || tx.status === 'submitted';

  return (
    <main className="dispute-page">
      <h1>Dispute resolution</h1>

      {loading && <p>Loading dispute status…</p>}
      {loadError && <p role="alert">Unable to load dispute status: {loadError.message}</p>}

      {data && (
        <section aria-labelledby="disputed-outcomes-heading">
          <h2 id="disputed-outcomes-heading">Disputed winner outcomes</h2>
          {disputedOutcomes.length === 0 ? (
            <p>No dispute has been filed for this market yet.</p>
          ) : (
            <ul className="dispute-page__outcomes">
              {disputedOutcomes.map((o) => (
                <li key={o.outcome_index}>
                  <span className="dispute-page__outcome-label">{o.label}</span>
                  <span className="dispute-page__outcome-votes">{o.vote_count} votes</span>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <section aria-labelledby="file-dispute-heading">
        <h2 id="file-dispute-heading">Raise a dispute</h2>

        {!onChainReady && (
          <p role="alert">
            On-chain submission is not configured for this deployment. Set{' '}
            <code>NEXT_PUBLIC_CONTRACT_ID</code> and <code>NEXT_PUBLIC_SOROBAN_RPC_URL</code> to
            enable filing a dispute.
          </p>
        )}

        <label htmlFor="dispute-reason">Reason</label>
        <textarea
          id="dispute-reason"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          disabled={busy}
          required
        />
        <button
          type="button"
          onClick={handleFileDispute}
          disabled={!reason || busy || !onChainReady}
        >
          {tx.status === 'pending'
            ? 'Awaiting signature…'
            : tx.status === 'signed' || tx.status === 'submitted'
              ? 'Submitting dispute…'
              : 'File dispute'}
        </button>

        {tx.status === 'confirmed' && <p role="status">Dispute filed successfully.</p>}
        {filingError && <p role="alert">{filingError}</p>}
      </section>
    </main>
  );
}
