export type BehaviorIntent =
  | { type: "blink"; intensity: 1 }
  | { type: "look"; target: "front" | "left" | "right" }
  | { type: "react-happy" }
  | { type: "react-curious" }
  | { type: "carried" }
  | { type: "falling" }
  | { type: "stroll" }
  | { type: "landed" }
  | { type: "sleep" }
  | { type: "awake" };
