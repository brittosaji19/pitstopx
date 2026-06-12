// Shared DTOs — mirror the Rust `serde` structs in `ui_events.rs` (camelCase).

export interface UiSnapshot {
  activeEmail: string | null;
  lastRefresh: string | null; // ISO; formatted client-side
  lastTopLevelError: string | null;
  rows: AccountRowDTO[]; // pre-sorted: active first, then emptiest
}

export interface AccountRowDTO {
  email: string;
  providerLabel: string; // "Anthropic"
  providerId: string; // "anthropic" — stable id for styling/future providers
  providerAccent: string; // hex accent color for the provider badge
  planLabel: string; // "Acme AI · Team · 5x"
  isActive: boolean;
  bars: UsageBarDTO[];
  modelsLine: string | null; // "Opus wk 12% · Sonnet wk 10% · Extra 4%"
  statusLine: string | null; // error / stale / loading
  switchable: boolean;
}

export interface UsageBarDTO {
  label: "5h" | "7d";
  utilization: number | null; // 0..1
  resetText: string; // "9:49 PM · 3h 34m"
}
