export function assertVisiblePixels(pixels: Uint8ClampedArray): void {
  for (let index = 3; index < pixels.length; index += 4) {
    if ((pixels[index] ?? 0) > 0) return;
  }
  throw new Error("blank-frame");
}

export function assertVisibleFrame(surface: HTMLCanvasElement): void {
  const scratch = document.createElement("canvas");
  scratch.width = Math.max(1, surface.width);
  scratch.height = Math.max(1, surface.height);
  const context = scratch.getContext("2d", { willReadFrequently: true });
  if (!context) throw new Error("frame-probe-unavailable");
  context.drawImage(surface, 0, 0);
  assertVisiblePixels(context.getImageData(0, 0, scratch.width, scratch.height).data);
}
