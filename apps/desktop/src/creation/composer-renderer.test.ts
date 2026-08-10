import { describe, expect, it, vi } from "vitest";
import type { ComposerRecipe } from "./contracts";
import { parseComposerPack } from "./composer-pack";
import type { ComposerPackManifest } from "./composer-pack";
import {
  exportComposerPng,
  motionProfileForRecipe,
  planComposerRecipe,
  renderComposerRecipe,
  type ComposerRenderPorts,
} from "./composer-renderer";

type FakeImage = { label: string };

class FakeCanvas {
  width = 1024;
  height = 1024;
  readonly calls: string[] = [];
  pixels = "transparent";

  constructor(readonly name: string) {}
}

class FakeContext {
  private operation = "source-over";
  private alpha = 1;
  private style: string | CanvasGradient | CanvasPattern = "#000000";

  constructor(
    readonly canvas: FakeCanvas,
    private readonly failures: {
      throwOnDraw?: string;
      failTargetDraw?: boolean;
      failTargetComposite?: boolean;
      failTargetRestore?: boolean;
    },
  ) {}

  get globalCompositeOperation(): GlobalCompositeOperation { return this.operation as GlobalCompositeOperation; }
  set globalCompositeOperation(value: GlobalCompositeOperation) {
    if (this.canvas.name === "target" && value === "copy" && this.failures.failTargetComposite) {
      throw new Error("target composite failed");
    }
    this.operation = value;
    this.canvas.calls.push(`gco:${value}`);
  }

  get globalAlpha(): number { return this.alpha; }
  set globalAlpha(value: number) {
    this.alpha = value;
    this.canvas.calls.push(`alpha:${value}`);
  }

  get fillStyle(): string | CanvasGradient | CanvasPattern { return this.style; }
  set fillStyle(value: string | CanvasGradient | CanvasPattern) {
    this.style = value;
    this.canvas.calls.push(`fillStyle:${String(value)}`);
  }

  save(): void { this.canvas.calls.push("save"); }
  restore(): void {
    this.canvas.calls.push("restore");
    if (this.canvas.name === "target" && this.failures.failTargetRestore) {
      throw new Error("target restore failed");
    }
  }
  clearRect(x: number, y: number, width: number, height: number): void {
    this.canvas.calls.push(`clear:${x},${y},${width},${height}`);
    this.canvas.pixels = "transparent";
  }
  fillRect(x: number, y: number, width: number, height: number): void {
    this.canvas.calls.push(`fill:${x},${y},${width},${height}`);
  }
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void {
    this.canvas.calls.push(`transform:${a},${b},${c},${d},${e},${f}`);
  }
  drawImage(source: CanvasImageSource, ..._args: number[]): void {
    const label = source instanceof FakeCanvas
      ? `surface:${source.name}`
      : (source as unknown as FakeImage).label;
    if (
      label.includes(this.failures.throwOnDraw ?? "\0")
      || (this.canvas.name === "target" && this.failures.failTargetDraw)
    ) throw new Error(`draw failed: ${label}`);
    this.canvas.calls.push(`draw:${label}`);
    this.canvas.pixels = `${this.operation}:${label}`;
  }
}

interface FakePorts extends ComposerRenderPorts {
  surfaces: FakeCanvas[];
  loaded: string[];
  target: FakeCanvas;
}

function fakePorts(options: {
  failLoad?: string;
  failContextAt?: number;
  throwOnDraw?: string;
  failTargetDraw?: boolean;
  failTargetComposite?: boolean;
  failTargetRestore?: boolean;
  png?: Blob;
} = {}): FakePorts {
  const surfaces: FakeCanvas[] = [];
  const loaded: string[] = [];
  let contextCount = 0;
  const target = new FakeCanvas("target");
  return {
    surfaces,
    loaded,
    target,
    createSurface(width, height) {
      const surface = new FakeCanvas(`offscreen-${surfaces.length + 1}`);
      surface.width = width;
      surface.height = height;
      surfaces.push(surface);
      return surface as unknown as HTMLCanvasElement;
    },
    context(surface) {
      contextCount += 1;
      if (contextCount === options.failContextAt) throw new Error("context failed");
      return new FakeContext(surface as unknown as FakeCanvas, options) as unknown as CanvasRenderingContext2D;
    },
    async loadImage(url) {
      loaded.push(url);
      if (url === options.failLoad) throw new Error(`load failed: ${url}`);
      return { label: url } as unknown as CanvasImageSource;
    },
    assetUrl(relativePath) { return `asset://${relativePath}`; },
    async toPng(surface) {
      if (options.png) return options.png;
      const trace = (surface as unknown as FakeCanvas).calls.join("|");
      return new Blob([trace], { type: "image/png" });
    },
  };
}

