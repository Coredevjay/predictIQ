/**
 * Client-side validation for the market-creation form
 * (frontend/src/app/markets/create/page.tsx).
 *
 * Kept separate from the page component so it can be extended independently
 * as later issues layer on more rules (outcome-count/duplicate checks,
 * settlement-asset validation) without growing the component itself.
 */

import { z } from 'zod';
import { isSupportedAsset } from '../assets';

// Mirrors the contract's supported outcome-count range
// (contracts/predict-iq/src/types.rs: MAX_OUTCOMES_PER_MARKET) plus a
// minimum of 2, since a market needs at least two distinct outcomes to be
// meaningful. Validating this client-side avoids a wasted, gas-costing
// failed transaction (#1372).
export const MIN_OUTCOMES = 2;
export const MAX_OUTCOMES = 100;

// Minimum lead time (in milliseconds) a market's close time must be in the
// future at submission. A close time that's merely non-past can still elapse
// while the user finishes the form, connects/signs a wallet transaction, and
// waits for on-chain confirmation, causing a revert after gas has already
// been spent (#1597). Requiring a 5-minute buffer keeps the timestamp valid
// through that window.
export const MIN_CLOSE_TIME_LEAD_MS = 5 * 60 * 1000;

// The contract requires resolution_deadline >= deadline + 86400 (24 hours).
// Mirror that here so the client rejects invalid submissions before spending
// gas on a doomed transaction.
export const MIN_RESOLUTION_LEAD_SECONDS = 86400;

/**
 * Two outcome labels are considered duplicates if they're equal once
 * surrounding whitespace is trimmed and case is normalized — e.g. "Yes",
 * " yes ", and "YES" all collide.
 */
function normalizeOutcome(label: string): string {
  return label.trim().toLowerCase();
}

export function hasDuplicateOutcome(outcomes: string[]): boolean {
  const seen = new Set<string>();
  for (const outcome of outcomes) {
    const normalized = normalizeOutcome(outcome);
    if (seen.has(normalized)) return true;
    seen.add(normalized);
  }
  return false;
}

export const marketFormSchema = z
  .object({
    title: z
      .string()
      .trim()
      .min(1, 'Title is required.')
      .max(200, 'Title must be 200 characters or fewer.'),
    description: z
      .string()
      .trim()
      .min(1, 'Description is required.')
      .max(2000, 'Description must be 2000 characters or fewer.'),
    outcomes: z
      .array(z.string().trim().min(1, 'Outcome labels cannot be empty.'))
      .min(MIN_OUTCOMES, `At least ${MIN_OUTCOMES} outcomes are required.`)
      .max(MAX_OUTCOMES, `A market can have at most ${MAX_OUTCOMES} outcomes.`)
      .refine((outcomes) => !hasDuplicateOutcome(outcomes), {
        message: 'Outcome labels must be unique (case and whitespace insensitive).',
      }),
    closeTime: z
      .string()
      .min(1, 'Close time is required.')
      .refine((value) => !Number.isNaN(Date.parse(value)), 'Close time must be a valid date.')
      .refine(
        (value) => Date.parse(value) >= Date.now() + MIN_CLOSE_TIME_LEAD_MS,
        'Close time must be at least 5 minutes in the future.'
      ),
    // An asset that's technically valid on-chain but not on the platform's
    // supported list (see lib/assets.ts) must be rejected here, not left for
    // the contract call to fail (#1373).
    asset: z
      .string()
      .min(1, 'Select a settlement asset.')
      .refine(isSupportedAsset, 'Select a supported settlement asset.'),

    // Resolution deadline must be a valid date and is cross-validated against
    // closeTime below (must be >= closeTime + 24 h) per the contract rule.
    resolutionDeadline: z
      .string()
      .min(1, 'Resolution deadline is required.')
      .refine(
        (value) => !Number.isNaN(Date.parse(value)),
        'Resolution deadline must be a valid date.'
      ),

    // Oracle configuration fields (OracleConfig in the contract).
    oracleAddress: z
      .string()
      .trim()
      .min(1, 'Oracle contract address is required.'),
    feedId: z
      .string()
      .trim()
      .min(1, 'Oracle feed ID is required.'),
    maxStalenessSeconds: z
      .number({ invalid_type_error: 'Max staleness must be a number.' })
      .int('Max staleness must be a whole number.')
      .positive('Max staleness must be greater than zero.'),
    maxConfidenceBps: z
      .number({ invalid_type_error: 'Max confidence must be a number.' })
      .int('Max confidence must be a whole number.')
      .min(0, 'Max confidence cannot be negative.'),

    // Market tier — mirrors MarketTier in the contract.
    tier: z.enum(['Basic', 'Pro', 'Institutional'], {
      errorMap: () => ({ message: 'Select a market tier.' }),
    }),

    // Optional parent market linkage. Default to 0 (no parent) when omitted.
    parentId: z
      .number({ invalid_type_error: 'Parent ID must be a number.' })
      .int('Parent ID must be a whole number.')
      .min(0, 'Parent ID cannot be negative.')
      .optional()
      .default(0),
    parentOutcomeIdx: z
      .number({ invalid_type_error: 'Parent outcome index must be a number.' })
      .int('Parent outcome index must be a whole number.')
      .min(0, 'Parent outcome index cannot be negative.')
      .optional()
      .default(0),
  })
  // Cross-field refinement: resolution deadline must be at least 24 hours
  // after close time, matching the on-chain invariant in the contract.
  .refine(
    (data) => {
      if (!data.closeTime || !data.resolutionDeadline) return true;
      const closeMs = Date.parse(data.closeTime);
      const resolutionMs = Date.parse(data.resolutionDeadline);
      if (Number.isNaN(closeMs) || Number.isNaN(resolutionMs)) return true;
      return resolutionMs >= closeMs + MIN_RESOLUTION_LEAD_SECONDS * 1000;
    },
    {
      message: 'Resolution deadline must be at least 24 hours after the close time.',
      path: ['resolutionDeadline'],
    }
  );

export type MarketFormValues = z.infer<typeof marketFormSchema>;

export type MarketFormErrors = Partial<
  Record<
    | keyof Omit<MarketFormValues, 'parentId' | 'parentOutcomeIdx'>
    | 'parentId'
    | 'parentOutcomeIdx'
    | 'form',
    string
  >
>;

/**
 * Validates raw form state and returns a flat map of the first error per
 * field, keyed the same way the form renders inline errors. Returns an empty
 * object when the form is valid.
 */
export function validateMarketForm(values: MarketFormValues): MarketFormErrors {
  const result = marketFormSchema.safeParse(values);
  if (result.success) return {};

  const errors: MarketFormErrors = {};
  for (const issue of result.error.issues) {
    const key = issue.path[0] as keyof MarketFormErrors | undefined;
    if (key && !errors[key]) {
      errors[key] = issue.message;
    }
  }
  return errors;
}
