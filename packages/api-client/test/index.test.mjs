import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  PerpetoApiError,
  PerpetoClient,
  isAuthTransition,
  isHealth,
  isOpportunity,
  isOpportunityScan,
  isPosition,
} from "../dist/index.js";

const paperHealth = {
  status: "OK",
  mode: "PAPER",
  stage: "M0_SCAFFOLD",
};

function response(status, body) {
  return new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json" },
    status,
  });
}

describe("health contract", () => {
  it("accepts only the paper M0 readiness shape", () => {
    assert.equal(isHealth(paperHealth), true);
    assert.equal(
      isHealth({ status: "OK", mode: "LIVE", stage: "M0_SCAFFOLD" }),
      false,
    );
    assert.equal(isHealth({ ...paperHealth, status: "DEGRADED" }), false);
    assert.equal(isHealth(null), false);
  });

  it("normalizes the base URL and returns validated health", async () => {
    let requestedUrl;
    const client = new PerpetoClient({
      baseUrl: "https://staging.example.test///",
      fetchImplementation: async (input) => {
        requestedUrl = input.toString();
        return response(200, paperHealth);
      },
    });

    assert.deepEqual(await client.health(), paperHealth);
    assert.equal(requestedUrl, "https://staging.example.test/readyz");
  });

  it("fails closed on an incompatible payload", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://staging.example.test",
      fetchImplementation: async () =>
        response(200, { status: "OK", mode: "LIVE", stage: "M0_SCAFFOLD" }),
    });

    await assert.rejects(client.health(), /contract is incompatible/u);
  });

  it("reports non-successful readiness responses", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://staging.example.test",
      fetchImplementation: async () => response(503, { status: "BLOCKED" }),
    });

    await assert.rejects(client.health(), /HTTP 503/u);
  });
});

describe("social authentication contract", () => {
  it("accepts only an explicit next state", () => {
    assert.equal(isAuthTransition({ next: "PENDING_APPROVAL", user: {} }), true);
    assert.equal(isAuthTransition({ next: "PASSWORD_REQUIRED", user: {} }), false);
    assert.equal(isAuthTransition({ next: "AUTHENTICATED" }), false);
  });

  it("creates and exchanges one-use provider challenges without authorization", async () => {
    const calls = [];
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "memory-only-access-token",
      fetchImplementation: async (url, init) => {
        calls.push({ url, init });
        if (String(url).endsWith("/challenges")) {
          return Response.json({
            id: "challenge-id",
            provider: "APPLE",
            state: "state",
            nonce: "nonce",
            expires_at: "2026-07-14T12:00:00Z",
          }, { status: 201 });
        }
        return Response.json({
          next: "PENDING_APPROVAL",
          user: {
            id: "user-id",
            status: "PENDING",
            display_name: null,
            email: null,
            roles: [],
            linked_providers: ["APPLE"],
            mfa_enrolled: false,
          },
        });
      },
    });

    await client.createSocialChallenge("APPLE", "signex://auth", "x".repeat(43));
    await client.exchangeSocialAuthorization({
      challenge_id: "challenge-id",
      authorization_code: "secret-code",
      identity_token: "secret-token",
      device: {
        installation_id: "installation-id",
        platform: "IOS",
        app_version: "0.2.0",
        display_name: "iPhone",
      },
    });

    assert.equal(calls.length, 2);
    assert.equal(new Headers(calls[0].init.headers).has("Authorization"), false);
    assert.equal(new Headers(calls[1].init.headers).has("Authorization"), false);
  });

  it("keeps bearer access tokens in the caller-owned memory provider", async () => {
    let authorization;
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (_url, init) => {
        authorization = new Headers(init.headers).get("Authorization");
        return Response.json({
          id: "user-id",
          status: "ACTIVE",
          display_name: null,
          email: null,
          roles: ["VIEWER"],
          linked_providers: ["GOOGLE"],
          mfa_enrolled: false,
        });
      },
    });
    await client.me();
    assert.equal(authorization, "Bearer ephemeral");
  });

  it("normalizes problem responses", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      fetchImplementation: async () => Response.json(
        { code: "REFRESH_TOKEN_REUSE", title: "Session revoked" },
        { status: 409 },
      ),
    });
    await assert.rejects(
      () => client.refresh("x".repeat(43), "device"),
      (error) => error instanceof PerpetoApiError && error.code === "REFRESH_TOKEN_REUSE",
    );
  });
});

