# Perpeto Backend

Perpeto is a private funding-rate arbitrage platform. This repository contains the Rust control plane, venue connectors, execution and risk services, persistence, API contracts, and Docker Compose deployment used by the Expo mobile client.

The backend is the only component allowed to evaluate opportunities, hold venue credentials, submit orders, supervise positions, or reconcile accounting. The mobile application is an authenticated operations client.

## Current status

M2 paper trading is complete and M3 CEX integration is in progress. The backend includes deterministic paper automation plus Binance, Bybit, and OKX exact-decimal connectors, encrypted server-side credentials, sandbox-only submit-once coordination, read-only shadow scanning, reconciliation, accounting monitoring, and certification evidence tooling. `LIVE` remains fail-closed; primary WebSocket streams and credentialed timed venue certification are still required. See [PROGRESS.md](docs/PROGRESS.md) and [the M3 certification runbook](docs/M3_CONNECTOR_CERTIFICATION.md).

## Authoritative documents

Read these before changing code:

1. [PRODUCT_SPEC.md](docs/PRODUCT_SPEC.md)
2. [DECISION.md](docs/DECISION.md)
3. [TRACEABILITY.md](docs/TRACEABILITY.md)
4. [IMPLEMENTATION.md](docs/IMPLEMENTATION.md)
5. [AGENTS.md](AGENTS.md)

API consumers should also read [API.md](docs/API.md) and the normative [OpenAPI 3.1 contract](schemas/openapi.yaml).

If these disagree, stop and resolve the conflict using the precedence defined in `AGENTS.md`.

## Repository shape

```text
crates/                 Shared Rust domain and service crates
bins/                   Deployable Rust binaries
schemas/                OpenAPI and WebSocket contracts
deploy/compose/         Staging Compose manifests
docs/                   Product, governance, progress, testing, and operations records
scripts/                Validation and deployment commands
.github/workflows/      CI, publishing, docs sync, and staging deployment
```

## Local development

Prerequisites are Rust 1.97.0, Docker with Compose v2, and PowerShell 7 or Bash.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
docker compose -f deploy/compose/docker-compose.staging.yml config
```

The API exposes `/livez`, `/readyz`, and `/entryz` for mode-aware infrastructure and entry validation. PAPER is deterministic, SANDBOX permits only explicitly configured testnet/demo writes, SHADOW is read-only, and LIVE remains locked.

## Security

Never commit exchange credentials, signer keys, TOTP material, database passwords, production data, or real account fixtures. GitHub Actions holds only deployment and automation credentials. Runtime secrets remain on the VPS. Report security concerns according to [SECURITY.md](.github/SECURITY.md).

## Deployment

Successful `main` builds publish immutable public GHCR images and deploy to one owner-managed, Docker-ready staging VPS. The workflow does not install Docker, provision hosts, or enable live trading. See [DEPLOYMENT.md](docs/DEPLOYMENT.md).

No license has been granted yet. Until a license decision is recorded, all rights are reserved.
