# Signex Control API

## Contract and implementation status

The normative machine-readable contract is [OpenAPI 3.1](../schemas/openapi.yaml), version `0.2.0`. It is bundled unchanged in `@ayenisholah/perpeto-api-client@0.2.0` and validated with Redocly in CI. The M1A authentication paths are contract-first and are not yet served by the health-only Rust runtime; consumers must not interpret a documented operation as deployed until `PROGRESS.md` records its runtime evidence.

The API is rooted at `/api/v1`. `/livez`, `/readyz`, and `/entryz` are public infrastructure probes outside the versioned resource namespace. Trading remains unavailable.

## Conventions

- Request and response bodies use `application/json`, except errors use [`application/problem+json`](https://www.rfc-editor.org/rfc/rfc9457.html).
- Authenticated operations use `Authorization: Bearer <access-token>`. Access tokens are 15-minute Ed25519 JWTs and remain in client memory.
- Secret request/response properties are marked `writeOnly` in OpenAPI and must not be logged, cached, placed in URLs, or included in ordinary resource responses.
- Identifiers use UUID strings where the schema declares `format: uuid`; timestamps use RFC 3339/ISO 8601 `date-time` strings in UTC.
- Unknown request properties are rejected where schemas specify `additionalProperties: false`.
- Each operation has a stable `operationId`, explicit success and failure responses, reusable schemas, and an authorization declaration.
- `request_id` in every problem response is the support-safe correlation identifier. Error `code` is stable for programmatic handling; `title` is safe for display but not for branching.
- HTTP `401` means authentication is absent, invalid, expired, or revoked; `403` means the authenticated actor lacks permission; `409` represents a state/invariant conflict; `423` represents an MFA lockout; `429` represents rate limiting.

## Authentication lifecycle

1. Create a one-use social challenge with `POST /api/v1/auth/social/challenges`.
2. Complete native Apple or Google authorization and submit the code, identity token, PKCE verifier, challenge ID, and iOS device registration to `POST /api/v1/auth/social/exchange`.
3. Follow exactly one returned state: `PENDING_APPROVAL`, `MFA_REQUIRED`, `MFA_ENROLLMENT_REQUIRED`, or `AUTHENTICATED`.
4. A first deployment Owner consumes the one-use bootstrap token. Other registrations remain isolated until Owner approval grants Viewer. Deployments with `PERPETO_OPEN_REGISTRATION` enabled (the shared v1 backend, DEC-0009) instead return `AUTHENTICATED` immediately with full access and no MFA.
5. Owner, Trader, and Approver complete TOTP enrollment and bind recovery-code acknowledgement before role activation.
6. Store only the opaque refresh token in device-only Secure Store. Rotate it once through `/auth/refresh`; reuse revokes the whole family.

Apple and Google verification occurs server-side. Accounts are keyed only by `(provider, provider_subject)` and never merged by email. Linking uses a fresh `LINK_IDENTITY` challenge; privileged users also supply TOTP. The final provider identity cannot be unlinked.

## Endpoint groups

| Group | Operations |
|---|---|
| Social authentication | Challenge creation and Apple/Google proof exchange |
| MFA | Enrollment, confirmation, recovery acknowledgement, verification, and action-bound step-up |
| Sessions | Refresh rotation, current-user lookup, logout, device/session list, and remote revocation |
| Identities | Linked-provider list, explicit link, and protected unlink |
| Access requests | Owner pending-user list and Viewer approval |
| Account | First-Owner bootstrap and privacy-preserving deletion |
| Provider notifications | Signed Apple server notifications processed idempotently |

Exact parameters, required fields, response schemas, status codes, and security requirements remain authoritative in `schemas/openapi.yaml`; this document explains usage rather than duplicating the contract.

## Compatibility and releases

Additive v1 changes increment the client minor version. Removing or renaming fields, changing security semantics, or changing financial meaning requires a new negotiated API major version. Mobile pins the client package exactly and must pass schema compatibility CI before release.

Provider references:

- [Sign in with Apple authentication](https://developer.apple.com/documentation/signinwithapple/authenticating-users-with-sign-in-with-apple)
- [Google OpenID Connect verification](https://developers.google.com/identity/openid-connect/openid-connect)
