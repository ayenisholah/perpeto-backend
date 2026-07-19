export const schemaVersion = "0.5.0" as const;

export type SocialProvider = "APPLE" | "GOOGLE";
export type AuthNextState =
  | "PENDING_APPROVAL"
  | "MFA_REQUIRED"
  | "MFA_ENROLLMENT_REQUIRED"
  | "AUTHENTICATED";
export type UserStatus = "PENDING" | "ACTIVE" | "SUSPENDED" | "DELETED";
export type Role = "VIEWER" | "OWNER" | "TRADER" | "APPROVER";

export interface Health {
  readonly status: "OK";
  readonly mode: "PAPER";
  readonly stage: "M0_SCAFFOLD" | "M1_AUTH";
}

export interface CurrentUser {
  readonly id: string;
  readonly status: UserStatus;
  readonly display_name: string | null;
  readonly email: string | null;
  readonly roles: readonly Role[];
  readonly linked_providers: readonly SocialProvider[];
  readonly mfa_enrolled: boolean;
}

export interface SessionCredentials {
  readonly access_token: string;
  readonly access_token_expires_at: string;
  readonly refresh_token: string;
  readonly session_id: string;
  readonly device_id: string;
}

export interface AuthTransition {
  readonly next: AuthNextState;
  readonly user: CurrentUser;
  readonly challenge_token?: string;
  readonly session?: SessionCredentials;
}

export interface DeviceRegistration {
  readonly installation_id: string;
  readonly platform: "IOS";
  readonly app_version: string;
  readonly display_name: string;
}

export interface SocialChallenge {
  readonly id: string;
  readonly provider: SocialProvider;
  readonly state: string;
  readonly nonce: string;
  readonly expires_at: string;
}

export interface SocialExchangeRequest {
  readonly challenge_id: string;
  readonly authorization_code: string;
  readonly identity_token: string;
  readonly code_verifier?: string;
  readonly apple_profile?: { readonly display_name?: string };
  readonly device: DeviceRegistration;
}

export interface Session {
  readonly id: string;
  readonly device_id: string;
  readonly device_name: string;
  readonly current: boolean;
  readonly created_at: string;
  readonly last_seen_at: string;
}

export interface LinkedIdentity {
  readonly provider: SocialProvider;
  readonly profile_email: string | null;
  readonly linked_at: string;
  readonly last_authenticated_at: string;
}

export interface AccessRequest {
  readonly user_id: string;
  readonly provider: SocialProvider;
  readonly display_name: string | null;
  readonly email: string | null;
  readonly requested_at: string;
}

export interface MfaEnrollment {
  readonly method_id: string;
  readonly secret: string;
  readonly otpauth_uri: string;
}

export type Venue = "BINANCE" | "BYBIT" | "OKX" | "DYDX" | "HYPERLIQUID";
export type RouteType = "SPOT_PERP" | "PERP_PERP";
export type FundingDirection = "POSITIVE" | "NEGATIVE";
export type OpportunitySort =
  | "default"
  | "net_apr"
  | "next_funding"
  | "probability"
  | "capacity"
  | "expected_pnl";

export interface OpportunityLeg {
  readonly venue: Venue;
  readonly predicted_rate: number;
  readonly p10_rate: number;
  readonly p90_rate: number;
  readonly direction_probability: number;
  readonly next_funding_at: string;
}

export interface Opportunity {
  readonly route_id: string;
  readonly route_type: RouteType;
  readonly underlying: string;
  readonly funding_direction: FundingDirection;
  readonly long_venue: Venue;
  readonly short_venue: Venue;
  readonly gross_funding_apr: number;
  readonly entry_cost: number;
  readonly exit_cost: number;
  readonly carry_cost: number;
  readonly risk_buffer: number;
  readonly break_even_funding: number;
  readonly net_apr: number;
  readonly expected_net_pnl: number;
  readonly profitability_probability: number;
  readonly entry_basis_bps: number;
  readonly leverage: number;
  readonly required_capital: number;
  readonly available_capacity: number;
  readonly reserved_notional: number;
  readonly reserved_capital: number;
  readonly capital_fraction: number;
  readonly within_limits: boolean;
  readonly long_leg: OpportunityLeg;
  readonly short_leg: OpportunityLeg;
  readonly next_funding_at: string;
}

