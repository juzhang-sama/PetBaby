export type PetEvent =
  | { type: "head-clicked" }
  | { type: "body-clicked" }
  | { type: "double-clicked" }
  | { type: "drag-start" }
  | { type: "drag-end" }
  | { type: "pet-shown" }
  | { type: "pet-hidden" }
  | { type: "idle-tick"; elapsedMs: number };
