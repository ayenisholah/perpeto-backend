BEGIN;

CREATE TYPE user_status AS ENUM ('PENDING', 'ACTIVE', 'SUSPENDED', 'DELETED');
CREATE TYPE social_provider AS ENUM ('APPLE', 'GOOGLE');
CREATE TYPE user_role AS ENUM ('VIEWER', 'OWNER', 'TRADER', 'APPROVER');
CREATE TYPE auth_challenge_purpose AS ENUM ('SIGN_IN', 'LINK_IDENTITY');
CREATE TYPE session_state AS ENUM ('ACTIVE', 'REVOKED', 'COMPROMISED', 'EXPIRED');

CREATE TABLE users (
    id uuid PRIMARY KEY,
    status user_status NOT NULL DEFAULT 'PENDING',
    email text,
    display_name text,
    permission_version bigint NOT NULL DEFAULT 1 CHECK (permission_version > 0),
    approved_by uuid REFERENCES users(id),
    approved_at timestamptz,
    deletion_pseudonym uuid UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CHECK ((status = 'DELETED') = (deleted_at IS NOT NULL)),
    CHECK (status <> 'DELETED' OR (email IS NULL AND display_name IS NULL AND deletion_pseudonym IS NOT NULL))
);

CREATE TABLE provider_identities (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    provider social_provider NOT NULL,
    provider_subject text NOT NULL,
    profile_email text,
    profile_is_private_relay boolean NOT NULL DEFAULT false,
    profile_display_name text,
    provider_refresh_token_ciphertext bytea,
    provider_refresh_token_key_version integer,
    linked_at timestamptz NOT NULL DEFAULT now(),
    last_authenticated_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    UNIQUE (provider, provider_subject),
    UNIQUE (user_id, provider),
    CHECK ((provider_refresh_token_ciphertext IS NULL) = (provider_refresh_token_key_version IS NULL))
);

CREATE TABLE user_roles (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role user_role NOT NULL,
    active boolean NOT NULL DEFAULT false,
    assigned_by uuid REFERENCES users(id),
    assigned_at timestamptz NOT NULL DEFAULT now(),
    activated_at timestamptz,
    PRIMARY KEY (user_id, role),
    CHECK ((active = false) OR role = 'VIEWER' OR activated_at IS NOT NULL)
);

CREATE TABLE social_challenges (
    id uuid PRIMARY KEY,
    purpose auth_challenge_purpose NOT NULL,
    provider social_provider NOT NULL,
    user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    state_hash bytea NOT NULL UNIQUE CHECK (octet_length(state_hash) = 32),
    nonce_hash bytea NOT NULL UNIQUE CHECK (octet_length(nonce_hash) = 32),
    pkce_challenge text NOT NULL CHECK (char_length(pkce_challenge) BETWEEN 43 AND 128),
    redirect_uri text NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((purpose = 'LINK_IDENTITY') = (user_id IS NOT NULL)),
    CHECK (expires_at > created_at)
);

CREATE TABLE devices (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    installation_id_hash bytea NOT NULL CHECK (octet_length(installation_id_hash) = 32),
    platform text NOT NULL CHECK (platform IN ('IOS')),
    app_version text NOT NULL,
    display_name text NOT NULL,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    UNIQUE (user_id, installation_id_hash)
);

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
    family_id uuid NOT NULL,
    state session_state NOT NULL DEFAULT 'ACTIVE',
    refresh_token_hash bytea NOT NULL UNIQUE CHECK (octet_length(refresh_token_hash) = 32),
    generation bigint NOT NULL DEFAULT 0 CHECK (generation >= 0),
    permission_version bigint NOT NULL CHECK (permission_version > 0),
    expires_at timestamptz NOT NULL,
    last_rotated_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    revoke_reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((state = 'ACTIVE') = (revoked_at IS NULL)),
    CHECK (expires_at > created_at)
);
CREATE UNIQUE INDEX one_active_session_per_family ON sessions(family_id) WHERE state = 'ACTIVE';

CREATE TABLE refresh_token_history (
    session_id uuid NOT NULL REFERENCES sessions(id) ON DELETE RESTRICT,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    generation bigint NOT NULL CHECK (generation >= 0),
    consumed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, generation)
);

CREATE TABLE mfa_methods (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    encrypted_totp_secret bytea NOT NULL,
    key_version integer NOT NULL CHECK (key_version > 0),
    confirmed_at timestamptz,
    disabled_at timestamptz,
    last_accepted_timestep bigint,
    failed_attempts integer NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    locked_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX one_active_totp_per_user ON mfa_methods(user_id) WHERE disabled_at IS NULL;

CREATE TABLE recovery_codes (
    id uuid PRIMARY KEY,
    mfa_method_id uuid NOT NULL REFERENCES mfa_methods(id) ON DELETE RESTRICT,
    code_hash bytea NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE recovery_code_acknowledgements (
    mfa_method_id uuid PRIMARY KEY REFERENCES mfa_methods(id) ON DELETE RESTRICT,
    acknowledged_at timestamptz NOT NULL,
    code_set_digest bytea NOT NULL CHECK (octet_length(code_set_digest) = 32)
);

CREATE TABLE bootstrap_state (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    deployment_token_hash bytea NOT NULL CHECK (octet_length(deployment_token_hash) = 32),
    expires_at timestamptz NOT NULL,
    consumed_by uuid REFERENCES users(id) ON DELETE RESTRICT,
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((consumed_by IS NULL) = (consumed_at IS NULL)),
    CHECK (expires_at > created_at)
);

CREATE TABLE step_up_proofs (
    id uuid PRIMARY KEY,
    session_id uuid NOT NULL REFERENCES sessions(id) ON DELETE RESTRICT,
    proof_hash bytea NOT NULL UNIQUE CHECK (octet_length(proof_hash) = 32),
    action text NOT NULL,
    resource_id text,
    resource_version bigint,
    command_digest bytea,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

CREATE TABLE audit_actors (
    id uuid PRIMARY KEY,
    user_id uuid UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    pseudonym uuid NOT NULL UNIQUE,
    erased_at timestamptz
);

CREATE TABLE audit_events (
    id uuid PRIMARY KEY,
    actor_id uuid NOT NULL REFERENCES audit_actors(id) ON DELETE RESTRICT,
    event_type text NOT NULL,
    subject_type text NOT NULL,
    subject_id text NOT NULL,
    session_id uuid REFERENCES sessions(id) ON DELETE RESTRICT,
    request_id uuid NOT NULL,
    source_ip_hash bytea,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    previous_hash bytea,
    event_hash bytea NOT NULL UNIQUE,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX users_pending_created_idx ON users(created_at) WHERE status = 'PENDING';
CREATE INDEX identities_user_idx ON provider_identities(user_id) WHERE revoked_at IS NULL;
CREATE INDEX sessions_user_active_idx ON sessions(user_id, created_at DESC) WHERE state = 'ACTIVE';
CREATE INDEX challenges_expiry_idx ON social_challenges(expires_at) WHERE consumed_at IS NULL;
CREATE INDEX audit_actor_time_idx ON audit_events(actor_id, occurred_at DESC);

COMMIT;
