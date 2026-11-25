# Connector evidence runbook

This runbook produces the connector evidence for M2 (public data) and M4 (testnet execution). Under DEC-0015 the evidence proves correct coordination, recovery and reconciliation for each venue. It is **not** a route certification, and LIVE stays fail-closed.

| Stage | Milestone | Credentials | Writes |
|---|---|---|---|
| Network-free fixtures | M1 (done) | None | None |
| Public market probe | M2 | None | None |
| Testnet/demo reads and execution | M4 | Operator's own testnet/demo key | SANDBOX only, testnet/demo funds |

Production read-only shadow probes and the certification soak windows (72 hours of sandbox, 14 days of shadow) are deferred (DEC-0018).

## Credential requirements

| Venue | Public market probe (M2) | Testnet/demo probe (M4) |
|---|---|---|
| Bybit | No API key | Testnet key and secret |
| Binance | No API key | Spot/Futures testnet key and secret |
| OKX | No API key | Demo key, secret, and passphrase |

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

This command uses venue networks but no API keys and performs no writes. It is the same keyless path that live-data PAPER uses (DEC-0016).

## Live-data PAPER snapshot (M2)

Take one snapshot exactly as the live-data PAPER engine will:

```text
funding-arb-connector-evidence snapshot
```

It needs no credentials or database. It prints each venue's feed status (`HEALTHY`, `DEGRADED` or `UNAVAILABLE`), the legs built and the newest observation time. `PERPETO_PAPER_UNDERLYINGS` overrides the default `BTC,ETH`. A venue that is unreachable from the host's network shows `UNAVAILABLE` without stopping the others, so run it from the host that will run the engine. Record results under `docs/evidence/m2/`.

## Import credentials

> Operator testnet flow. The passkey-authorized `venue-credentials` API is the
> implemented alternative. Either one is acceptable for testnet and demo keys.
> Never import a production key: `import` refuses one unless `--allow-production`
> is passed for SHADOW read-only use.

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

With `PERPETO_MODE=SANDBOX`, run:

```text
funding-arb-connector-evidence credentialed
```

Set `PERPETO_EVIDENCE_ACCOUNT_ID` to restrict the run to one configured account.
Each run performs public and private reads plus read-only reconciliation, records
counts/status rather than account values, and writes an append-only row to
`connector_certification_evidence`. It never submits, changes, or cancels an
order.

## M4 execution evidence

M4 runs through `funding-arb-sandbox`, which submits every order through the durable `ConnectorExecutionCoordinator`. That means each order is persisted before it is sent, never sent twice for one client order id, and refused outside SANDBOX or against a production endpoint. Testnet and demo funds only.

**Setup:**
- Create the operator tenant with `funding-arb-venue-admin tenant-ensure "M4 operator"` and export its id as `PERPETO_TENANT_ID`.
- Import each venue's key **twice**, once as `SPOT` and once as `LINEAR_PERPETUAL`. Binance uses its two separate testnet keys (spot testnet and USDⓈ-M futures testnet); Bybit and OKX reuse one key.
- The driver needs exactly one enrolled connection per venue and product.
- Run `funding-arb-connector-evidence credentialed` and get a pass for every account before any write.

**The commands**, all with `PERPETO_MODE=SANDBOX`:

```text
funding-arb-sandbox cycle   BYBIT ETH 150          # open, hold through one funding settlement + 2 min, close
funding-arb-sandbox recover BYBIT ETH              # forced unknown outcome, resolved by query before retry
funding-arb-sandbox report  BYBIT 2026-11-26T00:00:00Z docs/evidence/m4
```

- **`cycle`:**
  - opens a hedge: a spot BUY and a perpetual SELL of the same base quantity, sized to the notional and both lot steps, at marketable IOC limits 1% through the touch;
  - holds until the next funding time plus two minutes, then closes: perpetual BUY `reduce_only`, and spot SELL of what the account holds, up to the opened quantity.
  - If one leg misses at open, the filled leg is unwound and the cycle is `FAILED`.
  - Rerunning `cycle` or `close` resumes an unfinished cycle with its stored client order ids, so nothing is sent twice.
  - `open` and `close` are also available on their own.
- **`recover`:**
  - submits one minimum-size perpetual SELL resting 2% above the ask, with `PERPETO_FAULT_SUBMIT_TIMEOUT_MS` set low enough that the response is lost after the order leaves the host;
  - retries the same client order id, which must come back `Duplicate` without reaching the venue;
  - queries the venue, cancels the resting order and flattens anything that filled.
  - If the first submission was acknowledged anyway, the driver says so; lower the fault timeout and run again.
- **`report`:**
  - first reconciles both accounts, then writes `<VENUE>.md` and `<VENUE>.json` covering the runs, orders, fills, fees, funding entries, account snapshots before and after, and reconciliation runs with discrepancies;
  - account ids are hashed, and no key material is read.

Each venue's evidence is committed as `docs/evidence/m4/<VENUE>.md` and `.json`. Testnet books are thin, so the evidence proves coordination, not profitability. A venue whose testnet posts no funding entry has its observation recorded, and stays open for M4.

## Failure and rollback

Any failed probe, unresolved reconciliation discrepancy, stale market snapshot,
bad clock, permission mismatch, or unknown order outcome freezes new exposure.
Disable the account with `funding-arb-venue-admin disable ACCOUNT_ID`, reconcile
before retry, rotate compromised credentials through stdin, and retain the
append-only evidence record for review.

Connector assumptions must be rechecked against the current official venue
documentation before certification: Bybit V5, Binance Spot/USD-M Futures, and
OKX V5 API documentation linked from Product Spec section 22.

## Scope boundary

Passing this runbook does not make any route or account live-eligible. The
certification and pilot states in code (`PILOT`, `CERTIFIED_LIVE`) are retained
hardening and are not pursued (DEC-0015, DEC-0018).
