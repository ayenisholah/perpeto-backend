# M3 CEX connector certification

M3 engineering and venue certification are separate gates. Fixture, client, and
paper checks can pass without exchange credentials. A route remains `PAPER_ONLY`
until its credentialed sandbox and production-shadow evidence meets Product Spec
section 17.6 and is accepted by the Owner. `LIVE` remains unavailable.

## Credential requirements

| Venue | Public market probe | Private sandbox/demo probe | Production shadow probe |
|---|---|---|---|
| Bybit | No API key | Testnet key and secret | Read-only production key and secret |
| Binance | No API key | Spot/Futures testnet key and secret | Read-only production key and secret |
| OKX | No API key | Demo key, secret, and passphrase | Read-only production key, secret, and passphrase |

Keys must be IP-allowlisted where supported. Enable only read and, for sandbox
accounts, order permissions. Withdrawals, transfers, API administration, earn,
convert, and unrelated product permissions must remain disabled. Never put a key
in an environment variable, command argument, mobile payload, log, fixture, or
evidence payload.

## Network-free evidence

Verify the pinned, redacted fixture manifests and hashes:

```text
funding-arb-connector-evidence fixtures
```

This command needs neither PostgreSQL nor venue credentials.

## Public evidence

Probe production public health/time, exact instrument metadata, BTC perpetual
funding, and one market snapshot for all three CEX connectors:

```text
funding-arb-connector-evidence public
```

This command uses venue networks but no API keys and performs no writes.

## Import credentials

Run migrations, set `DATABASE_URL` and a base64-encoded 32-byte
`PERPETO_SECRET_MASTER_KEY`, then pipe one credential JSON document through stdin:

```text
funding-arb-venue-admin import BYBIT bybit-test LINEAR_PERPETUAL TESTNET
funding-arb-venue-admin import BINANCE binance-test LINEAR_PERPETUAL TESTNET
funding-arb-venue-admin import OKX okx-demo LINEAR_PERPETUAL DEMO
```

The Bybit/Binance JSON shape is `api_key` plus `api_secret`. OKX additionally
requires `passphrase`. The CLI encrypts the secret before persistence and prints
only the account identifier. Repeat imports for the spot connections used by a
cash-and-carry route.

## Credentialed evidence

With `PERPETO_MODE=SANDBOX` or `PERPETO_MODE=SHADOW`, run:

```text
funding-arb-connector-evidence credentialed
```

Set `PERPETO_EVIDENCE_ACCOUNT_ID` to restrict the run to one configured account.
Each run performs public and private reads plus read-only reconciliation, records
counts/status rather than account values, and writes an append-only row to
`connector_certification_evidence`. It never submits, changes, or cancels an
order.

Sandbox execution evidence uses the durable preview/submit endpoint and must be
run with dedicated non-production funds. The operator must preserve the 72-hour
soak and 100 paired-execution report. Production shadow evidence must span at
least 14 days and 20 funding settlements. Do not promote a route based on a
single probe.

## Failure and rollback

Any failed probe, unresolved reconciliation discrepancy, stale market snapshot,
bad clock, permission mismatch, or unknown order outcome freezes new exposure.
Disable the account with `funding-arb-venue-admin disable ACCOUNT_ID`, reconcile
before retry, rotate compromised credentials through stdin, and retain the
append-only evidence record for review.

Connector assumptions must be rechecked against the current official venue
documentation before certification: Bybit V5, Binance Spot/USD-M Futures, and
OKX V5 API documentation linked from Product Spec section 19.
