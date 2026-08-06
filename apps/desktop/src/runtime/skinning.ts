import type { FeatureRects, GridData, MeshParams, Rect } from "./mesh-rig";

export type BoneId =
  | "root"
  | "head"
  | "leftEar"
  | "rightEar"
  | "tail"
  | "leftEye"
  | "rightEye";

export interface Bone {
  id: BoneId;
  joint: { x: number; y: number };
  parent?: BoneId;
  radius: number;
}

export interface BonePose {
  rotation?: number;
  scaleY?: number;
}

export interface SkinRig {
  bones: Bone[];
  weights: Float32Array;
  boneCount: number;
  vertexCount: number;
}

/**
 * Build a small skeleton from the landmark feature boxes. Joints sit at the
 * natural pivots: ear bases, eye centers, tail base and the head/body centers.
 */
export function buildSkinBones(features: FeatureRects, subject: Rect): Bone[] {
  const centerX = subject.x + subject.width / 2;
  const headHeight = subject.height * 0.34;
  const earRadius = Math.max(features.leftEar.width, features.leftEar.height) * 1.4;
  const eyeRadius = Math.max(features.leftEye.width, features.leftEye.height) * 1.5;
  const tailRadius = Math.max(features.tail.width, features.tail.height) * 1.3;
  return [
    {
      id: "root",
      joint: { x: centerX, y: subject.y + subject.height },
      radius: Math.max(subject.width, subject.height) * 0.9,
    },
    {
      id: "head",
      parent: "root",
      joint: { x: centerX, y: subject.y + headHeight * 0.5 },
      radius: headHeight * 1.1,
    },
    {
      id: "leftEar",
      parent: "head",
      joint: {
        x: features.leftEar.x + features.leftEar.width / 2,
        y: features.leftEar.y + features.leftEar.height,
      },
      radius: earRadius,
    },
    {
      id: "rightEar",
      parent: "head",
      joint: {
        x: features.rightEar.x + features.rightEar.width / 2,
        y: features.rightEar.y + features.rightEar.height,
      },
      radius: earRadius,
    },
    {
      id: "tail",
      parent: "root",
      joint: {
        x: features.tail.x + features.tail.width * 0.1,
        y: features.tail.y + features.tail.height * 0.6,
      },
      radius: tailRadius,
    },
    {
      id: "leftEye",
      parent: "head",
      joint: {
        x: features.leftEye.x + features.leftEye.width / 2,
        y: features.leftEye.y + features.leftEye.height / 2,
      },
      radius: eyeRadius,
    },
    {
      id: "rightEye",
      parent: "head",
      joint: {
        x: features.rightEye.x + features.rightEye.width / 2,
        y: features.rightEye.y + features.rightEye.height / 2,
      },
      radius: eyeRadius,
    },
  ];
}

/**
 * Per-vertex bone weights (linear blend skinning on CPU). Every vertex keeps
 * a baseline root influence so breathing/body motion affects the whole pet,
 * and each bone pulls the region around its joint.
 */
export function computeBoneWeights(positions: Float32Array, bones: Bone[]): Float32Array {
  const vertexCount = positions.length / 2;
  const boneCount = bones.length;
  const weights = new Float32Array(vertexCount * boneCount);
  const rootIndex = Math.max(0, bones.findIndex((bone) => bone.id === "root"));
  for (let v = 0; v < vertexCount; v += 1) {
    const x = positions[v * 2]!;
    const y = positions[v * 2 + 1]!;
    let total = 0;
    for (let b = 0; b < boneCount; b += 1) {
      const bone = bones[b]!;
      const dx = x - bone.joint.x;
      const dy = y - bone.joint.y;
      const distance = Math.hypot(dx, dy);
      let weight = 0;
      if (distance < bone.radius) {
        const t = 1 - distance / bone.radius;
        weight = t * t;
      }
      if (b === rootIndex) weight += 0.25;
      weights[v * boneCount + b] = weight;
      total += weight;
    }
    if (total <= 0) {
      weights[v * boneCount + rootIndex] = 1;
      total = 1;
    }
    for (let b = 0; b < boneCount; b += 1) {
      weights[v * boneCount + b] = (weights[v * boneCount + b] ?? 0) / total;
    }
  }
  return weights;
}

function applyPose(
  x: number,
  y: number,
  joint: { x: number; y: number },
  pose: BonePose,
): { x: number; y: number } {
  let px = x;
  let py = y;
  const dy = py - joint.y;
  if (pose.scaleY !== undefined) {
    py = joint.y + dy * pose.scaleY;
  }
  if (pose.rotation !== undefined) {
    const dx = px - joint.x;
    const dyRot = py - joint.y;
    const cos = Math.cos(pose.rotation);
    const sin = Math.sin(pose.rotation);
    px = joint.x + dx * cos - dyRot * sin;
    py = joint.y + dx * sin + dyRot * cos;
  }
  return { x: px, y: py };
}

/** Linear-blend skinning: each vertex is a weighted blend of bone transforms. */
export function deformSkinnedGrid(
  grid: GridData,
  rig: SkinRig,
  poses: ReadonlyMap<BoneId, BonePose>,
  basePositions?: Float32Array,
): Float32Array {
  const base = basePositions ?? grid.positions;
  const out = new Float32Array(base);
  const { bones, weights, boneCount } = rig;
  for (let v = 0; v < rig.vertexCount; v += 1) {
    const bx = base[v * 2]!;
    const by = base[v * 2 + 1]!;
    let ax = 0;
    let ay = 0;
    for (let b = 0; b < boneCount; b += 1) {
      const weight = weights[v * boneCount + b]!;
      if (weight === 0) continue;
      const bone = bones[b]!;
      const pose = poses.get(bone.id);
      const moved = pose ? applyPose(bx, by, bone.joint, pose) : { x: bx, y: by };
      ax += moved.x * weight;
      ay += moved.y * weight;
    }
    out[v * 2] = ax;
    out[v * 2 + 1] = ay;
  }
  return out;
}

/** Translate mesh animation params into per-bone poses. */
export function meshParamsToPoses(
  params: MeshParams,
  _features: FeatureRects,
): Map<BoneId, BonePose> {
  const poses = new Map<BoneId, BonePose>();
  if (params.blink > 0) {
    const scaleY = 1 - 0.82 * params.blink;
    poses.set("leftEye", { scaleY });
    poses.set("rightEye", { scaleY });
  }
  if (params.earWobble !== 0) {
    const angle = params.earWobble * 0.14;
    poses.set("leftEar", { rotation: angle });
    poses.set("rightEar", { rotation: -angle });
  }
  if (params.tailSway !== 0) {
    poses.set("tail", { rotation: Math.sin(params.tailSway) * 0.35 });
  }
  if (params.headTurn) {
    poses.set("head", { rotation: params.headTurn * 0.12 });
  }
  return poses;
}