export interface OpportunityScan {
  readonly scanned_at: string;
  readonly model_version: string;
  readonly opportunities: readonly Opportunity[];
}

export interface RiskLimits {
  readonly reference_equity: number;
  readonly max_portfolio_fraction: number;
  readonly max_opportunity_fraction: number;
  readonly max_underlying_fraction: number;
  readonly max_venue_fraction: number;
  readonly max_leverage: number;
  readonly min_profitability: number;
  readonly min_net_apr: number;
  readonly max_entry_basis_bps: number;
  readonly min_depth_multiple: number;
  readonly max_unmatched_usd: number;
}

export type PositionState =
  | "QUALIFIED"
  | "RESERVED"
  | "OPENING"
  | "OPEN"
  | "OPEN_FAILED"
  | "REHEDGING"
  | "CLOSING"
  | "CLOSED";

export interface PaperOrder {
  readonly client_order_id: string;
  readonly side: "BUY" | "SELL";
  readonly target_qty: number;
  readonly price_cap: number;
  readonly time_in_force: string;
}

export interface PaperFill {
  readonly client_order_id: string;
  readonly qty: number;
  readonly price: number;
  readonly fee: number;
}

export interface PositionLeg {
  readonly venue: Venue;
  readonly symbol: string;
  readonly is_long: boolean;
  readonly target_qty: number;
  readonly filled_qty: number;
  readonly avg_fill_price: number;
  readonly fees: number;
  readonly orders: readonly PaperOrder[];
  readonly fills: readonly PaperFill[];
}

export interface Position {
  readonly id: string;
  readonly route_id: string;
  readonly underlying: string;
  readonly route_type: RouteType;
  readonly funding_direction: FundingDirection;
  readonly state: PositionState;
  readonly target_notional: number;
  readonly reserved_capital: number;
  readonly residual_delta_usd: number;
  readonly opened_at: string;
  readonly long_leg: PositionLeg;
  readonly short_leg: PositionLeg;
}

export interface PositionList {
  readonly positions: readonly Position[];
}

export interface OpenPositionRequest {
  readonly route_id: string;
  readonly notional?: number;
}

export interface OpportunityFilter {
  readonly route_type?: RouteType;
  readonly venue?: Venue;
  readonly underlying?: string;
  readonly funding_direction?: FundingDirection;
  readonly min_net_apr?: number;
  readonly min_probability?: number;
  readonly max_basis_bps?: number;
  readonly max_time_to_funding_secs?: number;
  readonly sort?: OpportunitySort;
}

export interface PerpetoClientOptions {
  readonly baseUrl: string;
  readonly fetchImplementation?: typeof fetch;
  readonly getAccessToken?: () => string | undefined;
}

export class PerpetoApiError extends Error {
  public constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "PerpetoApiError";
  }
}

export function isHealth(value: unknown): value is Health {
  if (typeof value !== "object" || value === null) return false;
  const health = value as Record<string, unknown>;
  return health.status === "OK" && health.mode === "PAPER" &&
    (health.stage === "M0_SCAFFOLD" || health.stage === "M1_AUTH");
}

export function isAuthTransition(value: unknown): value is AuthTransition {
  if (typeof value !== "object" || value === null) return false;
  const transition = value as Record<string, unknown>;
  return [
    "PENDING_APPROVAL",
    "MFA_REQUIRED",
    "MFA_ENROLLMENT_REQUIRED",
    "AUTHENTICATED",
  ].includes(String(transition.next)) && typeof transition.user === "object" &&
    transition.user !== null;
}

export function isOpportunityScan(value: unknown): value is OpportunityScan {
  if (typeof value !== "object" || value === null) return false;
  const scan = value as Record<string, unknown>;
  return typeof scan.scanned_at === "string" &&
    typeof scan.model_version === "string" &&
    Array.isArray(scan.opportunities);
}

