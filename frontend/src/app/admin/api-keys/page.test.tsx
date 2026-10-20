import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ApiKeys from './page';

const KEY = 'predictiq-admin-key';
const res = (ok: boolean, body: unknown) => ({
  ok,
  status: ok ? 200 : 422,
  statusText: '',
  headers: new Headers(),
  text: async () => JSON.stringify(body),
  json: async () => body,
});

describe('ApiKeys rotate', () => {
  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem(KEY, 'old-key');
  });

  it('sends the label, shows the new key and swaps the stored key on success', async () => {
    const f = jest
      .fn()
      .mockImplementation((_u: string, init: RequestInit) =>
        Promise.resolve(
          init.method === 'POST'
            ? res(true, { new_key: 'new-secret', new_key_label: 'l' })
            : res(true, { keys: [] })
        )
      );
    global.fetch = f as unknown as typeof fetch;
    render(<ApiKeys />);
    await userEvent.type(screen.getByLabelText(/key label/i), 'ci-1');
    await userEvent.click(screen.getByRole('button', { name: /rotate key/i }));
    await waitFor(() => expect(screen.getByTestId('new-key')).toHaveTextContent('new-secret'));
    const post = f.mock.calls.find((c) => c[1].method === 'POST')!;
    expect(JSON.parse(post[1].body)).toEqual({ key_label: 'ci-1' });
    expect(sessionStorage.getItem(KEY)).toBe('new-secret');
  });

  it('keeps the stored key when rotation fails', async () => {
    global.fetch = jest
      .fn()
      .mockImplementation((_u: string, init: RequestInit) =>
        Promise.resolve(
          init.method === 'POST' ? res(false, { code: 'X' }) : res(true, { keys: [] })
        )
      ) as unknown as typeof fetch;
    render(<ApiKeys />);
    await userEvent.type(screen.getByLabelText(/key label/i), 'ci-1');
    await userEvent.click(screen.getByRole('button', { name: /rotate key/i }));
    await waitFor(() => expect(screen.getByText(/rotation failed/i)).toBeInTheDocument());
    expect(sessionStorage.getItem(KEY)).toBe('old-key');
  });
});
