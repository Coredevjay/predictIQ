'use client';

import React from 'react';
import { useRouter } from 'next/navigation';
import { getEnvConfig } from '@/lib/env';
import { useWallet } from '@/lib/wallet/WalletProvider';
import { getSupportedAsset } from '@/lib/assets';
import { TokenSelector } from '@/components/markets/TokenSelector';
import {
  marketFormSchema,
  validateMarketForm,
  type MarketFormErrors,
  type MarketFormValues,
} from '@/lib/validation/marketForm';
import './page.css';

const EMPTY_FORM: MarketFormValues = {
  title: '',
  description: '',
  outcomes: ['', ''],
  closeTime: '',
  asset: '',
  resolutionDeadline: '',
  oracleAddress: '',
  feedId: '',
  maxStalenessSeconds: 300,
  maxConfidenceBps: 100,
  tier: 'Basic',
  parentId: 0,
  parentOutcomeIdx: 0,
};

interface CreateMarketResponse {
  market_id: number;
  tx_hash: string;
  status: string;
}

export default function CreateMarketPage() {
  const router = useRouter();
  const { address: walletAddress } = useWallet();
  const [values, setValues] = React.useState<MarketFormValues>(EMPTY_FORM);
  const [errors, setErrors] = React.useState<MarketFormErrors>({});
  const [submitting, setSubmitting] = React.useState(false);
  const [success, setSuccess] = React.useState(false);

  const setField = <K extends keyof MarketFormValues>(field: K, value: MarketFormValues[K]) => {
    setValues((prev) => ({ ...prev, [field]: value }));
  };

  const setOutcome = (index: number, value: string) => {
    setValues((prev) => ({
      ...prev,
      outcomes: prev.outcomes.map((o, i) => (i === index ? value : o)),
    }));
  };

  const addOutcome = () => {
    setValues((prev) => ({ ...prev, outcomes: [...prev.outcomes, ''] }));
  };

  const removeOutcome = (index: number) => {
    setValues((prev) => ({
      ...prev,
      outcomes: prev.outcomes.filter((_, i) => i !== index),
    }));
  };

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSuccess(false);

    const fieldErrors = validateMarketForm(values);
    if (Object.keys(fieldErrors).length > 0) {
      setErrors(fieldErrors);
      return;
    }

    const wallet = walletAddress;
    if (!wallet) {
      setErrors({ form: 'Connect a wallet before creating a market.' });
      return;
    }

    setErrors({});
    setSubmitting(true);
    try {
      const parsed = marketFormSchema.parse(values);
      const config = getEnvConfig();
      // NOTE: Wallet signing for the on-chain transaction is tracked under #77.
      const res = await fetch(
        `${config.NEXT_PUBLIC_API_URL.replace(/\/$/, '')}/api/v1/blockchain/markets`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            creator: wallet,
            description: parsed.description,
            options: parsed.outcomes,
            deadline: Math.floor(new Date(parsed.closeTime).getTime() / 1000),
            resolution_deadline: Math.floor(
              new Date(parsed.resolutionDeadline).getTime() / 1000
            ),
            oracle_config: {
              oracle_address: parsed.oracleAddress,
              feed_id: parsed.feedId,
              max_staleness_seconds: parsed.maxStalenessSeconds,
              max_confidence_bps: parsed.maxConfidenceBps,
            },
            tier: parsed.tier,
            native_token: getSupportedAsset(parsed.asset)?.address,
            parent_id: parsed.parentId ?? 0,
            parent_outcome_idx: parsed.parentOutcomeIdx ?? 0,
          }),
        }
      );

      if (!res.ok) {
        throw new Error(`Failed to create market (${res.status})`);
      }

      const data: CreateMarketResponse = await res.json();
      setSuccess(true);
      router.push(`/markets/${data.market_id}`);
    } catch (err) {
      setErrors({ form: err instanceof Error ? err.message : 'Failed to create market.' });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <main className="market-create">
      <h1 className="market-create__heading">Create a market</h1>
      <p className="market-create__intro">
        Define a question, its possible outcomes, and when betting closes.
      </p>

      {/* Placeholder success banner — replaced by the shared toast component from #17. */}
      {success && (
        <div className="market-create__success" role="status">
          Market created. Redirecting…
        </div>
      )}
      {errors.form && (
        <div className="market-create__form-error" role="alert">
          {errors.form}
        </div>
      )}

      <form onSubmit={handleSubmit} noValidate>
        <div className="market-create__field">
          <label htmlFor="market-title">Title</label>
          <input
            id="market-title"
            name="title"
            type="text"
            value={values.title}
            onChange={(e) => setField('title', e.target.value)}
            aria-invalid={Boolean(errors.title)}
            aria-describedby={errors.title ? 'market-title-error' : undefined}
          />
          {errors.title && (
            <span id="market-title-error" className="market-create__error" role="alert">
              {errors.title}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-description">Description</label>
          <textarea
            id="market-description"
            name="description"
            rows={4}
            value={values.description}
            onChange={(e) => setField('description', e.target.value)}
            aria-invalid={Boolean(errors.description)}
            aria-describedby={errors.description ? 'market-description-error' : undefined}
          />
          {errors.description && (
            <span id="market-description-error" className="market-create__error" role="alert">
              {errors.description}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-outcome-0">Outcomes</label>
          <div className="market-create__outcomes">
            {values.outcomes.map((outcome, index) => (
              <div className="market-create__outcome-row" key={index}>
                <input
                  id={`market-outcome-${index}`}
                  type="text"
                  value={outcome}
                  placeholder={`Outcome ${index + 1}`}
                  onChange={(e) => setOutcome(index, e.target.value)}
                  aria-invalid={Boolean(errors.outcomes)}
                />
                {values.outcomes.length > 1 && (
                  <button
                    type="button"
                    className="market-create__outcome-remove"
                    onClick={() => removeOutcome(index)}
                    aria-label={`Remove outcome ${index + 1}`}
                  >
                    &times;
                  </button>
                )}
              </div>
            ))}
          </div>
          <button type="button" className="market-create__add-outcome" onClick={addOutcome}>
            + Add outcome
          </button>
          {errors.outcomes && (
            <span className="market-create__error" role="alert">
              {errors.outcomes}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-close-time">Close time</label>
          <input
            id="market-close-time"
            name="closeTime"
            type="datetime-local"
            value={values.closeTime}
            onChange={(e) => setField('closeTime', e.target.value)}
            aria-invalid={Boolean(errors.closeTime)}
            aria-describedby={errors.closeTime ? 'market-close-time-error' : undefined}
          />
          {errors.closeTime && (
            <span id="market-close-time-error" className="market-create__error" role="alert">
              {errors.closeTime}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-resolution-deadline">Resolution deadline</label>
          <input
            id="market-resolution-deadline"
            name="resolutionDeadline"
            type="datetime-local"
            value={values.resolutionDeadline}
            onChange={(e) => setField('resolutionDeadline', e.target.value)}
            aria-invalid={Boolean(errors.resolutionDeadline)}
            aria-describedby={
              errors.resolutionDeadline ? 'market-resolution-deadline-error' : undefined
            }
          />
          {errors.resolutionDeadline && (
            <span
              id="market-resolution-deadline-error"
              className="market-create__error"
              role="alert"
            >
              {errors.resolutionDeadline}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-asset">Settlement asset</label>
          <TokenSelector
            id="market-asset"
            value={values.asset}
            onChange={(assetId) => setField('asset', assetId)}
            aria-invalid={Boolean(errors.asset)}
            aria-describedby={errors.asset ? 'market-asset-error' : undefined}
          />
          {errors.asset && (
            <span id="market-asset-error" className="market-create__error" role="alert">
              {errors.asset}
            </span>
          )}
        </div>

        <div className="market-create__field">
          <label htmlFor="market-tier">Market tier</label>
          <select
            id="market-tier"
            name="tier"
            value={values.tier}
            onChange={(e) => setField('tier', e.target.value as MarketFormValues['tier'])}
            aria-invalid={Boolean(errors.tier)}
            aria-describedby={errors.tier ? 'market-tier-error' : undefined}
          >
            <option value="Basic">Basic</option>
            <option value="Pro">Pro</option>
            <option value="Institutional">Institutional</option>
          </select>
          {errors.tier && (
            <span id="market-tier-error" className="market-create__error" role="alert">
              {errors.tier}
            </span>
          )}
        </div>

        <fieldset className="market-create__fieldset">
          <legend>Oracle configuration</legend>

          <div className="market-create__field">
            <label htmlFor="market-oracle-address">Oracle contract address</label>
            <input
              id="market-oracle-address"
              name="oracleAddress"
              type="text"
              value={values.oracleAddress}
              onChange={(e) => setField('oracleAddress', e.target.value)}
              aria-invalid={Boolean(errors.oracleAddress)}
              aria-describedby={errors.oracleAddress ? 'market-oracle-address-error' : undefined}
            />
            {errors.oracleAddress && (
              <span id="market-oracle-address-error" className="market-create__error" role="alert">
                {errors.oracleAddress}
              </span>
            )}
          </div>

          <div className="market-create__field">
            <label htmlFor="market-feed-id">Feed ID</label>
            <input
              id="market-feed-id"
              name="feedId"
              type="text"
              value={values.feedId}
              onChange={(e) => setField('feedId', e.target.value)}
              aria-invalid={Boolean(errors.feedId)}
              aria-describedby={errors.feedId ? 'market-feed-id-error' : undefined}
            />
            {errors.feedId && (
              <span id="market-feed-id-error" className="market-create__error" role="alert">
                {errors.feedId}
              </span>
            )}
          </div>

          <div className="market-create__field">
            <label htmlFor="market-max-staleness">Max staleness (seconds)</label>
            <input
              id="market-max-staleness"
              name="maxStalenessSeconds"
              type="number"
              min={1}
              step={1}
              value={values.maxStalenessSeconds}
              onChange={(e) =>
                setField('maxStalenessSeconds', parseInt(e.target.value, 10) || 0)
              }
              aria-invalid={Boolean(errors.maxStalenessSeconds)}
              aria-describedby={
                errors.maxStalenessSeconds ? 'market-max-staleness-error' : undefined
              }
            />
            {errors.maxStalenessSeconds && (
              <span id="market-max-staleness-error" className="market-create__error" role="alert">
                {errors.maxStalenessSeconds}
              </span>
            )}
          </div>

          <div className="market-create__field">
            <label htmlFor="market-max-confidence">Max confidence (basis points)</label>
            <input
              id="market-max-confidence"
              name="maxConfidenceBps"
              type="number"
              min={0}
              step={1}
              value={values.maxConfidenceBps}
              onChange={(e) =>
                setField('maxConfidenceBps', parseInt(e.target.value, 10) || 0)
              }
              aria-invalid={Boolean(errors.maxConfidenceBps)}
              aria-describedby={
                errors.maxConfidenceBps ? 'market-max-confidence-error' : undefined
              }
            />
            {errors.maxConfidenceBps && (
              <span
                id="market-max-confidence-error"
                className="market-create__error"
                role="alert"
              >
                {errors.maxConfidenceBps}
              </span>
            )}
          </div>
        </fieldset>

        <fieldset className="market-create__fieldset">
          <legend>Parent market (optional)</legend>

          <div className="market-create__field">
            <label htmlFor="market-parent-id">Parent market ID</label>
            <input
              id="market-parent-id"
              name="parentId"
              type="number"
              min={0}
              step={1}
              value={values.parentId ?? 0}
              onChange={(e) => setField('parentId', parseInt(e.target.value, 10) || 0)}
              aria-invalid={Boolean(errors.parentId)}
              aria-describedby={errors.parentId ? 'market-parent-id-error' : undefined}
            />
            {errors.parentId && (
              <span id="market-parent-id-error" className="market-create__error" role="alert">
                {errors.parentId}
              </span>
            )}
          </div>

          <div className="market-create__field">
            <label htmlFor="market-parent-outcome-idx">Parent outcome index</label>
            <input
              id="market-parent-outcome-idx"
              name="parentOutcomeIdx"
              type="number"
              min={0}
              step={1}
              value={values.parentOutcomeIdx ?? 0}
              onChange={(e) =>
                setField('parentOutcomeIdx', parseInt(e.target.value, 10) || 0)
              }
              aria-invalid={Boolean(errors.parentOutcomeIdx)}
              aria-describedby={
                errors.parentOutcomeIdx ? 'market-parent-outcome-idx-error' : undefined
              }
            />
            {errors.parentOutcomeIdx && (
              <span
                id="market-parent-outcome-idx-error"
                className="market-create__error"
                role="alert"
              >
                {errors.parentOutcomeIdx}
              </span>
            )}
          </div>
        </fieldset>

        <button type="submit" className="market-create__submit" disabled={submitting}>
          {submitting ? 'Creating…' : 'Create market'}
        </button>
      </form>
    </main>
  );
}
