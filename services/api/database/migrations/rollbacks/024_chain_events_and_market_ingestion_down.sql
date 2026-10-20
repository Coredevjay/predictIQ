ALTER TABLE markets DROP CONSTRAINT IF EXISTS markets_resolved_requires_outcome;
DROP INDEX IF EXISTS markets_on_chain_id_key;
ALTER TABLE markets DROP COLUMN IF EXISTS on_chain_id;
DROP TABLE IF EXISTS chain_events;
