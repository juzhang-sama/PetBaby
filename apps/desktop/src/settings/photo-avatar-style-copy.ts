export function photoAvatarStyleCopy(styleProfileId: unknown): string {
  if (styleProfileId === "pixel-style-v2-animation-ready") {
    return "风格：动画优先简约像素形象";
  }
  if (styleProfileId === "pixel-style-v1") {
    return "风格：PetBaby 高细节像素形象（历史）";
  }
  return "";
}
