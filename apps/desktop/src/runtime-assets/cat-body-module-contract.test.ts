import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  BODY_MODULE_IDS_V1,
  parseMotionSpatialProfileV1,
  type BodyModuleIdV1,
} from "./cat-motion-spatial-profile";

const moduleRoot = resolve(
  import.meta.dirname,
  "../../public/cat-character-modules/cat-a-live2d-v1",
);
const validator = resolve(import.meta.dirname, "../../../../scripts/验证猫咪形体模块.ps1");
const temporaryRoots: string[] = [];
const expectedCompatibility = {
  face: ["face-standard-v1"],
  ears: ["ears-independent-v1"],
  eyes: ["eyes-independent-v1"],
  tail: ["tail-independent-v1"],
};
const requiredParameters = [
  "ParamEyeLOpen",
  "ParamEyeROpen",
  "ParamEarL",
  "ParamEarR",
  "ParamTailAngle",
  "ParamTailCurl",
  "ParamTailTip",
  "ParamBreath",
  "ParamBodyStretch",
] as const;
const requiredMotions = [
  "breathing",
  "blink",
  "ear-twitch",
  "tail-idle",
  "pointer-focus",
  "pet-happy",
  "sleepy-yawn",
  "half-stand-stretch",
  "edge-tail-left",
  "edge-tail-right",
  "edge-tail-top",
  "edge-tail-bottom",
] as const;

afterEach(() => {
  while (temporaryRoots.length > 0) {
    rmSync(temporaryRoots.pop()!, { recursive: true, force: true });
  }
});

function json(path: string): Record<string, unknown> {
  return JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
}

function record(value: unknown, label: string): Record<string, unknown> {
  expect(value, label).toBeTypeOf("object");
  expect(value, label).not.toBeNull();
  expect(Array.isArray(value), label).toBe(false);
  return value as Record<string, unknown>;
}

function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function runValidator(root: string): string {
  try {
    return execFileSync(
      "powershell",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", validator, "-Root", root],
      { encoding: "utf8", stdio: "pipe" },
    );
  } catch (error) {
    const failure = error as { stdout?: string; stderr?: string };
    return `${failure.stdout ?? ""}\n${failure.stderr ?? ""}`;
  }
}

