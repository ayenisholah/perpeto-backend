# M2 public probe evidence

`funding-arb-connector-evidence public` and `snapshot` run on the AWS staging host (`eu-north-1`, DEC-0020) from the deployed image `ghcr.io/ayenisholah/perpeto-backend:sha-e35b31124e0054a8d13e7dabffd3dabcdcdb7fd5`. Both use the keyless public path that live-data PAPER uses (DEC-0016): no API keys and no writes. The containers ran `--read-only --cap-drop ALL`, started through SSM Run Command.

## 2026-11-24 09:05 UTC, `public`

```json
[
  { "venue": "BYBIT", "environment": "PRODUCTION", "status": "PASS", "health": "HEALTHY",
    "server_time": "2026-11-24T09:05:22.500+00:00", "clock_drift_ms": 124,
    "instrument_count": 4, "market_event": "BOOK_SNAPSHOT",
    "btc_funding_observed_at": "2026-11-24T09:05:23.523410221+00:00" },
  { "venue": "BINANCE", "environment": "PRODUCTION", "status": "PASS", "health": "HEALTHY",
    "server_time": "2026-11-24T09:05:24.878+00:00", "clock_drift_ms": 122,
    "instrument_count": 4, "market_event": "BOOK_SNAPSHOT",
    "btc_funding_observed_at": "2026-11-24T09:05:26.525887850+00:00" },
  { "venue": "OKX", "environment": "PRODUCTION", "status": "PASS", "health": "HEALTHY",
    "server_time": "2026-11-24T09:05:28.211+00:00", "clock_drift_ms": 132,
    "instrument_count": 4, "market_event": "BOOK_SNAPSHOT",
    "btc_funding_observed_at": "2026-11-24T09:05:30.006818094+00:00" }
]
```

## 2026-11-24 09:05 UTC, `snapshot`

```json
{
  "legs": 12,
  "taken_at": "2026-11-24T09:05:32.067195897+00:00",
  "venues": [
    { "venue": "BINANCE", "status": "HEALTHY", "legs": 4, "observed_at": "2026-11-24T09:05:38.683858735+00:00", "error": null },
    { "venue": "BYBIT", "status": "HEALTHY", "legs": 4, "observed_at": "2026-11-24T09:05:37.007036282+00:00", "error": null },
    { "venue": "OKX", "status": "HEALTHY", "legs": 4, "observed_at": "2026-11-24T09:05:39.718974519+00:00", "error": null }
  ]
}
```

What this run shows:
- **All three venues pass** the keyless probe: server time, instrument metadata for BTC and ETH spot and perpetual, a live order-book snapshot, and current BTC funding.
- **Clock drift** between the host and each venue is 122–132 ms, inside the engine's tolerance.
- **The snapshot holds 12 legs:** 2 underlyings × 2 products × 3 venues, each from a live book.
- **Before the fixes:** Bybit and OKX failed validation on live data (`BYBIT_TEXT_MISSING`, `OKX_TEXT_MISSING`) until #15. Bybit spot sizes by `basePrecision`/`minOrderAmt`, and OKX sends `seqId` as a JSON integer. The developer workstation cannot reach Bybit or OKX at all; see [PUBLIC_SNAPSHOT.md](PUBLIC_SNAPSHOT.md).
