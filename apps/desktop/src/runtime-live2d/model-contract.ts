export interface ModelContractResult {
  valid: boolean;
  errors: string[];
}

const REQUIRED_MOTIONS = ["idle", "react-happy", "react-curious", "sleep", "wake", "carried", "landed"] as const;
const REQUIRED_EXPRESSIONS = ["neutral", "happy", "curious", "sleepy", "sad", "angry"] as const;
const REQUIRED_HIT_AREAS = ["head", "body"] as const;
const REQUIRED_PARAMETERS = ["eyeOpen", "angleX", "angleY", "mouthOpen", "bodyBreath"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function hasStringMapping(group: unknown, key: string): boolean {
  return isRecord(group) && typeof group[key] === "string" && group[key]!.length > 0;
}

function hasMotionMapping(group: unknown, key: string): boolean {
  const mapping = isRecord(group) ? group[key] : undefined;
  return isRecord(mapping) && typeof mapping.group === "string" && mapping.group.length > 0;
}

export function validateModelContract(input: unknown): ModelContractResult {
  const errors: string[] = [];
  if (!isRecord(input)) return { valid: false, errors: ["manifest must be an object"] };

  if (input.schemaVersion !== 2) errors.push("manifest must use schemaVersion: 2");
  if (input.renderer !== "live2d-v1") errors.push("manifest must use renderer: live2d-v1");
  const files = Array.isArray(input.files) ? input.files.filter(isRecord) : [];
  const paths = files.map((file) => typeof file.relativePath === "string" ? file.relativePath.toLowerCase() : "");
  const modelEntry = typeof input.modelEntry === "string" ? input.modelEntry.toLowerCase() : "";
  const previewImage = typeof input.previewImage === "string" ? input.previewImage.toLowerCase() : "";
  if (!modelEntry.endsWith(".model3.json") || !paths.includes(modelEntry)) errors.push("missing file: model3");
  if (!previewImage.endsWith(".png") || !paths.includes(previewImage)) errors.push("missing file: preview");
  if (!paths.some((path) => path.endsWith(".moc3"))) errors.push("missing file: moc3");
  if (!paths.some((path) => path.endsWith(".png") && path !== previewImage)) {
    errors.push("missing file: texture");
  }
  if (!paths.some((path) => path.endsWith(".motion3.json"))) errors.push("missing file: motion3");
  if (!paths.some((path) => path.endsWith(".exp3.json"))) errors.push("missing file: exp3");

  const semantics = isRecord(input.semantics) ? input.semantics : undefined;
  const motions = semantics && semantics.motions;
  const expressions = semantics && semantics.expressions;
  const hitAreas = semantics && semantics.hitAreas;
  const parameters = semantics && semantics.parameters;
  for (const motion of REQUIRED_MOTIONS) {
    if (!hasMotionMapping(motions, motion)) errors.push(`missing motion: ${motion}`);
  }
  for (const expression of REQUIRED_EXPRESSIONS) {
    if (!hasStringMapping(expressions, expression)) errors.push(`missing expression: ${expression}`);
  }
  for (const hitArea of REQUIRED_HIT_AREAS) {
    if (!hasStringMapping(hitAreas, hitArea)) errors.push(`missing hit area: ${hitArea}`);
  }
  for (const parameter of REQUIRED_PARAMETERS) {
    if (!hasStringMapping(parameters, parameter)) errors.push(`missing parameter: ${parameter}`);
  }

  const license = isRecord(input.license) ? input.license : undefined;
  if (license?.commercialUse !== true || license?.redistributable !== true) {
    errors.push("license is not approved for commercial redistribution");
  }
  return { valid: errors.length === 0, errors };
}
