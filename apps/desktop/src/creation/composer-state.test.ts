import { describe, expect, it } from "vitest";
import type { ComposerRecipe } from "./contracts";
import { parseComposerPack, validateRecipe } from "./composer-pack";
import { ComposerState } from "./composer-state";

function packFixture() {
  return parseComposerPack({
    schemaVersion: 1,
    packId: "cat-cute-v1",
    packVersion: 3,
    species: "cat",
    canvas: { width: 1024, height: 1024 },
    layerContractVersion: 1,
    bodies: [
      {
        id: "body-round",
        image: "parts/body-round.png",
        compatibleBodyIds: ["body-round"],
        anchor: { x: 512, y: 512 },
        zIndex: 10,
        defaults: {
          earsId: "ears-round",
          eyesId: "eyes-shared",
          muzzleId: "muzzle-round",
          tailId: "tail-shared",
          colorId: "color-cream",
          patternId: "pattern-none",
        },
        alphaBounds: { left: 100, top: 50, right: 900, bottom: 1000 },
        faceSafeZone: { left: 300, top: 150, right: 720, bottom: 450 },
        breathZone: { left: 260, top: 500, right: 760, bottom: 900 },
        swayPivot: { x: 512, y: 780 },
      },
      {
        id: "body-fluffy",
        image: "parts/body-fluffy.png",
        compatibleBodyIds: ["body-fluffy"],
        anchor: { x: 512, y: 512 },
        zIndex: 10,
        defaults: {
          earsId: "ears-fluffy",
          eyesId: "eyes-shared",
          muzzleId: "muzzle-fluffy",
          tailId: "tail-shared",
          colorId: "color-gray",
          patternId: "pattern-tabby",
        },
        alphaBounds: { left: 80, top: 40, right: 920, bottom: 1000 },
        faceSafeZone: { left: 290, top: 140, right: 730, bottom: 440 },
        breathZone: { left: 250, top: 500, right: 770, bottom: 910 },
        swayPivot: { x: 512, y: 790 },
      },
    ],
    ears: [
      { id: "ears-round", image: "parts/ears-round.png", compatibleBodyIds: ["body-round"], anchor: { x: 512, y: 200 }, zIndex: 20 },
      { id: "ears-fluffy", image: "parts/ears-fluffy.png", compatibleBodyIds: ["body-fluffy"], anchor: { x: 512, y: 200 }, zIndex: 20 },
    ],
    eyes: [
      { id: "eyes-shared", openImage: "parts/eyes-open.png", closedImage: "parts/eyes-closed.png", compatibleBodyIds: ["body-round", "body-fluffy"], anchor: { x: 512, y: 340 }, zIndex: 30 },
    ],
    muzzles: [
      { id: "muzzle-round", image: "parts/muzzle-round.png", compatibleBodyIds: ["body-round"], anchor: { x: 512, y: 430 }, zIndex: 40 },
      { id: "muzzle-fluffy", image: "parts/muzzle-fluffy.png", compatibleBodyIds: ["body-fluffy"], anchor: { x: 512, y: 430 }, zIndex: 40 },
    ],
    tails: [
      { id: "tail-shared", image: "parts/tail.png", compatibleBodyIds: ["body-round", "body-fluffy"], anchor: { x: 700, y: 650 }, zIndex: 0 },
    ],
    colors: [
      { id: "color-cream", value: "#F4D6A0" },
      { id: "color-gray", value: "#A0A0A0" },
    ],
    patterns: [
      { id: "pattern-none", image: null },
      { id: "pattern-tabby", image: "patterns/tabby.png" },
    ],
  });
}

function roundRecipe(): ComposerRecipe {
  return {
    recipeVersion: 1,
    packId: "cat-cute-v1",
    packVersion: 3,
    layerContractVersion: 1,
    bodyId: "body-round",
    earsId: "ears-round",
    eyesId: "eyes-shared",
    muzzleId: "muzzle-round",
    tailId: "tail-shared",
    colorId: "color-cream",
    patternId: "pattern-none",
  };
}

