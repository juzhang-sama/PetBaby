import type { Live2DParameterSemantic } from "../runtime/pet-renderer";

export type ParameterValues = Partial<Record<Live2DParameterSemantic, number>>;

export interface ParameterMixInput {
  motion?: ParameterValues;
  expression?: ParameterValues;
  automation?: ParameterValues;
  look?: ParameterValues;
  lipSyncLayer?: ParameterValues;
  physics?: ParameterValues;
  blink?: number;
  breath?: number;
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
  port: ParameterWritePort;
  diagnose?: (semantic: Live2DParameterSemantic) => void;
}

const PARAMETER_ORDER: Live2DParameterSemantic[] = [
  "eyeOpen",
  "eyeBallX",
  "eyeBallY",
  "angleX",
  "angleY",
  "bodyBreath",
  "mouthOpen",
];

export function mixParameters(input: ParameterMixInput): ParameterValues {
  const result: ParameterValues = {};
  Object.assign(result, input.motion);
  Object.assign(result, input.expression);
  Object.assign(result, input.automation, {
    ...(input.blink === undefined ? {} : { eyeOpen: input.blink }),
    ...(input.breath === undefined ? {} : { bodyBreath: input.breath }),
  });
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
  private readonly diagnosed = new Set<Live2DParameterSemantic>();

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
      this.options.port.setParameter(parameterId, Math.min(range.max, Math.max(range.min, value)));
    }
  }

  private diagnoseOnce(semantic: Live2DParameterSemantic): void {
    if (this.diagnosed.has(semantic)) return;
    this.diagnosed.add(semantic);
    this.options.diagnose?.(semantic);
  }
}
