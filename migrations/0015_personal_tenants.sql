CREATE TYPE tenant_state AS ENUM ('PAPER_ONLY', 'PILOT_ELIGIBLE', 'PILOT_ARMED', 'SUSPENDED', 'DELETED');

CREATE TABLE tenants (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120),
    base_currency TEXT NOT NULL DEFAULT 'USD' CHECK (base_currency = 'USD'),
    state tenant_state NOT NULL DEFAULT 'PAPER_ONLY',
    permission_version BIGINT NOT NULL DEFAULT 1 CHECK (permission_version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK ((state = 'DELETED') = (deleted_at IS NOT NULL))
);

CREATE TABLE tenant_memberships (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role user_role NOT NULL DEFAULT 'OWNER',
    active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, user_id),
    UNIQUE (user_id),
    CHECK (role = 'OWNER'),
    CHECK (active = (revoked_at IS NULL))
);

-- Existing users each receive an independent paper-only personal tenant. A
-- deterministic same-UUID backfill keeps this migration restart-auditable;
-- newly created users and tenants use independent UUIDs.
INSERT INTO tenants (id, display_name)
SELECT id, LEFT(COALESCE(NULLIF(display_name, ''), 'Personal portfolio'), 120)
FROM users;

INSERT INTO tenant_memberships (tenant_id, user_id)
SELECT id, id FROM users;

ALTER TABLE sessions ADD COLUMN tenant_id UUID;
UPDATE sessions s
SET tenant_id = tm.tenant_id
FROM tenant_memberships tm
WHERE tm.user_id = s.user_id;
ALTER TABLE sessions ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE sessions
    ADD CONSTRAINT sessions_tenant_fk FOREIGN KEY (tenant_id) REFERENCES tenants(id) ON DELETE RESTRICT,
    ADD CONSTRAINT sessions_membership_fk FOREIGN KEY (tenant_id, user_id)
        REFERENCES tenant_memberships(tenant_id, user_id) ON DELETE RESTRICT;

CREATE INDEX tenant_memberships_active_idx ON tenant_memberships(tenant_id, user_id) WHERE active;
CREATE INDEX sessions_tenant_active_idx ON sessions(tenant_id, created_at DESC) WHERE state = 'ACTIVE';

-- Repositories use this transaction-local setting in the database-isolation
-- slice. NULL means no tenant context and must never authorize a tenant row.
CREATE FUNCTION app_current_tenant_id() RETURNS UUID
LANGUAGE sql STABLE PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('app.tenant_id', true), '')::UUID
$$;

COMMENT ON FUNCTION app_current_tenant_id() IS
    'Returns the transaction-local authenticated tenant; NULL fails tenant policies closed.';
