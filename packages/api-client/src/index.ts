export const schemaVersion = "0.1.0" as const;

export type HealthStatus = "OK" | "DEGRADED" | "BLOCKED";

export interface Health {
  readonly status: "OK";
  readonly mode: "PAPER";
  readonly stage: "M0_SCAFFOLD";
}

export interface EntryReadiness {
  readonly status: "BLOCKED";
  readonly reasons: readonly ["M0_SCAFFOLD_NO_TRADING_CAPABILITY"];
}

export interface SignexClientOptions {
  readonly baseUrl: string;
  readonly fetchImplementation?: typeof fetch;
}

export class SignexClient {
  readonly #baseUrl: string;
  readonly #fetch: typeof fetch;

  public constructor(options: SignexClientOptions) {
    this.#baseUrl = options.baseUrl.replace(/\/$/u, "");
    this.#fetch = options.fetchImplementation ?? fetch;
  }

  public async health(signal?: AbortSignal): Promise<Health> {
    const url = `${this.#baseUrl}/readyz`;
    const response = signal === undefined
      ? await this.#fetch(url)
      : await this.#fetch(url, { signal });
    if (!response.ok) {
      throw new Error(`Signex readiness failed with HTTP ${response.status}`);
    }
    return (await response.json()) as Health;
  }
}
