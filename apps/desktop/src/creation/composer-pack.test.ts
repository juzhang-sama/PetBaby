import { describe, expect, it, vi } from "vitest";
import type { ComposerRecipe } from "./contracts";
import {
  compatibleItems,
  loadComposerPack,
  parseComposerPack,
  validateRecipe,
} from "./composer-pack";

function validPack(): unknown {
  return {
    schemaVersion: 1,
    packId: "cat-cute-v1",
    packVersion: 1,
    species: "cat",
    canvas: { width: 1024, height: 1024 },
    layerContractVersion: 1,
    bodies: [{
      id: "body-round", image: "parts/body.png", colorMask: "parts/body-color.png",
      patternMask: "parts/body-pattern.png", compatibleBodyIds: ["body-round"],
      anchor: { x: 512, y: 512 }, zIndex: 10,
      defaults: { earsId: "ears-round", eyesId: "eyes-amber", muzzleId: "muzzle-soft", tailId: "tail-curl", colorId: "color-cream", patternId: "pattern-none" },
      alphaBounds: { left: 100, top: 50, right: 900, bottom: 1000 },
      faceSafeZone: { left: 300, top: 160, right: 720, bottom: 500 },
      breathZone: { left: 260, top: 500, right: 760, bottom: 900 },
      swayPivot: { x: 512, y: 780 },
    }, {
      id: "body-other", image: "parts/body-other.png", compatibleBodyIds: ["body-other"],
      anchor: { x: 512, y: 512 }, zIndex: 10,
      defaults: { earsId: "ears-other", eyesId: "eyes-amber", muzzleId: "muzzle-soft", tailId: "tail-curl", colorId: "color-cream", patternId: "pattern-none" },
      alphaBounds: { left: 100, top: 50, right: 900, bottom: 1000 },
      faceSafeZone: { left: 300, top: 160, right: 720, bottom: 500 },
      breathZone: { left: 260, top: 500, right: 760, bottom: 900 },
      swayPivot: { x: 512, y: 780 },
    }],
    ears: [
      { id: "ears-round", image: "parts/ears-round.png", compatibleBodyIds: ["body-round"], anchor: { x: 512, y: 230 }, zIndex: 20 },
      { id: "ears-folded", image: "parts/ears-folded.png", compatibleBodyIds: ["body-round"], anchor: { x: 512, y: 230 }, zIndex: 21 },
      { id: "ears-other", image: "parts/ears-other.png", compatibleBodyIds: ["body-other"], anchor: { x: 512, y: 230 }, zIndex: 22 },
    ],
    eyes: [{ id: "eyes-amber", openImage: "parts/eyes-open.png", closedImage: "parts/eyes-closed.png", compatibleBodyIds: ["body-round", "body-other"], anchor: { x: 512, y: 340 }, zIndex: 30 }],
    muzzles: [{ id: "muzzle-soft", image: "parts/muzzle.png", compatibleBodyIds: ["body-round", "body-other"], anchor: { x: 512, y: 430 }, zIndex: 40 }],
    tails: [{ id: "tail-curl", image: "parts/tail.png", compatibleBodyIds: ["body-round", "body-other"], anchor: { x: 700, y: 650 }, zIndex: 0 }],
    colors: [{ id: "color-cream", value: "#F4D6A0" }],
    patterns: [{ id: "pattern-none", image: null }, { id: "pattern-tabby", image: "patterns/tabby.png" }],
  };
}

function validRecipe(): ComposerRecipe {
  return {
    recipeVersion: 1, packId: "cat-cute-v1", packVersion: 1, layerContractVersion: 1,
    bodyId: "body-round", earsId: "ears-round", eyesId: "eyes-amber",
    muzzleId: "muzzle-soft", tailId: "tail-curl", colorId: "color-cream",
    patternId: "pattern-none",
  };
}

function changed(path: string, value: unknown): unknown {
  const pack = structuredClone(validPack()) as Record<string, unknown>;
  const keys = path.split(".");
  let cursor: Record<string, unknown> = pack;
  for (const key of keys.slice(0, -1)) cursor = cursor[key] as Record<string, unknown>;
  cursor[keys.at(-1)!] = value;
  return pack;
}

