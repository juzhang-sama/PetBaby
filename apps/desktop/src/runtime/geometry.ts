export interface Size { width: number; height: number }
export interface LayoutRect extends Size { x: number; y: number; scale: number }

export function computeContainRect(source: Size, viewport: Size): LayoutRect {
  if (source.width <= 0 || source.height <= 0 || viewport.width <= 0 || viewport.height <= 0) {
    throw new RangeError("sizes must be positive");
  }
  const scale = Math.min(viewport.width / source.width, viewport.height / source.height);
  const width = source.width * scale;
  const height = source.height * scale;
  return {
    x: (viewport.width - width) / 2,
    y: (viewport.height - height) / 2,
    width,
    height,
    scale,
  };
}

export interface PositionedRect extends Size { x: number; y: number }

const BASE_WINDOW_WIDTH = 420;
const BASE_WINDOW_HEIGHT = 520;
const MIN_DISPLAY_SCALE = 0.5;
const MAX_DISPLAY_SCALE = 1.5;

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function finiteOr(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

export function clampRectToWorkArea(
  rect: PositionedRect,
  workArea: PositionedRect,
  visiblePixels = 64,
): PositionedRect {
  const visibleX = Math.min(Math.max(0, visiblePixels), rect.width, Math.max(0, workArea.width));
  const visibleY = Math.min(Math.max(0, visiblePixels), rect.height, Math.max(0, workArea.height));
  return {
    ...rect,
    x: Math.min(workArea.x + workArea.width - visibleX, Math.max(workArea.x - rect.width + visibleX, rect.x)),
    y: Math.min(workArea.y + workArea.height - visibleY, Math.max(workArea.y - rect.height + visibleY, rect.y)),
  };
}

export function displayRectForScale(
  current: PositionedRect,
  scale: number,
  workArea: PositionedRect,
): PositionedRect {
  const logicalScale = clamp(Number.isFinite(scale) ? scale : 1, MIN_DISPLAY_SCALE, MAX_DISPLAY_SCALE);
  const width = Math.round(BASE_WINDOW_WIDTH * logicalScale);
  const height = Math.round(BASE_WINDOW_HEIGHT * logicalScale);
  const safeWorkArea = {
    x: finiteOr(workArea.x, 0),
    y: finiteOr(workArea.y, 0),
    width: Math.max(0, finiteOr(workArea.width, 0)),
    height: Math.max(0, finiteOr(workArea.height, 0)),
  };
  const fallbackCenterX = finiteOr(
    safeWorkArea.x + safeWorkArea.width / 2,
    safeWorkArea.x,
  );
  const fallbackBottomY = finiteOr(
    safeWorkArea.y + safeWorkArea.height,
    safeWorkArea.y,
  );

  // Pixel-grid convention: odd widths own the center pixel on their right half.
  // Recovering that integer anchor with ceil prevents repeated resizes from
  // losing half a pixel, while floor gives the required deterministic origin.
  const anchoredCenterX = current.x + Math.ceil(current.width / 2);
  const anchoredBottomY = current.y + current.height;
  const bottomCenterX = finiteOr(anchoredCenterX, fallbackCenterX);
  const bottomY = finiteOr(anchoredBottomY, fallbackBottomY);

  return clampRectToWorkArea({
    x: Math.floor(bottomCenterX - width / 2),
    y: Math.floor(bottomY - height),
    width,
    height,
  }, safeWorkArea, 64);
}
