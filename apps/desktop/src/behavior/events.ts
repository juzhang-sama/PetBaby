export type PetEvent =
  | { type: "head-clicked" }
  | { type: "body-clicked" }
  | { type: "double-clicked" }
  | { type: "drag-start" }
  | { type: "drag-end"; velocity?: { x: number; y: number } }
  | { type: "landed" }
  | { type: "petted" }
  | { type: "fed" }
  | { type: "played" }
  | { type: "pet-shown" }
  | { type: "pet-hidden" }
  | { type: "idle-tick"; elapsedMs: number };
