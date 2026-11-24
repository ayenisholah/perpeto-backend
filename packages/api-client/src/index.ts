export const schemaVersion = "0.13.0" as const;

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
  readonly mode: "PAPER" | "SANDBOX" | "SHADOW";
  readonly stage: "M0_SCAFFOLD" | "M1_AUTH" | "M3_CONNECTORS";
}

export interface VenueSummary {
  readonly venue: "BINANCE" | "BYBIT" | "OKX";
  readonly display_name: string;
  readonly products: readonly ("SPOT" | "LINEAR_PERPETUAL")[];
  readonly sandbox_environment: "TESTNET" | "DEMO";
  readonly configured_accounts: number;
}

export interface ProductConnection {
  readonly id: string;
  readonly product: "SPOT" | "LINEAR_PERPETUAL";
  readonly environment: "PRODUCTION" | "TESTNET" | "DEMO";
  readonly secret_ref: string | null;
  readonly read_permission: boolean;
  readonly trade_permission: boolean;
  readonly withdrawal_permission: boolean | null;
  readonly status: string;
  readonly last_tested_at: string | null;
}

export interface VenueAccount {
  readonly id: string;
  readonly venue: "BINANCE" | "BYBIT" | "OKX";
  readonly alias: string;
  readonly masked_identity: string;
  readonly enabled: boolean;
  readonly certification_state: "DISABLED" | "PAPER_ONLY" | "SHADOW" | "PILOT" | "CERTIFIED_LIVE";
  readonly reconciliation_state: "PENDING" | "RECONCILED" | "REQUIRED" | "FROZEN";
  readonly connections: readonly ProductConnection[];
  readonly last_reconciled_at: string | null;
  readonly created_at: string;
  readonly tombstoned_at: string | null;
}

export interface ConnectionTest {
  readonly account_id: string;
  readonly status: "PASS" | "FAIL";
  readonly checked_at: string;
  readonly checks: readonly string[];
}

export interface ConnectorOrderRequest {
  readonly product: "SPOT" | "LINEAR_PERPETUAL";
  readonly symbol: string;
  readonly side: "BUY" | "SELL";
  readonly base_quantity: string;
  readonly limit_price: string;
  readonly client_order_id: string;
  readonly reduce_only?: boolean;
}

export interface ConnectorOrderPreview {
  readonly venue: "BINANCE" | "BYBIT" | "OKX";
  readonly environment: "PRODUCTION" | "TESTNET" | "DEMO";
  readonly product: "SPOT" | "LINEAR_PERPETUAL";
  readonly native_symbol: string;
  readonly native_quantity: string;
  readonly equivalent_base_quantity: string;
  readonly native_price: string;
  readonly projected_delta_usd: string;
  readonly payload_hash: string;
  readonly payload: Readonly<Record<string, unknown>>;
}

export interface ConnectorSubmission {
  readonly intent_id: string;
  readonly state: "DUPLICATE" | "ACKNOWLEDGED" | "UNKNOWN_OUTCOME" | "FAILED";
  readonly venue_order_id: string | null;
  readonly client_order_id: string;
}

