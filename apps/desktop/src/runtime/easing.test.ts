import { describe, expect, it } from "vitest";
import { easeByName, easeInOutQuad, easeOutCubic, linear } from "./easing";

describe("easing", () => {
  it("linear maps t to t", () => {
    expect(linear(0)).toBe(0);
    expect(linear(0.5)).toBe(0.5);
    expect(linear(1)).toBe(1);
  });

  it("easeOutCubic starts at 0 and ends at 1", () => {
    expect(easeOutCubic(0)).toBe(0);
    expect(easeOutCubic(1)).toBe(1);
    expect(easeOutCubic(0.5)).toBeCloseTo(0.875, 3);
  });

  it("easeInOutQuad is symmetric", () => {
    expect(easeInOutQuad(0)).toBe(0);
    expect(easeInOutQuad(1)).toBe(1);
    expect(easeInOutQuad(0.5)).toBeCloseTo(0.5, 3);
    expect(easeInOutQuad(0.25)).toBeCloseTo(0.125, 3);
  });

  it("resolves easing by name", () => {
    expect(easeByName("linear")).toBe(linear);
    expect(easeByName("easeOutCubic")).toBe(easeOutCubic);
    expect(easeByName("easeInOutQuad")).toBe(easeInOutQuad);
  });

  it("falls back to linear for unknown names", () => {
    expect(easeByName("nope" as never)).toBe(linear);
  });
});
