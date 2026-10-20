'use client';
import { useEffect, useState } from 'react';
import { api } from '../../lib/api/public-client';

type State = { status: 'loading' } | { status: 'error' } | { status: 'ready'; text: string };

/** Applies a Pyth-style exponent (price * 10^expo) to an integer price. */
export function formatOraclePrice(raw: unknown, expo: unknown): string | null {
  const price = Number(raw);
  if (raw === null || raw === undefined || raw === '' || !Number.isFinite(price)) return null;
  const e = Number(expo ?? 0);
  if (!Number.isInteger(e)) return null;
  const value = price * Math.pow(10, e);
  if (!Number.isFinite(value)) return null;
  return value.toLocaleString('en-US', { maximumFractionDigits: Math.max(0, Math.min(-e, 20)) });
}

export default function OraclePriceWidget({ marketId }: { marketId: string | number }) {
  const [state, setState] = useState<State>({ status: 'loading' });

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    api
      .getOracleResult(marketId, controller.signal)
      .then((d) => {
        const r = d as Record<string, unknown>;
        const expo = r.expo ?? r.exponent ?? (r.decimals !== undefined ? -Number(r.decimals) : 0);
        const text = formatOraclePrice(r.price ?? r.value, expo);
        setState(text === null ? { status: 'error' } : { status: 'ready', text });
      })
      .catch(() => {
        if (!controller.signal.aborted) setState({ status: 'error' });
      });
    return () => controller.abort();
  }, [marketId]);

  return (
    <output aria-label="Oracle price">
      {state.status === 'loading' ? 'Loading price…' : state.status === 'error' ? 'Price unavailable' : state.text}
    </output>
  );
}
