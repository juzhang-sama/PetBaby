import type { PhotoAvatarSnapshot } from "../creation/api";
import { creationApi } from "../creation/api";
import { loadFrameSequenceAsset } from "../runtime/frame-sequence-asset-loader";
import { parseFrameSequenceManifest } from "../runtime/frame-sequence-manifest";
import { FrameSequenceRenderer } from "../runtime/frame-sequence-renderer";
import type {
  FrameSequenceRuntimeEvidence,
  PhotoAvatarPreviewHandle,
} from "./photo-avatar-preview-contract";
import {
  canvasPixels,
  changedPixelCount,
  decodeBase64,
  photoAvatarPreviewViewport,
  sha256Hex,
} from "./photo-avatar-pixel-preview";

/**
 * 写实风（`frame-video-v1`）的预览：把 schema 7 的整包帧序列播起来。
 *
 * ## 为什么复用 `FrameSequenceRenderer` 而不是另写一个
 *
 * 内置宠物（04/05/07）走的就是 `frame-sequence-v1`，渲染栈已经在那儿：解析、按动作
 * 解码、"帧并集轮廓"、窗口裁剪……**预览要看到的必须和装进去之后一模一样**，
 * 另写一个轻量播放器正好会在这点上骗人（预览好看、装完不一样）。
 *
 * ## 全播，不抽样（老王 2026-09-13 拍板）
 *
 * 动作**不一定在开头**：呼吸 + 眨眼 + 摇尾是焊在同一支 12 秒循环里的，
 * 截前 N 帧可能什么都没发生 —— 那样预览就失去意义了。这个包只有一个动作
 * （就是 `defaultAction`），`FrameSequenceRenderer.load()` 会**顺序解码它的全部帧**
 * 并常驻，所以「全播」不需要额外做什么，**别去优化成抽样**。
 *
 * 代价是内核里会有约 `帧数 × 宽 × 高 × 4` 的位图（288 帧 640² ≈ 470 MB），
 * 与内置宠物同一条路、同一个量级 —— 这是产品既有的取舍，不是这里引入的。
 */

/** 注入点：让这条链在没有真 IPC / 真 canvas 的地方也能测。 */
export interface FramePreviewPorts {
  api: {
    photoAvatarStatus(sessionId: string): Promise<PhotoAvatarSnapshot | null>;
    photoAvatarPreviewManifest(sessionId: string, revision: number): Promise<unknown>;
    photoAvatarPreviewFileB64(
      sessionId: string,
      revision: number,
      relativePath: string,
    ): Promise<string>;
    photoAvatarRuntimeCheckPassed(
      sessionId: string,
      revision: number,
      manifestSha256: string,
    ): Promise<PhotoAvatarSnapshot>;
  };
  createRenderer(root: HTMLElement): FrameSequenceRenderer;
  createObjectUrl(bytes: Uint8Array): string;
  revokeObjectUrl(url: string): void;
}

export function defaultFramePreviewPorts(): FramePreviewPorts {
  return {
    api: creationApi,
    createRenderer: (root) => new FrameSequenceRenderer(root),
    createObjectUrl: (bytes) =>
      URL.createObjectURL(new Blob([Uint8Array.from(bytes).buffer], { type: "image/webp" })),
    revokeObjectUrl: (url) => URL.revokeObjectURL(url),
  };
}

/** 运行自检的观察窗口：42ms/帧，700ms ≈ 16 帧，足够看出「确实在动」。 */
const MOTION_PROBE_MS = 700;
const MIN_CHANGED_PIXELS = 20;