function writeJson(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function malformedFixture(): string {
  const root = mkdtempSync(join(tmpdir(), "cat-body-modules-red-"));
  temporaryRoots.push(root);
  writeJson(join(root, "模块合同.json"), {
    schemaVersion: 1,
    semanticVersion: "cat-a-live2d-v1",
    readOnly: true,
    moduleIds: BODY_MODULE_IDS_V1,
  });
  for (const moduleId of BODY_MODULE_IDS_V1) {
    writeJson(join(root, moduleId, "模块.json"), {
      schemaVersion: 1,
      moduleId: moduleId === "body-balanced-v1" ? "body-wrong-v1" : moduleId,
      semanticVersion: "cat-a-live2d-v1",
      readOnly: true,
      compatibleModules: expectedCompatibility,
      requiredParameters,
      tailArtMesh: "ArtMeshTail",
      files: {
        moc3: `${moduleId}.moc3`,
        model3: `${moduleId}.model3.json`,
        displayInfo: `${moduleId}.cdi3.json`,
        neutralTexture: "textures/texture_00.png",
      },
      hashes: {},
      motions: {},
      approvedAmplitude: {},
      motionSpatialProfile: {
        schemaVersion: 1,
        bodyModuleId: moduleId,
        canvas: { width: 2_048, height: 2_048 },
        alphaBounds: { left: -0.1, top: 0.04, right: 0.93, bottom: 0.96 },
      },
    });
  }
  return root;
}

describe("cat-a-live2d-v1 body module contract", () => {
  it("contains the exact three read-only pre-bound modules", () => {
    const contract = json(join(moduleRoot, "模块合同.json"));
    expect(contract).toEqual({
      schemaVersion: 1,
      semanticVersion: "cat-a-live2d-v1",
      readOnly: true,
      moduleIds: BODY_MODULE_IDS_V1,
    });
  });

  it.each(BODY_MODULE_IDS_V1)("validates the exact contract and hashes for %s", (moduleId) => {
    const manifestPath = join(moduleRoot, moduleId, "模块.json");
    const manifest = json(manifestPath);
    expect(manifest.schemaVersion).toBe(1);
    expect(manifest.moduleId).toBe(moduleId);
    expect(manifest.semanticVersion).toBe("cat-a-live2d-v1");
    expect(manifest.readOnly).toBe(true);
    expect(manifest.compatibleModules).toEqual(expectedCompatibility);
    expect(manifest.requiredParameters).toEqual(requiredParameters);
    expect(manifest.tailArtMesh).toBe("ArtMeshTail");

    const files = record(manifest.files, `${moduleId}.files`);
    const hashes = record(manifest.hashes, `${moduleId}.hashes`);
    const motions = record(manifest.motions, `${moduleId}.motions`);
    expect(Object.keys(files).sort()).toEqual([
      "displayInfo",
      "moc3",
      "model3",
      "neutralTexture",
    ]);
    expect(Object.keys(hashes).sort()).toEqual(Object.keys(files).sort());
    for (const role of Object.keys(files)) {
      const relativePath = files[role];
      expect(relativePath).toBeTypeOf("string");
      const assetPath = join(moduleRoot, moduleId, relativePath as string);
      expect(sha256(assetPath)).toBe(hashes[role]);
    }
    expect(Object.keys(motions).sort()).toEqual([...requiredMotions].sort());
    for (const motionName of requiredMotions) {
      const motion = record(motions[motionName], `${moduleId}.motions.${motionName}`);
      expect(Object.keys(motion).sort()).toEqual(["relativePath", "sha256"]);
      expect(motion.relativePath).toBe(`motions/${motionName}.motion3.json`);
      const motionPath = join(moduleRoot, moduleId, motion.relativePath as string);
      expect(sha256(motionPath)).toBe(motion.sha256);
    }

    const mocPath = join(moduleRoot, moduleId, files.moc3 as string);
    const moc = readFileSync(mocPath);
    expect(moc.byteLength).toBeGreaterThan(1_024);
    expect(moc.subarray(0, 4).toString("ascii")).toBe("MOC3");

    const model = json(join(moduleRoot, moduleId, files.model3 as string));
    const references = record(model.FileReferences, `${moduleId}.model.FileReferences`);
    expect(references.Moc).toBe(files.moc3);
    expect(references.DisplayInfo).toBe(files.displayInfo);
    expect(references.Textures).toEqual([files.neutralTexture]);
    expect(Object.keys(record(references.Motions, `${moduleId}.model.FileReferences.Motions`)).sort())
      .toEqual([...requiredMotions].sort());

    const display = json(join(moduleRoot, moduleId, files.displayInfo as string));
    const parameterIds = (display.Parameters as Array<{ Id?: string }>).map(({ Id }) => Id);
    expect(parameterIds).toEqual(expect.arrayContaining([...requiredParameters]));

    const profile = parseMotionSpatialProfileV1(manifest.motionSpatialProfile);
    expect(profile.bodyModuleId).toBe(moduleId);
    expect(manifest.approvedAmplitude).toEqual(profile.amplitude);
  });

  it("uses three independently exported Cubism binaries", () => {
    const hashes = BODY_MODULE_IDS_V1.map((moduleId) => {
      const manifest = json(join(moduleRoot, moduleId, "模块.json"));
      const files = record(manifest.files, `${moduleId}.files`);
      return sha256(join(moduleRoot, moduleId, files.moc3 as string));
    });
    expect(new Set(hashes).size).toBe(BODY_MODULE_IDS_V1.length);
  });

  it("keeps body-specific breath and stretch approvals conservative and distinct", () => {
    const profiles = Object.fromEntries(BODY_MODULE_IDS_V1.map((moduleId) => {
      const manifest = json(join(moduleRoot, moduleId, "模块.json"));
      return [moduleId, parseMotionSpatialProfileV1(manifest.motionSpatialProfile)];
    })) as Record<BodyModuleIdV1, ReturnType<typeof parseMotionSpatialProfileV1>>;

    const slenderWidth = profiles["body-slender-v1"].breathZone.right
      - profiles["body-slender-v1"].breathZone.left;
    const balancedWidth = profiles["body-balanced-v1"].breathZone.right
      - profiles["body-balanced-v1"].breathZone.left;
    const roundedWidth = profiles["body-rounded-v1"].breathZone.right
      - profiles["body-rounded-v1"].breathZone.left;
    expect(slenderWidth).toBeLessThan(balancedWidth);
    expect(balancedWidth).toBeLessThan(roundedWidth);
    expect(profiles["body-slender-v1"].amplitude.breath.max).toBeLessThan(
      profiles["body-balanced-v1"].amplitude.breath.max,
    );
    expect(profiles["body-rounded-v1"].amplitude.bodyStretch.max).toBeLessThan(
      profiles["body-balanced-v1"].amplitude.bodyStretch.max,
    );
  });

  it("lists concrete missing, wrong-module, out-of-range, and hash diagnostics", () => {
    const output = runValidator(malformedFixture());
    expect(output).toMatch(/body-slender-v1\.moc3/i);
    expect(output).toMatch(/body-balanced-v1.*body-wrong-v1/i);
    expect(output).toMatch(/alphaBounds\.left.*\[0, 1\]/i);
    expect(output).toMatch(/hash/i);
  });

  it("passes the independent PowerShell validator", () => {
    expect(runValidator(moduleRoot)).toMatch(/3\/3.*PASS/i);
  });
});
