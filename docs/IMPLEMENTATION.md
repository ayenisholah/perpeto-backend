# Backend Implementation Plan

## Current objective

Deliver M2: live-data paper trading. Then deliver M3, the public demo, and M4, testnet execution (DEC-0015). M1 is closed. Every slice below must be closable by one maintainer without real capital.

## M2 — Live-data paper trading (next)

Most of the plumbing exists. The engine already builds each `MarketLeg` from `connector.market_stream` and `connector.current_funding` (`bins/engine/src/main.rs`), but only for configured accounts in SANDBOX/SHADOW. PAPER still replays `crates/scanner-service/src/fixtures.rs` on simulated time.

1. **Public market source — implemented (`crates/venue-service/src/public_market.rs`):** a credential-free `PublicMarketSource` over the existing Binance, Bybit and OKX adapters in `crates/connectors/src/{binance,bybit,okx}.rs`, using only their public REST and WebSocket endpoints. It returns instruments, funding (rate, interval, next settlement, mark and index), and order-book snapshots and deltas. It must not depend on `ConnectorRegistry::load` or any `SecretStore`.
2. **Snapshot persistence — implemented (migration `0019`, `ScannerRepo::record_scan_with_snapshot` / `latest_snapshot`).**
3. **Engine wiring — implemented (`PERPETO_PAPER_SOURCE`, one snapshot per tick):** in PAPER, feed public legs into `scanner.scan_legs`. Fixture replay (`PERPETO_SIM_START`, `PERPETO_SIM_TICK_SECS`) becomes an explicit selection, `PERPETO_PAPER_SOURCE=fixtures`, and stays the default under test.
4. **Paper fills against live depth — implemented (`ExecutionService::open_from`, `book_fill_price`; API opens from the stored snapshot with a freshness limit):** simulate each leg's fill by walking the live book with the existing `BookState::executable_price` (`crates/connectors/src/book.rs`), and apply configured fees. Funding accrues at real settlement times.
5. **Freshness and failure — implemented (entries refuse degraded legs and stale snapshots; `/health` reports per-venue feed status, client 0.23.0):** stale or failed feeds mark the affected legs stale. They block paper entries and surface in `/health` and the opportunity freshness fields. There is no silent fallback to fixtures.
6. **Reconnect integrity — implemented:** Bybit and OKX push a full snapshot on every subscribe (per their v5 docs), so a reconnect re-seeds the book without REST. Binance re-seeds over REST because its diff stream has none. An OKX sequence reset (`seqId < prevSeqId`) fails closed with `OKX_SEQUENCE_RESET` instead of freezing the book.
7. **Evidence:**
   - parser tests on recorded public payloads;
   - a stale-feed blocking test;
   - a test proving PAPER constructs no credential read path;
   - a 24-hour staging run with no crash.

## M3 — Public demo

1. Restore staging. The last successful deploy was `07cb3cc` on 2026-07-30, and the API returned 502 on 2026-11-22. Redeploy current `main` in live-data PAPER.
2. Add the read-only demo viewer and a seeded demo tenant (DEC-0017), with an authorization test that calls every mutating endpoint as the viewer and expects rejection.
3. Add a one-command local start: a Compose profile for PostgreSQL, Redis, migrate, API and engine in live-data PAPER.
4. Rewrite the README as a 5-minute tour, with links to the demo, an architecture diagram, local start and an evidence index.
5. Publish the API client so the registry (`0.7.0`) matches the vendored `0.22.0`.

The mobile side (hosted web build, TestFlight, viewer UX) is tracked in the mobile repository.

## M4 — Testnet execution

1. Enroll the operator's own Bybit testnet, Binance testnet and OKX demo credentials through `funding-arb-venue-admin import` or the passkey-gated `venue-credentials` API.
2. Per venue, run one coordinated open → funding → close cycle in SANDBOX through `ConnectorExecutionCoordinator`.
3. Per venue, force one partial fill or a timeout with an unknown outcome, and show it is reconciled before any retry.
4. Commit the redacted evidence and reconciliation report under `docs/evidence/<venue>/`, following `CONNECTOR_EVIDENCE.md`.

## Implemented hardening (kept, not a gate)

The following stay in the codebase and their suites must keep passing. Extending them is not on the roadmap:
- personal tenants and forced RLS (`0015`, `0016`);
- passkeys (`0017`);
- the Vault Transit store and the AES store (`0018`), plus the rewrap worker;
- passkey-gated credential lifecycle and preflight (endpoint and worker);
- the pilot state machine and `permit_live_risk`, with disarm → flatten;
- the hash-chained audit log and `GET /audit`.

## Change protocol

Each slice moves through these layers in order:
1. migrations and domain;
2. service and repository;
3. API, OpenAPI and client;
4. mobile;
5. tests and docs.

PAPER never loads credentials. SANDBOX writes only to explicitly configured testnet or demo accounts. LIVE stays fail-closed.

## Deferred (DEC-0018)

- DEX connectors and the signer
- Treasury, rebalancing and compounding
- Telegram/Discord
- Route certification soaks and limited-capital pilots
- Vault HA and auto-unseal
- Team tenants and custody
- Any real-money rollout
