import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { parseMotionSpatialProfileV1 } from "./cat-motion-spatial-profile";
import { parseCatSpatialManifest } from "./cat-spatial-manifest";

const roots: string[] = [];
const script = resolve(import.meta.dirname, "../../../../scripts/构建标准猫角色包.ps1");
const balancedModuleContract = JSON.parse(readFileSync(resolve(
  import.meta.dirname,
  "../../public/cat-character-modules/cat-a-live2d-v1/body-balanced-v1/模块.json",
), "utf8"));
const sha = "00".repeat(32);
const motions = [
  "breathing",
  "blink",
  "ear-twitch",
  "tail-idle",
  "pointer-focus",
  "pet-happy",
  "sleepy-yawn",
  "half-stand-stretch",
] as const;
const parameters = [
  "ParamEyeLOpen",
  "ParamEyeROpen",
  "ParamEyeBallX",
  "ParamEyeBallY",
  "ParamEarL",
  "ParamEarR",
  "ParamTailAngle",
  "ParamTailCurl",
  "ParamTailTip",
  "ParamBreath",
  "ParamBodyStretch",
  "ParamMouthOpenY",
] as const;

afterEach(() => {
  while (roots.length) rmSync(roots.pop()!, { recursive: true, force: true });
});

function fixture(): { root: string; source: string; output: string } {
  const root = mkdtempSync(join(tmpdir(), "standard-cat-package-"));
  roots.push(root);
  const source = join(root, "export");
  const output = join(root, "builtin");
  mkdirSync(join(source, "textures"), { recursive: true });
  mkdirSync(join(source, "motions"), { recursive: true });
  writeFileSync(join(source, "cat-a-standard-v1.moc3"), "moc");
  execFileSync(
    "python",
    [
      "-c",
      "from PIL import Image; import sys; i=Image.new('RGBA',(2048,2048)); p=i.load(); [p.__setitem__((x,y),(230,140,50,255)) for y in range(90,121) for x in range(1415,1431)]; i.save(sys.argv[1])",
      join(source, "textures", "texture_00.png"),
    ],
    { encoding: "utf8", stdio: "pipe" },
  );
  writeFileSync(join(source, "preview.png"), "preview");
  writeFileSync(join(source, "cat-a-standard-v1.physics3.json"), "{}");
  writeFileSync(
    join(source, "cat-a-standard-v1.cdi3.json"),
    JSON.stringify({ Parameters: parameters.map((Id) => ({ Id })) }),
  );
  const fileMotions: Record<string, { File: string }[]> = {};
  for (const name of motions) {
    const relative = `motions/${name}.motion3.json`;
    const motion = name === "breathing"
      ? {
          Curves: [
            { Target: "Parameter", Id: "ParamBreath", Segments: [0, 0.15, 0, 2, 1, 0, 4, 0.15] },
            { Target: "Parameter", Id: "ParamBodyStretch", Segments: [0, 0, 0, 2, 0.08, 0, 4, 0] },
          ],
        }
      : name === "ear-twitch"
        ? {
            Curves: [
              { Target: "Parameter", Id: "ParamEarL", Segments: [0, 0, 0, 0.18, -0.75, 0, 0.42, 0.35, 0, 0.8, 0] },
              { Target: "Parameter", Id: "ParamEarR", Segments: [0, 0, 0, 0.22, 0.65, 0, 0.48, -0.25, 0, 0.8, 0] },
            ],
          }
        : {};
    writeFileSync(join(source, ...relative.split("/")), JSON.stringify(motion));
    fileMotions[name] = [{ File: relative }];
  }
  for (const edge of ["left", "right", "top", "bottom"] as const) {
    const relative = `motions/edge-tail-${edge}.motion3.json`;
    writeFileSync(join(source, ...relative.split("/")), "{}");
    fileMotions[`edge-tail-${edge}`] = [{ File: relative }];
  }
  writeFileSync(
    join(source, "cat-a-standard-v1.model3.json"),
    JSON.stringify({
      Version: 3,
      FileReferences: {
        Moc: "cat-a-standard-v1.moc3",
        Textures: ["textures/texture_00.png"],
        Physics: "cat-a-standard-v1.physics3.json",
        DisplayInfo: "cat-a-standard-v1.cdi3.json",
        Motions: fileMotions,
      },
      HitAreas: [
        { Name: "body", Id: "ArtMeshBody" },
        { Name: "edgeTail", Id: "ArtMeshTail" },
      ],
    }),
  );
  return { root, source, output };
}

function build(source: string, output: string): void {
  execFileSync(
    "powershell",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      script,
      "-SourceDir",
      source,
      "-OutputDir",
      output,
    ],
    { encoding: "utf8", stdio: "pipe" },
  );
}