export interface ConnectorOrder {
  readonly id: string;
  readonly venue: "BINANCE" | "BYBIT" | "OKX";
  readonly venue_account_id: string;
  readonly client_order_id: string;
  readonly venue_order_id: string | null;
  readonly symbol: string;
  readonly product: "SPOT" | "LINEAR_PERPETUAL";
  readonly side: "BUY" | "SELL";
  readonly base_quantity: string;
  readonly limit_price: string;
  readonly native_quantity: string;
  readonly native_price: string;
  readonly equivalent_base_quantity: string;
  readonly projected_delta_usd: string;
  readonly reduce_only: boolean;
  readonly environment: "PRODUCTION" | "TESTNET" | "DEMO";
  readonly state: string;
  readonly payload_hash: string;
  readonly error_code: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface ReconciliationRun {
  readonly id: string;
  readonly venue: "BINANCE" | "BYBIT" | "OKX";
  readonly venue_account_id: string;
  readonly status: string;
  readonly started_at: string;
  readonly completed_at: string | null;
  readonly balances_seen: number;
  readonly positions_seen: number;
  readonly open_orders_seen: number;
  readonly fills_seen: number;
  readonly discrepancies: number;
}

export interface ExactBalance {
  readonly asset: string;
  readonly total: string;
  readonly available: string;
  readonly locked: string;
  readonly received_at: string;
}

export interface ExactVenuePosition {
  readonly symbol: string;
  readonly product: "SPOT" | "LINEAR_PERPETUAL";
  readonly quantity: string;
  readonly entry_price: string;
  readonly mark_price: string;
  readonly leverage: string;
  readonly liquidation_price: string | null;
  readonly received_at: string;
}

export interface ConnectorAccounting {
  readonly venue_account_id: string;
  readonly run_id: string | null;
  readonly balances: readonly ExactBalance[];
  readonly positions: readonly ExactVenuePosition[];
}

export interface TenantSummary {
  readonly id: string;
  readonly display_name: string;
  readonly base_currency: "USD";
  readonly state: "PAPER_ONLY" | "PILOT_ELIGIBLE" | "PILOT_ARMED" | "SUSPENDED" | "DELETED";
}

export interface CurrentUser {
  readonly id: string;
  readonly tenant: TenantSummary;
  readonly status: UserStatus;
  readonly display_name: string | null;
  readonly email: string | null;
  readonly roles: readonly Role[];
  readonly linked_providers: readonly SocialProvider[];
  readonly mfa_enrolled: boolean;
  /** The read-only demo principal (DEC-0017). Absent before 0.24.0, meaning false. */
  readonly demo?: boolean;
}

/** An access-only session for the read-only demo principal (DEC-0017). */
export interface DemoSession {
  readonly access_token: string;
  readonly access_token_expires_at: string;
  readonly session_id: string;
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

export interface AuditEntry {
  readonly occurred_at: string;
  readonly event_type: string;
  readonly subject_type: string;
  readonly subject_id: string;
  readonly actor: string;
  readonly details: Record<string, unknown>;
  readonly event_hash: string;
}

export interface AuditLog {
  readonly intact: boolean;
  readonly events: readonly AuditEntry[];
}

export interface MfaEnrollment {
  readonly method_id: string;
  readonly secret: string;
  readonly otpauth_uri: string;
}

export interface Passkey {
  readonly id: string;
  readonly label: string;
  readonly created_at: string;
  readonly last_used_at: string | null;
}

/** Opaque WebAuthn ceremony options relayed verbatim to the authenticator. */
export interface PasskeyCeremonyChallenge {
  readonly challenge_id: string;
  readonly options: Record<string, unknown>;
}

export interface PasskeyAssertionOutcome {
  readonly verified: boolean;
  readonly credential_id: string;
}

export type CertificationState =
  | "DISABLED"
  | "PAPER_ONLY"
  | "SHADOW"
  | "PILOT"
  | "CERTIFIED_LIVE";

/** Write-only exchange credential. Never returned by any endpoint. */
export interface CredentialSecret {
  readonly api_key: string;
  readonly api_secret: string;
  readonly passphrase?: string | null;
}

export interface PreflightReconciliation {
  readonly status: string;
  readonly balances_seen: number;
  readonly positions_seen: number;
  readonly open_orders_seen: number;
  readonly discrepancies: number;
}

export interface PreflightReport {
  readonly account_id: string;
  readonly product: string;
  readonly status: string;
  readonly checks: readonly string[];
  readonly reconciliation: PreflightReconciliation | null;
}

export type PilotState =
  | "PAPER_ONLY"
  | "PILOT_ELIGIBLE"
  | "PILOT_ARMED"
  | "SUSPENDED"
  | "DELETED";

export interface EligibleRoute {
  readonly venue_account_id: string;
  readonly alias: string;
  readonly product: string;
}

export interface PilotStatus {
  readonly state: PilotState;
  readonly eligible: readonly EligibleRoute[];
}

export type Venue = "BINANCE" | "BYBIT" | "OKX" | "DYDX" | "HYPERLIQUID";
/** Canonical base-10 decimal serialized without an IEEE-754 conversion. */
export type DecimalString = string;
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
  readonly predicted_rate: DecimalString;
  readonly p10_rate: DecimalString;
  readonly p90_rate: DecimalString;
  readonly direction_probability: DecimalString;
  readonly next_funding_at: string;
  /** When the leg's market data was observed; absent on scans before 0.23.0. */
  readonly observed_at?: string;
}

export interface Opportunity {
  readonly route_id: string;
  readonly route_type: RouteType;
  readonly underlying: string;
  readonly funding_direction: FundingDirection;
  readonly long_venue: Venue;
  readonly short_venue: Venue;
  readonly gross_funding_apr: DecimalString;
  readonly entry_cost: DecimalString;
  readonly exit_cost: DecimalString;
  readonly carry_cost: DecimalString;
  readonly risk_buffer: DecimalString;
  readonly break_even_funding: DecimalString;
  readonly net_apr: DecimalString;
  readonly expected_net_pnl: DecimalString;
  readonly profitability_probability: DecimalString;
  readonly entry_basis_bps: DecimalString;
  readonly leverage: DecimalString;
  readonly required_capital: DecimalString;
  readonly available_capacity: DecimalString;
  readonly reserved_notional: DecimalString;
  readonly reserved_capital: DecimalString;
  readonly capital_fraction: DecimalString;
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
  readonly reference_equity: DecimalString;
  readonly max_portfolio_fraction: DecimalString;
  readonly max_opportunity_fraction: DecimalString;
  readonly max_underlying_fraction: DecimalString;
  readonly max_venue_fraction: DecimalString;
  readonly max_leverage: DecimalString;
  readonly min_profitability: DecimalString;
  readonly min_net_apr: DecimalString;
  readonly max_entry_basis_bps: DecimalString;
  readonly min_depth_multiple: DecimalString;
  readonly max_unmatched_usd: DecimalString;
  readonly max_daily_loss_fraction: DecimalString;
  readonly max_drawdown_fraction: DecimalString;
}

export type PositionState =
  | "QUALIFIED"
  | "RESERVED"
  | "OPENING"
  | "OPEN"
  | "OPEN_FAILED"
  | "RECOVERING"
  | "REHEDGING"
  | "CLOSING"
  | "CLOSED";

export type ExitReason =
  | "FUNDING_FLIP"
  | "THRESHOLD_BREACH"
  | "VENUE_UNHEALTHY"
  | "MANUAL"
  | "RECOVERY_FAILURE"
  | "CIRCUIT_BREAKER";

export type FillOutcome =
  | { readonly outcome: "FULL" }
  | { readonly outcome: "PARTIAL"; readonly fraction: DecimalString }
  | { readonly outcome: "UNKNOWN"; readonly reconciled_fraction: DecimalString };

export interface PaperFillScenario {
  readonly long: FillOutcome;
  readonly short: FillOutcome;
}

export interface PaperOrder {
  readonly client_order_id: string;
  readonly side: "BUY" | "SELL";
  readonly target_qty: DecimalString;
  readonly price_cap: DecimalString;
  readonly time_in_force: string;
}

export interface PaperFill {
  readonly client_order_id: string;
  readonly qty: DecimalString;
  readonly price: DecimalString;
  readonly fee: DecimalString;
}

export interface PositionLeg {
  readonly venue: Venue;
  readonly symbol: string;
  readonly is_long: boolean;
  readonly mark_price: DecimalString;
  readonly funding_rate: DecimalString;
  readonly funding_interval_hours: number;
  readonly target_qty: DecimalString;
  readonly filled_qty: DecimalString;
  readonly avg_fill_price: DecimalString;
  readonly fees: DecimalString;
  readonly orders: readonly PaperOrder[];
  readonly fills: readonly PaperFill[];
}

export interface Position {
  readonly id: string;
  readonly route_id: string;
  readonly strategy_id: string | null;
  readonly underlying: string;
  readonly route_type: RouteType;
  readonly funding_direction: FundingDirection;
  readonly state: PositionState;
  readonly target_notional: DecimalString;
  readonly reserved_capital: DecimalString;
  readonly residual_delta_usd: DecimalString;
  readonly residual_breach_ticks: number;
  readonly exit_reason: ExitReason | null;
  readonly funding_captured: DecimalString;
  readonly realized_pnl: DecimalString;
  readonly opened_at: string;
  readonly closed_at?: string;
  readonly long_leg: PositionLeg;
  readonly short_leg: PositionLeg;
}

export interface PositionList {
  readonly positions: readonly Position[];
}

export interface OpenPositionRequest {
  readonly route_id: string;
  readonly notional?: DecimalString;
  readonly fill_scenario?: PaperFillScenario;
}

export type BreakerScope = "GLOBAL" | "VENUE" | "STRATEGY";
export type BreakerAction = "HALT_NEW_RISK" | "DISABLE_VENUE" | "READ_ONLY" | "FLATTEN";

export interface CircuitBreaker {
  readonly id: string;
  readonly scope: BreakerScope;
  readonly scope_key: string | null;
  readonly action: BreakerAction;
  readonly source: string;
  readonly reason: string;
  readonly created_by: string | null;
  readonly active: boolean;
  readonly created_at: string;
  readonly resolved_at: string | null;
}

export interface Controls {
  readonly effective_action: BreakerAction | null;
  readonly breakers: readonly CircuitBreaker[];
}

export interface StrategyAllocation {
  readonly method: "FIXED" | "PERCENT_EQUITY" | "BOUNDED_DYNAMIC";
  readonly value?: DecimalString;
  readonly minimum?: DecimalString;
  readonly maximum?: DecimalString;
}

export interface StrategyVersion {
  readonly version: number;
  readonly creator: string;
  readonly created_at: string;
  readonly mode: "PAPER";
  readonly route_types: readonly RouteType[];
  readonly allowed_venues: readonly Venue[];
  readonly allowed_assets: readonly string[];
  readonly min_net_apr: DecimalString;
  readonly min_profitability: DecimalString;
  readonly min_forecast_coverage: DecimalString;
  readonly max_time_to_funding_minutes: number;
  readonly max_basis_bps: DecimalString;
  readonly allocation: StrategyAllocation;
  readonly max_concurrent: number;
  readonly cooldown_seconds: number;
  readonly hold_seconds: number;
  readonly auto_compound: { readonly enabled: boolean; readonly reinvest_fraction: DecimalString; readonly minimum_reinvest_amount: DecimalString };
  readonly exit_net_apr: DecimalString;
  readonly take_profit_fraction?: DecimalString;
  readonly stop_loss_fraction?: DecimalString;
  readonly max_drawdown_fraction: DecimalString;
}

export interface Strategy { readonly id: string; readonly name: string; readonly enabled: boolean; readonly current: StrategyVersion }

export interface PortfolioAllocation { readonly key: string; readonly deployed_capital: DecimalString; readonly fraction: DecimalString }
export interface Portfolio { readonly equity: DecimalString; readonly deployable_equity: DecimalString; readonly deployed_capital: DecimalString; readonly by_venue: readonly PortfolioAllocation[]; readonly by_underlying: readonly PortfolioAllocation[]; readonly pnl: { readonly realized_funding: DecimalString; readonly realized_trading: DecimalString; readonly unrealized_leg: DecimalString; readonly fees: DecimalString; readonly net: DecimalString; readonly return_on_deployed_capital: DecimalString }; readonly as_of: string }
export type FeedStatus = "HEALTHY" | "DEGRADED" | "UNAVAILABLE" | "UNKNOWN";
export interface VenueHealth {
  readonly venue: Venue;
  readonly status: "HEALTHY" | "DEGRADED";
  readonly connected: boolean;
  readonly market_data_age_seconds: number;
  readonly clock_drift_ms: number;
  /** Public feed health from the latest stored market snapshot (0.23.0). */
  readonly feed_status: FeedStatus;
  readonly last_observed_at: string | null;
  readonly feed_error: string | null;
}
export interface ServiceHeartbeat { readonly service: string; readonly status: string; readonly observed_at: string }
export interface SystemHealth { readonly venues: readonly VenueHealth[]; readonly services: readonly ServiceHeartbeat[]; readonly as_of: string }
export interface Alert { readonly id: string; readonly dedup_key: string; readonly severity: "INFORMATIONAL" | "WARNING" | "CRITICAL"; readonly title: string; readonly message: string; readonly created_at: string; readonly acknowledged_at: string | null; readonly acknowledged_by: string | null }

export interface OpportunityFilter {
  readonly route_type?: RouteType;
  readonly venue?: Venue;
  readonly underlying?: string;
  readonly funding_direction?: FundingDirection;
  readonly min_net_apr?: DecimalString;
  readonly min_probability?: DecimalString;
  readonly max_basis_bps?: DecimalString;
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
  return health.status === "OK" &&
    (health.mode === "PAPER" || health.mode === "SANDBOX" || health.mode === "SHADOW") &&
    (health.stage === "M0_SCAFFOLD" || health.stage === "M1_AUTH" || health.stage === "M3_CONNECTORS");
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
    typeof opportunity.net_apr === "string" &&
    typeof opportunity.long_leg === "object" && opportunity.long_leg !== null;
}

export function isPosition(value: unknown): value is Position {
  if (typeof value !== "object" || value === null) return false;
  const position = value as Record<string, unknown>;
  return typeof position.id === "string" &&
    typeof position.route_id === "string" &&
    typeof position.state === "string" &&
    typeof position.residual_breach_ticks === "number" &&
    (position.exit_reason === null || [
      "FUNDING_FLIP",
      "THRESHOLD_BREACH",
      "VENUE_UNHEALTHY",
      "MANUAL",
      "RECOVERY_FAILURE",
      "CIRCUIT_BREAKER",
    ].includes(String(position.exit_reason))) &&
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

  /**
   * Starts a read-only demo session (DEC-0017). The session has no refresh
   * token; call again once `access_token_expires_at` passes.
   */
  public createDemoSession(): Promise<DemoSession> {
    return this.#request("/api/v1/demo/session", { method: "POST" }, false);
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

  public getAuditLog(): Promise<AuditLog> {
    return this.#request("/api/v1/audit");
  }

  public approveAccessRequest(userId: string): Promise<CurrentUser> {
    return this.#request(`/api/v1/access-requests/${encodeURIComponent(userId)}/approve`, { method: "POST" });
  }

  public deleteAccount(): Promise<void> {
    return this.#request("/api/v1/account", { method: "DELETE" });
  }

  public listPasskeys(): Promise<readonly Passkey[]> {
    return this.#request("/api/v1/passkeys");
  }

  public startPasskeyRegistration(): Promise<PasskeyCeremonyChallenge> {
    return this.#request("/api/v1/passkeys/register/start", { method: "POST" });
  }

  public finishPasskeyRegistration(request: {
    readonly challenge_id: string;
    readonly label: string;
    readonly credential: Record<string, unknown>;
  }): Promise<Passkey> {
    return this.#request("/api/v1/passkeys/register/finish", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public startPasskeyAuthentication(): Promise<PasskeyCeremonyChallenge> {
    return this.#request("/api/v1/passkeys/authenticate/start", { method: "POST" });
  }

  public finishPasskeyAuthentication(request: {
    readonly challenge_id: string;
    readonly credential: Record<string, unknown>;
  }): Promise<PasskeyAssertionOutcome> {
    return this.#request("/api/v1/passkeys/authenticate/finish", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public revokePasskey(passkeyId: string): Promise<void> {
    return this.#request(`/api/v1/passkeys/${encodeURIComponent(passkeyId)}/revoke`, {
      method: "POST",
    });
  }

