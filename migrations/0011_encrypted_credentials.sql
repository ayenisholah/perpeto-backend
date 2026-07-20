CREATE TABLE encrypted_credentials (
    secret_ref UUID PRIMARY KEY,
    venue_account_id UUID NOT NULL REFERENCES venue_accounts(id) ON DELETE CASCADE,
    venue TEXT NOT NULL,
    product TEXT NOT NULL,
    key_version INTEGER NOT NULL CHECK (key_version > 0),
    nonce BYTEA NOT NULL CHECK (octet_length(nonce) = 12),
    ciphertext BYTEA NOT NULL,
    plaintext_digest BYTEA NOT NULL CHECK (octet_length(plaintext_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    UNIQUE (venue_account_id, product)
);

CREATE INDEX encrypted_credentials_active_idx
    ON encrypted_credentials (venue_account_id, product)
    WHERE revoked_at IS NULL;
