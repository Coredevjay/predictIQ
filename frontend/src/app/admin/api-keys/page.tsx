'use client';

import { useCallback, useEffect, useState } from 'react';
import { api, ADMIN_KEY_STORAGE, type AdminApiKey } from '@/lib/api/admin-client';

export default function ApiKeys() {
  const [keys, setKeys] = useState<AdminApiKey[]>([]);
  const [message, setMessage] = useState('');
  const [label, setLabel] = useState('');
  const [newKey, setNewKey] = useState('');
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    try {
      const d = await api.listApiKeys();
      setKeys(Array.isArray(d) ? d : (d.keys ?? []));
    } catch {
      setMessage('Unable to load API keys.');
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const rotate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!label.trim()) return;
    try {
      const res = await api.rotateApiKey({ key_label: label.trim() });
      // Only swap the stored key once the server has confirmed the rotation.
      sessionStorage.setItem(ADMIN_KEY_STORAGE, res.new_key);
      setNewKey(res.new_key);
      setLabel('');
      setMessage('Key rotated. Copy the new key now; it will not be shown again.');
      load();
    } catch {
      setMessage('Rotation failed. Your current key is unchanged.');
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(newKey);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <main>
      <h1>API keys</h1>
      {message && <p role="alert">{message}</p>}
      {newKey && (
        <p>
          <code data-testid="new-key">{newKey}</code>{' '}
          <button onClick={copy}>{copied ? 'Copied' : 'Copy'}</button>
        </p>
      )}
      <form onSubmit={rotate}>
        <label>
          Key label
          <input value={label} onChange={(e) => setLabel(e.target.value)} required />
        </label>
        <button type="submit">Rotate key</button>
      </form>
      <ul>
        {keys.map((k) => (
          <li key={k.id}>
            {k.name ?? k.label ?? k.id} {k.created_at && `(${k.created_at})`}
          </li>
        ))}
      </ul>
    </main>
  );
}
