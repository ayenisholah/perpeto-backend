# Perpeto Control API

## Contract and implementation status

The implemented API is rooted at `/api/v1`. `/livez`, `/readyz` and `/entryz` are public infrastructure probes outside the versioned namespace. [`schemas/openapi.yaml`](../schemas/openapi.yaml) (version `0.22.0`) is the normative contract, and `@ayenisholah/perpeto-api-client` is generated from it. This page summarizes it.

Production exchange writes are unavailable: LIVE is permanently fail-closed (DEC-0015). Venue writes exist only in SANDBOX, against the operator's own testnet and demo accounts.

## Conventions

- The verified access token and active session determine `tenant_id`. Clients never submit a tenant header or body field. A resource from another tenant returns the same non-disclosing not-found response as an absent one.
- Mutations require an idempotency key and, where applicable, an expected resource version. Sensitive mutations also consume a short-lived, action-bound passkey proof.
- Decimals are strings, timestamps are UTC RFC 3339, errors are RFC 7807 `problem+json` with redacted detail, and credential endpoints return `Cache-Control: no-store`.

## Implemented resource groups

| Group | Paths | Notes |
|---|---|---|
| Identity and sessions | `/auth/social/*`, `/auth/refresh`, `/auth/logout`, `/auth/me`, `/auth/sessions*`, `/auth/identities*`, `/auth/mfa/*`, `/auth/step-up`, `/auth/bootstrap`, `/access-requests*`, `/account` | Google and Apple identity with Perpeto-authoritative sessions (DEC-0007) |
| Passkeys | `/passkeys`, `/passkeys/register/*`, `/passkeys/authenticate/*`, `/passkeys/{id}/revoke` | One-use action proofs bound to an exact action |
| Scanner and risk | `/opportunities`, `/opportunities/{route_id}`, `/risk/limits` | Ranked by conservative net return |
| Strategies | `/strategies*` | Versioned, with enable, disable and apply-to-open |
| Positions and controls | `/positions*`, `/controls*` | Halt, venue disable, flatten, resume |
| Monitoring | `/portfolio`, `/health`, `/alerts*`, `/audit` | Hash-chained audit with an integrity flag |
| Venues and connectors | `/venues`, `/venue-accounts*`, `/connector-orders`, `/reconciliations` | Masked reads; previews; SANDBOX-only orders |
| Credential lifecycle | `/venue-credentials/enroll/*`, `/{account_id}/rotate/*`, `/revoke/*`, `/delete/*`, `/certify/*`, `/preflight` | Passkey-gated; returns 503 in PAPER |
| Pilot gate | `/pilot`, `/pilot/eligibility`, `/pilot/arm/*`, `/pilot/disarm` | Control plane only; no live effect while LIVE is locked |

The credential, certification and pilot groups are implemented hardening (DEC-0015). They are kept and tested but are not on the roadmap.

## Planned changes

- **M2:** no new paths. Opportunity, health and venue reads gain live public-data freshness, and scanner output comes from live books in PAPER.
- **M3:** a read-only demo viewer (DEC-0017). Additive authentication for the viewer, and a role every mutating endpoint rejects, proven by an authorization test.
- **M4:** no new paths. SANDBOX evidence uses the existing venue-account, connector-order and reconciliation endpoints.

## Compatibility and releases

Additive endpoint or field releases take a semantic minor version of OpenAPI and the client. Removal or changed meaning takes a major version. A change to authorization meaning, tenant visibility, secret handling or idempotency scope needs an explicit compatibility review even when its JSON shape looks additive. See `VERSIONING.md`.
