import type { ManifestPart } from "./manifest-schema";
import {
  applyBoneTransform,
  composeTransforms,
  translateTransform,
  type AffineTransform,
  type Bone,
  type BonePose,
  type Vec2,
} from "./bone";

export interface PartTextureSize {
  width: number;
  height: number;
}

export interface RiggedPart {
  role: string;
  zIndex: number;
  deformable: boolean;
  boneId?: string;
  /** Maps texture pixel coordinates into the world (viewport) space. */
  transform: AffineTransform;
  /** World position of the part's pivot (joint). */
  pivotWorld: Vec2;
}

/**
 * Compute the world transform of every part by attaching it to its bone
 * (or directly to the root transform when no boneId is declared). Parts are
 * returned in draw order (zIndex ascending).
 */
export function computePartRig(
  parts: ManifestPart[],
  textureSizes: ReadonlyMap<string, PartTextureSize>,
  bones: Bone[],
  rootTransform: AffineTransform,
  poses: ReadonlyMap<string, BonePose> = new Map(),
): RiggedPart[] {
  const boneById = new Map(bones.map((bone) => [bone.id, bone] as const));
  const cache = new Map<string, AffineTransform>();

  const worldForBone = (bone: Bone, visiting: Set<string>): AffineTransform => {
    if (visiting.has(bone.id)) {
      throw new Error(`bone cycle detected: ${bone.id}`);
    }
    const cached = cache.get(bone.id);
    if (cached) return cached;
    const parent = bone.parent ? boneById.get(bone.parent) : undefined;
    const parentWorld = parent
      ? worldForBone(parent, new Set(visiting).add(bone.id))
      : rootTransform;
    const world = applyBoneTransform(bone, poses.get(bone.id) ?? {}, parentWorld);
    cache.set(bone.id, world);
    return world;
  };

  const rigged = parts.map((part) => {
    const size = textureSizes.get(part.role);
    if (!size || size.width <= 0 || size.height <= 0) {
      throw new Error(`missing texture size for part role: ${part.role}`);
    }
    const bone = part.boneId ? boneById.get(part.boneId) : undefined;
    const boneWorld = bone ? worldForBone(bone, new Set()) : rootTransform;
    const pivotPx = {
      x: part.pivot.x * size.width,
      y: part.pivot.y * size.height,
    };
    return {
      role: part.role,
      zIndex: part.zIndex,
      deformable: part.deformable,
      boneId: part.boneId,
      transform: composeTransforms(
        boneWorld,
        translateTransform(-pivotPx.x, -pivotPx.y),
      ),
      pivotWorld: { x: boneWorld.tx, y: boneWorld.ty },
    };
  });

  return rigged.sort((a, b) => a.zIndex - b.zIndex);
}
