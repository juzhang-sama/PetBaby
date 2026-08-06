export interface CubismAdapter {
  initialize(canvas: HTMLCanvasElement): Promise<void>;
  loadModel(modelUrl: string): Promise<void>;
  resize(width: number, height: number, dpr: number): void;
  update(deltaMs: number): void;
  draw(): void;
  destroy(): void;
}

/** Repository-side seam; the official Cubism SDK is intentionally supplied out-of-band. */
export class UnavailableCubismAdapter implements CubismAdapter {
  async initialize(_canvas: HTMLCanvasElement): Promise<void> {
    throw new Error("Cubism SDK 未提供：请先运行 scripts/准备CubismSDK.ps1");
  }
  async loadModel(_modelUrl: string): Promise<void> {
    throw new Error("Cubism SDK 未提供，无法加载模型");
  }
  resize(_width: number, _height: number, _dpr: number): void {}
  update(_deltaMs: number): void {}
  draw(): void {}
  destroy(): void {}
}
