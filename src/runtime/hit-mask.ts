import type { RegionSpan } from "./contracts";

export interface HitMaskOptions {
  alphaThreshold?: number;
  rowStep?: number;
}

export function alphaToRegionSpans(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  options: HitMaskOptions = {},
): RegionSpan[] {
  if (width <= 0 || height <= 0 || data.length !== width * height * 4) {
    throw new RangeError("RGBA data does not match width and height");
  }

  const threshold = options.alphaThreshold ?? 32;
  const rowStep = Math.max(1, Math.floor(options.rowStep ?? 2));
  const spans: RegionSpan[] = [];

  for (let top = 0; top < height; top += rowStep) {
    const bottom = Math.min(height, top + rowStep);
    let runStart: number | null = null;

    for (let x = 0; x <= width; x += 1) {
      const alpha = x < width ? data[(top * width + x) * 4 + 3] ?? 0 : 0;
      const opaque = alpha >= threshold;

      if (opaque && runStart === null) runStart = x;
      if (!opaque && runStart !== null) {
        spans.push({ left: runStart, top, right: x, bottom });
        runStart = null;
      }
    }
  }

  return spans;
}
