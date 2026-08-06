export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FeatureRects {
  leftEye: Rect;
  rightEye: Rect;
  leftEar: Rect;
  rightEar: Rect;
  tail: Rect;
}

export interface MeshParams {
  blink: number;
  earWobble: number;
  tailSway: number;
  headTurn?: number;
}

export interface GridData {
  cols: number;
  rows: number;
  positions: Float32Array;
  uvs: Float32Array;
  indices: Uint32Array;
}

export const MESH_COLS = 12;
export const MESH_ROWS = 12;

/** Build a regular triangle grid covering (0,0)-(width,height). */
export function buildGrid(
  width: number,
  height: number,
  cols = MESH_COLS,
  rows = MESH_ROWS,
): GridData {
  if (width <= 0 || height <= 0 || cols < 2 || rows < 2) {
    throw new RangeError("grid requires positive size and at least 2x2 cells");
  }
  const vertexCount = cols * rows;
  const positions = new Float32Array(vertexCount * 2);
  const uvs = new Float32Array(vertexCount * 2);
  const indices = new Uint32Array((cols - 1) * (rows - 1) * 6);
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      const index = row * cols + col;
      const u = col / (cols - 1);
      const v = row / (rows - 1);
      positions[index * 2] = u * width;
      positions[index * 2 + 1] = v * height;
      uvs[index * 2] = u;
      uvs[index * 2 + 1] = v;
    }
  }
  let cell = 0;
  for (let row = 0; row < rows - 1; row += 1) {
    for (let col = 0; col < cols - 1; col += 1) {
      const topLeft = row * cols + col;
      const topRight = topLeft + 1;
      const bottomLeft = topLeft + cols;
      const bottomRight = bottomLeft + 1;
      indices[cell] = topLeft;
      indices[cell + 1] = topRight;
      indices[cell + 2] = bottomLeft;
      indices[cell + 3] = topRight;
      indices[cell + 4] = bottomRight;
      indices[cell + 5] = bottomLeft;
      cell += 6;
    }
  }
  return { cols, rows, positions, uvs, indices };
}

/**
 * Heuristic feature placement for a front-facing cutout pet.
 *
 * Chibi/3D render pets have a large head at the top of the subject, ears at
 * the top corners, eyes in the upper-middle of the head, and a tail near the
 * bottom edge. These are intentionally coarse; the mesh deformation is smooth
 * enough that slight misalignment still reads as natural motion.
 */
export function heuristicFeatures(subject: Rect, sourceWidth: number, sourceHeight: number): FeatureRects {
  const sx = subject.x;
  const sy = subject.y;
  const sw = subject.width;
  const sh = subject.height;
  const centerX = sx + sw / 2;
  const headTop = sy;
  const headHeight = sh * 0.34;
  const eyeWidth = sw * 0.16;
  const eyeHeight = sh * 0.12;
  const earWidth = sw * 0.26;
  const earHeight = sh * 0.15;
  return {
    leftEye: {
      x: centerX - sw * 0.2,
      y: headTop + headHeight * 0.45,
      width: eyeWidth,
      height: eyeHeight,
    },
    rightEye: {
      x: centerX + sw * 0.04,
      y: headTop + headHeight * 0.45,
      width: eyeWidth,
      height: eyeHeight,
    },
    leftEar: {
      x: centerX - sw * 0.42,
      y: headTop + headHeight * 0.02,
      width: earWidth,
      height: earHeight,
    },
    rightEar: {
      x: centerX + sw * 0.16,
      y: headTop + headHeight * 0.02,
      width: earWidth,
      height: earHeight,
    },
    tail: {
      x: centerX + sw * 0.28,
      y: sy + sh * 0.66,
      width: sw * 0.26,
      height: sh * 0.2,
    },
  };
}

/** Smooth 0..1 influence with a falloff margin around a rect. */
function rectInfluence(x: number, y: number, rect: Rect, margin: number): number {
  const left = rect.x - margin;
  const right = rect.x + rect.width + margin;
  const top = rect.y - margin;
  const bottom = rect.y + rect.height + margin;
  const dx = Math.max(left - x, 0, x - right);
  const dy = Math.max(top - y, 0, y - bottom);
  const distance = Math.hypot(dx, dy);
  if (distance >= margin) return 0;
  const t = 1 - distance / margin;
  return t * t * (3 - 2 * t); // smoothstep
}

function rotatePoint(
  x: number,
  y: number,
  pivotX: number,
  pivotY: number,
  angle: number,
): { x: number; y: number } {
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const dx = x - pivotX;
  const dy = y - pivotY;
  return {
    x: pivotX + dx * cos - dy * sin,
    y: pivotY + dx * sin + dy * cos,
  };
}

/**
 * Deform a base grid for blink (eye region Y squash), ear wobble (rotation
 * around each ear's base) and tail sway (lateral shift). Returns a new
 * positions array of the same length.
 */
export function deformGrid(
  grid: GridData,
  features: FeatureRects,
  params: MeshParams,
  basePositions?: Float32Array,
): Float32Array {
  const base = basePositions ?? grid.positions;
  const out = new Float32Array(base);
  const leftEyeCenterY = features.leftEye.y + features.leftEye.height / 2;
  const rightEyeCenterY = features.rightEye.y + features.rightEye.height / 2;
  const leftPivot = {
    x: features.leftEar.x + features.leftEar.width / 2,
    y: features.leftEar.y + features.leftEar.height,
  };
  const rightPivot = {
    x: features.rightEar.x + features.rightEar.width / 2,
    y: features.rightEar.y + features.rightEar.height,
  };
  const earAngle = params.earWobble * 0.14;
  const tailShift = Math.sin(params.tailSway) * features.tail.width * 0.35;
  for (let row = 0; row < grid.rows; row += 1) {
    for (let col = 0; col < grid.cols; col += 1) {
      const index = row * grid.cols + col;
      const x = base[index * 2]!;
      const y = base[index * 2 + 1]!;
      let outX = x;
      let outY = y;

      if (params.blink > 0) {
        for (const eye of [features.leftEye, features.rightEye]) {
          const influence = rectInfluence(x, y, eye, 12);
          if (influence > 0) {
            const centerY = eye.y + eye.height / 2;
            const squash = 1 - 0.82 * params.blink * influence;
            outY = centerY + (y - centerY) * squash;
            break;
          }
        }
      }

      if (params.earWobble !== 0) {
        const leftInfluence = rectInfluence(x, y, features.leftEar, 12);
        if (leftInfluence > 0) {
          const rotated = rotatePoint(x, outY, leftPivot.x, leftPivot.y, earAngle * leftInfluence);
          outX = rotated.x;
          outY = rotated.y;
        }
        const rightInfluence = rectInfluence(x, y, features.rightEar, 12);
        if (rightInfluence > 0) {
          const rotated = rotatePoint(x, outY, rightPivot.x, rightPivot.y, -earAngle * rightInfluence);
          outX = rotated.x;
          outY = rotated.y;
        }
      }

      if (params.tailSway !== 0) {
        const influence = rectInfluence(x, y, features.tail, 16);
        if (influence > 0) {
          outX += tailShift * influence;
        }
      }

      out[index * 2] = outX;
      out[index * 2 + 1] = outY;
    }
  }
  return out;
}
