import { parseLive2DManifest, type RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import { validateModelContract } from "../runtime-live2d/model-contract";

export interface ModelLicenseReview {
  modelId: string;
  author: string;
  source: string;
  commercialUse: boolean;
  redistribution: boolean;
  reviewedAt: string;
}

export interface DirectAdoptionEntry {
  renderer: "live2d-v1";
  modelId: string;
  displayName: string;
  manifestPath: string;
  licensePath: string;
  manifest: RuntimeAssetManifestV2;
  licenseReview: ModelLicenseReview;
}

export interface DirectAdoptionInput {
  manifest: RuntimeAssetManifestV2;
  modelId: string;
  displayName: string;
  manifestPath: string;
  licensePath: string;
  licenseReview: ModelLicenseReview;
}

export function createDirectAdoptionEntry(input: DirectAdoptionInput): DirectAdoptionEntry {
  const manifest = parseLive2DManifest(input.manifest);
  const contract = validateModelContract(manifest);
  if (!contract.valid) throw new Error(`invalid Live2D model contract: ${contract.errors.join(", ")}`);
  if (!input.modelId || manifest.petId !== input.modelId) {
    throw new Error("modelId must match manifest.petId");
  }
  if (!input.displayName || !input.manifestPath || !input.licensePath) {
    throw new Error("direct adoption metadata is incomplete");
  }
  const review = input.licenseReview;
  if (
    !review
    || review.modelId !== input.modelId
    || !review.author
    || !review.source
    || review.commercialUse !== true
    || review.redistribution !== true
    || !review.reviewedAt
  ) {
    throw new Error("license review is incomplete or not approved");
  }
  if (Number.isNaN(Date.parse(review.reviewedAt))) {
    throw new Error("license review date is invalid");
  }
  if (
    review.author !== manifest.license.author
    || review.source !== manifest.license.source
    || review.commercialUse !== manifest.license.commercialUse
    || review.redistribution !== manifest.license.redistributable
  ) {
    throw new Error("license review does not match the manifest license");
  }
  return {
    renderer: "live2d-v1",
    modelId: input.modelId,
    displayName: input.displayName,
    manifestPath: input.manifestPath,
    licensePath: input.licensePath,
    manifest,
    licenseReview: review,
  };
}
