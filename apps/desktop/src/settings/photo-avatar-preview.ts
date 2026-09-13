import { creationApi } from "../creation/api";
import { mountFrameSequencePreview } from "./photo-avatar-frame-preview";
import { mountPhotoAvatarPreview as mountPixelPreview } from "./photo-avatar-pixel-preview";
import type { PhotoAvatarPreviewHandle } from "./photo-avatar-preview-contract";

/**
 * 预览入口：**按路线分派**给两条产线各自的渲染栈。
 *
 * 两条产线的预览是两回事（像素风 = 单图 + 动作档案；写实风 = schema 7 整包帧序列），
 * 但视图只应该看到一个「挂预览」的动作 —— 分派落在这一层，
 * 视图与命令层都不用知道有几条路线。
 *
 * ⚠️ 判据是 `snapshot.route`，**不是** manifest 里的 `renderer`：manifest 要先把预览
 * 文件取回来才知道长什么样，而 route 在状态里就有；拿错了会在解析那一步才炸，
 * 报错也难懂。
 */
export async function mountPhotoAvatarPreview(
  root: HTMLElement,
  sessionId: string,
): Promise<PhotoAvatarPreviewHandle> {
  const snapshot = await creationApi.photoAvatarStatus(sessionId);
  if (snapshot?.route === "frame-video-v1") {
    return mountFrameSequencePreview(root, sessionId);
  }
  return mountPixelPreview(root, sessionId);
}