function packFixture(options: {
  shuffled?: boolean;
  faceSafeZone?: { left: number; top: number; right: number; bottom: number };
  breathZone?: { left: number; top: number; right: number; bottom: number };
} = {}): ComposerPackManifest {
  const parts = {
    bodies: [{
      id: "body-round",
      image: "parts/body.png",
      colorMask: "masks/color.png",
      patternMask: "masks/pattern.png",
      compatibleBodyIds: ["body-round"],
      anchor: { x: 512, y: 512 },
      zIndex: 10,
      defaults: {
        earsId: "ears-round",
        eyesId: "eyes-amber",
        muzzleId: "muzzle-gentle",
        tailId: "tail-curl",
        colorId: "color-cream",
        patternId: "pattern-none",
      },
      alphaBounds: { left: 100, top: 50, right: 900, bottom: 1000 },
      faceSafeZone: options.faceSafeZone ?? { left: 300, top: 150, right: 720, bottom: 450 },
      breathZone: options.breathZone ?? { left: 260, top: 500, right: 760, bottom: 900 },
      swayPivot: { x: 512, y: 780 },
    }],
    ears: [{
      id: "ears-round",
      image: "parts/shared.png",
      colorMask: "masks/color.png",
      compatibleBodyIds: ["body-round"],
      anchor: { x: 512, y: 230 },
      zIndex: 10,
    }],
    eyes: [{
      id: "eyes-amber",
      openImage: "parts/eyes-open.png",
      closedImage: "parts/eyes-closed.png",
      compatibleBodyIds: ["body-round"],
      anchor: { x: 512, y: 340 },
      zIndex: 10,
    }],
    muzzles: [{
      id: "muzzle-gentle",
      image: "parts/muzzle.png",
      compatibleBodyIds: ["body-round"],
      anchor: { x: 512, y: 430 },
      zIndex: 10,
    }],
    tails: [{
      id: "tail-curl",
      image: "parts/shared.png",
      patternMask: "masks/pattern.png",
      compatibleBodyIds: ["body-round"],
      anchor: { x: 700, y: 650 },
      zIndex: 10,
    }],
  };
  if (options.shuffled) {
    parts.ears.reverse();
    parts.eyes.reverse();
    parts.muzzles.reverse();
    parts.tails.reverse();
  }
  return parseComposerPack({
    schemaVersion: 1,
    packId: "cat-cute-v1",
    packVersion: 1,
    species: "cat",
    canvas: { width: 1024, height: 1024 },
    layerContractVersion: 1,
    ...parts,
    colors: [{ id: "color-cream", value: "#F4D6A0" }],
    patterns: [
      { id: "pattern-none", image: null },
      { id: "pattern-tabby", image: "patterns/tabby.png" },
    ],
  });
}

function recipe(patternId = "pattern-none"): ComposerRecipe {
  return {
    recipeVersion: 1,
    packId: "cat-cute-v1",
    packVersion: 1,
    layerContractVersion: 1,
    bodyId: "body-round",
    earsId: "ears-round",
    eyesId: "eyes-amber",
    muzzleId: "muzzle-gentle",
    tailId: "tail-curl",
    colorId: "color-cream",
    patternId,
  };
}

function baseAssetFor(surface: FakeCanvas): string | undefined {
  return surface.calls.find((call) => call.startsWith("draw:asset://parts/"));
}

