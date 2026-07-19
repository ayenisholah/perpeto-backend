import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  PerpetoApiError,
  PerpetoClient,
  isAuthTransition,
  isHealth,
  isOpportunityScan,
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
        long_venue: "BINANCE",
        short_venue: "BYBIT",
        gross_funding_apr: 0.36,
        entry_cost: 0.04,
        exit_cost: 0.04,
        carry_cost: 0.02,
        risk_buffer: 0.036,
        net_apr: 0.22,
        expected_net_pnl: 1180.5,
        profitability_probability: 0.72,
        available_capacity: 140000,
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
});
