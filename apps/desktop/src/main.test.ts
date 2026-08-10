import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("desktop hit-region routing", () => {
  it("reads hit pixels from the slot hit surface while display mounting remains separate", () => {
    const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    const refreshHitRegion = source.slice(
      source.indexOf("const refreshHitRegion"),
      source.indexOf("const diagnose"),
    );

    expect(refreshHitRegion).toContain("const surface = slot.getHitSurface();");
    expect(refreshHitRegion).not.toContain("slot.getSurface()");
    expect(source).toContain("new PetRuntimeSlot(rendererRoot, initialRuntime)");
  });
});
