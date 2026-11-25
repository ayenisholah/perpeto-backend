# M3 public demo evidence

M3's exit gate: a reviewer reaches the live demo from the README without help, and a fresh clone runs locally from the README alone. Checked on 2026-11-24 and 2026-11-25.

## Live demo

- **URL:** `https://app.perpeto.xyz`, the Next.js web platform from `perpeto-web` (DEC-0021). It serves a Let's Encrypt certificate and redirects a signed-out visitor to `/sign-in`.
- **Browser run (headless Chrome, 2026-11-24 19:49 UTC):** "Open the live demo" on `/sign-in` lands on `/` with:
  - the PAPER and READ-ONLY DEMO badges;
  - $100,000.00 equity;
  - market feeds BINANCE, BYBIT and OKX all HEALTHY;
  - no active breakers;
  - no action buttons.
- **Deployed:** backend `984c70b`, then `93edf29`; web `cae3a54` (`ghcr.io/ayenisholah/perpeto-web:sha-cae3a54…`, rolled out through the `Perpeto-UpdateWeb` SSM document).

## Read-only enforcement on staging (2026-11-25 08:0x UTC)

The demo session came from `POST https://api.perpeto.xyz/api/v1/demo/session` (201, access-only):

| Request | Result |
|---|---|
| `GET /api/v1/auth/me` | 200, roles `["VIEWER"]`, `demo: true`, tenant "Perpeto demo" |
| `GET /api/v1/opportunities`, `/positions`, `/health` | 200; health shows all three venues HEALTHY |
| `POST /api/v1/controls/halt` | 403 `DEMO_READ_ONLY` |
| `POST /api/v1/strategies` | 403 `DEMO_READ_ONLY` |
| `POST /api/v1/positions` | 403 `DEMO_READ_ONLY` |
| `DELETE /api/v1/account` | 403 `DEMO_READ_ONLY` |
| `POST /api/v1/auth/logout` | 403 `DEMO_READ_ONLY` |

## Automated evidence

`bins/api/src/router_tests.rs` runs in CI against PostgreSQL:
- `every_protected_mutation_rejects_the_demo_principal`: every protected non-GET route parsed from `routes.rs` (43 when written) returns 403 `DEMO_READ_ONLY` for the demo.
- `the_demo_principal_reads_the_public_demo_surfaces`: ten read surfaces return 200, with `demo: true`, `VIEWER` only, and an enabled seeded strategy.
- `the_demo_endpoint_is_absent_unless_enabled`: 404 when the demo is off.
- `a_stale_market_snapshot_refuses_a_paper_entry` (M2) and `a_logged_out_access_token_is_refused_at_once`.

## Local start from a fresh clone

`docker compose up --build -d --wait` at the repository root, from empty volumes (2026-11-24):
- **Containers:**
  - `keys` generated the local keys and exited;
  - `migrate` applied migrations `0001`–`0020` and exited;
  - `api`, `engine` and `web` came up healthy.
- **API:** `http://localhost:8080/readyz` returned `{"mode":"PAPER","stage":"M3_CONNECTORS","status":"OK"}`.
- **Browser (headless Chrome):**
  - `http://localhost:3000` redirected to sign-in;
  - "Open the live demo" reached the read-only overview;
  - "Development sign-in" reached the Owner view.

## Deferred

Google and Apple sign-in on the web are implemented: WEB devices, the nonce-bound exchange, the Apple Services ID audience, and immediate revocation, in #18 and `perpeto-web`. They stay switched off until the provider origins are configured. The demo is the M3 surface; operator sign-in today is the iOS app.
