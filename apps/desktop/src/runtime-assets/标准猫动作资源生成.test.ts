import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const roots: string[] = [];
const script = resolve(import.meta.dirname, "../../../../scripts/生成标准猫动作资源.ps1");
const regularMotions = [
  "breathing",
  "blink",
  "ear-twitch",
  "tail-idle",
  "pointer-focus",
  "pet-happy",
  "sleepy-yawn",
  "half-stand-stretch",
] as const;
const edgeMotions = ["left", "right", "top", "bottom"].map((edge) => `edge-tail-${edge}`);

afterEach(() => {
  while (roots.length) rmSync(roots.pop()!, { recursive: true, force: true });
});

function fixture(): string {
  const root = mkdtempSync(join(tmpdir(), "standard-cat-motion-"));
  roots.push(root);
  mkdirSync(join(root, "cat-a-standard-v1.2048"), { recursive: true });
  writeFileSync(join(root, "cat-a-standard-v1.moc3"), "moc");
  writeFileSync(join(root, "cat-a-standard-v1.2048", "texture_00.png"), "texture");
  writeFileSync(join(root, "preview-source.png"), "preview");
  writeFileSync(
    join(root, "cat-a-standard-v1.model3.json"),
    JSON.stringify({
      Version: 3,
      FileReferences: {
        Moc: "cat-a-standard-v1.moc3",
        Textures: ["cat-a-standard-v1.2048/texture_00.png"],
        DisplayInfo: "cat-a-standard-v1.cdi3.json",
      },
    }),
  );
  writeFileSync(
    join(root, "cat-a-standard-v1.cdi3.json"),
    JSON.stringify({
      Version: 3,
      Parameters: [
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
      ].map((Id) => ({ Id, Name: Id })),
    }),
  );
  return root;
}

function generate(root: string): void {
  execFileSync(
    "powershell",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      script,
      "-ExportDir",
      root,
      "-PreviewSource",
      join(root, "preview-source.png"),
    ],
    { encoding: "utf8", stdio: "pipe" },
  );
}

describe("标准猫动作资源生成", () => {
  it("生成十二个带真实参数变化的 motion3 并写回 model3", () => {
    const root = fixture();
    generate(root);

    const model = JSON.parse(readFileSync(join(root, "cat-a-standard-v1.model3.json"), "utf8"));
    expect(model.HitAreas).toEqual([
      { Name: "body", Id: "ArtMeshBody" },
      { Name: "edgeTail", Id: "ArtMeshTail" },
    ]);

    for (const name of [...regularMotions, ...edgeMotions]) {
      expect(model.FileReferences.Motions[name]).toHaveLength(1);
      const relative = model.FileReferences.Motions[name][0].File as string;
      const motion = JSON.parse(readFileSync(join(root, ...relative.split("/")), "utf8"));
      expect(motion.Meta.CurveCount).toBeGreaterThan(0);
      expect(motion.Curves.length).toBe(motion.Meta.CurveCount);
      expect(
        motion.Curves.some((curve: { Segments: number[] }) => {
          const values = curve.Segments.filter((_: number, index: number) => index % 3 === 1);
          return new Set(values).size > 1;
        }),
      ).toBe(true);
    }
    const breathing = JSON.parse(readFileSync(join(root, "motions", "breathing.motion3.json"), "utf8"));
    expect(breathing.Curves.map((curve: { Id: string }) => curve.Id)).toEqual([
      "ParamBreath",
      "ParamBodyStretch",
    ]);
    const breath = breathing.Curves.find((curve: { Id: string }) => curve.Id === "ParamBreath");
    const breathValues = breath.Segments.filter((_: number, index: number) => index % 3 === 1);
    expect(breathValues).toEqual([0.15, 0.15, 0.15]);
    const bodyStretch = breathing.Curves.find((curve: { Id: string }) => curve.Id === "ParamBodyStretch");
    const stretchValues = bodyStretch.Segments.filter((_: number, index: number) => index % 3 === 1);
    expect(stretchValues).toEqual([0, 1, 0]);
    expect(Math.max(...stretchValues)).toBeLessThanOrEqual(1);
    expect(readFileSync(join(root, "preview.png"), "utf8")).toBe("preview");
  });

  it("缺少 Cubism 必需参数时拒绝生成", () => {
    const root = fixture();
    const cdiPath = join(root, "cat-a-standard-v1.cdi3.json");
    const cdi = JSON.parse(readFileSync(cdiPath, "utf8"));
    cdi.Parameters = cdi.Parameters.filter(({ Id }: { Id: string }) => Id !== "ParamTailTip");
    writeFileSync(cdiPath, JSON.stringify(cdi));
    expect(() => generate(root)).toThrow();
    expect(() => readFileSync(join(root, "motions", "tail-idle.motion3.json"))).toThrow();
  });
});
