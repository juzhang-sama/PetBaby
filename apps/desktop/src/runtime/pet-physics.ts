export type PhysicsMode = "idle" | "falling" | "strolling" | "chasing";

export interface PetPhysicsState {
  x: number;
  y: number;
  vx: number;
  vy: number;
  mode: PhysicsMode;
  direction: 1 | -1;
}

export interface PhysicsBounds {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface PhysicsConfig {
  gravity: number;
  airDrag: number;
  bounceFactor: number;
  settleSpeed: number;
  strollSpeed: number;
  chaseSpeed: number;
}

export const DEFAULT_PHYSICS_CONFIG: PhysicsConfig = {
  gravity: 1_800,
  airDrag: 0.12,
  bounceFactor: 0.45,
  settleSpeed: 24,
  strollSpeed: 120,
  chaseSpeed: 220,
};

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export function throwPet(
  state: PetPhysicsState,
  vx: number,
  vy: number,
): PetPhysicsState {
  return { ...state, vx, vy, mode: "falling" };
}

/** Integrate gravity and air drag for a falling pet. */
export function stepPhysics(
  state: PetPhysicsState,
  dtMs: number,
  bounds: PhysicsBounds,
  config: PhysicsConfig,
): PetPhysicsState {
  if (state.mode !== "falling") return state;
  const dt = dtMs / 1_000;
  let { x, y, vx, vy } = state;
  vx *= Math.max(0, 1 - config.airDrag * dt);
  vy += config.gravity * dt;
  x = clamp(x + vx * dt, bounds.left, bounds.right);
  y += vy * dt;
  if (y >= bounds.bottom) {
    y = bounds.bottom;
    if (Math.abs(vy) > config.settleSpeed) {
      vy = -vy * config.bounceFactor;
      vx *= 0.6;
    } else {
      return { x, y, vx: 0, vy: 0, mode: "idle", direction: state.direction };
    }
  }
  if (Math.abs(vx) < 1 && Math.abs(vy) < 1 && y >= bounds.bottom - 0.01) {
    return { x, y, vx: 0, vy: 0, mode: "idle", direction: state.direction };
  }
  return { x, y, vx, vy, mode: "falling", direction: state.direction };
}

/** Walk along the bottom edge, bouncing off the left/right walls. */
export function edgeStrollStep(
  state: PetPhysicsState,
  dtMs: number,
  bounds: PhysicsBounds,
  config: PhysicsConfig,
): PetPhysicsState {
  const dt = dtMs / 1_000;
  let x = state.x + state.direction * config.strollSpeed * dt;
  let direction = state.direction;
  if (x <= bounds.left) {
    x = bounds.left;
    direction = 1;
  }
  if (x >= bounds.right) {
    x = bounds.right;
    direction = -1;
  }
  return {
    ...state,
    x,
    y: bounds.bottom,
    direction,
    mode: "strolling",
  };
}

/** Walk along the floor toward the cursor x, stopping on arrival. */
export function chaseStep(
  state: PetPhysicsState,
  dtMs: number,
  targetX: number,
  bounds: PhysicsBounds,
  config: PhysicsConfig,
): PetPhysicsState {
  const dt = dtMs / 1_000;
  const clampedTarget = clamp(targetX, bounds.left, bounds.right);
  const dx = clampedTarget - state.x;
  const direction: 1 | -1 = dx > 0 ? 1 : -1;
  const step = config.chaseSpeed * dt;
  const moved = Math.min(Math.abs(dx), step) * (dx === 0 ? 0 : direction);
  const x = state.x + moved;
  if (Math.abs(clampedTarget - x) < 2) {
    return {
      ...state,
      x: clampedTarget,
      y: bounds.bottom,
      vx: 0,
      vy: 0,
      mode: "idle",
      direction,
    };
  }
  return { ...state, x, y: bounds.bottom, direction, mode: "chasing" };
}
