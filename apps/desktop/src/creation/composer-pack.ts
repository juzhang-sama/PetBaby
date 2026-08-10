import type { ComposerRecipe } from "./contracts";

export interface ComposerPoint { x: number; y: number }
export interface ComposerRect { left: number; top: number; right: number; bottom: number }

export interface ComposerPart {
  id: string;
  image: string;
  colorMask?: string;
  patternMask?: string;
  compatibleBodyIds: string[];
  anchor: ComposerPoint;
  zIndex: number;
}

export type ComposerEyePart = Omit<ComposerPart, "image"> & {
  openImage: string;
  closedImage: string;
};

export interface ComposerBodyPart extends ComposerPart {
  defaults: {
    earsId: string;
    eyesId: string;
    muzzleId: string;
    tailId: string;
    colorId: string;
    patternId: string;
  };
  alphaBounds: ComposerRect;
  faceSafeZone: ComposerRect;
  breathZone: ComposerRect;
  swayPivot: ComposerPoint;
}

export interface ComposerPackManifest {
  schemaVersion: 1;
  packId: string;
  packVersion: number;
  species: "cat";
  canvas: { width: 1024; height: 1024 };
  layerContractVersion: 1;
  bodies: ComposerBodyPart[];
  ears: ComposerPart[];
  eyes: ComposerEyePart[];
  muzzles: ComposerPart[];
  tails: ComposerPart[];
  colors: Array<{ id: string; value: string }>;
  patterns: Array<{ id: string; image: string | null }>;
}

type JsonRecord = Record<string, unknown>;
type FetchResponse = { ok: boolean; status: number; json(): Promise<unknown> };

const ID = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const COLOR = /^#[0-9a-fA-F]{6}$/;

function record(value: unknown, label: string, required: string[], optional: string[] = []): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new TypeError(`${label} must be an object`);
  const result = value as JsonRecord;
  const allowed = new Set([...required, ...optional]);
  for (const key of Object.keys(result)) {
    if (!allowed.has(key)) throw new TypeError(`${label} has unknown field: ${key}`);
  }
  for (const key of required) {
    if (!Object.prototype.hasOwnProperty.call(result, key))
      throw new TypeError(`${label}.${key} is required`);
  }
  return result;
}

function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value) || value.length === 0) throw new TypeError(`${label} must be a non-empty array`);
  return value;
}

function integer(value: unknown, label: string, positive = false): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < -2_147_483_648
      || value > 4_294_967_295 || (positive && value <= 0))
    throw new TypeError(`${label} must be ${positive ? "a positive " : "an "}integer`);
  return value;
}

function id(value: unknown, label: string): string {
  if (typeof value !== "string" || !ID.test(value)) throw new TypeError(`${label} is an invalid ID`);
  return value;
}

function imagePath(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0 || value.includes("\\") || value.includes("%")
      || value.includes(":")
      || value.startsWith("/") || /^[A-Za-z]:/.test(value) || value.includes("//")
      || value.split("/").some((part) => part === "" || part === "." || part === "..")
      || !value.toLowerCase().endsWith(".png"))
    throw new TypeError(`${label} must be a safe relative PNG path`);
  return value;
}

function point(value: unknown, label: string): ComposerPoint {
  const source = record(value, label, ["x", "y"]);
  const coordinate = (key: "x" | "y") => {
    const result = source[key];
    if (typeof result !== "number" || !Number.isFinite(result) || result < 0 || result > 1024)
      throw new TypeError(`${label}.${key} must be a finite canvas coordinate`);
    return result;
  };
  return { x: coordinate("x"), y: coordinate("y") };
}

function rect(value: unknown, label: string): ComposerRect {
  const source = record(value, label, ["left", "top", "right", "bottom"]);
  const read = (key: "left" | "top" | "right" | "bottom") => {
    const result = source[key];
    if (typeof result !== "number" || !Number.isFinite(result) || result < 0 || result > 1024)
      throw new TypeError(`${label}.${key} must be a finite canvas coordinate`);
    return result;
  };
  const result = { left: read("left"), top: read("top"), right: read("right"), bottom: read("bottom") };
  if (result.left >= result.right || result.top >= result.bottom)
    throw new TypeError(`${label} must have positive area and forward bounds`);
  return result;
}

function compatibleBodies(value: unknown, label: string): string[] {
  const values = array(value, label).map((entry, index) => id(entry, `${label}[${index}]`));
  if (new Set(values).size !== values.length) throw new TypeError(`${label} has duplicate body IDs`);
  return values;
}

function zIndex(value: unknown, label: string): number {
  const result = integer(value, label);
  if (result > 2_147_483_647) throw new TypeError(`${label} must fit a signed 32-bit integer`);
  return result;
}

const PART_REQUIRED = ["id", "image", "compatibleBodyIds", "anchor", "zIndex"];
const PART_OPTIONAL = ["colorMask", "patternMask"];