describe("opportunity scan contract", () => {
  const scan = {
    scanned_at: "2026-07-19T00:00:00Z",
    model_version: "funding-forecast-v1",
    opportunities: [
      {
        route_id: "PERP_PERP:BINANCE>BYBIT:BTC",
        route_type: "PERP_PERP",
        underlying: "BTC",
        funding_direction: "POSITIVE",
        long_venue: "BINANCE",
        short_venue: "BYBIT",
        gross_funding_apr: 0.36,
        entry_cost: 0.04,
        exit_cost: 0.04,
        carry_cost: 0.02,
        risk_buffer: 0.036,
        break_even_funding: 0.136,
        net_apr: 0.22,
        expected_net_pnl: 1180.5,
        profitability_probability: 0.72,
        entry_basis_bps: 5,
        leverage: 2,
        required_capital: 105000,
        available_capacity: 140000,
        reserved_notional: 100000,
        reserved_capital: 75000,
        capital_fraction: 0.075,
        within_limits: true,
        long_leg: {
          venue: "BINANCE",
          predicted_rate: 0.1,
          p10_rate: 0.05,
          p90_rate: 0.15,
          direction_probability: 0.9,
          next_funding_at: "2026-07-19T08:00:00Z",
        },
        short_leg: {
          venue: "BYBIT",
          predicted_rate: 0.46,
          p10_rate: 0.34,
          p90_rate: 0.58,
          direction_probability: 0.88,
          next_funding_at: "2026-07-19T08:00:00Z",
        },
        next_funding_at: "2026-07-19T08:00:00Z",
      },
    ],
  };

  it("validates the scan shape", () => {
    assert.equal(isOpportunityScan(scan), true);
    assert.equal(isOpportunityScan({ scanned_at: "x", model_version: "y" }), false);
    assert.equal(isOpportunityScan(null), false);
  });

  it("sends bearer auth and encodes filters into the query", async () => {
    let requestedUrl;
    let authorization;
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url, init) => {
        requestedUrl = String(url);
        authorization = new Headers(init.headers).get("Authorization");
        return Response.json(scan);
      },
    });

    const result = await client.listOpportunities({ route_type: "PERP_PERP", min_net_apr: 0.1 });
    assert.equal(result.opportunities[0].route_id, "PERP_PERP:BINANCE>BYBIT:BTC");
    assert.equal(authorization, "Bearer ephemeral");
    assert.match(requestedUrl, /route_type=PERP_PERP/u);
    assert.match(requestedUrl, /min_net_apr=0\.1/u);
  });

  it("fails closed on an incompatible scan payload", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async () => Response.json({ scanned_at: 5 }),
    });
    await assert.rejects(client.listOpportunities(), /contract is incompatible/u);
  });

  it("fetches a single opportunity by route id", async () => {
    let requestedUrl;
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url) => {
        requestedUrl = String(url);
        return Response.json(scan.opportunities[0]);
      },
    });
    const opportunity = await client.getOpportunity("PERP_PERP:BINANCE>BYBIT:BTC");
    assert.equal(isOpportunity(opportunity), true);
    assert.equal(opportunity.within_limits, true);
    assert.equal(opportunity.long_leg.venue, "BINANCE");
    assert.match(requestedUrl, /\/api\/v1\/opportunities\/PERP_PERP/u);
  });

  it("reads the active risk limits", async () => {
    const limits = {
      reference_equity: 1000000,
      max_portfolio_fraction: 0.7,
      max_opportunity_fraction: 0.1,
      max_underlying_fraction: 0.2,
      max_venue_fraction: 0.35,
      max_leverage: 2,
      min_profitability: 0.7,
      min_net_apr: 0.15,
      max_entry_basis_bps: 100,
      min_depth_multiple: 5,
    };
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async () => Response.json(limits),
    });
    assert.deepEqual(await client.getRiskLimits(), limits);
  });
});

