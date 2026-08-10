export interface CubismShaderContext {
  isContextLost?(): boolean;
}

export interface CubismShaderContextManager<Context> {
  getShader(context: Context): { release(): void } | undefined;
  deleteShader(context: Context): boolean;
}

export interface CubismShaderContextLease {
  release(): void;
}

export class WebViewCubismShaderContextLifetime<Context extends CubismShaderContext> {
  private readonly owners = new Map<Context, number>();

  constructor(
    private readonly getManager: () => CubismShaderContextManager<Context>,
    private readonly diagnose: (error: unknown) => void = () => undefined,
  ) {}

  acquire(context: Context): CubismShaderContextLease {
    this.owners.set(context, (this.owners.get(context) ?? 0) + 1);
    let released = false;
    return {
      release: () => {
        if (released) return;
        released = true;
        const remaining = (this.owners.get(context) ?? 1) - 1;
        if (remaining > 0) {
          this.owners.set(context, remaining);
          return;
        }
        this.owners.delete(context);

        let manager: CubismShaderContextManager<Context> | undefined;
        try {
          manager = this.getManager();
          if (!context.isContextLost?.()) manager.getShader(context)?.release();
        } catch (error) {
          this.diagnose(error);
        } finally {
          try {
            manager?.deleteShader(context);
          } catch (error) {
            this.diagnose(error);
          }
        }
      },
    };
  }
}
