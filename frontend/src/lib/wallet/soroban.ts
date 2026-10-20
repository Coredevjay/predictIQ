/**
 * Minimal Soroban contract client for public write flows (dispute, claim).
 *
 * The backend exposes no HTTP routes for the on-chain write entry points
 * (`file_dispute`, `cast_vote`, `claim_winnings`), so pages that need them must
 * build the contract invocation themselves, have the connected wallet sign it,
 * and submit it to the network. This module is the single place that does that
 * — pages never touch `@stellar/stellar-sdk`'s Soroban bindings directly.
 *
 * Contract id and RPC endpoint are read from
 * `NEXT_PUBLIC_CONTRACT_ID` / `NEXT_PUBLIC_SOROBAN_RPC_URL`. When either is
 * unset the client reports itself as unconfigured instead of throwing at import
 * time, so a deployment that has not wired these yet degrades to a clear
 * "on-chain submission is not configured" message rather than a broken page
 * (see #1755 / #1756).
 */

// `rpc` is the namespace name used by recent `@stellar/stellar-sdk` releases;
// the older `SorobanRpc` alias was removed in v17, so importing `rpc` keeps this
// module working across the versions sibling repos pin (12.x and 17.x).
import {
  BASE_FEE,
  Contract,
  Networks,
  TransactionBuilder,
  nativeToScVal,
  rpc,
  scValToNative,
  xdr,
} from '@stellar/stellar-sdk';

/** The on-chain write entry points this client can invoke. */
export type ContractWriteMethod = 'file_dispute' | 'cast_vote' | 'claim_winnings';

/** One positional argument for a contract invocation. */
export interface ContractArg {
  /** Soroban type to encode the value as. */
  type: 'address' | 'u64' | 'u32' | 'i128';
  value: string | number | bigint;
}

export interface ContractCallParams {
  method: ContractWriteMethod;
  args: ContractArg[];
}

/** Signs an unsigned transaction envelope XDR; satisfied by the wallet provider. */
export type SignTransaction = (transactionXdr: string) => Promise<{ signedTxXdr: string }>;

export interface SorobanClientConfig {
  contractId: string;
  rpcUrl: string;
  networkPassphrase: string;
}

/**
 * Resolve the client config from the environment.
 *
 * Returns `null` when `NEXT_PUBLIC_CONTRACT_ID` or
 * `NEXT_PUBLIC_SOROBAN_RPC_URL` is missing/empty, so callers can render a
 * configuration message instead of failing.
 */
export function resolveSorobanClientConfig(): SorobanClientConfig | null {
  const contractId = process.env.NEXT_PUBLIC_CONTRACT_ID?.trim();
  const rpcUrl = process.env.NEXT_PUBLIC_SOROBAN_RPC_URL?.trim();
  if (!contractId || !rpcUrl) return null;

  const networkPassphrase =
    process.env.NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE?.trim() || Networks.TESTNET;

  return { contractId, rpcUrl, networkPassphrase };
}

/** True once a contract id and RPC endpoint are configured for this deployment. */
export function isSorobanConfigured(): boolean {
  return resolveSorobanClientConfig() !== null;
}

function toScVal(arg: ContractArg): xdr.ScVal {
  switch (arg.type) {
    case 'u64':
      return nativeToScVal(BigInt(arg.value), { type: 'u64' });
    case 'u32':
      return nativeToScVal(Number(arg.value), { type: 'u32' });
    case 'i128':
      return nativeToScVal(BigInt(arg.value), { type: 'i128' });
    case 'address':
      throw new Error('address arguments are encoded in encodeContractArg');
    default:
      throw new Error(`Unsupported contract argument type: ${(arg as { type: string }).type}`);
  }
}

/**
 * Encode a single call argument to its `xdr.ScVal` form.
 *
 * Addresses are encoded as `scvAddress`; numeric types use the matching Soroban
 * integer encoding (i128 is 128-bit, so it must go through `BigInt`).
 * Exported for tests.
 */
export function encodeContractArg(arg: ContractArg): xdr.ScVal {
  if (arg.type === 'address') {
    return nativeToScVal(arg.value as string, { type: 'address' });
  }
  return toScVal(arg);
}

/**
 * Build the unsigned invocation transaction for `params`.
 *
 * Returns the base64 transaction envelope XDR ready for the wallet to sign.
 * The source account is a placeholder derived from the connected address; the
 * wallet's `signAndSubmit` re-derives the real account, and the Soroban RPC's
 * `prepareTransaction` fills in the footprint and fees before submission.
 *
 * Exported for tests.
 */
export async function buildInvocation(
  server: rpc.Server,
  config: SorobanClientConfig,
  sourceAddress: string,
  params: ContractCallParams
): Promise<string> {
  const account = await server.getAccount(sourceAddress);
  const contract = new Contract(config.contractId);

  const tx = new TransactionBuilder(account, {
    fee: BASE_FEE,
    networkPassphrase: config.networkPassphrase,
  })
    .addOperation(contract.call(params.method, ...params.args.map(encodeContractArg)))
    .setTimeout(30)
    .build();

  const prepared = await server.prepareTransaction(tx);
  return prepared.toEnvelope().toXDR('base64');
}

/**
 * Submit a signed invocation transaction and wait for it to settle.
 *
 * Returns the transaction hash on success and throws with the on-chain failure
 * reason otherwise. Exported for tests.
 */
export async function submitInvocation(
  server: rpc.Server,
  signedTxXdr: string,
  networkPassphrase: string
): Promise<string> {
  const signed = TransactionBuilder.fromXDR(signedTxXdr, networkPassphrase);
  const result = await server.sendTransaction(signed);

  if (result.status === 'ERROR') {
    throw new Error(`Contract call failed: ${describeSendError(result)}`);
  }

  const hash = result.hash;
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const status = await server.getTransaction(hash);
    if (status.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      return hash;
    }
    if (status.status === rpc.Api.GetTransactionStatus.FAILED) {
      throw new Error('Contract call failed on-chain.');
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  // Accepted but not yet settled — the caller still gets the hash to poll.
  return hash;
}

function describeSendError(result: rpc.Api.SendTransactionResponse): string {
  const errorResult = (result as { errorResult?: xdr.TransactionResult }).errorResult;
  if (!errorResult) return result.status;
  try {
    return errorResult.result().switch().name;
  } catch {
    return result.status;
  }
}

/**
 * Invoke a contract write method: build -> wallet-sign -> submit.
 *
 * `sign` is the wallet provider's `signAndSubmit`. Throws a descriptive error
 * (including config guidance) so callers can surface it directly.
 */
export async function invokeContract(
  sourceAddress: string,
  params: ContractCallParams,
  sign: SignTransaction
): Promise<string> {
  const config = resolveSorobanClientConfig();
  if (!config) {
    throw new Error(
      'On-chain submission is not configured. Set NEXT_PUBLIC_CONTRACT_ID and ' +
        'NEXT_PUBLIC_SOROBAN_RPC_URL to enable it.'
    );
  }

  const server = new rpc.Server(config.rpcUrl);
  const unsignedXdr = await buildInvocation(server, config, sourceAddress, params);
  const { signedTxXdr } = await sign(unsignedXdr);
  return submitInvocation(server, signedTxXdr, config.networkPassphrase);
}

/** Decode an `xdr.ScVal` result to a native value; exported for tests. */
export function decodeResult(scVal: xdr.ScVal): unknown {
  return scValToNative(scVal);
}
