export interface CubismAdapter {
  initialize(canvas: HTMLCanvasElement): Promise<void>;
  loadModel(modelUrl: string): Promise<void>;
  resize(width: number, height: number, dpr: number): void;
  update(deltaMs: number): void;
  draw(): void;
  destroy(): void;
}

const ABSOLUTE_URL = /^[a-z][a-z\d+.-]*:/i;

export function resolveCubismResourceUrl(modelUrl: string, resourceUrl: string): string {
  if (ABSOLUTE_URL.test(resourceUrl) || resourceUrl.startsWith("//") || resourceUrl.startsWith("/")) {
    return resourceUrl;
  }
  const directoryEnd = modelUrl.lastIndexOf("/");
  return directoryEnd < 0 ? resourceUrl : `${modelUrl.slice(0, directoryEnd + 1)}${resourceUrl}`;
}

interface CubismRuntimeModule {
  createCubismAdapter(): CubismAdapter;
}

interface ConfiguredAdapterOptions {
  loadCore?: () => Promise<void>;
  loadRuntime?: () => Promise<CubismRuntimeModule>;
}

const CUBISM_CORE_URL = "/live2d/Core/live2dcubismcore.min.js";

let coreLoadPromise: Promise<void> | undefined;

function loadCoreScript(url: string): Promise<void> {
  if ((globalThis as { Live2DCubismCore?: unknown }).Live2DCubismCore) {
    return Promise.resolve();
  }
  if (coreLoadPromise) return coreLoadPromise;

  coreLoadPromise = new Promise<void>((resolve, reject) => {
    const script = document.createElement("script");
    script.src = url;
    script.async = true;
    script.addEventListener("load", () => resolve(), { once: true });
    script.addEventListener("error", () => reject(new Error(`Cubism Core 加载失败: ${url}`)), { once: true });
    document.head.appendChild(script);
  });
  return coreLoadPromise;
}

export async function createConfiguredCubismAdapter(options: ConfiguredAdapterOptions = {}): Promise<CubismAdapter> {
  await (options.loadCore ?? (() => loadCoreScript(CUBISM_CORE_URL)))();
  const runtime = await (options.loadRuntime ?? (() => import("@cubism-runtime") as Promise<CubismRuntimeModule>))();
  return runtime.createCubismAdapter();
}

export class UnavailableCubismAdapter implements CubismAdapter {
  async initialize(_canvas: HTMLCanvasElement): Promise<void> {
    throw new Error("Cubism SDK 未准备：请先运行 npm run prepare:cubism");
  }
  async loadModel(_modelUrl: string): Promise<void> {
    throw new Error("Cubism SDK 未准备，无法加载模型");
  }
  resize(_width: number, _height: number, _dpr: number): void {}
  update(_deltaMs: number): void {}
  draw(): void {}
  playMotion(): { cancel(): void } { return { cancel() {} }; }
  stopAllMotions(): void {}
  setExpression(): void {}
  setParameter(): void {}
  getParameterRange(): null { return null; }
  hitTest(): boolean { return false; }
  destroy(): void {}
}
