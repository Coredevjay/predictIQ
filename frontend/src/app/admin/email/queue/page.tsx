'use client';

import { useEffect, useState } from 'react';
import { api } from '@/lib/api/admin-client';

type Job = { id: string; recipient?: string; template?: string; failure?: string; status?: string };

export default function EmailQueue() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [stats, setStats] = useState<Record<string, unknown>>({});
  const [error, setError] = useState('');

  const load = async () => {
    try {
      const d = (await api.getEmailDeadLetter()) as {
        jobs?: Job[];
        items?: Job[];
        stats?: Record<string, unknown>;
      };
      setJobs(d.jobs ?? d.items ?? []);
      setStats(d.stats ?? {});
    } catch {
      setError('Unable to load email queue');
    }
  };

  useEffect(() => {
    load();
  }, []);

  const requeue = async (id: string) => {
    try {
      await api.requeueEmailJob(id);
      setJobs((x) => x.filter((j) => j.id !== id));
    } catch {
      setError('Requeue failed');
    }
  };

  return (
    <main>
      <h1>Email queue</h1>
      {error && <p role="alert">{error}</p>}
      <pre>{JSON.stringify(stats, null, 2)}</pre>
      {jobs.length ? (
        <table>
          <thead>
            <tr>
              <th>Recipient</th>
              <th>Template</th>
              <th>Failure</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {jobs.map((j) => (
              <tr key={j.id}>
                <td>{j.recipient}</td>
                <td>{j.template}</td>
                <td>{j.failure}</td>
                <td>
                  <button onClick={() => requeue(j.id)}>Requeue</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p role="status">No dead-letter jobs.</p>
      )}
    </main>
  );
}