function part(value: unknown, label: string): ComposerPart {
  const source = record(value, label, PART_REQUIRED, PART_OPTIONAL);
  const result: ComposerPart = {
    id: id(source.id, `${label}.id`),
    image: imagePath(source.image, `${label}.image`),
    compatibleBodyIds: compatibleBodies(source.compatibleBodyIds, `${label}.compatibleBodyIds`),
    anchor: point(source.anchor, `${label}.anchor`),
    zIndex: zIndex(source.zIndex, `${label}.zIndex`),
  };
  if (source.colorMask !== undefined) result.colorMask = imagePath(source.colorMask, `${label}.colorMask`);
  if (source.patternMask !== undefined) result.patternMask = imagePath(source.patternMask, `${label}.patternMask`);
  return result;
}

function eye(value: unknown, label: string): ComposerEyePart {
  const source = record(value, label,
    ["id", "openImage", "closedImage", "compatibleBodyIds", "anchor", "zIndex"], PART_OPTIONAL);
  const result: ComposerEyePart = {
    id: id(source.id, `${label}.id`),
    openImage: imagePath(source.openImage, `${label}.openImage`),
    closedImage: imagePath(source.closedImage, `${label}.closedImage`),
    compatibleBodyIds: compatibleBodies(source.compatibleBodyIds, `${label}.compatibleBodyIds`),
    anchor: point(source.anchor, `${label}.anchor`),
    zIndex: zIndex(source.zIndex, `${label}.zIndex`),
  };
  if (source.colorMask !== undefined) result.colorMask = imagePath(source.colorMask, `${label}.colorMask`);
  if (source.patternMask !== undefined) result.patternMask = imagePath(source.patternMask, `${label}.patternMask`);
  return result;
}

function body(value: unknown, label: string): ComposerBodyPart {
  const source = record(value, label,
    [...PART_REQUIRED, "defaults", "alphaBounds", "faceSafeZone", "breathZone", "swayPivot"], PART_OPTIONAL);
  const base = part(Object.fromEntries([...PART_REQUIRED, ...PART_OPTIONAL]
    .filter((key) => source[key] !== undefined).map((key) => [key, source[key]])), label);
  const defaults = record(source.defaults, `${label}.defaults`,
    ["earsId", "eyesId", "muzzleId", "tailId", "colorId", "patternId"]);
  return {
    ...base,
    defaults: {
      earsId: id(defaults.earsId, `${label}.defaults.earsId`),
      eyesId: id(defaults.eyesId, `${label}.defaults.eyesId`),
      muzzleId: id(defaults.muzzleId, `${label}.defaults.muzzleId`),
      tailId: id(defaults.tailId, `${label}.defaults.tailId`),
      colorId: id(defaults.colorId, `${label}.defaults.colorId`),
      patternId: id(defaults.patternId, `${label}.defaults.patternId`),
    },
    alphaBounds: rect(source.alphaBounds, `${label}.alphaBounds`),
    faceSafeZone: rect(source.faceSafeZone, `${label}.faceSafeZone`),
    breathZone: rect(source.breathZone, `${label}.breathZone`),
    swayPivot: point(source.swayPivot, `${label}.swayPivot`),
  };
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child);
  }
  return value;
}

function byId<T extends { id: string }>(items: T[]): Map<string, T> {
  return new Map(items.map((item) => [item.id, item]));
}

