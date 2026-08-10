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

  it("forwards request ownership through every backend switch port", () => {
    const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");

    expect(source).toContain('prepare: (requestId, petId) => invoke("pet_prepare_switch", { requestId, petId })');
    expect(source).toContain('commit: (request) => invoke("pet_commit_switch", { ...request })');
    expect(source).toContain('rollbackCommit: (previousPetId, request) => invoke("pet_rollback_switch", {');
    expect(source).toContain('cancel: (requestId) => invoke("pet_cancel_switch", { requestId })');
    expect(source).toContain('finish: (requestId) => invoke("pet_finish_switch", { requestId })');
  });
});
