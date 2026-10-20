/**
 * Tests for the Soroban write client used by the dispute and claim pages
 * (#1755 / #1756).
 *
 * The SDK is mocked so these run without a live RPC endpoint, and so the tests
 * pin the exact contract method names and argument encodings the pages rely on
 * rather than the SDK's internals.
 */

const mockGetAccount = jest.fn();
const mockPrepareTransaction = jest.fn();
const mockSendTransaction = jest.fn();
const mockGetTransaction = jest.fn();
const mockBuild = jest.fn();
const mockAddOperation = jest.fn();
const mockSetTimeout = jest.fn();
const mockCall = jest.fn();
const mockToEnvelope = jest.fn();
const mockFromXDR = jest.fn();

jest.mock('@stellar/stellar-sdk', () => {
  const actualSwitch = { name: 'scvI128' };
  return {
    BASE_FEE: '100',
    Contract: jest.fn().mockImplementation((id: string) => ({
      call: (...args: unknown[]) => mockCall(id, ...args),
    })),
    Networks: { TESTNET: 'Test SDF Network ; September 2015' },
    TransactionBuilder: Object.assign(
      jest.fn().mockImplementation(() => ({
        addOperation: (op: unknown) => {
          mockAddOperation(op);
          return txBuilder;
        },
        setTimeout: (t: number) => {
          mockSetTimeout(t);
          return txBuilder;
        },
        build: () => mockBuild(),
      })),
      { fromXDR: (...args: unknown[]) => mockFromXDR(...args) }
    ),
    nativeToScVal: (value: unknown, opts: { type: string }) => ({
      __scval: true,
      value,
      type: opts.type,
      switch: () => actualSwitch,
    }),
    scValToNative: (v: { value: unknown }) => v.value,
    rpc: {
      Server: jest.fn().mockImplementation(() => ({
        getAccount: mockGetAccount,
        prepareTransaction: mockPrepareTransaction,
        sendTransaction: mockSendTransaction,
        getTransaction: mockGetTransaction,
      })),
      Api: {
        GetTransactionStatus: {
          SUCCESS: 'SUCCESS',
          FAILED: 'FAILED',
          NOT_FOUND: 'NOT_FOUND',
        },
      },
    },
    xdr: {},
  };
});

// The mock above needs a stable object returned by the TransactionBuilder chain
// methods; declared after the factory to avoid a temporal-dead-zone reference.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let txBuilder: any;

import {
  buildInvocation,
  encodeContractArg,
  invokeContract,
  isSorobanConfigured,
  resolveSorobanClientConfig,
  submitInvocation,
  type SorobanClientConfig,
} from '../soroban';

const CONFIG: SorobanClientConfig = {
  contractId: 'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4',
  rpcUrl: 'https://soroban-testnet.stellar.org',
  networkPassphrase: 'Test SDF Network ; September 2015',
};

describe('soroban client configuration', () => {
  const OLD_ENV = process.env;

  beforeEach(() => {
    process.env = { ...OLD_ENV };
    delete process.env.NEXT_PUBLIC_CONTRACT_ID;
    delete process.env.NEXT_PUBLIC_SOROBAN_RPC_URL;
    delete process.env.NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE;
  });

  afterAll(() => {
    process.env = OLD_ENV;
  });

  it('reports unconfigured when the contract id is missing', () => {
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL = 'https://rpc.example';
    expect(resolveSorobanClientConfig()).toBeNull();
    expect(isSorobanConfigured()).toBe(false);
  });

  it('reports unconfigured when the RPC url is missing', () => {
    process.env.NEXT_PUBLIC_CONTRACT_ID = CONFIG.contractId;
    expect(resolveSorobanClientConfig()).toBeNull();
  });

  it('treats whitespace-only values as unset', () => {
    process.env.NEXT_PUBLIC_CONTRACT_ID = '   ';
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL = '  ';
    expect(isSorobanConfigured()).toBe(false);
  });

  it('defaults the network passphrase to testnet', () => {
    process.env.NEXT_PUBLIC_CONTRACT_ID = CONFIG.contractId;
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL = CONFIG.rpcUrl;
    expect(resolveSorobanClientConfig()).toEqual({
      contractId: CONFIG.contractId,
      rpcUrl: CONFIG.rpcUrl,
      networkPassphrase: 'Test SDF Network ; September 2015',
    });
  });

  it('honours an explicit network passphrase', () => {
    process.env.NEXT_PUBLIC_CONTRACT_ID = CONFIG.contractId;
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL = CONFIG.rpcUrl;
    process.env.NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE =
      'Public Global Stellar Network ; September 2015';
    expect(resolveSorobanClientConfig()?.networkPassphrase).toBe(
      'Public Global Stellar Network ; September 2015'
    );
  });
});