export async function mountFrameSequencePreview(
  root: HTMLElement,
  sessionId: string,
  ports: FramePreviewPorts = defaultFramePreviewPorts(),
): Promise<PhotoAvatarPreviewHandle> {
  const snapshot = await ports.api.photoAvatarStatus(sessionId);
  if (
    snapshot === null
    || (snapshot.step !== "runtimeCheckPending" && snapshot.step !== "previewReady")
  ) {
    throw new Error("写实分身尚未进入运行检查阶段");
  }
  const revision = snapshot.revision;
  const manifest = parseFrameSequenceManifest(
    await ports.api.photoAvatarPreviewManifest(sessionId, revision),
  );

  // manifest 的哈希**必须对磁盘上的原始字节算**：后端拿这份哈希把「用户看到的预览」
  // 与「即将安装的文件」绑在一起，重建 JSON 再算等于换了一个没人验证过的约定。
  const manifestSha256 = await sha256Hex(
    decodeBase64(await ports.api.photoAvatarPreviewFileB64(sessionId, revision, "manifest.json")),
  );

  // 全播：基础帧 + 所有动作的每一帧。**这里不做抽样**，理由见模块注释。
  const framePaths = [
    ...new Set([manifest.baseImage, ...manifest.actions.flatMap((action) => action.frames)]),
  ];
  const urls = new Map<string, string>();
  let renderer: FrameSequenceRenderer | null = null;
  let destroyed = false;
  try {
    await Promise.all(
      framePaths.map(async (path) => {
        const bytes = decodeBase64(
          await ports.api.photoAvatarPreviewFileB64(sessionId, revision, path),
        );
        urls.set(path, ports.createObjectUrl(bytes));
      }),
    );
    const asset = await loadFrameSequenceAsset(manifest.petId, manifest, (_petId, path) => {
      const url = urls.get(path);
      if (url === undefined) throw new Error(`预览资源里没有这一项：${path}`);
      return url;
    });

    renderer = ports.createRenderer(root);
    const active = renderer;
    const resize = () => active.resize(photoAvatarPreviewViewport(root));
    const resizeObserver = new ResizeObserver(resize);
    try {
      resizeObserver.observe(root);
      resize();
      await active.load(asset);
      active.setVisibility(true);
      const idle = active.playMotion("idle", { loop: true, priority: 10 });

      const canvas = root.querySelector("canvas");
      if (!(canvas instanceof HTMLCanvasElement)) throw new Error("写实预览画布不可用");
      active.update(0);
      const before = canvasPixels(canvas);
      active.update(MOTION_PROBE_MS);
      const changedPixels = changedPixelCount(before, canvasPixels(canvas));
      const frameCount = asset.actions.reduce((total, action) => total + action.frameUrls.length, 0);
      if (changedPixels <= MIN_CHANGED_PIXELS) {
        throw new Error("写实预览运行检查未检测到动作变化");
      }
      const evidence: FrameSequenceRuntimeEvidence = {
        renderer: "frame-sequence-v1",
        frameCount,
        changedPixels,
        manifestSha256,
      };
      if (snapshot.step === "runtimeCheckPending") {
        await ports.api.photoAvatarRuntimeCheckPassed(sessionId, revision, manifestSha256);
      }

      let previousTime = performance.now();
      let animationFrame = 0;
      const frame = (now: number) => {
        if (destroyed) return;
        active.update(Math.min(100, Math.max(0, now - previousTime)));
        previousTime = now;
        animationFrame = window.requestAnimationFrame(frame);
      };
      animationFrame = window.requestAnimationFrame(frame);

      return {
        get evidence() {
          return evidence;
        },
        destroy: () => {
          if (destroyed) return;
          destroyed = true;
          idle.cancel();
          window.cancelAnimationFrame(animationFrame);
          resizeObserver.disconnect();
          active.destroy();
          for (const url of urls.values()) ports.revokeObjectUrl(url);
          urls.clear();
        },
      };
    } catch (error) {
      resizeObserver.disconnect();
      active.destroy();
      throw error;
    }
  } catch (error) {
    destroyed = true;
    for (const url of urls.values()) ports.revokeObjectUrl(url);
    urls.clear();
    throw error;
  }
}
