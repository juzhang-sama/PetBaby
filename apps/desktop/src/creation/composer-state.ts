import type { ComposerRecipe } from "./contracts";
import type { ComposerPackManifest } from "./composer-pack";
import { validateRecipe } from "./composer-pack";

export type ComposerStep =
  | "body"
  | "ears"
  | "eyes"
  | "muzzle"
  | "tail"
  | "coat"
  | "name"
  | "preview";

export type ComposerSelectionKind =
  | "body"
  | "ears"
  | "eyes"
  | "muzzle"
  | "tail"
  | "color"
  | "pattern";

const STEPS: readonly ComposerStep[] = [
  "body",
  "ears",
  "eyes",
  "muzzle",
  "tail",
  "coat",
  "name",
  "preview",
];

function assertValidRecipe(pack: ComposerPackManifest, recipe: ComposerRecipe): void {
  const errors = validateRecipe(pack, recipe);
  if (errors.length > 0) throw new Error(`invalid composer recipe: ${errors.join("; ")}`);
}

function recipeForBody(pack: ComposerPackManifest, bodyId: string): ComposerRecipe {
  const body = pack.bodies.find((item) => item.id === bodyId);
  if (!body) throw new Error(`composer body does not exist: ${bodyId}`);
  const recipe: ComposerRecipe = {
    recipeVersion: 1,
    packId: pack.packId,
    packVersion: pack.packVersion,
    layerContractVersion: pack.layerContractVersion,
    bodyId: body.id,
    ...body.defaults,
  };
  assertValidRecipe(pack, recipe);
  return recipe;
}

function compatibleWithBody(
  item: { compatibleBodyIds: readonly string[] } | undefined,
  bodyId: string,
): boolean {
  return item?.compatibleBodyIds.includes(bodyId) === true;
}

export class ComposerState {
  private constructor(
    private readonly pack: ComposerPackManifest,
    private currentRecipe: ComposerRecipe,
    private stepIndex: number,
  ) {}

  static start(pack: ComposerPackManifest, bodyId: string): ComposerState {
    return new ComposerState(pack, recipeForBody(pack, bodyId), STEPS.indexOf("ears"));
  }

  static fromRecipe(pack: ComposerPackManifest, recipe: ComposerRecipe): ComposerState {
    assertValidRecipe(pack, recipe);
    return new ComposerState(pack, {
      recipeVersion: 1,
      packId: pack.packId,
      packVersion: pack.packVersion,
      layerContractVersion: pack.layerContractVersion,
      bodyId: recipe.bodyId,
      earsId: recipe.earsId,
      eyesId: recipe.eyesId,
      muzzleId: recipe.muzzleId,
      tailId: recipe.tailId,
      colorId: recipe.colorId,
      patternId: recipe.patternId,
    }, STEPS.indexOf("preview"));
  }

  select(kind: ComposerSelectionKind, id: string): void {
    if (kind === "body") {
      this.selectBody(id);
      return;
    }

    const bodyId = this.currentRecipe.bodyId;
    const selections = {
      ears: { items: this.pack.ears, field: "earsId" },
      eyes: { items: this.pack.eyes, field: "eyesId" },
      muzzle: { items: this.pack.muzzles, field: "muzzleId" },
      tail: { items: this.pack.tails, field: "tailId" },
    } as const;

    let next: ComposerRecipe;
    if (kind === "color") {
      if (!this.pack.colors.some((item) => item.id === id)) {
        throw new Error(`composer color does not exist: ${id}`);
      }
      next = { ...this.currentRecipe, colorId: id };
    } else if (kind === "pattern") {
      if (!this.pack.patterns.some((item) => item.id === id)) {
        throw new Error(`composer pattern does not exist: ${id}`);
      }
      next = { ...this.currentRecipe, patternId: id };
    } else {
      const selection = selections[kind];
      const item = selection.items.find((candidate) => candidate.id === id);
      if (!item) throw new Error(`composer ${kind} does not exist: ${id}`);
      if (!item.compatibleBodyIds.includes(bodyId)) {
        throw new Error(`composer ${kind} is incompatible with body ${bodyId}: ${id}`);
      }
      next = { ...this.currentRecipe, [selection.field]: id };
    }

    assertValidRecipe(this.pack, next);
    this.currentRecipe = next;
  }

  goBack(): void {
    this.stepIndex = Math.max(0, this.stepIndex - 1);
  }

  goNext(): void {
    this.stepIndex = Math.min(STEPS.length - 1, this.stepIndex + 1);
  }

  step(): ComposerStep {
    return STEPS[this.stepIndex]!;
  }

  recipe(): ComposerRecipe {
    return { ...this.currentRecipe };
  }

  private selectBody(bodyId: string): void {
    const body = this.pack.bodies.find((item) => item.id === bodyId);
    if (!body) throw new Error(`composer body does not exist: ${bodyId}`);

    const current = this.currentRecipe;
    const next: ComposerRecipe = {
      recipeVersion: 1,
      packId: this.pack.packId,
      packVersion: this.pack.packVersion,
      layerContractVersion: this.pack.layerContractVersion,
      bodyId,
      earsId: compatibleWithBody(this.pack.ears.find((item) => item.id === current.earsId), bodyId)
        ? current.earsId : body.defaults.earsId,
      eyesId: compatibleWithBody(this.pack.eyes.find((item) => item.id === current.eyesId), bodyId)
        ? current.eyesId : body.defaults.eyesId,
      muzzleId: compatibleWithBody(this.pack.muzzles.find((item) => item.id === current.muzzleId), bodyId)
        ? current.muzzleId : body.defaults.muzzleId,
      tailId: compatibleWithBody(this.pack.tails.find((item) => item.id === current.tailId), bodyId)
        ? current.tailId : body.defaults.tailId,
      colorId: this.pack.colors.some((item) => item.id === current.colorId)
        ? current.colorId : body.defaults.colorId,
      patternId: this.pack.patterns.some((item) => item.id === current.patternId)
        ? current.patternId : body.defaults.patternId,
    };

    assertValidRecipe(this.pack, next);
    this.currentRecipe = next;
  }
}