export function isOpportunity(value: unknown): value is Opportunity {
  if (typeof value !== "object" || value === null) return false;
  const opportunity = value as Record<string, unknown>;
  return typeof opportunity.route_id === "string" &&
    typeof opportunity.net_apr === "number" &&
    typeof opportunity.long_leg === "object" && opportunity.long_leg !== null;
}

export function isPosition(value: unknown): value is Position {
  if (typeof value !== "object" || value === null) return false;
  const position = value as Record<string, unknown>;
  return typeof position.id === "string" &&
    typeof position.route_id === "string" &&
    typeof position.state === "string" &&
    typeof position.long_leg === "object" && position.long_leg !== null;
}

export class PerpetoClient {
  readonly #baseUrl: string;
  readonly #fetch: typeof fetch;
  readonly #getAccessToken: () => string | undefined;

  public constructor(options: PerpetoClientOptions) {
    this.#baseUrl = options.baseUrl.replace(/\/+$/u, "");
    this.#fetch = options.fetchImplementation ?? fetch;
    this.#getAccessToken = options.getAccessToken ?? (() => undefined);
  }

  async #request<T>(path: string, init: RequestInit = {}, authenticated = true): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    if (init.body !== undefined) headers.set("Content-Type", "application/json");
    const accessToken = authenticated ? this.#getAccessToken() : undefined;
    if (accessToken !== undefined) headers.set("Authorization", `Bearer ${accessToken}`);
    const response = await this.#fetch(`${this.#baseUrl}${path}`, { ...init, headers });
    if (!response.ok) {
      let code = "REQUEST_FAILED";
      let message = `Perpeto request failed with HTTP ${response.status}`;
      try {
        const problem = await response.json() as { code?: string; title?: string };
        code = problem.code ?? code;
        message = problem.title ?? message;
      } catch {
        // Keep the generic failure; authentication responses must not leak details.
      }
      throw new PerpetoApiError(response.status, code, message);
    }
    if (response.status === 204) return undefined as T;
    return await response.json() as T;
  }

  public async health(signal?: AbortSignal): Promise<Health> {
    const payload = await this.#request<unknown>(
      "/readyz",
      signal === undefined ? {} : { signal },
      false,
    );
    if (!isHealth(payload)) throw new Error("Perpeto readiness contract is incompatible");
    return payload;
  }

  public createSocialChallenge(
    provider: SocialProvider,
    redirectUri: string,
    codeChallenge: string,
    purpose: "SIGN_IN" | "LINK_IDENTITY" = "SIGN_IN",
  ): Promise<SocialChallenge> {
    return this.#request("/api/v1/auth/social/challenges", {
      method: "POST",
      body: JSON.stringify({ provider, redirect_uri: redirectUri, code_challenge: codeChallenge, purpose }),
    }, false);
  }

  public async exchangeSocialAuthorization(request: SocialExchangeRequest): Promise<AuthTransition> {
    const payload = await this.#request<unknown>("/api/v1/auth/social/exchange", {
      method: "POST",
      body: JSON.stringify(request),
    }, false);
    if (!isAuthTransition(payload)) throw new Error("Perpeto auth transition is incompatible");
    return payload;
  }

  public bootstrap(deploymentToken: string): Promise<AuthTransition> {
    return this.#request("/api/v1/auth/bootstrap", {
      method: "POST",
      body: JSON.stringify({ deployment_token: deploymentToken }),
    });
  }

  public enrollMfa(): Promise<MfaEnrollment> {
    return this.#request("/api/v1/auth/mfa/enroll", { method: "POST" });
  }

  public confirmMfa(challengeToken: string, code: string): Promise<{ recovery_codes: string[] }> {
    return this.#request("/api/v1/auth/mfa/confirm", {
      method: "POST",
      body: JSON.stringify({ challenge_token: challengeToken, code }),
    });
  }

  public verifyMfa(
    challengeToken: string,
    code: string,
    recoveryCode = false,
    recoveryCodesAcknowledged = false,
  ): Promise<SessionCredentials> {
    return this.#request("/api/v1/auth/mfa/verify", {
      method: "POST",
      body: JSON.stringify({
        challenge_token: challengeToken,
        code,
        recovery_code: recoveryCode,
        recovery_codes_acknowledged: recoveryCodesAcknowledged,
      }),
    });
  }

  public acknowledgeRecoveryCodes(
    challengeToken: string,
    codeSetDigest: string,
  ): Promise<SessionCredentials> {
    return this.#request("/api/v1/auth/mfa/recovery-codes/acknowledge", {
      method: "POST",
      body: JSON.stringify({ challenge_token: challengeToken, code_set_digest: codeSetDigest }),
    });
  }

  public refresh(refreshToken: string, deviceId: string): Promise<SessionCredentials> {
    return this.#request("/api/v1/auth/refresh", {
      method: "POST",
      body: JSON.stringify({ refresh_token: refreshToken, device_id: deviceId }),
    }, false);
  }

  public me(): Promise<CurrentUser> {
    return this.#request("/api/v1/auth/me");
  }

  public logout(): Promise<void> {
    return this.#request("/api/v1/auth/logout", { method: "POST" });
  }

  public listSessions(): Promise<readonly Session[]> {
    return this.#request("/api/v1/auth/sessions");
  }

  public revokeSession(sessionId: string): Promise<void> {
    return this.#request(`/api/v1/auth/sessions/${encodeURIComponent(sessionId)}`, { method: "DELETE" });
  }

  public listIdentities(): Promise<readonly LinkedIdentity[]> {
    return this.#request("/api/v1/auth/identities");
  }

  public linkIdentity(request: {
    readonly challenge_id: string;
    readonly authorization_code: string;
    readonly identity_token: string;
    readonly code_verifier: string;
    readonly totp_code?: string;
  }): Promise<LinkedIdentity> {
    return this.#request("/api/v1/auth/identities/link", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public unlinkIdentity(provider: SocialProvider): Promise<void> {
    return this.#request(`/api/v1/auth/identities/${provider}`, { method: "DELETE" });
  }

  public listAccessRequests(): Promise<readonly AccessRequest[]> {
    return this.#request("/api/v1/access-requests");
  }

  public approveAccessRequest(userId: string): Promise<CurrentUser> {
    return this.#request(`/api/v1/access-requests/${encodeURIComponent(userId)}/approve`, { method: "POST" });
  }

  public deleteAccount(): Promise<void> {
    return this.#request("/api/v1/account", { method: "DELETE" });
  }

  public async listOpportunities(filter: OpportunityFilter = {}): Promise<OpportunityScan> {
    const params = Object.entries(filter)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => `${key}=${encodeURIComponent(String(value))}`);
    const suffix = params.length > 0 ? `?${params.join("&")}` : "";
    const payload = await this.#request<unknown>(`/api/v1/opportunities${suffix}`);
    if (!isOpportunityScan(payload)) {
      throw new Error("Perpeto opportunity scan contract is incompatible");
    }
    return payload;
  }

  public async getOpportunity(routeId: string): Promise<Opportunity> {
    const payload = await this.#request<unknown>(
      `/api/v1/opportunities/${encodeURIComponent(routeId)}`,
    );
    if (!isOpportunity(payload)) {
      throw new Error("Perpeto opportunity contract is incompatible");
    }
    return payload;
  }

  public getRiskLimits(): Promise<RiskLimits> {
    return this.#request("/api/v1/risk/limits");
  }

  public async createPosition(
    request: OpenPositionRequest,
    idempotencyKey: string = randomIdempotencyKey(),
  ): Promise<Position> {
    const payload = await this.#request<unknown>("/api/v1/positions", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey },
      body: JSON.stringify(request),
    });
    if (!isPosition(payload)) {
      throw new Error("Perpeto position contract is incompatible");
    }
    return payload;
  }

  public listPositions(): Promise<PositionList> {
    return this.#request("/api/v1/positions");
  }

  public async getPosition(id: string): Promise<Position> {
    const payload = await this.#request<unknown>(`/api/v1/positions/${encodeURIComponent(id)}`);
    if (!isPosition(payload)) {
      throw new Error("Perpeto position contract is incompatible");
    }
    return payload;
  }
}

function randomIdempotencyKey(): string {
  const cryptoRef = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (cryptoRef?.randomUUID !== undefined) return cryptoRef.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}
