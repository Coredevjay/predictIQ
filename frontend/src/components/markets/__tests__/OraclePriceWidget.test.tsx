import React from 'react';
import { render, screen } from '@testing-library/react';
import OraclePriceWidget, { formatOraclePrice } from '../OraclePriceWidget';

const mockGet = jest.fn();
jest.mock('../../../lib/api/public-client', () => ({
  api: { getOracleResult: (...a: unknown[]) => mockGet(...a) },
}));

describe('OraclePriceWidget', () => {
  it('applies the exponent', () => {
    expect(formatOraclePrice(12345, -2)).toBe('123.45');
  });

  it('shows loading then the formatted price', async () => {
    mockGet.mockResolvedValue({ price: 250000, expo: -4 });
    render(<OraclePriceWidget marketId={1} />);
    expect(screen.getByLabelText('Oracle price')).toHaveTextContent('Loading price…');
    expect(await screen.findByText('25')).toBeInTheDocument();
  });

  it('shows an error state when the request fails', async () => {
    mockGet.mockRejectedValue(new Error('boom'));
    render(<OraclePriceWidget marketId={1} />);
    expect(await screen.findByText('Price unavailable')).toBeInTheDocument();
  });
});
