export interface CubismFrameworkPort<Options> {
  isStarted(): boolean;
  startUp(options: Options): boolean;
  isInitialized(): boolean;
  initialize(): void;
}

export interface CubismFrameworkLease {
  release(): void;
}

export class WebViewCubismFrameworkLifetime<Options> {
  constructor(
    private readonly framework: CubismFrameworkPort<Options>,
    private readonly options: Options,
  ) {}

  acquire(): CubismFrameworkLease {
    if (!this.framework.isStarted() && !this.framework.startUp(this.options)) {
      throw new Error("Cubism Framework 启动失败");
    }
    if (!this.framework.isInitialized()) this.framework.initialize();

    // The Core logging callback has no corresponding removeFunction API in the
    // pinned Emscripten wrapper, so Framework ownership belongs to the WebView.
    return { release() {} };
  }
}
