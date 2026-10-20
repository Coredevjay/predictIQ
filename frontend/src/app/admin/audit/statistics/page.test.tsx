import { render, screen } from '@testing-library/react';
import AuditStatistics from './page';

it('fetches statistics with the admin key', async () => {
  sessionStorage.setItem('predictiq-admin-key', 'k');
  const f = jest.fn().mockResolvedValue({
    ok: true,
    status: 200,
    statusText: '',
    headers: new Headers(),
    text: async () => JSON.stringify({ actions_by_actor: { alice: 3 } }),
  });
  global.fetch = f as unknown as typeof fetch;
  render(<AuditStatistics />);
  expect(await screen.findByText('alice: 3')).toBeInTheDocument();
  expect(f.mock.calls[0][0]).toContain('/api/v1/audit/statistics');
  expect(f.mock.calls[0][1].headers['X-API-Key']).toBe('k');
});