describe('encodeContractArg', () => {
  it('encodes u64 through BigInt', () => {
    const scval = encodeContractArg({ type: 'u64', value: '9007199254740993' });
    expect(scval).toMatchObject({ type: 'u64', value: 9007199254740993n });
  });

  it('encodes u32 as a number', () => {
    const scval = encodeContractArg({ type: 'u32', value: 3 });
    expect(scval).toMatchObject({ type: 'u32', value: 3 });
  });

  it('encodes i128 through BigInt', () => {
    const scval = encodeContractArg({ type: 'i128', value: '1000000' });
    expect(scval).toMatchObject({ type: 'i128', value: 1000000n });
  });

  it('encodes addresses as address scvals', () => {
    const scval = encodeContractArg({ type: 'address', value: 'GABC' });
    expect(scval).toMatchObject({ type: 'address', value: 'GABC' });
  });
});

describe('buildInvocation', () => {
  beforeEach(() => {
    jest.clearAllMocks();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    txBuilder = {
      addOperation: undefined,
      setTimeout: undefined,
      build: undefined,
    };
    mockGetAccount.mockResolvedValue({ accountId: () => 'GSOURCE' });
    mockPrepareTransaction.mockResolvedValue({
      toEnvelope: () => ({ toXDR: () => 'PREPARED_XDR' }),
    });
  });

  it('builds a file_dispute invocation with a u64 market id', async () => {
    const server = { getAccount: mockGetAccount, prepareTransaction: mockPrepareTransaction };

    const xdrOut = await buildInvocation(server as never, CONFIG, 'GSOURCE', {
      method: 'file_dispute',
      args: [{ type: 'u64', value: '123' }],
    });

    expect(xdrOut).toBe('PREPARED_XDR');
    expect(mockCall).toHaveBeenCalledWith(
      CONFIG.contractId,
      'file_dispute',
      expect.objectContaining({ type: 'u64', value: 123n })
    );
    expect(mockGetAccount).toHaveBeenCalledWith('GSOURCE');
  });

  it('builds a claim_winnings invocation', async () => {
    const server = { getAccount: mockGetAccount, prepareTransaction: mockPrepareTransaction };

    await buildInvocation(server as never, CONFIG, 'GSOURCE', {
      method: 'claim_winnings',
      args: [{ type: 'u64', value: '7' }],
    });

    expect(mockCall).toHaveBeenCalledWith(
      CONFIG.contractId,
      'claim_winnings',
      expect.objectContaining({ value: 7n })
    );
  });

  it('passes every argument positionally', async () => {
    const server = { getAccount: mockGetAccount, prepareTransaction: mockPrepareTransaction };

    await buildInvocation(server as never, CONFIG, 'GSOURCE', {
      method: 'cast_vote',
      args: [
        { type: 'u64', value: '1' },
        { type: 'u32', value: 2 },
      ],
    });

    expect(mockCall).toHaveBeenCalledWith(
      CONFIG.contractId,
      'cast_vote',
      expect.objectContaining({ value: 1n }),
      expect.objectContaining({ value: 2 })
    );
  });
});

