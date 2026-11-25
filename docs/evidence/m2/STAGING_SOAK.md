# M2 24-hour staging run

M2's exit gate: the engine runs 24 hours on staging with no crash and no unreported stale feed, and the scanner lists live routes with freshness.

- **Window:** 2026-11-24 08:57 UTC → 2026-11-25 08:57 UTC. It starts with the deploy of `e35b311` (#14), the first `main` with the live Bybit and OKX parsers (#15) and the PAPER-only service set.
- **Host:** the AWS staging host (`eu-north-1`, DEC-0020), PAPER mode, `PERPETO_PAPER_SOURCE=live`, underlyings BTC and ETH.
- **Sources:** CloudWatch Logs Insights over `/perpeto/staging` for exactly that window; the `scan_runs` table on the host; and `docker inspect` at the end of the window. Queried 2026-11-25 08:58 UTC.

## Result

| Measure | Value |
|---|---|
| Completed scans (`scan complete` in the engine stream) | **2,301** (first 08:57:24, last 08:56:40) |
| Scans per full hour | 95–97 (one every ~37.5 s); the partial first and last hours hold 5 and 3 |
| 5-minute slots with fewer than 7 scans | Only the window's two partial edge slots |
| Stored market snapshots in the window | 2,301 |
| Binance feed status across snapshots | HEALTHY 2,301 |
| Bybit feed status across snapshots | HEALTHY 2,301 |
| OKX feed status across snapshots | HEALTHY 2,300, UNAVAILABLE 1 |
| Crash lines (`panicked`, `panic`, `segfault`, `out of memory`, `SIGKILL`) in any container | **0** |
| Docker restart count of every running container at the end | **0** |

## The one feed outage, reported

At 2026-11-24 09:39:56 UTC the engine logged:

```text
venue feed OKX is UNAVAILABLE: OKX market snapshot: OKX_CONNECT_FAILED: OKX WebSocket transport is unavailable
```

The scan stored that snapshot with OKX `UNAVAILABLE`, so `/health` reported it and entries on OKX legs were refused (DEC-0016). The next scan was HEALTHY again. No stale feed went unreported.

## Planned restarts during the window

Three deploys restarted the services. Each one is a planned rollout, not a crash. Postgres logged "terminating connection due to administrator command" during the two evening rollouts, which is a clean shutdown. Even across the deploys, no full 5-minute slot dropped below 7 scans.

| Time (UTC) | Deploy | Engine and API restarted |
|---|---|---|
| 2026-11-24 15:29 | `50935e6` (#16, read-only demo viewer) | yes |
| 2026-11-24 19:39 | `984c70b` (#17 web hosting, #18 web sign-in) | yes; the demo enabled |
| 2026-11-24 19:51 | `93edf29` (#19, one-command local start) | yes; Postgres and Caddy recreated |

`perpeto-web` was also rolled out (2026-11-24 19:49 and 2026-11-25 08:13), restarting only the `web` container.

## Scanner output

The engine scanned live routes on every tick. 2,111 scans ranked no route as eligible under the default risk limits and the demo strategy's thresholds. The other 190 found 1 to 4 eligible routes (176 scans with 1, 12 with 2, 1 with 3, 1 with 4). Each stored opportunity carries the observation time of its legs, so freshness is visible in the API and the clients.
