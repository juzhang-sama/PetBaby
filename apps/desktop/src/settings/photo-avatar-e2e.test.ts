import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { createCreationApi, type InvokePort } from "../creation/api";

describe("photo avatar fake acceptance wiring", () => {
  it("forwards two photos and every v5 preview coordinate through the Tauri API", async () => {
    const invoke = vi.fn(async <T>(_command: string, _args?: Record<string, unknown>) => ({} as T)) as unknown as InvokePort;
    const api = createCreationApi(invoke);
    const photos = [
      { bytesB64: "ZmFrZS1mYWNl", sha256: "face-sha256" },
      { bytesB64: "ZmFrZS1ib2R5", sha256: "body-sha256" },
    ];

    await api.photoAvatarBegin("session-fake", "photo-avatar-consent-v1", photos);
    await api.photoAvatarPreviewManifest("session-fake", 2);
    await api.photoAvatarPreviewFileB64("session-fake", 2, "textures/atlas.png");
    await api.photoAvatarRuntimeCheckPassed("session-fake", 2, "manifest-sha256");

    expect(invoke).toHaveBeenNthCalledWith(1, "creation_photo_avatar_begin", {
      sessionId: "session-fake",
      consentVersion: "photo-avatar-consent-v1",
      photos,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "creation_photo_avatar_preview_manifest", {
      sessionId: "session-fake",
      revision: 2,
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "creation_photo_avatar_preview_file_b64", {
      sessionId: "session-fake",
      revision: 2,
      relativePath: "textures/atlas.png",
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "creation_photo_avatar_runtime_check_passed", {
      sessionId: "session-fake",
      revision: 2,
      manifestSha256: "manifest-sha256",
    });
  });

  // 归档（2026-08-20）：照片分身运行时验收.html 属于 Live2D 技术路线
  // （mountPhotoAvatarPreview 为 Live2D 预览入口，前端已不引用）。
  // Live2D 回归时恢复：移除 .skip。详见 docs/Live2D休眠资产清单.md。
  it.skip("mounts the browser fixture through the photo avatar preview entrypoint", () => {
    const source = readFileSync(
      resolve(process.cwd(), "照片分身运行时验收.html"),
      "utf8",
    );
    const recorder = readFileSync(
      resolve(process.cwd(), "../../scripts/录制照片分身动作证据.ps1"),
      "utf8",
    );

    expect(source).toContain("mountPhotoAvatarPreview");
    expect(source).toContain("renderCatMotionEvidencePhase");
    expect(source).toContain("rawManifestBytes");
    expect(source).toContain("window.photoAvatarEvidence");
    expect(source).toContain("renderedPixelCount: photoAvatarRenderedPixelCount");
    expect(source).toContain("renderedFrame: photoAvatarRenderedFrame");
    expect(source).toContain("frameSha256: photoAvatarFrameSha256");
    expect(source).toContain("photoAvatarMotionEvidence");
    expect(source).toContain("prefersReducedMotion: () => true");
    expect(source).not.toContain("JSON.stringify(manifest)");
    expect(source).not.toContain("cat-a-standard-v1/manifest.json");
    expect(source).not.toContain("createPetRendererRuntime");
    expect(recorder).toContain("CAT_BODY_MODULE_IDS");
    expect(recorder).toContain("body-slender-v1");
    expect(recorder).toContain("body-balanced-v1");
    expect(recorder).toContain("body-rounded-v1");
    expect(recorder).toContain("interrupted-pet");
    expect(recorder).toContain("interrupted-drag");
    expect(recorder).toContain("evidenceFrozen");
    expect(recorder).not.toContain("dataset.motion='$motion'");
  });
});
