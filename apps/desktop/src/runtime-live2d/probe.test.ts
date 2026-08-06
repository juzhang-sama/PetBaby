import { describe, expect, it } from "vitest";
import { evaluateProbe } from "./probe";

describe("Live2D probe evaluation", () => {
  it("rejects a blank frame", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 0, contextLost: false }),
    ).toEqual({ ok: false, reason: "blank-frame" });
  });

  it("accepts a rendered frame", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 1200, contextLost: false }),
    ).toEqual({ ok: true });
  });

  it("reports unavailable WebGL before frame state", () => {
    expect(
      evaluateProbe({ webgl: false, nonTransparentPixels: 1200, contextLost: false }),
    ).toEqual({ ok: false, reason: "webgl-unavailable" });
  });

  it("reports a lost context before frame state", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 1200, contextLost: true }),
    ).toEqual({ ok: false, reason: "context-lost" });
  });
});
