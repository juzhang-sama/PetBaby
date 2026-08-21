export type PetEvent =
  | { type: "head-clicked" }
  | { type: "body-clicked" }
  | { type: "double-clicked" }
  | { type: "drag-start" }
  | { type: "drag-end" }
  | { type: "pet-shown" }
  | { type: "pet-hidden" }
  | { type: "idle-tick"; elapsedMs: number };

export type CatMotionEvent =
  | { type: "start" }
  | { type: "tick"; elapsedMs: number }
  | { type: "autonomous-due" }
  | { type: "pointer-enter" }
  | { type: "pointer-leave" }
  | { type: "pet" }
  | { type: "drag-start" }
  | { type: "drag-end" }
  | { type: "edge-hidden" }
  | { type: "edge-recall" }
  | { type: "motion-complete"; token: number };
