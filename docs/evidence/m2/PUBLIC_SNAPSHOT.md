# M2 public snapshot evidence

Live runs of `funding-arb-connector-evidence snapshot`: one live-data PAPER snapshot from the keyless public source (DEC-0016), with default underlyings `BTC,ETH`. No credentials are configured. The output records feed health and leg counts only, never account data.

## 2026-11-22, developer workstation

```json
{
  "legs": 4,
  "taken_at": "2026-11-22T13:10:35.490018248+00:00",
  "venues": [
    { "venue": "BINANCE", "status": "HEALTHY", "legs": 4, "observed_at": "2026-11-22T13:11:01.021377678+00:00", "error": null },
    { "venue": "BYBIT", "status": "UNAVAILABLE", "legs": 0, "observed_at": null, "error": "health timed out" },
    { "venue": "OKX", "status": "UNAVAILABLE", "legs": 0, "observed_at": null, "error": "health timed out" }
  ]
}
```

What this run shows:
- **Binance:** spot and USDⓈ-M perpetual legs for BTC and ETH (4 legs) were built from live books and funding.
- **Bybit and OKX:** unreachable from this workstation's network, independent of Perpeto. `curl` to `api.bybit.com` fails to connect, and `www.okx.com/api/v5/public/time` does not answer within 10 seconds. The source reported both as `UNAVAILABLE` and kept scanning Binance, which is the intended fail-closed behaviour.
- **Two connector defects surfaced and fixed in the same change set (#7):**
  - Binance futures `MIN_NOTIONAL` was parsed with the spot field name, so futures instruments never loaded.
  - Spot `exchangeInfo` was fetched unfiltered (about 18 MB), which timed out on this link.

Still required for M2: a run from the staging VPS where all three venues are reachable, then the 24-hour engine run.
