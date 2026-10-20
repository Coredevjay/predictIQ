'use client';

import { useEffect, useState } from 'react';

export default function OraclePriceWidget({ marketId }: { marketId: string | number }) {
  const [price, setPrice] = useState<number | null>(null);

  useEffect(() => {
    fetch(`/api/v1/blockchain/oracle/${marketId}`)
      .then((r) => r.json())
      .then((d) => {
        const n = Number(d.price ?? d.value);
        setPrice(Number.isFinite(n) && Math.abs(n) < Number.MAX_SAFE_INTEGER ? n : null);
      })
      .catch(() => setPrice(null));
  }, [marketId]);

  return <output aria-label="Oracle price">{price === null ? 'Price unavailable' : price}</output>;
}
