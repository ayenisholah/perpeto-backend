-- M4 security slice: WebAuthn passkeys and one-use action-authorization proofs.
--
-- Passkeys are the credential-authorization primitive gating every downstream
-- M4 live action (credential enrollment, account arming). Every table is
-- tenant-owned and forced-RLS so a missing transaction-local context
-- (app_current_tenant_id() IS NULL) matches no row, exactly like migration 0016.

CREATE TYPE webauthn_ceremony AS ENUM ('REGISTER', 'AUTHENTICATE');

-- One registered authenticator (platform or roaming) per row. credential_id is
-- unique within the relying party. `authenticator` holds the authoritative
-- serialized WebAuthn credential (public key + counter + backup flags) owned by
-- the verification adapter; sign_count is denormalized for monitoring and its
-- monotonicity is a cloned-authenticator signal cross-checked in the domain.
CREATE TABLE passkey_credentials (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    credential_id BYTEA NOT NULL,
    authenticator BYTEA NOT NULL,
    sign_count BIGINT NOT NULL DEFAULT 0 CHECK (sign_count >= 0),
    label TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 120),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    UNIQUE (credential_id),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, credential_id),
    FOREIGN KEY (tenant_id, user_id)
        REFERENCES tenant_memberships(tenant_id, user_id) ON DELETE RESTRICT
);

-- Pending registration/assertion ceremonies. A challenge is short-lived and
-- single-use; a non-NULL action_descriptor binds the resulting proof to one
-- exact action so an assertion for action A can never authorize action B.
CREATE TABLE webauthn_challenges (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    ceremony webauthn_ceremony NOT NULL,
    state BYTEA NOT NULL,
    challenge BYTEA NOT NULL,
    action_descriptor JSONB,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, id),
    CHECK (expires_at > created_at),
    FOREIGN KEY (tenant_id, user_id)
        REFERENCES tenant_memberships(tenant_id, user_id) ON DELETE RESTRICT
);
-- At most one live ceremony of a kind per user; consumed rows are exempt.
CREATE UNIQUE INDEX webauthn_challenges_live_idx
    ON webauthn_challenges (tenant_id, user_id, ceremony)
    WHERE consumed_at IS NULL;

-- Audit and replay guard for consumed one-use proofs. The unique challenge
-- makes any attempt to replay a verified assertion fail closed.
CREATE TABLE passkey_action_proofs (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    credential_id BYTEA NOT NULL,
    action_descriptor JSONB NOT NULL,
    challenge BYTEA NOT NULL,
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, challenge),
    FOREIGN KEY (tenant_id, credential_id)
        REFERENCES passkey_credentials(tenant_id, credential_id) ON DELETE RESTRICT
);

CREATE INDEX passkey_credentials_active_idx
    ON passkey_credentials (tenant_id, user_id)
    WHERE revoked_at IS NULL;
CREATE INDEX webauthn_challenges_expiry_idx
    ON webauthn_challenges (expires_at)
    WHERE consumed_at IS NULL;

-- Forced tenant isolation, identical policy shape to migration 0016. A NULL
-- transaction-local tenant context authorizes no row.
DO $passkey_rls$
DECLARE
    table_name TEXT;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'passkey_credentials', 'webauthn_challenges', 'passkey_action_proofs'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_current_tenant_id()) WITH CHECK (tenant_id = app_current_tenant_id())',
            table_name
        );
    END LOOP;
END
$passkey_rls$;
