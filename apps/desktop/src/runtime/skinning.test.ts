import { describe, expect, it } from "vitest";
import { buildGrid, heuristicFeatures, type FeatureRects, type GridData } from "./mesh-rig";
import {
  buildSkinBones,
  computeBoneWeights,
  deformSkinnedGrid,
  meshParamsToPoses,
  type BoneId,
} from "./skinning";

const SUBJECT = { x: 100, y: 80, width: 400, height: 500 };
const SIZE = { width: 600, height: 700 };

function makeGrid(cols = 8, rows = 8): GridData {
  return buildGrid(SIZE.width, SIZE.height, cols, rows);
}

function nearestIndex(positions: Float32Array, x: number, y: number): number {
  let best = 0;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (let i = 0; i < positions.length / 2; i += 1) {
    const dx = positions[i * 2]! - x;
    const dy = positions[i * 2 + 1]! - y;
    const distance = dx * dx + dy * dy;
    if (distance < bestDistance) {
      bestDistance = distance;
      best = i;
    }
  }
  return best;
}

describe("buildSkinBones", () => {
  const features = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);
  const bones = buildSkinBones(features, SUBJECT);

  it("creates the seven motion bones", () => {
    const ids = bones.map((bone) => bone.id).sort();
    expect(ids).toEqual([
      "head",
      "leftEar",
      "leftEye",
      "rightEar",
      "rightEye",
      "root",
      "tail",
    ]);
  });

  it("places the ear joints at the ear bases", () => {
    const left = bones.find((bone) => bone.id === "leftEar")!;
    expect(left.joint.x).toBeCloseTo(features.leftEar.x + features.leftEar.width / 2, 1);
    expect(left.joint.y).toBeCloseTo(features.leftEar.y + features.leftEar.height, 1);
  });

  it("places the eye joints at the eye centers", () => {
    const left = bones.find((bone) => bone.id === "leftEye")!;
    expect(left.joint.x).toBeCloseTo(features.leftEye.x + features.leftEye.width / 2, 1);
    expect(left.joint.y).toBeCloseTo(features.leftEye.y + features.leftEye.height / 2, 1);
  });

  it("places the head joint in the upper part of the subject", () => {
    const head = bones.find((bone) => bone.id === "head")!;
    expect(head.joint.x).toBeCloseTo(SUBJECT.x + SUBJECT.width / 2, 1);
    expect(head.joint.y).toBeLessThan(SUBJECT.y + SUBJECT.height * 0.25);
  });

  it("places the tail joint inside the tail rect", () => {
    const tail = bones.find((bone) => bone.id === "tail")!;
    expect(tail.joint.x).toBeGreaterThanOrEqual(features.tail.x);
    expect(tail.joint.x).toBeLessThanOrEqual(features.tail.x + features.tail.width);
    expect(tail.joint.y).toBeGreaterThanOrEqual(features.tail.y);
    expect(tail.joint.y).toBeLessThanOrEqual(features.tail.y + features.tail.height);
  });
});

describe("computeBoneWeights", () => {
  const features = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);
  const bones = buildSkinBones(features, SUBJECT);
  const grid = makeGrid(100, 100);
  const weights = computeBoneWeights(grid.positions, bones);

  it("produces one weight per vertex per bone", () => {
    expect(weights).toHaveLength(grid.positions.length / 2 * bones.length);
  });

  it("normalizes every vertex to a total weight of one", () => {
    for (let v = 0; v < grid.positions.length / 2; v += 1) {
      let total = 0;
      for (let b = 0; b < bones.length; b += 1) {
        total += weights[v * bones.length + b]!;
      }
      expect(total).toBeCloseTo(1, 5);
    }
  });

  it("gives every vertex at least a root influence", () => {
    const rootIndex = bones.findIndex((bone) => bone.id === "root");
    for (let v = 0; v < grid.positions.length / 2; v += 1) {
      expect(weights[v * bones.length + rootIndex]!).toBeGreaterThan(0);
    }
  });

  it("weights vertices near the head joint toward the head bone", () => {
    const headIndex = bones.findIndex((bone) => bone.id === "head");
    const head = bones[headIndex]!;
    const vertex = nearestIndex(grid.positions, head.joint.x, head.joint.y);
    expect(weights[vertex * bones.length + headIndex]!).toBeGreaterThan(0.3);
  });

  it("weights the left ear region more strongly to the left ear bone", () => {
    const leftIndex = bones.findIndex((bone) => bone.id === "leftEar");
    const rightIndex = bones.findIndex((bone) => bone.id === "rightEar");
    const left = bones[leftIndex]!;
    const right = bones[rightIndex]!;
    const nearLeft = nearestIndex(grid.positions, left.joint.x, left.joint.y);
    const nearRight = nearestIndex(grid.positions, right.joint.x, right.joint.y);
    expect(weights[nearLeft * bones.length + leftIndex]!)
      .toBeGreaterThan(weights[nearRight * bones.length + leftIndex]!);
    expect(weights[nearRight * bones.length + rightIndex]!)
      .toBeGreaterThan(weights[nearLeft * bones.length + rightIndex]!);
  });
});