  public startCredentialEnrollment(request: {
    readonly venue: Venue;
    readonly alias: string;
    readonly product: string;
    readonly environment: string;
  }): Promise<PasskeyCeremonyChallenge> {
    return this.#request("/api/v1/venue-credentials/enroll/start", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public finishCredentialEnrollment(request: {
    readonly challenge_id: string;
    readonly venue: Venue;
    readonly alias: string;
    readonly product: string;
    readonly environment: string;
    readonly assertion: Record<string, unknown>;
    readonly credential: CredentialSecret;
  }): Promise<VenueAccount> {
    return this.#request("/api/v1/venue-credentials/enroll/finish", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public startCredentialRotation(
    accountId: string,
    request: { readonly product: string },
  ): Promise<PasskeyCeremonyChallenge> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/rotate/start`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public finishCredentialRotation(
    accountId: string,
    request: {
      readonly challenge_id: string;
      readonly product: string;
      readonly assertion: Record<string, unknown>;
      readonly credential: CredentialSecret;
    },
  ): Promise<VenueAccount> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/rotate/finish`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public startCredentialRevocation(
    accountId: string,
    request: { readonly product: string },
  ): Promise<PasskeyCeremonyChallenge> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/revoke/start`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public finishCredentialRevocation(
    accountId: string,
    request: {
      readonly challenge_id: string;
      readonly product: string;
      readonly assertion: Record<string, unknown>;
    },
  ): Promise<VenueAccount> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/revoke/finish`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public startCredentialDeletion(accountId: string): Promise<PasskeyCeremonyChallenge> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/delete/start`,
      { method: "POST" },
    );
  }

  public finishCredentialDeletion(
    accountId: string,
    request: {
      readonly challenge_id: string;
      readonly assertion: Record<string, unknown>;
    },
  ): Promise<void> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/delete/finish`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public startRouteCertification(
    accountId: string,
    request: { readonly target: CertificationState },
  ): Promise<PasskeyCeremonyChallenge> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/certify/start`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public finishRouteCertification(
    accountId: string,
    request: {
      readonly challenge_id: string;
      readonly target: CertificationState;
      readonly assertion: Record<string, unknown>;
    },
  ): Promise<VenueAccount> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/certify/finish`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public runPreflight(
    accountId: string,
    request: { readonly product: string },
  ): Promise<PreflightReport> {
    return this.#request(
      `/api/v1/venue-credentials/${encodeURIComponent(accountId)}/preflight`,
      { method: "POST", body: JSON.stringify(request) },
    );
  }

  public getPilotStatus(): Promise<PilotStatus> {
    return this.#request("/api/v1/pilot");
  }

  public grantPilotEligibility(): Promise<PilotStatus> {
    return this.#request("/api/v1/pilot/eligibility", { method: "POST" });
  }

  public startPilotArm(request: {
    readonly venue_account_id: string;
    readonly product: string;
  }): Promise<PasskeyCeremonyChallenge> {
    return this.#request("/api/v1/pilot/arm/start", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public finishPilotArm(request: {
    readonly challenge_id: string;
    readonly venue_account_id: string;
    readonly product: string;
    readonly assertion: Record<string, unknown>;
    readonly confirmation: string;
  }): Promise<PilotStatus> {
    return this.#request("/api/v1/pilot/arm/finish", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  public disarmPilot(): Promise<PilotStatus> {
    return this.#request("/api/v1/pilot/disarm", { method: "POST" });
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

  public async closePosition(id: string): Promise<Position> {
    const payload = await this.#request<unknown>(
      `/api/v1/positions/${encodeURIComponent(id)}/close`,
      { method: "POST" },
    );
    if (!isPosition(payload)) {
      throw new Error("Perpeto position contract is incompatible");
    }
    return payload;
  }

  public getControls(): Promise<Controls> {
    return this.#request("/api/v1/controls");
  }

  public halt(
    action: "HALT_NEW_RISK" | "READ_ONLY" = "HALT_NEW_RISK",
    reason = "Operator emergency control",
  ): Promise<CircuitBreaker> {
    return this.#request("/api/v1/controls/halt", {
      method: "POST",
      body: JSON.stringify({ action, reason }),
    });
  }

  public disableVenue(venue: Venue, reason = "Operator disabled venue"): Promise<CircuitBreaker> {
    return this.#request(`/api/v1/controls/venues/${venue}/disable`, {
      method: "POST",
      body: JSON.stringify({ reason }),
    });
  }

  public flatten(reason = "Operator emergency flatten"): Promise<CircuitBreaker> {
    return this.#request("/api/v1/controls/flatten", {
      method: "POST",
      body: JSON.stringify({ reason }),
    });
  }

  public resumeControls(): Promise<{ readonly resolved: number }> {
    return this.#request("/api/v1/controls/resume", { method: "POST" });
  }

  public listStrategies(): Promise<readonly Strategy[]> { return this.#request("/api/v1/strategies"); }
  public getStrategy(id: string): Promise<Strategy> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}`); }
  public createStrategy(strategy: Strategy): Promise<Strategy> { return this.#request("/api/v1/strategies", { method: "POST", body: JSON.stringify(strategy) }); }
  public updateStrategy(id: string, strategy: Strategy): Promise<Strategy> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}`, { method: "PUT", body: JSON.stringify(strategy) }); }
  public deleteStrategy(id: string): Promise<void> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}`, { method: "DELETE" }); }
  public enableStrategy(id: string): Promise<Strategy> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}/enable`, { method: "POST" }); }
  public disableStrategy(id: string): Promise<Strategy> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}/disable`, { method: "POST" }); }
  public applyStrategyToOpen(id: string): Promise<{ readonly applied: number }> { return this.#request(`/api/v1/strategies/${encodeURIComponent(id)}/apply-to-open`, { method: "POST" }); }
  public getPortfolio(): Promise<Portfolio> { return this.#request("/api/v1/portfolio"); }
  public getSystemHealth(): Promise<SystemHealth> { return this.#request("/api/v1/health"); }
  public listAlerts(): Promise<readonly Alert[]> { return this.#request("/api/v1/alerts"); }
  public acknowledgeAlert(id: string): Promise<Alert> { return this.#request(`/api/v1/alerts/${encodeURIComponent(id)}/ack`, { method: "POST" }); }
  public listVenues(): Promise<readonly VenueSummary[]> { return this.#request("/api/v1/venues"); }
  public listVenueAccounts(): Promise<readonly VenueAccount[]> { return this.#request("/api/v1/venue-accounts"); }
  public getVenueAccount(id: string): Promise<VenueAccount> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}`); }
  public testVenueAccount(id: string): Promise<ConnectionTest> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}/test`, { method: "POST" }); }
  public disableVenueAccount(id: string): Promise<VenueAccount> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}/disable`, { method: "POST" }); }
  public previewConnectorOrder(id: string, request: ConnectorOrderRequest): Promise<ConnectorOrderPreview> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}/orders/preview`, { method: "POST", body: JSON.stringify(request) }); }
  public submitConnectorOrder(id: string, request: ConnectorOrderRequest): Promise<ConnectorSubmission> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}/orders`, { method: "POST", body: JSON.stringify(request) }); }
  public listConnectorOrders(): Promise<readonly ConnectorOrder[]> { return this.#request("/api/v1/connector-orders"); }
  public listReconciliations(): Promise<readonly ReconciliationRun[]> { return this.#request("/api/v1/reconciliations"); }
  public getConnectorAccounting(id: string): Promise<ConnectorAccounting> { return this.#request(`/api/v1/venue-accounts/${encodeURIComponent(id)}/accounting`); }
}

function randomIdempotencyKey(): string {
  const cryptoRef = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (cryptoRef?.randomUUID !== undefined) return cryptoRef.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}