describe("parseComposerPack", () => {
  it("parses the complete manifest and returns a deeply immutable value", () => {
    const pack = parseComposerPack(validPack());
    expect(pack.packId).toBe("cat-cute-v1");
    expect(Object.isFrozen(pack)).toBe(true);
    expect(Object.isFrozen(pack.bodies)).toBe(true);
    const body = pack.bodies[0]!;
    expect(Object.isFrozen(body.defaults)).toBe(true);
    expect(Object.isFrozen(body.anchor)).toBe(true);
    expect(() => { (body.anchor as { x: number }).x = 0; }).toThrow();
  });

  it("filters compatible parts in manifest order", () => {
    const pack = parseComposerPack(validPack());
    expect(compatibleItems(pack.ears, { bodyId: "body-round" }).map((item) => item.id))
      .toEqual(["ears-round", "ears-folded"]);
  });

  it.each([
    ["schemaVersion", 2], ["species", "dog"], ["packVersion", 0],
    ["layerContractVersion", 2], ["canvas.width", 512], ["ears", []],
    ["ears.0.id", ""], ["ears.0.id", "Bad ID"],
    ["ears.0.compatibleBodyIds", ["missing-body"]],
    ["bodies.0.defaults.earsId", "missing-ears"],
    ["bodies.0.alphaBounds.right", 99], ["bodies.0.faceSafeZone.left", -1],
    ["bodies.0.breathZone.bottom", 1025], ["bodies.0.swayPivot.x", Number.NaN],
    ["ears.0.anchor.y", Number.POSITIVE_INFINITY], ["colors.0.value", "cream"],
    ["patterns.0.image", "patterns/none.png"], ["patterns.1.image", null],
    ["bodies.0.colorMask", null],
    ["ears.0.compatibleBodyIds", ["body-other"]],
  ])("rejects invalid %s", (path, value) => {
    expect(() => parseComposerPack(changed(path, value))).toThrow();
  });

  it("rejects duplicate IDs across categories", () => {
    expect(() => parseComposerPack(changed("ears.0.id", "body-round"))).toThrow(/duplicate/i);
  });

  it("rejects unknown fields at every shape", () => {
    expect(() => parseComposerPack({ ...(validPack() as object), extra: true })).toThrow(/unknown/i);
    const nested = changed("bodies.0.anchor.extra", 1);
    expect(() => parseComposerPack(nested)).toThrow(/unknown/i);
  });

  it("rejects missing closed eye and malformed image paths", () => {
    const missing = structuredClone(validPack()) as any;
    delete missing.eyes[0].closedImage;
    expect(() => parseComposerPack(missing)).toThrow(/closedImage/);
    for (const path of ["", "../secret.png", "/secret.png", "C:/secret.png", "parts/%2e%2e/eye.png", "parts\\eye.png", "parts//eye.png"])
      expect(() => parseComposerPack(changed("eyes.0.closedImage", path))).toThrow(/path/i);
  });

  it("requires the explicit no-pattern recipe value", () => {
    expect(() => parseComposerPack(changed("patterns", [{ id: "pattern-tabby", image: "patterns/tabby.png" }]))).toThrow(/pattern-none/);
  });

  it("requires pattern.image while preserving explicit null semantics", () => {
    const missing = structuredClone(validPack()) as { patterns: Array<Record<string, unknown>> };
    delete missing.patterns[0]!.image;
    expect(() => parseComposerPack(missing)).toThrow(/image is required/);
    const pack = parseComposerPack(validPack());
    expect(pack.patterns.map((pattern) => pattern.image)).toEqual([null, "patterns/tabby.png"]);
  });
});

describe("validateRecipe", () => {
  it("returns no errors for the complete default recipe", () => {
    expect(validateRecipe(parseComposerPack(validPack()), validRecipe())).toEqual([]);
  });

  it.each([
    ["recipeVersion", 2, "recipeVersion must be 1"],
    ["packId", "other", "packId must match cat-cute-v1"],
    ["packVersion", 2, "packVersion must match 1"],
    ["layerContractVersion", 2, "layerContractVersion must match 1"],
    ["earsId", "missing", "earsId does not exist: missing"],
    ["earsId", "ears-other", "earsId is incompatible with bodyId body-round: ears-other"],
  ])("reports deterministic %s errors", (key, value, message) => {
    const recipe = { ...validRecipe(), [key]: value };
    expect(validateRecipe(parseComposerPack(validPack()), recipe)).toContain(message);
  });
});

describe("loadComposerPack", () => {
  it("validates a successful response", async () => {
    const fetch = vi.fn(async () => ({ ok: true, status: 200, json: async () => validPack() }));
    const pack = await loadComposerPack("/pack.json", { fetch });
    expect(pack.packId).toBe("cat-cute-v1");
  });

  it("rejects non-2xx, malformed json and invalid manifests without caching failures", async () => {
    await expect(loadComposerPack("/pack.json", { fetch: vi.fn(async () => ({ ok: false, status: 503, json: async () => ({}) })) }))
      .rejects.toThrow(/503/);
    await expect(loadComposerPack("/pack.json", { fetch: vi.fn(async () => ({ ok: true, status: 200, json: async () => { throw new SyntaxError("bad json"); } })) }))
      .rejects.toThrow(/JSON/i);
    let calls = 0;
    const fetch = vi.fn(async () => ({ ok: true, status: 200, json: async () => (++calls === 1 ? { nope: true } : validPack()) }));
    await expect(loadComposerPack("/retry.json", { fetch })).rejects.toThrow();
    await expect(loadComposerPack("/retry.json", { fetch })).resolves.toMatchObject({ packId: "cat-cute-v1" });
    expect(fetch).toHaveBeenCalledTimes(2);
  });
});