function finalLayerOrder(ports: FakePorts): string[] {
  const byName = new Map(ports.surfaces.map((surface) => [surface.name, surface]));
  const final = ports.surfaces.find((surface) =>
    surface.calls.filter((call) => call.startsWith("draw:surface:")).length === 5,
  );
  if (!final) return [];
  return final.calls
    .filter((call) => call.startsWith("draw:surface:"))
    .map((call) => call.slice("draw:surface:".length))
    .map((name) => baseAssetFor(byName.get(name)!) ?? "missing");
}

describe("deterministic composer rendering", () => {
  it("plans tail body ears open-eyes and muzzle in stable semantic order for equal z", () => {
    const expected = ["tail", "body", "ears", "eyes-open", "muzzle"];
    expect(planComposerRecipe(packFixture(), recipe()).map((layer) => layer.kind)).toEqual(expected);
    expect(planComposerRecipe(packFixture({ shuffled: true }), recipe()).map((layer) => layer.kind)).toEqual(expected);
  });

  it("does not delegate deterministic tie breaking to the host locale", () => {
    const localeCompare = vi.spyOn(String.prototype, "localeCompare")
      .mockImplementation(() => { throw new Error("host locale must not affect the plan"); });
    try {
      expect(planComposerRecipe(packFixture(), recipe()).map((layer) => layer.kind))
        .toEqual(["tail", "body", "ears", "eyes-open", "muzzle"]);
    } finally {
      localeCompare.mockRestore();
    }
  });

  it("renders the exact stable order and commits the final surface only once", async () => {
    const ports = fakePorts();
    await renderComposerRecipe(packFixture(), recipe(), ports.target as unknown as HTMLCanvasElement, ports);

    expect(finalLayerOrder(ports)).toEqual([
      "draw:asset://parts/shared.png",
      "draw:asset://parts/body.png",
      "draw:asset://parts/shared.png",
      "draw:asset://parts/eyes-open.png",
      "draw:asset://parts/muzzle.png",
    ]);
    expect(ports.target.calls.filter((call) => call.startsWith("clear:"))).toEqual([]);
    expect(ports.target.calls.filter((call) => call.startsWith("draw:"))).toHaveLength(1);
  });

  it("loads only open eyes and deduplicates every relative asset path", async () => {
    const ports = fakePorts();
    await renderComposerRecipe(packFixture(), recipe("pattern-tabby"), ports.target as unknown as HTMLCanvasElement, ports);

    expect(ports.loaded).not.toContain("asset://parts/eyes-closed.png");
    expect(ports.loaded.filter((path) => path === "asset://parts/shared.png")).toHaveLength(1);
    expect(ports.loaded.filter((path) => path === "asset://masks/color.png")).toHaveLength(1);
    expect(ports.loaded.filter((path) => path === "asset://masks/pattern.png")).toHaveLength(1);
    expect(new Set(ports.loaded).size).toBe(ports.loaded.length);
  });

  it("uses isolated multiply/source-atop color and pattern-mask intersection sequences", async () => {
    const ports = fakePorts();
    await renderComposerRecipe(packFixture(), recipe("pattern-tabby"), ports.target as unknown as HTMLCanvasElement, ports);

    const color = ports.surfaces.find((surface) =>
      surface.calls.includes("draw:asset://masks/color.png")
      && surface.calls.includes("fillStyle:#F4D6A0"),
    );
    expect(color?.calls).toEqual([
      "save",
      "alpha:1",
      "transform:1,0,0,1,0,0",
      "gco:source-over",
      "draw:asset://masks/color.png",
      "gco:source-in",
      "fillStyle:#F4D6A0",
      "fill:0,0,1024,1024",
      "gco:multiply",
      expect.stringMatching(/^draw:asset:\/\/parts\//),
      "restore",
    ]);
    const bodyLayer = ports.surfaces.find((surface) =>
      surface.calls.includes("draw:asset://parts/body.png")
      && surface.calls.includes("gco:source-atop"),
    );
    expect(bodyLayer?.calls).toContain("gco:source-atop");
    expect(bodyLayer?.calls).toContain("gco:source-over");
    const pattern = ports.surfaces.find((surface) =>
      surface.calls.includes("draw:asset://patterns/tabby.png"),
    );
    expect(pattern?.calls).toEqual([
      "save",
      "alpha:1",
      "transform:1,0,0,1,0,0",
      "gco:source-over",
      "draw:asset://patterns/tabby.png",
      "gco:destination-in",
      "draw:asset://masks/pattern.png",
      "gco:destination-in",
      expect.stringMatching(/^draw:asset:\/\/parts\//),
      "restore",
    ]);
  });

  it("normalizes alpha transform and compositing inside every saved context", async () => {
    const ports = fakePorts();
    await renderComposerRecipe(packFixture(), recipe("pattern-tabby"), ports.target as unknown as HTMLCanvasElement, ports);

    for (const surface of ports.surfaces) {
      expect(surface.calls.slice(0, 4)).toEqual([
        "save",
        "alpha:1",
        "transform:1,0,0,1,0,0",
        "gco:source-over",
      ]);
      expect(surface.calls.at(-1)).toBe("restore");
    }
    expect(ports.target.calls.slice(0, 4)).toEqual([
      "save",
      "alpha:1",
      "transform:1,0,0,1,0,0",
      "gco:copy",
    ]);
    expect(ports.target.calls.at(-1)).toBe("restore");
  });

  it("preserves old target pixels when the single final copy draw fails", async () => {
    const ports = fakePorts({ failTargetDraw: true });
    ports.target.pixels = "old-preview";

    await expect(renderComposerRecipe(
      packFixture(),
      recipe(),
      ports.target as unknown as HTMLCanvasElement,
      ports,
    )).rejects.toThrow(/draw failed/);

    expect(ports.target.pixels).toBe("old-preview");
    expect(ports.target.calls.some((call) => call.startsWith("clear:"))).toBe(false);
    expect(ports.target.calls.some((call) => call.startsWith("draw:"))).toBe(false);
  });

  it("preserves old target pixels when configuring final copy compositing fails", async () => {
    const ports = fakePorts({ failTargetComposite: true });
    ports.target.pixels = "old-preview";

    await expect(renderComposerRecipe(
      packFixture(),
      recipe(),
      ports.target as unknown as HTMLCanvasElement,
      ports,
    )).rejects.toThrow(/composite failed/);

    expect(ports.target.pixels).toBe("old-preview");
    expect(ports.target.calls.some((call) => call.startsWith("clear:"))).toBe(false);
    expect(ports.target.calls.some((call) => call.startsWith("draw:"))).toBe(false);
  });

  it("does not report failure when an abnormal restore throws after the final copy committed", async () => {
    const ports = fakePorts({ failTargetRestore: true });
    ports.target.pixels = "old-preview";

    await expect(renderComposerRecipe(
      packFixture(),
      recipe(),
      ports.target as unknown as HTMLCanvasElement,
      ports,
    )).resolves.toBeUndefined();

    expect(ports.target.pixels).toMatch(/^copy:surface:/);
    expect(ports.target.calls.filter((call) => call.startsWith("draw:"))).toHaveLength(1);
    expect(ports.target.calls).toContain("restore");
  });

  it("skips all pattern loading and compositing for pattern-none", async () => {
    const ports = fakePorts();
    await renderComposerRecipe(packFixture(), recipe(), ports.target as unknown as HTMLCanvasElement, ports);

    expect(ports.loaded.some((path) => path.includes("patterns/"))).toBe(false);
    expect(ports.surfaces.some((surface) => surface.calls.includes("draw:asset://masks/pattern.png"))).toBe(false);
  });

  it.each([
    ["load", { failLoad: "asset://parts/muzzle.png" }],
    ["context", { failContextAt: 1 }],
    ["draw", { throwOnDraw: "parts/body.png" }],
  ])("leaves the target untouched when %s fails", async (_case, failure) => {
    const ports = fakePorts(failure);
    await expect(renderComposerRecipe(packFixture(), recipe(), ports.target as unknown as HTMLCanvasElement, ports)).rejects.toThrow();
    expect(ports.target.calls).toEqual([]);
  });

  it("rejects an invalid recipe before any render side effect", async () => {
    const ports = fakePorts();
    const invalid = { ...recipe(), earsId: "missing-ears" };
    await expect(renderComposerRecipe(packFixture(), invalid, ports.target as unknown as HTMLCanvasElement, ports)).rejects.toThrow(/recipe/i);
    expect(ports.surfaces).toEqual([]);
    expect(ports.loaded).toEqual([]);
    expect(ports.target.calls).toEqual([]);
  });

  it("rejects a target whose logical dimensions are not 1024 without clearing it", async () => {
    const ports = fakePorts();
    ports.target.width = 512;
    await expect(renderComposerRecipe(packFixture(), recipe(), ports.target as unknown as HTMLCanvasElement, ports)).rejects.toThrow(/1024/);
    expect(ports.loaded).toEqual([]);
    expect(ports.target.calls).toEqual([]);
  });

  it("exports through the same rendering core and produces stable bytes", async () => {
    const preview = fakePorts();
    await renderComposerRecipe(packFixture(), recipe(), preview.target as unknown as HTMLCanvasElement, preview);
    const firstPorts = fakePorts();
    const secondPorts = fakePorts();
    const first = await exportComposerPng(packFixture(), recipe(), firstPorts);
    const second = await exportComposerPng(packFixture({ shuffled: true }), recipe(), secondPorts);

    expect(finalLayerOrder(firstPorts)).toEqual(finalLayerOrder(preview));
    expect(finalLayerOrder(secondPorts)).toEqual(finalLayerOrder(preview));
    expect(new Uint8Array(await first.arrayBuffer())).toEqual(new Uint8Array(await second.arrayBuffer()));
  });

  it("rejects empty and non-PNG export blobs", async () => {
    await expect(exportComposerPng(packFixture(), recipe(), fakePorts({ png: new Blob([], { type: "image/png" }) })))
      .rejects.toThrow(/PNG/i);
    await expect(exportComposerPng(packFixture(), recipe(), fakePorts({ png: new Blob(["x"], { type: "image/jpeg" }) })))
      .rejects.toThrow(/PNG/i);
  });
});

describe("motionProfileForRecipe", () => {
  it("normalizes body geometry exactly and returns a fresh runtime-valid profile", () => {
    const pack = packFixture();
    const before = structuredClone(pack.bodies[0]);
    const first = motionProfileForRecipe(pack, recipe());
    const second = motionProfileForRecipe(pack, recipe());

    expect(first).toEqual({
      profileVersion: 1,
      engineProfile: "life-v1",
      alphaBounds: { left: 100 / 1024, top: 50 / 1024, right: 900 / 1024, bottom: 1000 / 1024 },
      breathZone: { left: 260 / 1024, top: 500 / 1024, right: 760 / 1024, bottom: 900 / 1024 },
      swayPivot: { x: 512 / 1024, y: 780 / 1024 },
    });
    expect(first).not.toBe(second);
    expect(first.alphaBounds).not.toBe(second.alphaBounds);
    expect(pack.bodies[0]).toEqual(before);
  });

  it("rejects positive-area face overlap, runtime face-safety violations, and invalid recipes", () => {
    const overlap = packFixture({
      faceSafeZone: { left: 300, top: 150, right: 720, bottom: 520 },
      breathZone: { left: 260, top: 500, right: 760, bottom: 900 },
    });
    expect(() => motionProfileForRecipe(overlap, recipe())).toThrow(/face|overlap/i);

    const runtimeUnsafe = packFixture({
      faceSafeZone: { left: 300, top: 100, right: 720, bottom: 300 },
      breathZone: { left: 260, top: 350, right: 760, bottom: 900 },
    });
    expect(() => motionProfileForRecipe(runtimeUnsafe, recipe())).toThrow(/face safety/i);
    expect(() => motionProfileForRecipe(packFixture(), { ...recipe(), bodyId: "missing-body" })).toThrow(/recipe/i);
  });

  it("allows face and breath zones to touch at their boundary", () => {
    const pack = packFixture({
      faceSafeZone: { left: 300, top: 150, right: 720, bottom: 500 },
      breathZone: { left: 260, top: 500, right: 760, bottom: 900 },
    });
    expect(() => motionProfileForRecipe(pack, recipe())).not.toThrow();
  });
});
