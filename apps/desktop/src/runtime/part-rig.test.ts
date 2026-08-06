import { describe, expect, it } from "vitest";
import { computePartRig } from "./part-rig";
import type { ManifestPart } from "./manifest-schema";
import { identityTransform, transformPoint, translateTransform } from "./bone";

function part(overrides: Partial<ManifestPart> & { role: string }): ManifestPart {
  return {
    relativePath: `${overrides.role}.png`,
    anchor: { x: 0.5, y: 1 },
    pivot: { x: 0.5, y: 0.5 },
    zIndex: 0,
    deformable: true,
    ...overrides,
  };
}

describe("computePartRig", () => {
  it("attaches a root part so its pivot lands on the root transform", () => {
    const rig = computePartRig(
      [part({ role: "body", pivot: { x: 0.5, y: 0.5 } })],
      new Map([["body", { width: 100, height: 100 }]]),
      [],
      translateTransform(200, 300),
    );
    expect(rig).toHaveLength(1);
    const body = rig[0]!;
    expect(transformPoint(body.transform, { x: 50, y: 50 })).toEqual({ x: 200, y: 300 });
    expect(body.pivotWorld).toEqual({ x: 200, y: 300 });
  });

  it("sorts parts by zIndex ascending", () => {
    const rig = computePartRig(
      [
        part({ role: "tail", zIndex: 3 }),
        part({ role: "head", zIndex: 1 }),
        part({ role: "body", zIndex: 0 }),
      ],
      new Map([
        ["tail", { width: 20, height: 20 }],
        ["head", { width: 40, height: 40 }],
        ["body", { width: 100, height: 100 }],
      ]),
      [],
      identityTransform(),
    );
    expect(rig.map((entry) => entry.role)).toEqual(["body", "head", "tail"]);
  });

  it("places a child part at its bone joint offset from the parent", () => {
    const bones = [
      { id: "spine", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 1 } },
      { id: "head", parent: "spine", offset: { x: 0, y: -20 }, pivot: { x: 0.5, y: 0.5 } },
    ];
    const rig = computePartRig(
      [
        part({ role: "body", pivot: { x: 0.5, y: 1 }, boneId: "spine" }),
        part({ role: "head", pivot: { x: 0.5, y: 0.5 }, boneId: "head", zIndex: 1 }),
      ],
      new Map([
        ["body", { width: 100, height: 100 }],
        ["head", { width: 50, height: 50 }],
      ]),
      bones,
      identityTransform(),
    );
    const head = rig.find((entry) => entry.role === "head")!;
    expect(transformPoint(head.transform, { x: 25, y: 25 })).toEqual({ x: 0, y: -20 });
    expect(head.pivotWorld).toEqual({ x: 0, y: -20 });
  });

  it("rotates the part around its pivot from the bone pose", () => {
    const bones = [
      { id: "root", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 0.5 } },
    ];
    const rig = computePartRig(
      [part({ role: "head", pivot: { x: 0.5, y: 0.5 }, boneId: "root" })],
      new Map([["head", { width: 100, height: 100 }]]),
      bones,
      identityTransform(),
      new Map([["root", { rotation: Math.PI / 2 }]]),
    );
    const head = rig[0]!;
    // texture point just above the pivot (0,-10) rotates to the right (+10,0)
    const rotated = transformPoint(head.transform, { x: 50, y: 40 });
    expect(rotated.x).toBeCloseTo(10, 5);
    expect(rotated.y).toBeCloseTo(0, 5);
  });

  it("throws when a part has no texture size", () => {
    expect(() => computePartRig(
      [part({ role: "body" })],
      new Map(),
      [],
      identityTransform(),
    )).toThrow(/texture size/i);
  });

  it("throws on a bone cycle", () => {
    const bones = [
      { id: "a", parent: "b", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 0.5 } },
      { id: "b", parent: "a", offset: { x: 0, y: 0 }, pivot: { x: 0.5, y: 0.5 } },
    ];
    expect(() => computePartRig(
      [part({ role: "body", boneId: "a" })],
      new Map([["body", { width: 10, height: 10 }]]),
      bones,
      identityTransform(),
    )).toThrow(/cycle/i);
  });

  it("attaches a part without boneId directly to the root transform", () => {
    const rig = computePartRig(
      [part({ role: "body", pivot: { x: 0.5, y: 0.5 } })],
      new Map([["body", { width: 10, height: 10 }]]),
      [],
      translateTransform(5, 6),
    );
    expect(transformPoint(rig[0]!.transform, { x: 5, y: 5 })).toEqual({ x: 5, y: 6 });
  });
});