describe('submitInvocation', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('returns the hash once the transaction is successful', async () => {
    mockFromXDR.mockReturnValue({ signed: true });
    mockSendTransaction.mockResolvedValue({ status: 'PENDING', hash: 'abc123' });
    mockGetTransaction.mockResolvedValue({ status: 'SUCCESS' });

    const server = { sendTransaction: mockSendTransaction, getTransaction: mockGetTransaction };
    const hash = await submitInvocation(server as never, 'SIGNED_XDR', CONFIG.networkPassphrase);

    expect(hash).toBe('abc123');
    expect(mockFromXDR).toHaveBeenCalledWith('SIGNED_XDR', CONFIG.networkPassphrase);
  });

  it('throws when the network rejects the submission', async () => {
    mockFromXDR.mockReturnValue({ signed: true });
    mockSendTransaction.mockResolvedValue({ status: 'ERROR', hash: 'abc123' });

    const server = { sendTransaction: mockSendTransaction, getTransaction: mockGetTransaction };
    await expect(
      submitInvocation(server as never, 'SIGNED_XDR', CONFIG.networkPassphrase)
    ).rejects.toThrow(/Contract call failed/);
  });

  it('throws when the transaction fails on-chain', async () => {
    mockFromXDR.mockReturnValue({ signed: true });
    mockSendTransaction.mockResolvedValue({ status: 'PENDING', hash: 'abc123' });
    mockGetTransaction.mockResolvedValue({ status: 'FAILED' });

    const server = { sendTransaction: mockSendTransaction, getTransaction: mockGetTransaction };
    await expect(
      submitInvocation(server as never, 'SIGNED_XDR', CONFIG.networkPassphrase)
    ).rejects.toThrow(/failed on-chain/);
  });

  it('includes the error result when the send fails with one', async () => {
    mockFromXDR.mockReturnValue({ signed: true });
    mockSendTransaction.mockResolvedValue({
      status: 'ERROR',
      hash: 'abc123',
      errorResult: { result: () => ({ switch: () => ({ name: 'txFailed' }) }) },
    });

    const server = { sendTransaction: mockSendTransaction, getTransaction: mockGetTransaction };
    await expect(
      submitInvocation(server as never, 'SIGNED_XDR', CONFIG.networkPassphrase)
    ).rejects.toThrow(/txFailed/);
  });
});

describe('invokeContract', () => {
  const OLD_ENV = process.env;

  beforeEach(() => {
    jest.clearAllMocks();
    process.env = { ...OLD_ENV };
    process.env.NEXT_PUBLIC_CONTRACT_ID = CONFIG.contractId;
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL = CONFIG.rpcUrl;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    txBuilder = {};
    mockGetAccount.mockResolvedValue({ accountId: () => 'GSOURCE' });
    mockPrepareTransaction.mockResolvedValue({
      toEnvelope: () => ({ toXDR: () => 'PREPARED_XDR' }),
    });
    mockFromXDR.mockReturnValue({ signed: true });
    mockSendTransaction.mockResolvedValue({ status: 'PENDING', hash: 'abc123' });
    mockGetTransaction.mockResolvedValue({ status: 'SUCCESS' });
  });

  afterAll(() => {
    process.env = OLD_ENV;
  });

  it('builds, signs with the wallet and submits', async () => {
    const sign = jest.fn().mockResolvedValue({ signedTxXdr: 'SIGNED_XDR' });

    const hash = await invokeContract(
      'GSOURCE',
      { method: 'file_dispute', args: [{ type: 'u64', value: '1' }] },
      sign
    );

    expect(sign).toHaveBeenCalledWith('PREPARED_XDR');
    expect(mockSendTransaction).toHaveBeenCalledTimes(1);
    expect(hash).toBe('abc123');
  });

  it('throws a config error before touching the SDK when unconfigured', async () => {
    delete process.env.NEXT_PUBLIC_SOROBAN_RPC_URL;
    const sign = jest.fn();

    await expect(
      invokeContract('GSOURCE', { method: 'file_dispute', args: [] }, sign)
    ).rejects.toThrow(/On-chain submission is not configured/);
    expect(sign).not.toHaveBeenCalled();
  });
});
