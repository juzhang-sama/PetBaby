import { describe, expect, it, vi } from "vitest";
import type { PetRenderAsset } from "../runtime/pet-renderer";
import { Live2DRenderer } from "./live2d-renderer";
import type { LoadedCubismModel } from "./cubism-model-loader";
import { MicroMotionController } from "./micro-motion";
import { DEFAULT_PET_CALIBRATION } from "../runtime/pet-calibration";
import type { RuntimeAssetManifestV4 } from "../runtime-assets/cat-character-manifest";
import { motionSpatialProfileForTest } from "../runtime-assets/cat-motion-spatial-profile-test-fixtures";

function liveAsset(id = "a"): Extract<PetRenderAsset, { kind: "live2d" }> {
  return {
    kind: "live2d",
    modelUrl: `blob:model-${id}`,
    previewUrl: `blob:preview-${id}`,
    semantics: {
      motions: { idle: { group: "Idle", index: 0 }, carried: { group: "Carry", index: 0 } },
      expressions: { happy: "Happy" },
      hitAreas: { head: "Head" },
      parameters: {
        eyeOpen: "Eye",
        eyeBallX: "EyeBallX",
        eyeBallY: "EyeBallY",
        angleX: "AngleX",
        angleY: "AngleY",
        bodyBreath: "Breath",
        bodySway: "Sway",
        mouthOpen: "Mouth",
      },
    },
    dispose: vi.fn(),
  };
}

