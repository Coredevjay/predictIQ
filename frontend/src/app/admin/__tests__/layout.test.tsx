jest.mock('next/navigation', () => ({ usePathname: () => '/admin' }));
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import AdminLayout from '../layout';

const KEY = 'predictiq-admin-key';

function mockFetch(ok: boolean) {
  return jest.fn().mockResolvedValue({
    ok,
    status: ok ? 200 : 401,
    statusText: ok ? 'OK' : 'Unauthorized',
    headers: new Headers(),
    text: async () => (ok ? '{"keys":[]}' : '{"code":"UNAUTHORIZED"}'),
    json: async () => ({ code: 'UNAUTHORIZED' }),
  });
}

const renderLayout = () =>
  render(
    <AdminLayout>
      <div>admin content</div>
    </AdminLayout>
  );

describe('AdminAuthGate', () => {
  beforeEach(() => sessionStorage.clear());

  it('shows the form and makes no request without a stored key', () => {
    const f = mockFetch(true);
    global.fetch = f as unknown as typeof fetch;
    renderLayout();
    expect(screen.queryByText('admin content')).not.toBeInTheDocument();
    expect(f).not.toHaveBeenCalled();
  });

  it('validates a submitted key against the api-keys endpoint before unlocking', async () => {
    const f = mockFetch(true);
    global.fetch = f as unknown as typeof fetch;
    renderLayout();
    await userEvent.type(document.querySelector('input')!, 'good-key');
    await userEvent.click(screen.getAllByRole('button')[0]);
    await waitFor(() => expect(screen.getByText('admin content')).toBeInTheDocument());
    expect(f.mock.calls[0][0]).toContain('/api/v1/admin/api-keys');
    expect(f.mock.calls[0][1].headers['X-API-Key']).toBe('good-key');
    expect(sessionStorage.getItem(KEY)).toBe('good-key');
  });

  it('rejects an invalid key without storing it', async () => {
    global.fetch = mockFetch(false) as unknown as typeof fetch;
    renderLayout();
    await userEvent.type(document.querySelector('input')!, 'bad-key');
    await userEvent.click(screen.getAllByRole('button')[0]);
    await waitFor(() => expect(screen.getByText(/invalid api key/i)).toBeInTheDocument());
    expect(screen.queryByText('admin content')).not.toBeInTheDocument();
    expect(sessionStorage.getItem(KEY)).toBeNull();
  });
});
