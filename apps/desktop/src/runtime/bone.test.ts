import { describe, expect, it } from "vitest";
import {
  applyBoneTransform,
  composeTransforms,
  identityTransform,
  rotateTransform,
  scaleTransform,
  transformPoint,
  translateTransform,
} from "./bone";

describe("bone affine transforms", () => {
  it("identity maps points unchanged", () => {
    const world = identityTransform();
    expect(transformPoint(world, { x: 3, y: 4 })).toEqual({ x: 3, y: 4 });
  });

  it("root bone places its joint at the offset and rotates around it", () => {
    const bone = { id: "root", offset: { x: 10, y: 20 }, pivot: { x: 0.5, y: 0.5 } };
    const world = applyBoneTransform(bone, { rotation: Math.PI / 2 }, null);
    expect(transformPoint(world, { x: 0, y: 0 })).toEqual({ x: 10, y: 20 });
    // screen y grows downward: rotating (1,0) by +90° gives (0,1)
    const rotated = transformPoint(world, { x: 1, y: 0 });
    expect(rotated.x).toBeCloseTo(10, 5);
    expect(rotated.y).toBeCloseTo(21, 5);
  });

  it("child bone inherits the parent rotation", () => {
    const root = { id: "root", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 0.5 } };
    const head = {
      id: "head",
      parent: "root",
      offset: { x: 0, y: -50 },
      pivot: { x: 0.5, y: 0.5 },
    };
    const rootWorld = applyBoneTransform(root, { rotation: Math.PI / 2 }, null);
    const headWorld = applyBoneTransform(head, {}, rootWorld);
    // the parent's rotated frame maps the child offset (0,-50) to (50,0)
    const joint = transformPoint(headWorld, { x: 0, y: 0 });
    expect(joint.x).toBeCloseTo(50, 5);
    expect(joint.y).toBeCloseTo(0, 5);
  });

  it("parent scale propagates to the child offset", () => {
    const root = { id: "root", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 0.5 } };
    const head = {
      id: "head",
      parent: "root",
      offset: { x: 0, y: -50 },
      pivot: { x: 0.5, y: 0.5 },
    };
    const rootWorld = applyBoneTransform(root, { scaleX: 2, scaleY: 2 }, null);
    const headWorld = applyBoneTransform(head, {}, rootWorld);
    expect(transformPoint(headWorld, { x: 0, y: 0 })).toEqual({ x: 0, y: -100 });
  });

  it("pose scale applies before the joint offset", () => {
    const bone = { id: "root", offset: { x: 10, y: 0 }, pivot: { x: 0.5, y: 0.5 } };
    const world = applyBoneTransform(bone, { scaleX: 2, scaleY: 3 }, null);
    const point = transformPoint(world, { x: 1, y: 1 });
    expect(point.x).toBeCloseTo(12, 5);
    expect(point.y).toBeCloseTo(3, 5);
  });

  it("composes translate and rotate helpers like a matrix chain", () => {
    const world = composeTransforms(translateTransform(10, 20), rotateTransform(Math.PI / 2));
    const point = transformPoint(world, { x: 1, y: 0 });
    expect(point.x).toBeCloseTo(10, 5);
    expect(point.y).toBeCloseTo(21, 5);
  });

  it("scale helper scales before the parent translation", () => {
    const world = composeTransforms(translateTransform(10, 20), scaleTransform(2, 3));
    const point = transformPoint(world, { x: 1, y: 1 });
    expect(point.x).toBeCloseTo(12, 5);
    expect(point.y).toBeCloseTo(23, 5);
  });
});