describe("standard cat v5 package builder", () => {
  // 归档（2026-08-20）：以下 3 个用例走完整构建管线，依赖
  // 修复Cubism纹理透明孔.py（需要 cv2/OpenCV），测试环境默认 python 未安装。
  // Live2D 回归时恢复：移除 .skip 并使用装有 cv2 的解释器。详见 docs/Live2D休眠资产清单.md。
  it.skip("builds from the documented Cubism export directory when SourceDir is omitted", () => {
    const root = mkdtempSync(join(tmpdir(), "standard-cat-default-source-"));
    roots.push(root);
    const output = join(root, "builtin");

    execFileSync(
      "powershell",
      [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script,
        "-OutputDir",
        output,
      ],
      { encoding: "utf8", stdio: "pipe" },
    );

    expect(JSON.parse(readFileSync(join(output, "manifest.json"), "utf8"))).toMatchObject({
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a-standard-v1",
      bodyModuleId: "body-balanced-v1",
      motionSpatialProfile: "motion-spatial-profile.json",
    });
  }, 15_000);

  it.skip("builds a spatial package with a hashed, geometrically valid balanced profile", () => {
    const test = fixture();
    build(test.source, test.output);
    const manifest = parseCatSpatialManifest(
      JSON.parse(readFileSync(join(test.output, "manifest.json"), "utf8")),
    );
    const profileBytes = readFileSync(join(test.output, manifest.motionSpatialProfile));
    const profile = parseMotionSpatialProfileV1(JSON.parse(profileBytes.toString("utf8")));
    const profileEntry = manifest.files.find(
      (file) => file.role === "motion-spatial-profile",
    );

    expect(manifest).toMatchObject({
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a-standard-v1",
      bodyModuleId: "body-balanced-v1",
    });
    expect(profile.bodyModuleId).toBe("body-balanced-v1");
    expect(profileEntry).toMatchObject({
      relativePath: "motion-spatial-profile.json",
      sha256: createHash("sha256").update(profileBytes).digest("hex"),
    });
    expect(manifest.files.find((file) => file.role === "moc")?.sha256).toBe(
      balancedModuleContract.hashes.moc3,
    );
    expect(Object.keys(manifest.motions)).toHaveLength(8);
    expect(Object.keys(manifest.edgeTailStates)).toHaveLength(4);
    expect(manifest.license).toEqual({
      id: "cat-a-standard-v1-project-owned",
      author: "PetBaby",
      source: "Project-owned standard cat artwork and Cubism binding",
      commercialUse: true,
      redistributable: true,
    });
    expect(manifest.edgeTailStates.left.tailArtMesh).toBe("ArtMeshTail");
    expect(manifest.files.every((file) => file.sha256 !== sha)).toBe(true);

    const breathing = JSON.parse(readFileSync(
      join(test.output, "motions", "breathing.motion3.json"),
      "utf8",
    ));
    expect(breathing.Curves).toEqual([
      { Target: "Parameter", Id: "ParamBreath", Segments: [0, 0.15, 0, 2, 0.15, 0, 4, 0.15] },
      { Target: "Parameter", Id: "ParamBodyStretch", Segments: [0, 0, 0, 2, 0.9, 0, 4, 0] },
    ]);
    const earMotion = JSON.parse(readFileSync(
      join(test.output, "motions", "ear-twitch.motion3.json"),
      "utf8",
    ));
    expect(earMotion.Curves).toEqual([
      { Target: "Parameter", Id: "ParamEarL", Segments: [0, 0, 0, 0.18, -0.15, 0, 0.42, 0.15, 0, 0.8, 0] },
      { Target: "Parameter", Id: "ParamEarR", Segments: [0, 0, 0, 0.22, 0.15, 0, 0.48, -0.15, 0, 0.8, 0] },
    ]);
  });

  it.skip("repairs diagnosed Cubism UV cutouts in staging before hashing", () => {
    const test = fixture();
    build(test.source, test.output);
    const manifest = JSON.parse(readFileSync(join(test.output, "manifest.json"), "utf8"));
    const texturePath = manifest.files.find(
      (file: { role: string; relativePath: string }) => file.role === "texture",
    ).relativePath;
    const alpha = execFileSync(
      "python",
      [
        "-c",
        "from PIL import Image; import sys; i=Image.open(sys.argv[1]).convert('RGBA'); print(i.getpixel((1436,105))[3])",
        join(test.output, ...texturePath.split("/")),
      ],
      { encoding: "utf8", stdio: "pipe" },
    ).trim();
    expect(alpha).toBe("255");
  });

  it("rejects a missing motion without replacing the current package", () => {
    const test = fixture();
    mkdirSync(test.output);
    writeFileSync(join(test.output, "current.txt"), "keep-me");
    rmSync(join(test.source, "motions", "tail-idle.motion3.json"));
    expect(() => build(test.source, test.output)).toThrow();
    expect(readFileSync(join(test.output, "current.txt"), "utf8")).toBe("keep-me");
    expect(() => readFileSync(join(test.output, "manifest.json"))).toThrow();
  });

  it("rejects a model reference outside the export directory", () => {
    const test = fixture();
    const modelPath = join(test.source, "cat-a-standard-v1.model3.json");
    const model = JSON.parse(readFileSync(modelPath, "utf8"));
    model.FileReferences.Physics = "../escape.physics3.json";
    writeFileSync(modelPath, JSON.stringify(model));
    expect(() => build(test.source, test.output)).toThrow();
    expect(() => readFileSync(join(test.output, "manifest.json"))).toThrow();
  });

});
