import { describe, it, expect } from "vitest";
import { barClass, pctText, fillWidth } from "./usage";

describe("usage helpers", () => {
  it("colors bars by threshold", () => {
    expect(barClass(0.5)).toContain("green");
    expect(barClass(0.75)).toContain("orange");
    expect(barClass(0.92)).toContain("red");
    expect(barClass(null)).toContain("unknown");
  });

  it("formats percentage text", () => {
    expect(pctText(0.426)).toBe("43%");
    expect(pctText(null)).toBe("–");
  });

  it("clamps fill width to 100%", () => {
    expect(fillWidth({ label: "5h", utilization: 0.4, resetText: "" })).toBe("40%");
    expect(fillWidth({ label: "7d", utilization: 1.5, resetText: "" })).toBe("100%");
    expect(fillWidth({ label: "5h", utilization: null, resetText: "" })).toBe("0%");
  });
});
