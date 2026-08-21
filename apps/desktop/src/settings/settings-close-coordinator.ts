export interface CloseRequestedLike {
  preventDefault(): void;
}

interface CloseClock {
  setTimeout(callback: () => void, delayMs: number): number;
  clearTimeout(id: number): void;
}

export interface SettingsCloseCoordinatorOptions {
  onCloseRequested(handler: (event: CloseRequestedLike) => void): Promise<() => void>;
  destroy(): Promise<void>;
  freeze(): void;
  unfreeze(): void;
  settle(): Promise<void>;
  restore(): Promise<void>;
  hasActive(): boolean;
  cleanup(): void;
  diagnose(error: unknown): void;
  clock?: CloseClock;
  timeoutMs?: number;
}

const browserClock: CloseClock = {
  setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
  clearTimeout: (id) => window.clearTimeout(id),
};

export class SettingsCloseCoordinator {
  private readonly options: SettingsCloseCoordinatorOptions;
  private unlisten: (() => void) | undefined;
  private closeStarted = false;
  private cleaned = false;

  constructor(options: SettingsCloseCoordinatorOptions) {
    this.options = options;
  }

  async mount(): Promise<void> {
    if (this.unlisten) return;
    this.unlisten = await this.options.onCloseRequested((event) => {
      event.preventDefault();
      if (this.closeStarted) return;
      this.closeStarted = true;
      void this.coordinateClose();
    });
  }

  beforeUnload(): void {
    this.finalCleanup();
  }

  destroy(): void {
    this.finalCleanup();
  }

  private async coordinateClose(): Promise<void> {
    this.options.freeze();
    const timeoutMs = this.options.timeoutMs ?? 5_500;
    const clock = this.options.clock ?? browserClock;
    const settlement = this.options.settle();
    try {
      await withTimeout(settlement, timeoutMs, clock, "save settlement");
    } catch (error) {
      if (error instanceof CloseTimeoutError) {
        this.options.diagnose(new Error("保存仍在进行，暂不能关闭；保存完成后将继续关闭。"));
        try {
          await settlement;
        } catch (settlementError) {
          this.options.diagnose(settlementError);
        }
      } else {
        this.options.diagnose(error);
      }
    }
    if (this.options.hasActive()) {
      try {
        await withTimeout(this.options.restore(), timeoutMs, clock, "restore");
      } catch (error) {
        this.options.diagnose(error);
      }
    }
    try {
      // The original close request was prevented above. Calling close() here would emit
      // another close request; destroy() is Tauri's documented terminal operation.
      await this.options.destroy();
    } catch (error) {
      this.options.diagnose(error);
      this.closeStarted = false;
      this.options.unfreeze();
      return;
    }
    this.finalCleanup();
  }

  private finalCleanup(): void {
    if (this.cleaned) return;
    this.cleaned = true;
    const unlisten = this.unlisten;
    this.unlisten = undefined;
    try { unlisten?.(); } catch { /* Final cleanup is best effort. */ }
    this.options.cleanup();
  }
}

class CloseTimeoutError extends Error {}

function withTimeout(
  promise: Promise<void>,
  timeoutMs: number,
  clock: CloseClock,
  operation: string,
): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = clock.setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new CloseTimeoutError(`Calibration ${operation} timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    promise.then(() => {
      if (settled) return;
      settled = true;
      clock.clearTimeout(timer);
      resolve();
    }, (error: unknown) => {
      if (settled) return;
      settled = true;
      clock.clearTimeout(timer);
      reject(error);
    });
  });
}
