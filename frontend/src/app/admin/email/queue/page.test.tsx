import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import EmailQueue from './page';

const res = (body: unknown) => ({
  ok: true,
  status: 200,
  statusText: '',
  headers: new Headers(),
  text: async () => JSON.stringify(body),
});

it('loads and requeues through the authenticated admin client', async () => {
  sessionStorage.setItem('predictiq-admin-key', 'k');
  const f = jest
    .fn()
    .mockImplementation((_u: string, init: RequestInit) =>
      Promise.resolve(
        res(init.method === 'POST' ? {} : { jobs: [{ id: 'j1', recipient: 'a@b.c' }] })
      )
    );
  global.fetch = f as unknown as typeof fetch;
  render(<EmailQueue />);
  await userEvent.click(await screen.findByRole('button', { name: /requeue/i }));
  await waitFor(() => expect(screen.getByRole('status')).toBeInTheDocument());
  const [getUrl, getInit] = f.mock.calls[0];
  expect(getUrl).toContain('/api/v1/email/queue/dead-letter');
  expect(getInit.headers['X-API-Key']).toBe('k');
  const post = f.mock.calls[1][1];
  expect(post.headers['X-API-Key']).toBe('k');
  expect(post.headers['Idempotency-Key']).toBeTruthy();
});
