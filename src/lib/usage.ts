// Pure presentation helpers for usage bars, mirrored from the spec's color
// thresholds. Kept out of the component so they're unit-testable.

import type { UsageBarDTO } from "./types";

/**
 * Meter color modifier. When near the cap the warning tier wins (orange 70–90%,
 * red ≥ 90%); while safe each window shows its own identity hue so the two
 * limits read apart — teal for the weekly/monthly window, indigo for 5-hour.
 * Unknown → grey.
 */
export function barClass(util: number | null, weekly = false): string {
  if (util === null) return "unknown";
  const pct = util * 100;
  if (pct >= 90) return "red";
  if (pct >= 70) return "orange";
  return weekly ? "teal" : "indigo";
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
