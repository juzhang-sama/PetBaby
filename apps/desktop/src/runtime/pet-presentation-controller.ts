import type { BehaviorIntent } from "../behavior/intents";
import type { RenderTier } from "./contracts";
import type { PetRenderer } from "./pet-renderer";

export type PetEffect = "hearts" | "sparkles" | "landing";

export interface PetPresentationPorts {
  renderer: PetRenderer;
  effects: { play(effect: PetEffect): void };
  windowMotion: {
    shake(options: { amplitude: number; durationMs: number }): void;
    bounce(options: { amplitude: number; durationMs: number }): void;
  };
  scheduler: { setTier(tier: RenderTier): void };
}

export class PetPresentationController {
  constructor(private readonly ports: PetPresentationPorts) {}

  dispatch(intent: BehaviorIntent): void {
    const { renderer, effects, windowMotion, scheduler } = this.ports;
    switch (intent.type) {
      case "blink":
        return;
      case "look": {
        const target = intent.target === "front"
          ? null
          : { x: intent.target === "left" ? -1 : 1, y: 0 };
        renderer.setLookTarget(target);
        renderer.playMotion(intent.target === "front" ? "idle" : `look-${intent.target}`, { priority: 20 });
        return;
      }
      case "react-happy":
        renderer.setExpression("happy");
        renderer.playMotion("react-happy", { priority: 60 });
        effects.play("hearts");
        windowMotion.shake({ amplitude: 4, durationMs: 180 });
        scheduler.setTier("active");
        return;
      case "react-curious":
        renderer.setExpression("curious");
        renderer.playMotion("react-curious", { priority: 60 });
        effects.play("sparkles");
        scheduler.setTier("active");
        return;
      case "carried":
        renderer.playMotion("carried", { priority: 80, loop: true });
        scheduler.setTier("active");
        return;
      case "landed":
        renderer.playMotion("landed", { priority: 80 });
        effects.play("landing");
        windowMotion.bounce({ amplitude: 8, durationMs: 240 });
        scheduler.setTier("active");
        return;
      case "sleep":
        renderer.setExpression("sleepy");
        renderer.playMotion("sleep", { priority: 50, loop: true });
        scheduler.setTier("companion");
        return;
      case "awake":
        renderer.setExpression("neutral");
        renderer.playMotion("idle", { priority: 10, loop: true });
        renderer.playMotion("wake", { priority: 50 });
        scheduler.setTier("companion");
    }
  }
}
