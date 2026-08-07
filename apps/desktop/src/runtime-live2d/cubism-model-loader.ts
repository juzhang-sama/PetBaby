import type { PetMotionHandle } from "../runtime/pet-renderer";
import {
  createConfiguredCubismAdapter,
  resolveCubismResourceUrl,
  type CubismAdapter,
} from "./cubism-adapter";
import type { ParameterRange } from "./parameter-mixer";

export interface CubismMotionOptions {
  priority: number;
  loop: boolean;
}

export interface CubismControlAdapter extends CubismAdapter {
  playMotion(
    group: string,
    index: number,
    options: CubismMotionOptions,
    onFinished: () => void,
  ): PetMotionHandle;
  stopAllMotions(): void;
  setExpression(name: string, weight: number): void;
  setParameter(parameterId: string, value: number): void;
  getParameterRange(parameterId: string): ParameterRange | null;
  hitTest(name: string, point: { x: number; y: number }): boolean;
}

export interface LoadedCubismModel {
  resize(width: number, height: number, dpr: number): void;
  update(deltaMs: number): void;
  draw(): void;
  release(): void;
  playMotion(
    group: string,
    index: number,
    options: CubismMotionOptions,
    onFinished: () => void,
  ): PetMotionHandle;
  stopAllMotions(): void;
  setExpression(name: string, weight: number): void;
  setParameter(parameterId: string, value: number): void;
  getParameterRange(parameterId: string): ParameterRange | null;
  hitTest(name: string, point: { x: number; y: number }): boolean;
}

export interface CubismModelLoaderPort {
  load(canvas: HTMLCanvasElement, modelUrl: string): Promise<LoadedCubismModel>;
}

type AdapterFactory = () => Promise<CubismControlAdapter>;

const CONTROL_METHODS = [
  "playMotion",
  "stopAllMotions",
  "setExpression",
  "setParameter",
  "getParameterRange",
  "hitTest",
] as const;

export { resolveCubismResourceUrl };

export class CubismModelLoader implements CubismModelLoaderPort {
  constructor(
    private readonly createAdapter: AdapterFactory = async () =>
      createConfiguredCubismAdapter() as Promise<CubismControlAdapter>,
  ) {}

  async load(canvas: HTMLCanvasElement, modelUrl: string): Promise<LoadedCubismModel> {
    const adapter = await this.createAdapter();
    try {
      for (const method of CONTROL_METHODS) {
        if (typeof adapter[method] !== "function") {
          throw new Error(`Cubism adapter is missing required control method: ${method}`);
        }
      }
      await adapter.initialize(canvas);
      await adapter.loadModel(modelUrl);
    } catch (error) {
      adapter.destroy();
      throw error;
    }

    let released = false;
    return {
      resize: (width, height, dpr) => adapter.resize(width, height, dpr),
      update: (deltaMs) => adapter.update(deltaMs),
      draw: () => adapter.draw(),
      release: () => {
        if (released) return;
        released = true;
        adapter.destroy();
      },
      playMotion: (group, index, options, onFinished) =>
        adapter.playMotion(group, index, options, onFinished),
      stopAllMotions: () => adapter.stopAllMotions(),
      setExpression: (name, weight) => adapter.setExpression(name, weight),
      setParameter: (parameterId, value) => adapter.setParameter(parameterId, value),
      getParameterRange: (parameterId) => adapter.getParameterRange(parameterId),
      hitTest: (name, point) => adapter.hitTest(name, point),
    };
  }
}