describe("paper position contract", () => {
  const position = {
    id: "0190abcd-1111-7000-8000-000000000001",
    route_id: "PERP_PERP:BINANCE>BYBIT:BTC",
    underlying: "BTC",
    route_type: "PERP_PERP",
    funding_direction: "POSITIVE",
    state: "OPEN",
    target_notional: 100000,
    reserved_capital: 75000,
    residual_delta_usd: 90,
    residual_breach_ticks: 1,
    exit_reason: null,
    funding_captured: 0,
    realized_pnl: 0,
    opened_at: "2026-07-19T00:00:00Z",
    long_leg: {
      venue: "BINANCE",
      symbol: "BTCUSDT",
      is_long: true,
      mark_price: 30000,
      funding_rate: 0.0001,
      funding_interval_hours: 8,
      target_qty: 3.333,
      filled_qty: 3.333,
      avg_fill_price: 30009,
      fees: 50,
      orders: [
        { client_order_id: "fa-0190abcd-open-long-1", side: "BUY", target_qty: 3.333, price_cap: 30009, time_in_force: "IOC" },
      ],
      fills: [
        { client_order_id: "fa-0190abcd-open-long-1", qty: 3.333, price: 30009, fee: 50 },
      ],
    },
    short_leg: {
      venue: "BYBIT",
      symbol: "BTCUSDT",
      is_long: false,
      mark_price: 30030,
      funding_rate: 0.003,
      funding_interval_hours: 8,
      target_qty: 3.33,
      filled_qty: 3.33,
      avg_fill_price: 30021,
      fees: 50,
      orders: [
        { client_order_id: "fa-0190abcd-open-short-1", side: "SELL", target_qty: 3.33, price_cap: 30021, time_in_force: "IOC" },
      ],
      fills: [
        { client_order_id: "fa-0190abcd-open-short-1", qty: 3.33, price: 30021, fee: 50 },
      ],
    },
  };

  it("validates the position shape", () => {
    assert.equal(isPosition(position), true);
    assert.equal(isPosition({ id: "x" }), false);
    assert.equal(isPosition({ ...position, residual_breach_ticks: undefined }), false);
    assert.equal(isPosition({ ...position, exit_reason: "NOT_REAL" }), false);
    assert.equal(isPosition(null), false);
  });

  it("opens a position with an idempotency key header", async () => {
    let requestedUrl;
    let idempotencyKey;
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url, init) => {
        requestedUrl = String(url);
        idempotencyKey = new Headers(init.headers).get("Idempotency-Key");
        return Response.json(position, { status: 201 });
      },
    });

    const opened = await client.createPosition({ route_id: position.route_id }, "idem-42");
    assert.equal(opened.state, "OPEN");
    assert.equal(idempotencyKey, "idem-42");
    assert.match(requestedUrl, /\/api\/v1\/positions$/u);
  });

  it("sends deterministic per-leg fill recovery scenarios", async () => {
    let body;
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (_url, init) => {
        body = JSON.parse(String(init.body));
        return Response.json(position, { status: 201 });
      },
    });
    await client.createPosition({
      route_id: position.route_id,
      fill_scenario: {
        long: { outcome: "UNKNOWN", reconciled_fraction: 0.5 },
        short: { outcome: "FULL" },
      },
    }, "recover-1");
    assert.equal(body.fill_scenario.long.reconciled_fraction, 0.5);
  });

  it("lists and fetches positions", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url) =>
        String(url).endsWith("/positions")
          ? Response.json({ positions: [position] })
          : Response.json(position),
    });

    const list = await client.listPositions();
    assert.equal(list.positions[0].route_id, position.route_id);
    const fetched = await client.getPosition(position.id);
    assert.equal(fetched.id, position.id);
  });

  it("fails closed on an incompatible position payload", async () => {
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async () => Response.json({ id: 5 }),
    });
    await assert.rejects(client.getPosition("x"), /contract is incompatible/u);
  });

  it("closes a position via POST", async () => {
    let requestedUrl;
    let method;
    const closed = { ...position, state: "CLOSED", exit_reason: "MANUAL", funding_captured: 250, realized_pnl: 130, closed_at: "2026-07-20T00:00:00Z" };
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url, init) => {
        requestedUrl = String(url);
        method = init.method;
        return Response.json(closed);
      },
    });

    const result = await client.closePosition(position.id);
    assert.equal(result.state, "CLOSED");
    assert.equal(result.funding_captured, 250);
    assert.equal(result.exit_reason, "MANUAL");
    assert.equal(method, "POST");
    assert.match(requestedUrl, /\/api\/v1\/positions\/.*\/close$/u);
  });
});

