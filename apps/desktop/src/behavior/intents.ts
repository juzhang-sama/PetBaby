export type BehaviorIntent =
  | { type: "blink"; intensity: 1 }
  | { type: "look"; target: "front" | "left" | "right" }
  | { type: "react-happy" }
  | { type: "react-curious" }
  | { type: "carried" }
  | { type: "landed" }
  | { type: "sleep" }
  | { type: "awake" };

import type { CatMotionNameV1 } from "../runtime-live2d/cat-motion-contract";

export interface CatMotionTransition {
  fadeInMs: number;
  fadeOutMs: number;
}

export type CatMotionCommand =
  | ({
      type: "play";
      token: number;
      motion: CatMotionNameV1;
      priority: number;
      loop: boolean;
    } & CatMotionTransition)
  | ({ type: "cancel"; token: number } & CatMotionTransition)
  | ({ type: "hold"; priority: 100 } & CatMotionTransition);
