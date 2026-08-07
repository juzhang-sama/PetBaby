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
  | "landed";

export type PetHitArea = "head" | "body";

export type Live2DParameterSemantic =
  | "eyeOpen"
  | "eyeBallX"
  | "eyeBallY"
  | "angleX"
  | "angleY"
  | "bodyBreath"
  | "mouthOpen";

export interface Live2DSemantics {
  motions: Partial<Record<PetMotion, { group: string; index?: number }>>;
  expressions: Partial<Record<PetExpression, string>>;
  hitAreas: Partial<Record<PetHitArea, string>>;
  parameters: Partial<Record<Live2DParameterSemantic, string>>;
}

export type PetRenderAsset =
  | { kind: "static-png"; imageUrl: string }
  | {
      kind: "live2d";
      modelUrl: string;
      previewUrl: string;
      semantics: Live2DSemantics;
      dispose(): void;
    };

export interface PetMotionHandle {
  cancel(): void;
}

export interface PetRenderer {
  load(asset: PetRenderAsset): Promise<void>;
  resize(viewport: { width: number; height: number; dpr: number }): void;
  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle;
  setExpression(expression: PetExpression, weight?: number): void;
  setLookTarget(target: { x: number; y: number } | null): void;
  setLipSync(value: number): void;
  hitTest(point: { x: number; y: number }): PetHitArea | null;
  setVisibility(visible: boolean): void;
  update(deltaMs: number): void;
  destroy(): void;
}

export function isLive2DRenderAsset(
  asset: PetRenderAsset,
): asset is Extract<PetRenderAsset, { kind: "live2d" }> {
  return asset.kind === "live2d";
}
