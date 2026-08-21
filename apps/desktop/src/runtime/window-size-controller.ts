import { displayRectForScale, type PositionedRect } from "./geometry";

export type WindowRect = PositionedRect;
export type WorkArea = PositionedRect;

export interface WindowSizePort {
  getRect(): Promise<WindowRect>;
  getWorkArea(): Promise<WorkArea>;
  setRect(rect: WindowRect): Promise<void>;
  resizeRenderer(): Promise<void>;
  refreshHitRegion(): Promise<void>;
}

export interface WindowSizeAck {
  requestedScale: number;
  appliedScale: number;
  rect: WindowRect;
}

export interface WindowSizeRollbackError {
  stage: "setRect" | "resizeRenderer" | "refreshHitRegion";
  error: unknown;
}

export interface WindowSizeApplyError extends Error {
  rollbackErrors?: WindowSizeRollbackError[];
}

export class WindowSizeController {
  private queue: Promise<void> = Promise.resolve();

  constructor(private readonly port: WindowSizePort) {}

  apply(scale: number, commit?: (ack: WindowSizeAck) => Promise<void>): Promise<WindowSizeAck> {
    const transaction = this.queue.then(() => this.applyTransaction(scale, commit));
    this.queue = transaction.then(() => undefined, () => undefined);
    return transaction;
  }

  private async applyTransaction(
    scale: number,
    commit?: (ack: WindowSizeAck) => Promise<void>,
  ): Promise<WindowSizeAck> {
    let originalRect: WindowRect | undefined;
    let windowChanged = false;

    try {
      originalRect = await this.port.getRect();
      const workArea = await this.port.getWorkArea();
      const nextRect = displayRectForScale(originalRect, scale, workArea);

      windowChanged = true;
      await this.port.setRect(nextRect);
      await this.port.resizeRenderer();
      await this.port.refreshHitRegion();
      const actualRect = await this.port.getRect();
      const ack = {
        requestedScale: scale,
        appliedScale: scaleForActualRect(actualRect),
        rect: actualRect,
      };
      await commit?.(ack);
      return ack;
    } catch (error) {
      if (!windowChanged || !originalRect) throw normalizeApplyError(error, []);

      const rollbackErrors: WindowSizeRollbackError[] = [];
      await this.rollbackStage("setRect", () => this.port.setRect(originalRect!), rollbackErrors);
      await this.rollbackStage("resizeRenderer", () => this.port.resizeRenderer(), rollbackErrors);
      await this.rollbackStage("refreshHitRegion", () => this.port.refreshHitRegion(), rollbackErrors);

      throw normalizeApplyError(error, rollbackErrors);
    }
  }

  private async rollbackStage(
    stage: WindowSizeRollbackError["stage"],
    action: () => Promise<void>,
    failures: WindowSizeRollbackError[],
  ): Promise<void> {
    try {
      await action();
    } catch (error) {
      failures.push({ stage, error });
    }
  }
}

function scaleForActualRect(rect: WindowRect): number {
  if (![rect.x, rect.y, rect.width, rect.height].every(Number.isFinite)) {
    throw new RangeError("Invalid resized window acknowledgement: rectangle values must be finite");
  }
  if (rect.width <= 0 || rect.height <= 0) {
    throw new RangeError("Invalid resized window acknowledgement: width and height must be positive");
  }

  const measuredScale = Math.min(rect.width / 420, rect.height / 520);
  if (!Number.isFinite(measuredScale) || measuredScale < 0.5 || measuredScale > 1.5) {
    throw new RangeError(
      "Invalid resized window acknowledgement: actual scale must be between 0.5 and 1.5",
    );
  }
  return measuredScale;
}

function normalizeApplyError(
  error: unknown,
  rollbackErrors: WindowSizeRollbackError[],
): WindowSizeApplyError {
  if (error instanceof Error) {
    try {
      Object.defineProperty(error, "rollbackErrors", {
        configurable: true,
        enumerable: true,
        value: rollbackErrors,
        writable: true,
      });
      return error as WindowSizeApplyError;
    } catch {
      // Frozen or otherwise non-extensible failures are wrapped below.
    }
  }

  const normalized = new Error(error instanceof Error ? error.message : String(error), {
    cause: error,
  }) as WindowSizeApplyError;
  normalized.rollbackErrors = rollbackErrors;
  return normalized;
}
