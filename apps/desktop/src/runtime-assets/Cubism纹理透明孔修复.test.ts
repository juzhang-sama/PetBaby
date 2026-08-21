import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const roots: string[] = [];
const script = resolve(import.meta.dirname, "../../../../scripts/修复Cubism纹理透明孔.py");

afterEach(() => {
  while (roots.length) rmSync(roots.pop()!, { recursive: true, force: true });
});

// 归档（2026-08-20）：Live2D/Cubism 技术路线休眠，测试依赖 cv2/OpenCV
// 而测试环境默认 python 未安装。Live2D 回归时恢复：移除 .skip 并使用
// 装有 cv2 的解释器（如 D:\DevTools\Python312）。详见 docs/Live2D休眠资产清单.md。
describe.skip("Cubism 纹理透明孔修复", () => {
  it("只填封闭透明孔并保留连通画布边缘的透明背景", () => {
    const root = mkdtempSync(join(tmpdir(), "cubism-texture-hole-"));
    roots.push(root);
    const path = join(root, "texture.png");
    execFileSync(
      "python",
      [
        "-c",
        "from PIL import Image; import sys; p=sys.argv[1]; i=Image.new('RGBA',(9,9)); px=i.load(); [px.__setitem__((x,y),(230,140,50,255)) for y in range(2,7) for x in range(2,7)]; px[4,4]=(0,0,0,0); i.save(p)",
        path,
      ],
      { encoding: "utf8", stdio: "pipe" },
    );

    execFileSync("python", [script, path], { encoding: "utf8", stdio: "pipe" });

    const alpha = execFileSync(
      "python",
      [
        "-c",
        "from PIL import Image; import sys; i=Image.open(sys.argv[1]).convert('RGBA'); print(i.getpixel((4,4))[3], i.getpixel((0,0))[3])",
        path,
      ],
      { encoding: "utf8", stdio: "pipe" },
    ).trim();
    expect(alpha).toBe("255 0");
  });

  it("只在明确指定的 UV 采样邻域补齐网格透明切口", () => {
    const root = mkdtempSync(join(tmpdir(), "cubism-texture-sample-"));
    roots.push(root);
    const path = join(root, "texture.png");
    execFileSync(
      "python",
      [
        "-c",
        "from PIL import Image; import sys; p=sys.argv[1]; i=Image.new('RGBA',(21,21)); px=i.load(); [px.__setitem__((x,y),(230,140,50,255)) for y in range(5,16) for x in range(5,10)]; i.save(p)",
        path,
      ],
      { encoding: "utf8", stdio: "pipe" },
    );

    execFileSync(
      "python",
      [script, path, "--sample", "11,10,3"],
      { encoding: "utf8", stdio: "pipe" },
    );

    const alpha = execFileSync(
      "python",
      [
        "-c",
        "from PIL import Image; import sys; i=Image.open(sys.argv[1]).convert('RGBA'); print(i.getpixel((11,10))[3], i.getpixel((13,10))[3], i.getpixel((20,20))[3])",
        path,
      ],
      { encoding: "utf8", stdio: "pipe" },
    ).trim();
    expect(alpha).toBe("255 255 0");
  });
});