export function parseComposerPack(value: unknown): ComposerPackManifest {
  const source = record(value, "composer pack", [
    "schemaVersion", "packId", "packVersion", "species", "canvas", "layerContractVersion",
    "bodies", "ears", "eyes", "muzzles", "tails", "colors", "patterns",
  ]);
  if (source.schemaVersion !== 1) throw new TypeError("schemaVersion must be 1");
  if (source.species !== "cat") throw new TypeError("species must be cat");
  if (source.layerContractVersion !== 1) throw new TypeError("layerContractVersion must be 1");
  const canvas = record(source.canvas, "canvas", ["width", "height"]);
  if (canvas.width !== 1024 || canvas.height !== 1024) throw new TypeError("canvas must be 1024x1024");

  const pack: ComposerPackManifest = {
    schemaVersion: 1,
    packId: id(source.packId, "packId"),
    packVersion: integer(source.packVersion, "packVersion", true),
    species: "cat",
    canvas: { width: 1024, height: 1024 },
    layerContractVersion: 1,
    bodies: array(source.bodies, "bodies").map((entry, index) => body(entry, `bodies[${index}]`)),
    ears: array(source.ears, "ears").map((entry, index) => part(entry, `ears[${index}]`)),
    eyes: array(source.eyes, "eyes").map((entry, index) => eye(entry, `eyes[${index}]`)),
    muzzles: array(source.muzzles, "muzzles").map((entry, index) => part(entry, `muzzles[${index}]`)),
    tails: array(source.tails, "tails").map((entry, index) => part(entry, `tails[${index}]`)),
    colors: array(source.colors, "colors").map((entry, index) => {
      const color = record(entry, `colors[${index}]`, ["id", "value"]);
      if (typeof color.value !== "string" || !COLOR.test(color.value))
        throw new TypeError(`colors[${index}].value must be #RRGGBB`);
      return { id: id(color.id, `colors[${index}].id`), value: color.value };
    }),
    patterns: array(source.patterns, "patterns").map((entry, index) => {
      const pattern = record(entry, `patterns[${index}]`, ["id", "image"]);
      const patternId = id(pattern.id, `patterns[${index}].id`);
      const patternImage = pattern.image === null ? null : imagePath(pattern.image, `patterns[${index}].image`);
      if ((patternId === "pattern-none") !== (patternImage === null))
        throw new TypeError(`patterns[${index}].image has invalid null semantics`);
      return { id: patternId, image: patternImage };
    }),
  };

  if (!pack.patterns.some((pattern) => pattern.id === "pattern-none"))
    throw new TypeError("patterns must declare pattern-none with a null image");

  const allItems: Array<{ id: string }> = [
    ...pack.bodies, ...pack.ears, ...pack.eyes, ...pack.muzzles, ...pack.tails,
    ...pack.colors, ...pack.patterns,
  ];
  const ids = new Set<string>();
  for (const item of allItems) {
    if (ids.has(item.id)) throw new TypeError(`duplicate ID: ${item.id}`);
    ids.add(item.id);
  }
  const bodies = byId(pack.bodies);
  for (const item of [...pack.bodies, ...pack.ears, ...pack.eyes, ...pack.muzzles, ...pack.tails]) {
    for (const bodyId of item.compatibleBodyIds) {
      if (!bodies.has(bodyId)) throw new TypeError(`${item.id} references unknown body: ${bodyId}`);
    }
  }
  const categories = {
    earsId: byId(pack.ears), eyesId: byId(pack.eyes), muzzleId: byId(pack.muzzles),
    tailId: byId(pack.tails), colorId: byId(pack.colors), patternId: byId(pack.patterns),
  };
  for (const body of pack.bodies) {
    if (!body.compatibleBodyIds.includes(body.id)) throw new TypeError(`${body.id} is not compatible with itself`);
    for (const [key, category] of Object.entries(categories) as Array<[keyof typeof categories, Map<string, { id: string; compatibleBodyIds?: string[] }>]>) {
      const selected = category.get(body.defaults[key]);
      if (!selected) throw new TypeError(`${body.id} defaults reference unknown ${key}: ${body.defaults[key]}`);
      if (selected.compatibleBodyIds && !selected.compatibleBodyIds.includes(body.id))
        throw new TypeError(`${body.id} defaults select incompatible ${key}: ${selected.id}`);
    }
  }
  return deepFreeze(pack);
}

export function compatibleItems<T extends { compatibleBodyIds: readonly string[] }>(
  items: readonly T[], selection: { bodyId: string },
): T[] {
  return items.filter((item) => item.compatibleBodyIds.includes(selection.bodyId));
}

export function validateRecipe(pack: ComposerPackManifest, recipe: ComposerRecipe): string[] {
  const errors: string[] = [];
  if (recipe.recipeVersion !== 1) errors.push("recipeVersion must be 1");
  if (recipe.packId !== pack.packId) errors.push(`packId must match ${pack.packId}`);
  if (recipe.packVersion !== pack.packVersion) errors.push(`packVersion must match ${pack.packVersion}`);
  if (recipe.layerContractVersion !== pack.layerContractVersion)
    errors.push(`layerContractVersion must match ${pack.layerContractVersion}`);
  const body = pack.bodies.find((item) => item.id === recipe.bodyId);
  if (!body) errors.push(`bodyId does not exist: ${recipe.bodyId}`);
  const checks = [
    ["earsId", recipe.earsId, pack.ears], ["eyesId", recipe.eyesId, pack.eyes],
    ["muzzleId", recipe.muzzleId, pack.muzzles], ["tailId", recipe.tailId, pack.tails],
  ] as const;
  for (const [field, selectedId, items] of checks) {
    const selected = items.find((item) => item.id === selectedId);
    if (!selected) errors.push(`${field} does not exist: ${selectedId}`);
    else if (body && !selected.compatibleBodyIds.includes(body.id))
      errors.push(`${field} is incompatible with bodyId ${body.id}: ${selectedId}`);
  }
  if (!pack.colors.some((item) => item.id === recipe.colorId)) errors.push(`colorId does not exist: ${recipe.colorId}`);
  if (!pack.patterns.some((item) => item.id === recipe.patternId)) errors.push(`patternId does not exist: ${recipe.patternId}`);
  return errors;
}

export async function loadComposerPack(
  url: string,
  ports: { fetch: (url: string) => Promise<FetchResponse> } = { fetch: (value) => fetch(value) },
): Promise<ComposerPackManifest> {
  const response = await ports.fetch(url);
  if (!response.ok) throw new Error(`composer pack request failed with HTTP ${response.status}`);
  let value: unknown;
  try {
    value = await response.json();
  } catch (error) {
    throw new Error(`composer pack JSON could not be parsed: ${error instanceof Error ? error.message : String(error)}`);
  }
  return parseComposerPack(value);
}
