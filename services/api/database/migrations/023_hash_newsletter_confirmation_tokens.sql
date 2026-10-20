-- Store newsletter confirmation tokens as SHA-256 hex digests (matches unsubscribe tokens).
UPDATE newsletter_subscribers
SET confirmation_token = encode(sha256(convert_to(confirmation_token, 'UTF8')), 'hex')
WHERE confirmation_token IS NOT NULL
  AND confirmation_token !~ '^[0-9a-f]{64}$';
