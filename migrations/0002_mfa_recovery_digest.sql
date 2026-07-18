-- Stores the expected SHA-256 digest of the recovery-code set so the
-- acknowledgement step can verify the client saw the exact codes issued.
ALTER TABLE mfa_methods
    ADD COLUMN recovery_code_set_digest bytea
        CHECK (recovery_code_set_digest IS NULL OR octet_length(recovery_code_set_digest) = 32);