describe("ComposerState", () => {
  it("starts after body selection with a complete compatible default recipe", () => {
    const pack = packFixture();
    const state = ComposerState.start(pack, "body-round");

    expect(state.recipe()).toEqual(roundRecipe());
    expect(validateRecipe(pack, state.recipe())).toEqual([]);
    expect(state.step()).toBe("ears");
  });

  it("rejects an unknown starting body", () => {
    expect(() => ComposerState.start(packFixture(), "body-missing")).toThrow(/body/i);
  });

  it("changing body replaces only incompatible choices and preserves compatible coat choices", () => {
    const pack = packFixture();
    const state = ComposerState.fromRecipe(pack, roundRecipe());

    state.select("body", "body-fluffy");

    expect(state.recipe()).toEqual({
      ...roundRecipe(),
      bodyId: "body-fluffy",
      earsId: "ears-fluffy",
      muzzleId: "muzzle-fluffy",
    });
    expect(validateRecipe(pack, state.recipe())).toEqual([]);
    expect(state.step()).toBe("preview");
  });

  it("rejects missing and incompatible selections atomically without advancing", () => {
    const state = ComposerState.start(packFixture(), "body-round");
    state.goNext();
    const before = state.recipe();
    const step = state.step();

    expect(() => state.select("ears", "ears-fluffy")).toThrow(/compatible|incompatible/i);
    expect(state.recipe()).toEqual(before);
    expect(state.step()).toBe(step);
    expect(() => state.select("color", "color-missing")).toThrow(/exist|missing/i);
    expect(state.recipe()).toEqual(before);
    expect(state.step()).toBe(step);
  });

  it("moves through fixed steps, saturates at both ends, and select never advances", () => {
    const state = ComposerState.start(packFixture(), "body-round");
    expect(state.step()).toBe("ears");
    state.select("eyes", "eyes-shared");
    expect(state.step()).toBe("ears");

    state.goBack();
    state.goBack();
    expect(state.step()).toBe("body");
    for (let index = 0; index < 20; index += 1) state.goNext();
    expect(state.step()).toBe("preview");
    state.goNext();
    expect(state.step()).toBe("preview");
    state.goBack();
    expect(state.step()).toBe("name");
  });

  it("restores a valid recipe at preview and rejects every invalid recipe without repair", () => {
    const pack = packFixture();
    const state = ComposerState.fromRecipe(pack, roundRecipe());
    expect(state.step()).toBe("preview");
    expect(state.recipe()).toEqual(roundRecipe());

    for (const invalid of [
      { ...roundRecipe(), packId: "other-pack" },
      { ...roundRecipe(), packVersion: 99 },
      { ...roundRecipe(), layerContractVersion: 2 },
      { ...roundRecipe(), earsId: "ears-fluffy" },
    ]) {
      expect(() => ComposerState.fromRecipe(pack, invalid)).toThrow(/recipe/i);
    }
  });

  it("returns independent recipe values and never leaks references into state or pack", () => {
    const pack = packFixture();
    const state = ComposerState.fromRecipe(pack, roundRecipe());
    const first = state.recipe();
    first.bodyId = "body-fluffy";
    first.packVersion = 999;

    expect(state.recipe()).toEqual(roundRecipe());
    expect(pack.bodies[0]!.id).toBe("body-round");
    expect(state.recipe()).not.toBe(state.recipe());
  });

  it("rebuilds restored recipes from the pack contract instead of retaining caller fields", () => {
    const input = { ...roundRecipe(), untrusted: "discard-me" } as ComposerRecipe;
    const restored = ComposerState.fromRecipe(packFixture(), input).recipe() as ComposerRecipe & {
      untrusted?: string;
    };

    expect(restored.untrusted).toBeUndefined();
    expect(Object.keys(restored).sort()).toEqual(Object.keys(roundRecipe()).sort());
  });
});
