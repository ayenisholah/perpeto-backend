# Backend Implementation Plan

## Current objective

Establish a reproducible, paper-only foundation that compiles, tests, publishes immutable artifacts, and can be deployed without implying that trading capability exists.

## Ordered implementation

1. Domain types, state-machine invariants, decimal conventions, command envelopes, and deterministic simulator.
2. PostgreSQL migrations, repositories, transactional outbox and audit chain. For auth, implement DEC-0007 provider verification ports first, then Perpeto RBAC/TOTP/session handlers; never auto-link email or admit pending users to private resources, except under the DEC-0009 `PERPETO_OPEN_REGISTRATION` policy that admits new users to full access on the shared v1 backend.
3. Normalized connector contract, fixture replay, order books, funding forecasts, scanner, risk reservations, and paper execution.
4. Binance, Bybit, and OKX connectors through paper, sandbox, shadow, pilot, and certification states.
5. dYdX and Hyperliquid connectors with isolated allowlisted signer and nonce/chain reconciliation.
6. Treasury proposals, operations, fault injection, backups, observability, release evidence, and controlled-live preparation.

## Change protocol

Every implementation slice must start from a `TRACEABILITY.md` row and finish with unit/contract/fault tests, API compatibility evidence, structured telemetry, documented failure behavior, and an updated status. Venue work is never considered complete from a successful happy-path API call alone.

## Explicitly deferred

Production deployment, live-mode arming, automatic withdrawals, shared tenancy, dated futures, inverse contracts, options, and infrastructure provisioning remain out of scope.
