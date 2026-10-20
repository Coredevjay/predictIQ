'use client';

import { useEffect, useState } from 'react';
import { api } from '@/lib/api/admin-client';

type Stats = Record<string, Record<string, unknown> | undefined>;

function Section({ title, rows }: { title: string; rows: Record<string, unknown> }) {
  return (
    <section>
      <h2>{title}</h2>
      {Object.entries(rows).map(([k, v]) => (
        <p key={k}>
          {k}: {String(v)}
        </p>
      ))}
    </section>
  );
}

export default function AuditStatistics() {
  const [data, setData] = useState<Stats>();
  const [error, setError] = useState('');

  useEffect(() => {
    api
      .getAuditStatistics()
      .then((d) => setData(d as Stats))
      .catch(() => setError('Unable to load audit statistics'));
  }, []);

  if (error) return <p role="alert">{error}</p>;
  if (!data) return <p>Loading…</p>;

  const actors = data.actions_by_actor ?? data.by_actor ?? {};
  const days = data.actions_by_day ?? data.by_day ?? {};
  if (!Object.keys(actors).length && !Object.keys(days).length) {
    return <p role="status">No audit events recorded.</p>;
  }

  return (
    <main>
      <h1>Audit statistics</h1>
      <Section title="Actions by actor" rows={actors} />
      <Section title="Actions by day" rows={days} />
    </main>
  );
}
