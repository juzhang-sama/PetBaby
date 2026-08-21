import type { MotionProfileV1 } from "./animated-image-manifest";
import type { PetCalibrationV1 } from "./pet-calibration";
import type { CatMotionNameV1, CatParameterNameV1 } from "../runtime-live2d/cat-motion-contract";
import type { CatAutomationMode } from "../runtime-live2d/cat-automation";
import type { MotionSpatialProfileV1 } from "../runtime-assets/cat-motion-spatial-profile";

export type PetExpression = "neutral" | "happy" | "curious" | "sleepy" | "sad" | "angry";

export type PetMotion =
  | "idle"
  | "look-left"
  | "look-right"
  | "react-happy"
  | "react-curious"
  | "sleep"
  | "wake"
  | "carried"
  | "landed"
  | CatMotionNameV1;

export type PetHitArea = "head" | "body" | "edgeTail";

export type Live2DParameterSemantic =
  | "eyeOpen"
  | "eyeBallX"
  | "eyeBallY"
  | "angleX"
  | "angleY"
  | "bodyBreath"
  | "bodySway"
  | "mouthOpen"
  | CatParameterNameV1;

export interface Live2DSemantics {
  motions: Partial<Record<PetMotion, { group: string; index?: number }>>;
  expressions: Partial<Record<PetExpression, string>>;
  hitAreas: Partial<Record<PetHitArea, string>>;
  parameters: Partial<Record<Live2DParameterSemantic, string>>;
}

export type PetRenderAsset =
  | { kind: "static-png"; imageUrl: string }
  | { kind: "animated-image"; imageUrl: string; motionProfile: MotionProfileV1 }
  | {
      kind: "live2d";
      modelUrl: string;
      previewUrl: string;
      blinkOverlayUrl?: string;
      semantics: Live2DSemantics;
      catV4?: true;
      readonly motionSpatialProfile?: Readonly<MotionSpatialProfileV1>;
      dispose(): void;
    };

export interface PetMotionHandle {
  cancel(): void;
}

export interface PetRenderer {
  load(asset: PetRenderAsset): Promise<void>;
  resize(viewport: { width: number; height: number; dpr: number }): void;
  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle;
  supportsCatMotionV1?(): boolean;
  setCatAutomationMode?(mode: CatAutomationMode): void;
  playCatMotion?(
    motion: CatMotionNameV1,
    transition?: {
      loop?: boolean;
      priority?: number;
      fadeInMs?: number;
      fadeOutMs?: number;
    },
    onFinished?: () => void,
  ): PetMotionHandle;
  setExpression(expression: PetExpression, weight?: number): void;
  setLookTarget(target: { x: number; y: number } | null): void;
  setLipSync(value: number): void;
  setCalibration(value: PetCalibrationV1): void;
  hitTest(point: { x: number; y: number }): PetHitArea | null;
  setVisibility(visible: boolean): void;
  update(deltaMs: number): void;
  getHitSurface?(): HTMLCanvasElement | null;
  destroy(): void;
}

export function isLive2DRenderAsset(
  asset: PetRenderAsset,
): asset is Extract<PetRenderAsset, { kind: "live2d" }> {
  return asset.kind === "live2d";
}
