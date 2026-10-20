'use client';

import { useEffect, useState } from 'react';
import { useParams } from 'next/navigation';

type Bet = {
  market_id: string | number;
  outcome?: string;
  winning?: boolean;
  status?: string;
  claimed?: boolean;
  payout?: string | number;
};

export default function ClaimPayoutPage() {
  const { betId } = useParams<{ betId: string }>();
  const [bet, setBet] = useState<Bet | null>(null);
  const [message, setMessage] = useState('Loading bet…');
  const [claiming, setClaiming] = useState(false);
  useEffect(() => {
    fetch(`/api/v1/blockchain/bets/${encodeURIComponent(betId)}`)
      .then((r) => (r.ok ? r.json() : Promise.reject()))
      .then(setBet)
      .catch(() => setMessage('Unable to load this bet.'));
  }, [betId]);

  const reason = !bet
    ? 'Loading…'
    : bet.claimed || bet.status === 'claimed'
      ? 'This bet has already been claimed.'
      : bet.winning === false || bet.status === 'lost'
        ? 'This position did not win.'
        : '';
  async function claim() {
    if (!bet || reason) return;
    setClaiming(true);
    setMessage('');
    try {
      const res = await fetch(`/api/v1/blockchain/markets/${bet.market_id}/claim`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ bet_id: betId }),
      });
      if (!res.ok) throw new Error();
      const data = await res.json();
      setBet({ ...bet, claimed: true });
      setMessage(`Payout confirmed: ${data.amount ?? bet.payout ?? 'your winnings'}.`);
    } catch {
      setMessage('Claim failed. Please try again.');
    } finally {
      setClaiming(false);
    }
  }
  return (
    <main>
      <h1>Claim payout</h1>
      <p>Bet: {betId}</p>
      {bet && <p>Position: {bet.outcome ?? '—'}</p>}
      {reason && <p role="status">{reason}</p>}
      {message && !reason && <p role="status">{message}</p>}
      <button disabled={Boolean(reason) || claiming} onClick={claim}>
        {claiming ? 'Claiming…' : 'Claim winnings'}
      </button>
    </main>
  );
}