function catAsset(): Extract<PetRenderAsset, { kind: "live2d" }> {
  const motions = Object.fromEntries([
    "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
    "sleepy-yawn", "half-stand-stretch",
  ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV4["motions"];
  return {
    kind: "live2d",
    catV4: true,
    modelUrl: "blob:cat",
    previewUrl: "blob:cat-preview",
    semantics: {
      motions,
      expressions: {},
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      parameters: {
        eyeOpenLeft: "EyeL",
        eyeOpenRight: "EyeR",
        eyeBallX: "EyeBallX",
        eyeBallY: "EyeBallY",
        earLeft: "EarL",
        earRight: "EarR",
        tailAngle: "TailAngle",
        tailCurl: "TailCurl",
        tailTip: "TailTip",
        bodyBreath: "Breath",
      },
    },
    dispose: vi.fn(),
  };
}

function spatialCatAsset(): Extract<PetRenderAsset, { kind: "live2d" }> {
  return {
    ...catAsset(),
    motionSpatialProfile: motionSpatialProfileForTest("body-slender-v1", 0.65),
  };
}

function fakeModel(): LoadedCubismModel {
  return {
    resize: vi.fn(),
    update: vi.fn(),
    draw: vi.fn(),
    release: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    stopAllMotions: vi.fn(),
    setExpression: vi.fn(),
    setParameter: vi.fn(),
    getParameterRange: vi.fn((parameterId: string) => parameterId === "Sway"
      ? { min: -10, max: 10 }
      : { min: 0, max: 1 }),
    hitTest: vi.fn(() => true),
  };
}

function fakeCanvas() {
  const listeners = new Map<string, EventListener>();
  return {
    width: 0,
    height: 0,
    style: {},
    addEventListener: vi.fn((name: string, listener: EventListener) => listeners.set(name, listener)),
    removeEventListener: vi.fn((name: string) => listeners.delete(name)),
    emit(name: string, event: Event) { listeners.get(name)?.(event); },
  } as unknown as HTMLCanvasElement & { emit(name: string, event: Event): void };
}

describe("Live2DRenderer", () => {
  it("rejects static assets", async () => {
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn() } });
    await expect(renderer.load({ kind: "static-png", imageUrl: "pet.png" })).rejects.toThrow(/live2d/i);
  });

  it("releases the old model and asset before replacing them", async () => {
    const firstModel = fakeModel();
    const secondModel = fakeModel();
    const loader = { load: vi.fn().mockResolvedValueOnce(firstModel).mockResolvedValueOnce(secondModel) };
    const first = liveAsset("first");
    const second = liveAsset("second");
    const renderer = new Live2DRenderer(fakeCanvas(), { loader });

    await renderer.load(first);
    await renderer.load(second);

    expect(firstModel.release).toHaveBeenCalledOnce();
    expect(first.dispose).toHaveBeenCalledOnce();
    expect(renderer.state().status).toBe("ready");
  });

  it("does not let an older asynchronous load replace the current model", async () => {
    let resolveFirst!: (model: LoadedCubismModel) => void;
    const firstPending = new Promise<LoadedCubismModel>((resolve) => { resolveFirst = resolve; });
    const staleModel = fakeModel();
    const currentModel = fakeModel();
    const loader = { load: vi.fn().mockReturnValueOnce(firstPending).mockResolvedValueOnce(currentModel) };
    const renderer = new Live2DRenderer(fakeCanvas(), { loader });

    const firstLoad = renderer.load(liveAsset("first"));
    await renderer.load(liveAsset("second"));
    resolveFirst(staleModel);
    await firstLoad;

    expect(staleModel.release).toHaveBeenCalledOnce();
    renderer.setVisibility(true);
    renderer.update(16);
    expect(currentModel.update).toHaveBeenCalledOnce();
  });

  it("pauses on context loss and reloads at most once after restoration", async () => {
    const canvas = fakeCanvas();
    const firstModel = fakeModel();
    const restoredModel = fakeModel();
    const loader = { load: vi.fn().mockResolvedValueOnce(firstModel).mockResolvedValueOnce(restoredModel) };
    const renderer = new Live2DRenderer(canvas, { loader });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);

    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    renderer.update(16);
    expect(firstModel.update).not.toHaveBeenCalled();

    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));
    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));
    await vi.waitFor(() => expect(loader.load).toHaveBeenCalledTimes(2));
    expect(renderer.state().status).toBe("ready");
  });

  it("restores the interrupted cat action and ignores its stale completion after context loss", async () => {
    const canvas = fakeCanvas();
    const firstModel = fakeModel();
    const restoredModel = fakeModel();
    let staleFinish: (() => void) | undefined;
    vi.mocked(firstModel.playMotion).mockImplementation((_group, _index, _options, onFinished) => {
      staleFinish = onFinished;
      return { cancel: vi.fn() };
    });
    const loader = { load: vi.fn().mockResolvedValueOnce(firstModel).mockResolvedValueOnce(restoredModel) };
    const renderer = new Live2DRenderer(canvas, { loader });
    await renderer.load(catAsset());
    renderer.playCatMotion("pet-happy", { priority: 60 });

    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    staleFinish?.();
    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));

    await vi.waitFor(() => expect(loader.load).toHaveBeenCalledTimes(2));
    expect(restoredModel.playMotion).toHaveBeenCalledWith(
      "pet-happy",
      0,
      { priority: 60, loop: false },
      expect.any(Function),
    );
  });

  it("does not let a pending load become ready after context loss", async () => {
    let resolveLoad!: (model: LoadedCubismModel) => void;
    const pending = new Promise<LoadedCubismModel>((resolve) => { resolveLoad = resolve; });
    const canvas = fakeCanvas();
    const staleModel = fakeModel();
    const renderer = new Live2DRenderer(canvas, { loader: { load: vi.fn(() => pending) } });

    const load = renderer.load(liveAsset());
    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    resolveLoad(staleModel);
    await load;

    expect(staleModel.release).toHaveBeenCalledOnce();
    expect(renderer.state().status).toBe("context-lost");
  });

  it("does not let a missing high-priority motion block a mapped motion", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());

    renderer.playMotion("landed", { priority: 80 });
    renderer.playMotion("idle", { priority: 10, loop: true });

    expect(model.playMotion).toHaveBeenCalledOnce();
    expect(model.playMotion).toHaveBeenCalledWith("Idle", 0, { priority: 10, loop: true }, expect.any(Function));
  });

  it("resumes the background state loop after a one-shot motion finishes", async () => {
    const model = fakeModel();
    let finishCurrent: (() => void) | undefined;
    vi.mocked(model.playMotion).mockImplementation((_group, _index, _options, onFinished) => {
      finishCurrent = onFinished;
      return { cancel: vi.fn() };
    });
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());

    renderer.playMotion("idle", { priority: 10, loop: true });
    renderer.playMotion("carried", { priority: 60 });
    finishCurrent?.();

    expect(model.playMotion).toHaveBeenLastCalledWith(
      "Idle",
      0,
      { priority: 10, loop: true },
      expect.any(Function),
    );
  });

  it("queues semantic parameter writes before the SDK update and draw", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);
    renderer.setLipSync(0.6);

    renderer.update(16);

    expect(model.setParameter).toHaveBeenCalledWith("Mouth", 0.6);
    expect(vi.mocked(model.setParameter).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(model.update).mock.invocationCallOrder[0]!);
    expect(vi.mocked(model.update).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(model.draw).mock.invocationCallOrder[0]!);
  });

  it("plays the stable cat motion set and writes independent cat automation controls", async () => {
    const model = fakeModel();
    vi.mocked(model.getParameterRange).mockReturnValue({ min: -30, max: 30 });
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(catAsset());
    renderer.setVisibility(true);

    renderer.playCatMotion("tail-idle", { priority: 20, loop: true });
    renderer.setCatAutomation({
      breath: 0.6,
      eyeLeftOpen: 0.1,
      eyeRightOpen: 0.9,
      earLeft: -0.3,
      earRight: 0.4,
      tailAngle: 12,
      tailCurl: -0.5,
      tailTip: 0.7,
    });
    renderer.update(16);

    expect(model.playMotion).toHaveBeenCalledWith(
      "tail-idle", 0, { priority: 20, loop: true }, expect.any(Function),
    );
    expect(model.setParameter).toHaveBeenCalledWith("EyeL", 0.1);
    expect(model.setParameter).toHaveBeenCalledWith("EyeR", 0.9);
    expect(model.setParameter).toHaveBeenCalledWith("EarL", -0.3);
    expect(model.setParameter).toHaveBeenCalledWith("EarR", 0.4);
    expect(model.setParameter).toHaveBeenCalledWith("TailAngle", 12);
    expect(model.setParameter).toHaveBeenCalledWith("TailCurl", -0.5);
    expect(model.setParameter).toHaveBeenCalledWith("TailTip", 0.7);
  });

  it("applies the v5 character amplitude before writing automation parameters", async () => {
    const model = fakeModel();
    vi.mocked(model.getParameterRange).mockReturnValue({ min: -30, max: 30 });
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(spatialCatAsset());
    renderer.setVisibility(true);
    renderer.setCatAutomation({
      breath: 1,
      eyeLeftOpen: 1,
      eyeRightOpen: 1,
      earLeft: 0,
      earRight: 0,
      tailAngle: 0,
      tailCurl: 0,
      tailTip: 0,
    });

    renderer.update(16);

    expect(model.setParameter).toHaveBeenCalledWith("Breath", 0.65);
  });

  it("diagnoses incompatible model and character ranges without writing the parameter", async () => {
    const model = fakeModel();
    vi.mocked(model.getParameterRange).mockImplementation((parameterId: string) => parameterId === "Breath"
      ? { min: 2, max: 3 }
      : { min: -30, max: 30 });
    const diagnose = vi.fn();
    const renderer = new Live2DRenderer(fakeCanvas(), {
      loader: { load: vi.fn(async () => model) },
      diagnose,
    });
    await renderer.load(spatialCatAsset());
    renderer.setVisibility(true);
    renderer.setCatAutomation({
      breath: 1,
      eyeLeftOpen: 1,
      eyeRightOpen: 1,
      earLeft: 0,
      earRight: 0,
      tailAngle: 0,
      tailCurl: 0,
      tailTip: 0,
    });

    renderer.update(16);

    expect(model.setParameter).not.toHaveBeenCalledWith("Breath", expect.any(Number));
    expect(diagnose).toHaveBeenCalledWith(
      "Live2D model/profile parameter ranges are incompatible: bodyBreath",
    );
    expect(diagnose).not.toHaveBeenCalledWith("Live2D parameter mapping is missing: bodyBreath");
  });

  it("drives a verified blink overlay with independent eye-open values", async () => {
    const model = fakeModel();
    const asset = catAsset();
    asset.blinkOverlayUrl = "blob:blink-overlay";
    const blinkOverlay = {
      setEyesOpen: vi.fn(),
      setVisible: vi.fn(),
      destroy: vi.fn(),
    };
    const createCatBlinkOverlay = vi.fn(() => blinkOverlay);
    const renderer = new Live2DRenderer(fakeCanvas(), {
      loader: { load: vi.fn(async () => model) },
      createCatBlinkOverlay,
    });

    await renderer.load(asset);
    renderer.setVisibility(true);
    renderer.setCatAutomation({
      breath: 0.5,
      eyeLeftOpen: 0.1,
      eyeRightOpen: 0.8,
      earLeft: 0,
      earRight: 0,
      tailAngle: 0,
      tailCurl: 0,
      tailTip: 0,
    });
    renderer.update(16);

    expect(createCatBlinkOverlay).toHaveBeenCalledWith(expect.anything(), "blob:blink-overlay");
    expect(blinkOverlay.setEyesOpen).toHaveBeenLastCalledWith(0.1, 0.8);
    renderer.setVisibility(false);
    expect(blinkOverlay.setVisible).toHaveBeenLastCalledWith(false);
    renderer.destroy();
    expect(blinkOverlay.destroy).toHaveBeenCalledOnce();
  });

  it("forwards scheduler fade timing to the Cubism motion", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(catAsset());

    renderer.playCatMotion("pet-happy", {
      priority: 90,
      loop: false,
      fadeInMs: 180,
      fadeOutMs: 140,
    });

    expect(model.playMotion).toHaveBeenCalledWith(
      "pet-happy",
      0,
      { priority: 90, loop: false, fadeInMs: 180, fadeOutMs: 140 },
      expect.any(Function),
    );
  });

  it("keeps a v4 cat at its authored neutral pose until cat automation is supplied", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(catAsset());
    renderer.setVisibility(true);

    renderer.update(1_000);

    expect(model.setParameter).toHaveBeenCalledWith("EyeBallX", 0);
    expect(model.setParameter).toHaveBeenCalledWith("EyeBallY", 0);
    expect(model.setParameter).not.toHaveBeenCalledWith("Breath", expect.any(Number));
    expect(model.setParameter).not.toHaveBeenCalledWith("TailAngle", expect.any(Number));
  });

  it("advances internal v4 automation only after the stage selects an active mode", async () => {
    const model = fakeModel();
    vi.mocked(model.getParameterRange).mockReturnValue({ min: -30, max: 30 });
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(catAsset());
    renderer.setVisibility(true);

    renderer.update(16);
    expect(model.setParameter).not.toHaveBeenCalledWith("EyeL", expect.any(Number));

    renderer.setCatAutomationMode("idle");
    renderer.update(16);
    expect(model.setParameter).toHaveBeenCalledWith("EyeL", expect.any(Number));
    expect(model.setParameter).toHaveBeenCalledWith("EyeR", expect.any(Number));
    expect(model.setParameter).toHaveBeenCalledWith("TailAngle", expect.any(Number));
  });

  it("maps a v4 pointer target only to eye, ear and tail parameters", async () => {
    const model = fakeModel();
    vi.mocked(model.getParameterRange).mockReturnValue({ min: -30, max: 30 });
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(catAsset());
    renderer.setVisibility(true);
    renderer.setCatAutomationMode("pointerFocus");

    renderer.setLookTarget({ x: 0.5, y: -0.25 });
    renderer.update(16);

    expect(model.setParameter).toHaveBeenCalledWith("EyeBallX", 0.5);
    expect(model.setParameter).toHaveBeenCalledWith("EyeBallY", -0.25);
    expect(model.setParameter).toHaveBeenCalledWith("EarL", expect.any(Number));
    expect(model.setParameter).toHaveBeenCalledWith("EarR", expect.any(Number));
    expect(model.setParameter).toHaveBeenCalledWith("TailAngle", expect.any(Number));
    expect(model.setParameter).toHaveBeenCalledWith("Breath", expect.any(Number));
  });

  it("maps calibration into breathing without automating the frozen eye-open parameter", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setCalibration({ ...DEFAULT_PET_CALIBRATION, breathAmplitudePercent: 4 });
    renderer.setVisibility(true);

    for (let index = 0; index < 10; index += 1) renderer.update(100);

    expect(model.setParameter).toHaveBeenLastCalledWith("Sway", expect.any(Number));
    expect(model.setParameter).not.toHaveBeenCalledWith("Eye", expect.any(Number));
    const breathWrites = vi.mocked(model.setParameter).mock.calls
      .filter(([parameterId]) => parameterId === "Breath");
    expect(breathWrites.at(-1)?.[1]).toBeCloseTo(0.54);
  });

  it("does not diagnose or simulate a missing eye semantic while blink is frozen", async () => {
    const model = fakeModel();
    const asset = liveAsset();
    delete asset.semantics.parameters.eyeOpen;
    const canvas = fakeCanvas();
    const diagnose = vi.fn();
    const renderer = new Live2DRenderer(canvas, {
      loader: { load: vi.fn(async () => model) },
      diagnose,
    });
    await renderer.load(asset);
    renderer.setVisibility(true);

    renderer.update(16);
    renderer.update(16);

    expect(diagnose).not.toHaveBeenCalledWith("Live2D parameter mapping is missing: eyeOpen");
    expect(canvas.style.transform).toBeUndefined();
  });

  it("does not force the mouth closed before lip sync is enabled", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);

    renderer.update(16);

    expect(model.setParameter).not.toHaveBeenCalledWith("Mouth", expect.any(Number));
  });

  it("automates only chest breathing and whole-body sway", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);
    renderer.setLookTarget({ x: 1, y: -1 });

    renderer.update(100);

    const parameterIds = vi.mocked(model.setParameter).mock.calls.map(([parameterId]) => parameterId);
    expect(parameterIds).toContain("Breath");
    expect(parameterIds).toContain("Sway");
    expect(parameterIds).not.toContain("AngleX");
    expect(parameterIds).not.toContain("AngleY");
    expect(parameterIds).not.toContain("EyeBallX");
    expect(parameterIds).not.toContain("EyeBallY");
    expect(parameterIds).not.toContain("Eye");
    expect(parameterIds).not.toContain("Mouth");
  });

  it("suppresses sway while carried and restores it for landed state without a mapped motion", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);

    renderer.playMotion("carried");
    renderer.update(100);
    expect(model.setParameter).toHaveBeenLastCalledWith("Sway", 0);

    vi.mocked(model.setParameter).mockClear();
    renderer.playMotion("landed");
    renderer.update(100);
    expect(vi.mocked(model.setParameter).mock.calls.find(([parameterId]) => parameterId === "Sway")?.[1]).not.toBe(0);
  });

  it("does not advance micro-motion while hidden", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    const control = new MicroMotionController();
    await renderer.load(liveAsset());
    renderer.setVisibility(true);
    renderer.update(100);
    control.update(100);

    renderer.setVisibility(false);
    renderer.update(60_000);
    renderer.setVisibility(true);
    renderer.update(100);
    const expected = control.update(100);

    const breathWrites = vi.mocked(model.setParameter).mock.calls
      .filter(([parameterId]) => parameterId === "Breath");
    expect(breathWrites).toHaveLength(2);
    expect(breathWrites[1]?.[1]).toBeCloseTo(0.5 + expected.breath * 0.02);
  });

  it("keeps a missing eye parameter dormant across context re-attach", async () => {
    const canvas = fakeCanvas();
    const firstModel = fakeModel();
    const restoredModel = fakeModel();
    const asset = liveAsset();
    delete asset.semantics.parameters.eyeOpen;
    const diagnose = vi.fn();
    const loader = {
      load: vi.fn()
        .mockResolvedValueOnce(firstModel)
        .mockResolvedValueOnce(restoredModel),
    };
    const renderer = new Live2DRenderer(canvas, { loader, diagnose });
    await renderer.load(asset);
    renderer.setVisibility(true);
    renderer.update(16);

    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));
    await vi.waitFor(() => expect(renderer.state().status).toBe("ready"));
    renderer.update(16);

    expect(diagnose).not.toHaveBeenCalledWith("Live2D parameter mapping is missing: eyeOpen");
  });

  it("destroys model, asset and listeners idempotently", async () => {
    const canvas = fakeCanvas();
    const model = fakeModel();
    const asset = liveAsset();
    const renderer = new Live2DRenderer(canvas, { loader: { load: vi.fn(async () => model) } });
    await renderer.load(asset);

    renderer.destroy();
    renderer.destroy();

    expect(model.release).toHaveBeenCalledOnce();
    expect(asset.dispose).toHaveBeenCalledOnce();
    expect(canvas.removeEventListener).toHaveBeenCalledTimes(2);
    expect(renderer.state()).toMatchObject({ status: "destroyed", visible: false });
  });
});
