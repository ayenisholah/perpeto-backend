-- M4 Slice 5: allow encrypted_credentials to hold Vault Transit ciphertext
-- alongside the transitional AES-256-GCM rows. The table is already
-- tenant-owned and FORCE ROW LEVEL SECURITY (migration 0016); this change is
-- purely a storage shape, so the tenant_isolation policy is untouched.
--
-- Backend discriminator: 'AES' rows keep the original nonce/ciphertext/digest
-- columns; 'VAULT' rows store only the opaque Transit key name and the
-- `vault:vN:...` ciphertext. A CHECK makes the two shapes mutually exclusive so
-- a row can never be half-populated.

ALTER TABLE encrypted_credentials
    ADD COLUMN backend TEXT NOT NULL DEFAULT 'AES' CHECK (backend IN ('AES', 'VAULT')),
    ADD COLUMN vault_key_name TEXT,
    ADD COLUMN vault_ciphertext TEXT;

-- The AES columns become optional so VAULT rows can leave them null.
ALTER TABLE encrypted_credentials
    ALTER COLUMN nonce DROP NOT NULL,
    ALTER COLUMN ciphertext DROP NOT NULL,
    ALTER COLUMN plaintext_digest DROP NOT NULL;

-- Exactly one backend's columns are populated per row.
ALTER TABLE encrypted_credentials
    ADD CONSTRAINT encrypted_credentials_backend_shape CHECK (
        (backend = 'AES'
            AND nonce IS NOT NULL
            AND ciphertext IS NOT NULL
            AND plaintext_digest IS NOT NULL
            AND vault_key_name IS NULL
            AND vault_ciphertext IS NULL)
        OR
        (backend = 'VAULT'
            AND vault_key_name IS NOT NULL
            AND vault_ciphertext IS NOT NULL
            AND nonce IS NULL
            AND ciphertext IS NULL
            AND plaintext_digest IS NULL)
    );
