-- Durable store for chain events (Redis is only a cache).
CREATE TABLE IF NOT EXISTS chain_events (
    event_id    TEXT PRIMARY KEY,
    ledger      BIGINT NOT NULL,
    topic       TEXT NOT NULL,
    tx_hash     TEXT,
    payload     JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS chain_events_ledger_idx ON chain_events (ledger);

-- Markets: on-chain identity + resolved-outcome consistency.
ALTER TABLE markets ADD COLUMN IF NOT EXISTS on_chain_id BIGINT;
CREATE UNIQUE INDEX IF NOT EXISTS markets_on_chain_id_key ON markets (on_chain_id);
ALTER TABLE markets DROP CONSTRAINT IF EXISTS markets_resolved_requires_outcome;
ALTER TABLE markets ADD CONSTRAINT markets_resolved_requires_outcome
    CHECK (status <> 'resolved' OR outcome_index IS NOT NULL) NOT VALID;
