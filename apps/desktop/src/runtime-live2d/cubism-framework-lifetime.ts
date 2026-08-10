export interface CubismFrameworkPort<Options> {
  isStarted(): boolean;
  startUp(options: Options): boolean;
  isInitialized(): boolean;
  initialize(): void;
  dispose(): void;
}

export interface CubismFrameworkLease {
  release(): void;
}

export class WebViewCubismFrameworkLifetime<Options> {
  private activeLeases = 0;

  constructor(
    private readonly framework: CubismFrameworkPort<Options>,
    private readonly options: Options,
  ) {}

  acquire(): CubismFrameworkLease {
    if (!this.framework.isStarted() && !this.framework.startUp(this.options)) {
      throw new Error("Cubism Framework 启动失败");
    }
    if (!this.framework.isInitialized()) this.framework.initialize();
    this.activeLeases += 1;

    let released = false;
    return {
      release: () => {
        if (released) return;
        released = true;
        this.activeLeases -= 1;
        if (this.activeLeases === 0 && this.framework.isInitialized()) {
          // dispose releases Framework renderer caches, but deliberately do
          // not cleanUp: Core logging has no matching removeFunction API.
          this.framework.dispose();
        }
      },
    };
  }
}
