import type { Live2DParameterSemantic } from "../runtime/pet-renderer";
import {
  clampCatMotionValue,
  type CatMotionAmplitudeSemanticV1,
  type MotionSpatialProfileV1,
} from "../runtime-assets/cat-motion-spatial-profile";

export type ParameterValues = Partial<Record<Live2DParameterSemantic, number>>;

export interface ParameterMixInput {
  motion?: ParameterValues;
  expression?: ParameterValues;
  automation?: ParameterValues;
  interaction?: ParameterValues;
  look?: ParameterValues;
  lipSyncLayer?: ParameterValues;
  physics?: ParameterValues;
  blink?: number;
  breath?: number;
  sway?: number;
  lookX?: number;
  lookY?: number;
  angleX?: number;
  angleY?: number;
  lipSync?: number;
}

export interface ParameterRange {
  min: number;
  max: number;
}

export interface ParameterWritePort {
  getParameterRange(parameterId: string): ParameterRange | null;
  setParameter(parameterId: string, value: number): void;
}

export interface ParameterMixerOptions {
  semantics: Partial<Record<Live2DParameterSemantic, string>>;
  motionSpatialProfile?: Readonly<MotionSpatialProfileV1>;
  port: ParameterWritePort;
  diagnose?: (
    semantic: Live2DParameterSemantic,
    issue?: "incompatible-range",
  ) => void;
  silentMissing?: ReadonlySet<Live2DParameterSemantic>;
}

const PARAMETER_ORDER: Live2DParameterSemantic[] = [
  "eyeOpen",
  "eyeOpenLeft",
  "eyeOpenRight",
  "eyeBallX",
  "eyeBallY",
  "angleX",
  "angleY",
  "bodyBreath",
  "bodySway",
  "bodyStretch",
  "mouthOpen",
  "earLeft",
  "earRight",
  "tailAngle",
  "tailCurl",
  "tailTip",
];

const AMPLITUDE_SEMANTIC: Partial<
  Record<Live2DParameterSemantic, CatMotionAmplitudeSemanticV1>
> = {
  eyeOpen: "blink",
  eyeOpenLeft: "blink",
  eyeOpenRight: "blink",
  bodyBreath: "breath",
  bodyStretch: "bodyStretch",
  earLeft: "ear",
  earRight: "ear",
  tailAngle: "tailAngle",
  tailCurl: "tailCurl",
  tailTip: "tailTip",
};

export function mixParameters(input: ParameterMixInput): ParameterValues {
  const result: ParameterValues = {};
  Object.assign(result, input.motion);
  Object.assign(result, input.expression);
  Object.assign(result, input.automation, {
    ...(input.blink === undefined ? {} : { eyeOpen: input.blink }),
    ...(input.breath === undefined ? {} : { bodyBreath: input.breath }),
    ...(input.sway === undefined ? {} : { bodySway: input.sway }),
  });
  Object.assign(result, input.interaction);
  Object.assign(result, input.look, {
    ...(input.lookX === undefined ? {} : { eyeBallX: input.lookX }),
    ...(input.lookY === undefined ? {} : { eyeBallY: input.lookY }),
    ...(input.angleX === undefined ? {} : { angleX: input.angleX }),
    ...(input.angleY === undefined ? {} : { angleY: input.angleY }),
  });
  Object.assign(result, input.lipSyncLayer, input.lipSync === undefined ? {} : { mouthOpen: input.lipSync });
  Object.assign(result, input.physics);
  return result;
}

export class ParameterMixer {
  private readonly diagnosed = new Set<string>();

  constructor(private readonly options: ParameterMixerOptions) {}

  apply(input: ParameterMixInput): void {
    const values = mixParameters(input);
    for (const semantic of PARAMETER_ORDER) {
      const value = values[semantic];
      if (value === undefined) continue;
      const parameterId = this.options.semantics[semantic];
      if (!parameterId) {
        this.diagnoseOnce(semantic);
        continue;
      }
      const range = this.options.port.getParameterRange(parameterId);
      if (!range) {
        this.diagnoseOnce(semantic);
        continue;
      }
      const modelClamped = Math.min(range.max, Math.max(range.min, value));
      const amplitudeSemantic = AMPLITUDE_SEMANTIC[semantic];
      const amplitudeRange = this.options.motionSpatialProfile && amplitudeSemantic
        ? this.options.motionSpatialProfile.amplitude[amplitudeSemantic]
        : undefined;
      if (amplitudeRange === undefined || amplitudeSemantic === undefined || !this.options.motionSpatialProfile) {
        this.options.port.setParameter(parameterId, modelClamped);
        continue;
      }
      const intersectionMin = Math.max(range.min, amplitudeRange.min);
      const intersectionMax = Math.min(range.max, amplitudeRange.max);
      if (intersectionMin > intersectionMax) {
        this.diagnoseOnce(semantic, "incompatible-range");
        continue;
      }
      this.options.port.setParameter(
        parameterId,
        clampCatMotionValue(this.options.motionSpatialProfile, amplitudeSemantic, modelClamped),
      );
    }
  }

  private diagnoseOnce(
    semantic: Live2DParameterSemantic,
    issue?: "incompatible-range",
  ): void {
    if (issue === undefined && this.options.silentMissing?.has(semantic)) return;
    const key = issue === undefined ? semantic : `${semantic}:${issue}`;
    if (this.diagnosed.has(key)) return;
    this.diagnosed.add(key);
    if (issue === undefined) this.options.diagnose?.(semantic);
    else this.options.diagnose?.(semantic, issue);
  }
}
