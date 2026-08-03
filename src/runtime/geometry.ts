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

export function clampRectToWorkArea(
  rect: PositionedRect,
  workArea: PositionedRect,
  visiblePixels = 64,
): PositionedRect {
  const visibleX = Math.min(visiblePixels, rect.width);
  const visibleY = Math.min(visiblePixels, rect.height);
  return {
    ...rect,
    x: Math.min(workArea.x + workArea.width - visibleX, Math.max(workArea.x - rect.width + visibleX, rect.x)),
    y: Math.min(workArea.y + workArea.height - visibleY, Math.max(workArea.y - rect.height + visibleY, rect.y)),
  };
}