describe("emergency controls contract", () => {
  it("loads controls and sends emergency mutations", async () => {
    const requests = [];
    const breaker = {
      id: "0190abcd-1111-7000-8000-000000000099",
      scope: "GLOBAL",
      scope_key: null,
      action: "HALT_NEW_RISK",
      source: "OPERATOR",
      reason: "test",
      created_by: "user-1",
      active: true,
      created_at: "2026-07-19T00:00:00Z",
      resolved_at: null,
    };
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url, init = {}) => {
        requests.push([String(url), init.method ?? "GET"]);
        if (String(url).endsWith("/controls/resume")) return Response.json({ resolved: 1 });
        if (String(url).endsWith("/controls")) {
          return Response.json({ effective_action: "HALT_NEW_RISK", breakers: [breaker] });
        }
        return Response.json(breaker);
      },
    });
    assert.equal((await client.getControls()).breakers.length, 1);
    await client.halt();
    await client.disableVenue("BYBIT");
    await client.flatten();
    assert.equal((await client.resumeControls()).resolved, 1);
    assert.equal(requests.filter(([, method]) => method === "POST").length, 4);
  });
});

describe("venue account contract", () => {
  it("exposes only masked account data and sends explicit mutations", async () => {
    const requests = [];
    const account = {
      id: "0190abcd-1111-7000-8000-000000000077",
      venue: "BYBIT",
      alias: "sandbox",
      masked_identity: "****1234",
      enabled: true,
      certification_state: "PAPER_ONLY",
      reconciliation_state: "PENDING",
      connections: [],
      last_reconciled_at: null,
      created_at: "2026-07-20T00:00:00Z",
      tombstoned_at: null,
    };
    const client = new PerpetoClient({
      baseUrl: "https://perpeto.test",
      getAccessToken: () => "ephemeral",
      fetchImplementation: async (url, init = {}) => {
        requests.push([String(url), init.method ?? "GET"]);
        if (String(url).endsWith("/venues")) return Response.json([]);
        if (String(url).endsWith("/venue-accounts")) return Response.json([account]);
        if (String(url).endsWith("/test")) {
          return Response.json({ account_id: account.id, status: "PASS", checked_at: account.created_at, checks: [] });
        }
        return Response.json(account);
      },
    });
    assert.deepEqual(await client.listVenues(), []);
    assert.equal((await client.listVenueAccounts())[0].masked_identity, "****1234");
    assert.equal((await client.getVenueAccount(account.id)).masked_identity, "****1234");
    assert.equal((await client.testVenueAccount(account.id)).status, "PASS");
    await client.disableVenueAccount(account.id);
    assert.equal(requests.filter(([, method]) => method === "POST").length, 2);
    assert.equal(JSON.stringify(requests).includes("api_secret"), false);
  });
});
