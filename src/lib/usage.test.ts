import { describe, it, expect } from "vitest";
import { barClass, pctText, fillWidth } from "./usage";

describe("usage helpers", () => {
  it("colors bars by threshold, with a per-window identity hue while safe", () => {
    expect(barClass(0.5)).toBe("indigo"); // 5-hour window, safe
    expect(barClass(0.5, true)).toBe("teal"); // weekly window, safe
    expect(barClass(0.75)).toBe("orange");
    expect(barClass(0.75, true)).toBe("orange"); // warning tier ignores window
    expect(barClass(0.92)).toBe("red");
    expect(barClass(null)).toBe("unknown");
  });

  it("formats percentage text", () => {
    expect(pctText(0.426)).toBe("43%");
    expect(pctText(null)).toBe("–");
  });

  it("clamps fill width to 100%", () => {
    expect(fillWidth({ label: "5h", utilization: 0.4, resetText: "", weekly: false })).toBe("40%");
    expect(fillWidth({ label: "7d", utilization: 1.5, resetText: "", weekly: true })).toBe("100%");
    expect(fillWidth({ label: "5h", utilization: null, resetText: "", weekly: false })).toBe("0%");
  });
});
