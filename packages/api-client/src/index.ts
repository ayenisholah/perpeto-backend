export const schemaVersion = "0.3.0" as const;

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

export interface Opportunity {
  readonly route_id: string;
  readonly route_type: RouteType;
  readonly underlying: string;
  readonly long_venue: Venue;
  readonly short_venue: Venue;
  readonly gross_funding_apr: number;
  readonly entry_cost: number;
  readonly exit_cost: number;
  readonly carry_cost: number;
  readonly risk_buffer: number;
  readonly net_apr: number;
  readonly expected_net_pnl: number;
  readonly profitability_probability: number;
  readonly available_capacity: number;
  readonly next_funding_at: string;
}

export interface OpportunityScan {
  readonly scanned_at: string;
  readonly model_version: string;
  readonly opportunities: readonly Opportunity[];
}

export interface OpportunityFilter {
  readonly route_type?: RouteType;
  readonly venue?: Venue;
  readonly min_net_apr?: number;
  readonly min_probability?: number;
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
}
