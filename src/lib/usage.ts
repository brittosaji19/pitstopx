// Pure presentation helpers for usage bars, mirrored from the spec's color
// thresholds. Kept out of the component so they're unit-testable.

import type { UsageBarDTO } from "./types";

/** Meter color modifier: green < 70%, orange 70–90%, red ≥ 90%, grey unknown. */
export function barClass(util: number | null): string {
  if (util === null) return "unknown";
  const pct = util * 100;
  if (pct >= 90) return "red";
  if (pct >= 70) return "orange";
  return "green";
}

/** Right-aligned percentage text, `–` when unknown. */
export function pctText(util: number | null): string {
  return util === null ? "–" : `${Math.round(util * 100)}%`;
}

/** Clamped fill width as a CSS percentage string. */
export function fillWidth(bar: UsageBarDTO): string {
  const pct = bar.utilization === null ? 0 : Math.min(100, bar.utilization * 100);
  return `${pct}%`;
}