describe("deformSkinnedGrid", () => {
  const features: FeatureRects = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);
  const bones = buildSkinBones(features, SUBJECT);
  const grid = makeGrid();
  const rig = {
    bones,
    weights: computeBoneWeights(grid.positions, bones),
    boneCount: bones.length,
    vertexCount: grid.positions.length / 2,
  };
  const eyeVertex = nearestIndex(
    grid.positions,
    features.leftEye.x + features.leftEye.width / 2,
    features.leftEye.y + features.leftEye.height / 2,
  );

  it("returns the base positions for identity poses", () => {
    const out = deformSkinnedGrid(grid, rig, new Map(), grid.positions);
    expect(out.length).toBe(grid.positions.length);
    for (let i = 0; i < out.length; i += 1) {
      expect(out[i]!).toBeCloseTo(grid.positions[i]!, 3);
    }
  });

  it("squashes the eye region toward the eye center during a blink", () => {
    const poses = new Map<BoneId, { scaleY: number }>([
      ["leftEye", { scaleY: 0.2 }],
      ["rightEye", { scaleY: 0.2 }],
    ]);
    const out = deformSkinnedGrid(grid, rig, poses, grid.positions);
    const eyeCenterY = features.leftEye.y + features.leftEye.height / 2;
    const baseY = grid.positions[eyeVertex * 2 + 1]!;
    const outY = out[eyeVertex * 2 + 1]!;
    expect(outY).toBeLessThan(baseY);
    expect(Math.abs(outY - eyeCenterY)).toBeLessThan(Math.abs(baseY - eyeCenterY));
  });

  it("rotates the ear vertices around their ear bases", () => {
    const leftEar = bones.find((bone) => bone.id === "leftEar")!;
    const vertex = nearestIndex(grid.positions, leftEar.joint.x, leftEar.joint.y);
    const poses = new Map<BoneId, { rotation: number }>([["leftEar", { rotation: 0.3 }]]);
    const out = deformSkinnedGrid(grid, rig, poses, grid.positions);
    const moved = out[vertex * 2] !== grid.positions[vertex * 2]
      || out[vertex * 2 + 1] !== grid.positions[vertex * 2 + 1];
    expect(moved).toBe(true);
  });

  it("sways the tail vertices around the tail base", () => {
    const tail = bones.find((bone) => bone.id === "tail")!;
    const vertex = nearestIndex(grid.positions, tail.joint.x + 10, tail.joint.y + 20);
    const poses = new Map<BoneId, { rotation: number }>([["tail", { rotation: 0.3 }]]);
    const out = deformSkinnedGrid(grid, rig, poses, grid.positions);
    expect(out[vertex * 2]).not.toBe(grid.positions[vertex * 2]);
  });

  it("turns the head region sideways for a head turn", () => {
    const head = bones.find((bone) => bone.id === "head")!;
    const vertex = nearestIndex(grid.positions, head.joint.x - 30, head.joint.y);
    const poses = new Map<BoneId, { rotation: number }>([["head", { rotation: 0.2 }]]);
    const out = deformSkinnedGrid(grid, rig, poses, grid.positions);
    expect(out[vertex * 2]).not.toBe(grid.positions[vertex * 2]);
  });
});

describe("meshParamsToPoses", () => {
  const features = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);

  it("maps blink to a downward scale on both eyes", () => {
    const poses = meshParamsToPoses({ blink: 1, earWobble: 0, tailSway: 0, headTurn: 0 }, features);
    expect(poses.get("leftEye")?.scaleY).toBeLessThan(1);
    expect(poses.get("rightEye")?.scaleY).toBeLessThan(1);
  });

  it("maps headTurn to a head rotation", () => {
    const poses = meshParamsToPoses({ blink: 0, earWobble: 0, tailSway: 0, headTurn: 1 }, features);
    expect(poses.get("head")?.rotation).toBeGreaterThan(0);
  });

  it("maps tailSway to a tail rotation", () => {
    const poses = meshParamsToPoses(
      { blink: 0, earWobble: 0, tailSway: Math.PI / 2, headTurn: 0 },
      features,
    );
    expect(poses.get("tail")?.rotation).toBeGreaterThan(0);
  });

  it("rotates the ears in opposite directions for a wobble", () => {
    const poses = meshParamsToPoses({ blink: 0, earWobble: 1, tailSway: 0, headTurn: 0 }, features);
    const left = poses.get("leftEar")?.rotation ?? 0;
    const right = poses.get("rightEar")?.rotation ?? 0;
    expect(left).toBeGreaterThan(0);
    expect(right).toBeLessThan(0);
  });
});
