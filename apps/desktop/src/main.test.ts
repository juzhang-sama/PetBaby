import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { DEFAULT_PET_CALIBRATION, type PetCalibrationV1 } from "./runtime/pet-calibration";
import * as petStageModule from "./runtime/pet-stage";

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

    expect(source).toContain('invoke<RuntimePetDescriptor>("pet_prepare_startup", { petId })');
    expect(source).toContain('finalizeStartupRecovery(activePetId, petId, {');
    expect(source).not.toContain('invoke("pet_commit_switch", { petId })');
    expect(source).toContain('prepare: (requestId, petId) => invoke("pet_prepare_switch", { requestId, petId })');
    expect(source).toContain('commit: (request) => invoke("pet_commit_switch", { ...request })');
    expect(source).toContain('rollbackCommit: (previousPetId, request) => invoke("pet_rollback_switch", {');
    expect(source).toContain('reconcileCommit: (previousPetId, request) => invoke("pet_reconcile_switch_commit", {');
    expect(source).toContain('abortCreation: (sessionId, error) => invoke("creation_abort_finalize", { sessionId, error })');
    expect(source).toContain('cancel: (requestId) => invoke("pet_cancel_switch", { requestId })');
    expect(source).toContain('finish: (requestId) => invoke("pet_finish_switch", { requestId })');
  });

  it("executes calibration startup, switch, and teardown through one lifecycle", async () => {
    const wire = petStageModule.wirePetCalibrationRuntime as unknown as ((options: Record<string, unknown>) => Promise<{
      afterPetSwitch(result: { ok: boolean; petId: string }): Promise<void>;
      destroy(): void;
    }>) | undefined;
    expect(wire).toEqual(expect.any(Function));
    if (!wire) return;
    let activePetId = "pet-a";
    let receive!: (value: unknown) => void;
    const unlisten = vi.fn();
    const load = vi.fn(async (petId: string): Promise<PetCalibrationV1> => ({
      ...DEFAULT_PET_CALIBRATION,
      feedbackStrength: petId === "pet-b" ? 0.8 : 0.6,
    }));
    const setCalibration = vi.fn();
    const wiring = await wire({
      activePetId: () => activePetId,
      load,
      setCalibration,
      previewFeedback: vi.fn(),
      listen: async (handler: (value: unknown) => void) => { receive = handler; return unlisten; },
      emit: vi.fn(async () => undefined),
    });
    expect(load).toHaveBeenCalledWith("pet-a");

    activePetId = "pet-b";
    await wiring.afterPetSwitch({ ok: true, petId: "pet-b" });
    await wiring.afterPetSwitch({ ok: false, petId: "pet-c" });
    expect(load.mock.calls.map(([petId]) => petId)).toEqual(["pet-a", "pet-b"]);

    wiring.destroy();
    wiring.destroy();
    receive({ requestId: "late", petId: "pet-b", action: "preview", value: DEFAULT_PET_CALIBRATION });
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledOnce();
    expect(setCalibration).toHaveBeenCalledTimes(4);
  });

  it("anchors the lifecycle to the real Tauri preview listener and window teardown", () => {
    const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    expect(source).toMatch(/listen<unknown>\(\s*PET_CALIBRATION_PREVIEW_REQUEST,/);
    expect(source).toMatch(/addEventListener\(\s*"beforeunload",\s*calibrationWiring\.destroy/);
  });

  it("routes fullscreen facts and runtime transitions only through the controller protocol", () => {
    const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    expect(source).not.toContain("hiddenForFullscreen");
    expect(source).toContain("wireFullscreenProbeLoop");
    expect(source).toContain("probe: probeFullscreen");
    expect(source).toContain("update: updateWindowFullscreen");
    expect(source).toContain('addEventListener("beforeunload", fullscreenWiring.destroy');
    expect(source).not.toContain("petWindow.hide");
    expect(source).not.toContain("petWindow.show");
    expect(source).toContain("wireWindowModeRuntime");
    expect(source).toMatch(/ack:\s*\(requestId,\s*cycle,\s*phase\)\s*=>\s*invoke<boolean>\(\s*"window_mode_runtime_ack",\s*\{\s*requestId,\s*cycle,\s*phase,?\s*\}/);
    expect(source).toContain("resume: (effectiveVisible) => stage.resumeWindowModeTransition(effectiveVisible)");
    expect(source).toContain("abort: () => stage.abortWindowModeTransition()");
    expect(source).toContain('addEventListener("beforeunload", windowModeWiring.destroy');
  });
});
