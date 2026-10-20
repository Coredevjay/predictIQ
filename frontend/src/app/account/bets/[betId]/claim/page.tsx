'use client';

import { useCallback, useEffect, useState } from 'react';
import { useParams } from 'next/navigation';
import { useAsync } from '@/lib/hooks/useAsync';
import { useTransaction } from '@/lib/hooks/useTransaction';
import { describeWriteError } from '@/lib/api/contractErrors';
import { api } from '@/lib/api/public-client';
import { isSorobanConfigured, invokeContract } from '@/lib/wallet/soroban';
import { useWallet } from '@/lib/wallet/WalletProvider';

interface Bet {
  id?: string;
  market_id: string | number;
  outcome?: string;
  winning?: boolean;
  status?: string;
  claimed?: boolean;
  payout?: string | number;
}

/**
 * Load the bet from the connected wallet's bet history.
 *
 * There is no `/api/v1/blockchain/bets/{id}` route (#1756), so the bet is
 * found within the typed client's `getUserBets` response for the connected
 * address. Returns `null` when the bet is not in that address's history.
 */
async function findBet(address: string, betId: string, signal: AbortSignal): Promise<Bet | null> {
  const raw = (await api.getUserBets(address, undefined, signal)) as { bets?: Bet[] };
  const bets = raw.bets ?? [];
  return bets.find((b) => String(b.id ?? '') === betId) ?? null;
}

export default function ClaimPayoutPage() {
  const { betId } = useParams<{ betId: string }>();
  const wallet = useWallet();
  const tx = useTransaction<{ marketId: string }>();
  const [message, setMessage] = useState<string | null>(null);

  const load = useCallback(
    async (signal: AbortSignal): Promise<Bet | null> => {
      if (!wallet.address) return null;
      return findBet(wallet.address, betId, signal);
    },
    [wallet.address, betId]
  );

  const {
    data: bet,
    status,
    error: loadError,
    retry: reload,
  } = useAsync(load, { immediate: false });

  // The wallet address often arrives after first paint, so kick the load off
  // once it is known (and again if the user switches accounts).
  useEffect(() => {
    if (wallet.address) void reload();
  }, [wallet.address, reload]);

  const claimError =
    tx.status === 'failed' && tx.error ? describeWriteError(tx.error, 'claim') : null;
  const onChainReady = isSorobanConfigured();

  // `tx.status` is the reactive source of truth: surface success and refresh
  // the bet once the transaction lands. Reading `tx.status` right after
  // `await tx.run(...)` would see the pre-run value from this render's closure.
  useEffect(() => {
    if (tx.status === 'confirmed') {
      setMessage('Payout confirmed.');
      void reload();
    }
  }, [tx.status, reload]);

  const reason = !wallet.address
    ? 'Connect your wallet to claim this payout.'
    : status === 'loading'
      ? 'Loading…'
      : !bet
        ? 'This bet was not found for the connected wallet.'
        : bet.claimed || bet.status === 'claimed'
          ? 'This bet has already been claimed.'
          : bet.winning === false || bet.status === 'lost'
            ? 'This position did not win.'
            : '';

  async function claim() {
    if (!bet || reason || !wallet.address) return;
    setMessage(null);

    await tx.run({
      sign: async () => ({ marketId: String(bet.market_id) }),
      submit: async (signed) =>
        invokeContract(
          wallet.address!,
          {
            method: 'claim_winnings',
            args: [{ type: 'u64', value: signed.marketId }],
          },
          wallet.signAndSubmit
        ),
    });
  }

  const busy = tx.status === 'pending' || tx.status === 'signed' || tx.status === 'submitted';

  return (
    <main>
      <h1>Claim payout</h1>
      <p>Bet: {betId}</p>
      {bet && <p>Position: {bet.outcome ?? '—'}</p>}
      {reason && <p role="status">{reason}</p>}
      {loadError && <p role="alert">Unable to load this bet.</p>}

      {!onChainReady && (
        <p role="alert">
          On-chain submission is not configured for this deployment. Set{' '}
          <code>NEXT_PUBLIC_CONTRACT_ID</code> and <code>NEXT_PUBLIC_SOROBAN_RPC_URL</code> to
          enable claiming.
        </p>
      )}

      {claimError && <p role="alert">{claimError}</p>}
      {message && <p role="status">{message}</p>}

      <button disabled={Boolean(reason) || busy || !onChainReady} onClick={claim}>
        {tx.status === 'pending' ? 'Awaiting signature…' : busy ? 'Claiming…' : 'Claim winnings'}
      </button>
    </main>
  );
}
