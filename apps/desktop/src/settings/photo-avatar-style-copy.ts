import type { PhotoAvatarRoute } from "../creation/api";

/**
 * 「这条会话是什么画风」的人话。
 *
 * ⚠️ **画风是独立维度，但它的载体在两条产线上不同**：
 * - 像素风的画风挂在 profile 的 `styleProfileId` 上；
 * - 写实风**没有画风档位**（服务侧那一列是 NULL），它的画风就等于 **route 本身**。
 *
 * 所以这里两个入参各管一半：`route` 优先（它更外层），认不出来再退回 `styleProfileId`。
 */
export function photoAvatarStyleCopy(
  styleProfileId: unknown,
  route?: PhotoAvatarRoute | string | null,
): string {
  if (route === "frame-video-v1") return "风格：写实照片分身（逐帧循环）";
  if (styleProfileId === "pixel-style-v2-animation-ready") {
    return "风格：动画优先简约像素形象";
  }
  if (styleProfileId === "pixel-style-v1") {
    return "风格：PetBaby 高细节像素形象（历史）";
  }
  return "";
}

/** 生成中的进度文案。两条产线的步骤量级差很多，不该共用一句「正在生成」。 */
export function photoAvatarProgressCopy(
  step: string,
  route?: PhotoAvatarRoute | string | null,
): string {
  if (route === "frame-video-v1") {
    if (step === "generateMotionSource") {
      // 这一步要花钱（约 5.7 算力）而且要一两分钟，说清楚，别让人以为卡住了。
      return "正在根据照片生成动态片段，这一步需要一两分钟，请不要关闭窗口。";
    }
    if (step === "packFrameSequence") {
      return "正在把动态片段处理成逐帧循环，大约需要两三分钟。";
    }
    return "正在生成写实照片分身，请稍候。";
  }
  return "正在生成完整像素宠物，请稍候。";
}
