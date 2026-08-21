import type { BehaviorIntent } from "../behavior/intents";
import type { CatMotionCommand } from "../behavior/intents";
import type { RenderTier } from "./contracts";
import type { PetRenderer } from "./pet-renderer";
import {
  canonicalPetCalibration,
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "./pet-calibration";

export type PetEffect = "hearts" | "sparkles" | "landing";
export interface PetEffectVisualOptions {
  opacity: number;
  intensity: number;
}

export interface PetPresentationPorts {
  renderer: PetRenderer;
  effects: { play(effect: PetEffect, options: PetEffectVisualOptions): void };
  windowMotion: {
    shake(options: { amplitude: number; durationMs: number }): void;
    bounce(options: { amplitude: number; durationMs: number }): void;
  };
  scheduler: { setTier(tier: RenderTier): void };
}

export class PetPresentationController {
  private calibration: PetCalibrationV1 = { ...DEFAULT_PET_CALIBRATION };
  private readonly catMotionHandles = new Map<number, ReturnType<PetRenderer["playMotion"]>>();

  constructor(private readonly ports: PetPresentationPorts) {}

  setCalibration(value: PetCalibrationV1): void {
    this.calibration = canonicalPetCalibration(value);
  }

  dispatch(intent: BehaviorIntent): void {
    const { renderer, windowMotion, scheduler } = this.ports;
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
        this.playEffect("hearts");
        if (this.calibration.feedbackStrength > 0) {
          windowMotion.shake({
            amplitude: 4 * this.calibration.feedbackStrength,
            durationMs: 180,
          });
        }
        scheduler.setTier("active");
        return;
      case "react-curious":
        renderer.setExpression("curious");
        renderer.playMotion("react-curious", { priority: 60 });
        this.playEffect("sparkles");
        scheduler.setTier("active");
        return;
      case "carried":
        renderer.playMotion("carried", { priority: 80, loop: true });
        scheduler.setTier("active");
        return;
      case "landed":
        renderer.playMotion("landed", { priority: 80 });
        this.playEffect("landing");
        if (this.calibration.feedbackStrength > 0) {
          windowMotion.bounce({
            amplitude: 8 * this.calibration.feedbackStrength,
            durationMs: 240,
          });
        }
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

  dispatchCatMotion(
    commands: readonly CatMotionCommand[],
    onFinished: (token: number) => void,
  ): void {
    for (const command of commands) {
      if (command.type === "cancel") {
        this.catMotionHandles.get(command.token)?.cancel();
        this.catMotionHandles.delete(command.token);
        continue;
      }
      if (command.type === "hold") continue;
      const renderer = this.ports.renderer;
      if (!renderer.playCatMotion) continue;
      let completedSynchronously = false;
      const handle = renderer.playCatMotion(
        command.motion,
        {
          priority: command.priority,
          loop: command.loop,
          fadeInMs: command.fadeInMs,
          fadeOutMs: command.fadeOutMs,
        },
        () => {
          completedSynchronously = true;
          this.catMotionHandles.delete(command.token);
          onFinished(command.token);
        },
      );
      if (!completedSynchronously) this.catMotionHandles.set(command.token, handle);
    }
  }

  cancelCatMotions(): void {
    for (const handle of this.catMotionHandles.values()) handle.cancel();
    this.catMotionHandles.clear();
  }

  private playEffect(effect: PetEffect): void {
    const strength = this.calibration.feedbackStrength;
    if (strength === 0) return;
    this.ports.effects.play(effect, { opacity: strength, intensity: strength });
  }
}
