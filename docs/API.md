# Perpeto Control API

## Contract and implementation status

The implemented API is rooted at `/api/v1`. `/livez`, `/readyz`, and `/entryz` are public infrastructure probes outside the versioned resource namespace. Production exchange writes remain unavailable.

M4 will evolve this into a tenant-safe multi-user API. The endpoint families below describe the accepted target where marked; they are not present in the current OpenAPI contract until implemented and versioned.

## Tenant and authorization conventions

- The verified access token and active session determine `tenant_id`; clients must not submit a free-form tenant header or use a body field to cross tenant boundaries.
- Every tenant resource, idempotency key, WebSocket subscription, export and background command is tenant-scoped. A resource from another tenant returns the same non-disclosing not-found response as an absent resource.
- Platform-operator endpoints use a separate authority and namespace. Operator status does not grant access to a trader's plaintext credentials or authority to trade as that tenant.
- Mutations require an idempotency key and expected resource version where applicable. Sensitive mutations also consume a short-lived, action-bound passkey proof.
- Decimal values remain strings, timestamps are UTC RFC 3339, errors are redacted, and credential endpoints return `Cache-Control: no-store`.

## Authentication and passkeys

The existing Apple/Google challenge, exchange, refresh, logout, session, identity and deletion APIs remain the identity/session foundation. DEC-0012 replaces universal privileged open registration: a new trader receives a personal tenant and paper access, not platform administration or live eligibility.

M4 adds these versioned endpoint families:

| Method/path | Target purpose |
|---|---|
| `POST /auth/passkeys/registration/options` | Issue a one-use WebAuthn registration challenge for the current tenant/user |
| `POST /auth/passkeys/registration/verify` | Verify attestation and register the passkey |
| `GET /auth/passkeys` | List masked registered passkeys |
| `DELETE /auth/passkeys/{id}` | Revoke a passkey under recovery/final-credential policy |
| `POST /auth/passkeys/assertion/options` | Issue a challenge bound to an exact sensitive action and resource digest |
| `POST /auth/passkeys/assertion/verify` | Verify origin, RP ID, user verification, ownership and replay state; return a one-use proof |

A passkey proof authorizes credential enrollment/rotation/revocation, live arming/disarming and flattening. It never decrypts or returns an exchange credential.

## Venue credentials and live eligibility

M4 adds passkey-authorized account lifecycle endpoints to the existing masked venue reads:

| Method/path | Target purpose |
|---|---|
| `POST /venue-accounts/enrollment` | Create a short-lived, one-use credential enrollment operation |
| `POST /venue-accounts` | Submit an ephemeral credential to Vault Transit and create a disabled account record |
| `POST /venue-accounts/{id}/preflight` | Validate least privilege, account/margin mode, clocks, streams, balances, reconciliation and shadow readiness |
| `POST /venue-accounts/{id}/rotate-credentials` | Validate and atomically activate a new credential version |
| `POST /venue-accounts/{id}/revoke-credentials` | Disarm, terminate use and mark the credential version revoked |
| `POST /venue-accounts/{id}/arm-live` | Consume passkey proof plus typed confirmation for an eligible certified route |
| `POST /venue-accounts/{id}/disarm-live` | Block new risk for the tenant/account |

Raw secrets are accepted once, sent directly to the tenant's encrypt-only Vault path, zeroized where practical and never returned. Read endpoints expose only masked key metadata, permission validation, account environment, connector certification, preflight state, live eligibility/arm state and health.

## Existing resource groups

Scanner, strategies, positions, controls, portfolio, health, alerts, connector observability and audit APIs remain tenant-scoped in the M4 target. Public instruments and connector certification are platform-scoped. Both tenant and global circuit breakers apply; a tenant breaker cannot affect another tenant, while a platform breaker can halt affected routes globally.

## Compatibility and releases

M4 contract work is additive until a security boundary requires an incompatible change. OpenAPI and the TypeScript client receive a semantic minor version for additive endpoint/field releases; removal or changed meaning requires a major version. The current implementation must not expose placeholder secret endpoints before Vault, RLS and passkey verification are complete together.
