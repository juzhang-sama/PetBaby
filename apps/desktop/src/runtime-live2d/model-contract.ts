export interface ModelContractResult {
  valid: boolean;
  errors: string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
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

  const license = isRecord(input.license) ? input.license : undefined;
  if (license?.commercialUse !== true || license?.redistributable !== true) {
    errors.push("license is not approved for commercial redistribution");
  }
  return { valid: errors.length === 0, errors };
}
