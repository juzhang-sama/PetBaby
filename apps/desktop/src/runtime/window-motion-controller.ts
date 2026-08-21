export interface WindowPoint {
  x: number;
  y: number;
}

export interface WindowMotionPort {
  getPosition(): Promise<WindowPoint>;
  setPosition(position: WindowPoint): Promise<void>;
  persistPosition(position: WindowPoint): Promise<void>;
}

interface TransientMotion {
  kind: "shake" | "bounce";
  amplitude: number;
  durationMs: number;
  elapsedMs: number;
}

export interface WindowMotionSuspension {
  release(): void;
}

export interface SuspendableWindowMotion {
  suspend(): Promise<WindowMotionSuspension>;
}

export async function runWithWindowMotionSuspended<T>(
  motion: SuspendableWindowMotion,
  flushGeometry: () => Promise<void>,
  operation: () => Promise<T>,
): Promise<T> {
  const suspension = await motion.suspend();
  try {
    await flushGeometry();
    return await operation();
  } finally {
    suspension.release();
  }
}

export class WindowMotionController {
  private baseline: WindowPoint | null = null;
  private transient: TransientMotion | null = null;
  private drag: { pointer: WindowPoint; window: WindowPoint; dpr: number; current: WindowPoint } | null = null;
  private readonly inFlight = new Set<Promise<unknown>>();
  private suspensionToken: symbol | null = null;

  constructor(private readonly port: WindowMotionPort) {}

  shake(options: { amplitude: number; durationMs: number }): void {
    this.startTransient("shake", options);
  }

  bounce(options: { amplitude: number; durationMs: number }): void {
    this.startTransient("bounce", options);
  }

  update(deltaMs: number): Promise<void> {
    if (this.suspensionToken) return Promise.resolve();
    return this.track(this.updateActiveMotion(deltaMs));
  }

  beginDrag(pointer: WindowPoint, dpr = 1): Promise<void> {
    if (this.suspensionToken) return Promise.resolve();
    return this.track(this.beginActiveDrag(pointer, dpr));
  }

  dragTo(pointer: WindowPoint): Promise<void> {
    const drag = this.drag;
    if (this.suspensionToken || !drag) return Promise.resolve();
    const current = {
      x: Math.round(drag.window.x + (pointer.x - drag.pointer.x) * drag.dpr),
      y: Math.round(drag.window.y + (pointer.y - drag.pointer.y) * drag.dpr),
    };
    drag.current = current;
    return this.track(this.port.setPosition(current));
  }

  endDrag(): Promise<void> {
    if (this.suspensionToken || !this.drag) return Promise.resolve();
    const finalPosition = this.drag.current;
    this.drag = null;
    this.baseline = { ...finalPosition };
    return this.track(this.port.persistPosition(finalPosition));
  }

  suspend(): Promise<WindowMotionSuspension> {
    if (this.suspensionToken) {
      return Promise.reject(new Error("Window motion is already suspended"));
    }
    const token = Symbol("window-motion-suspension");
    this.suspensionToken = token;
    this.drag = null;
    this.transient = null;
    this.baseline = null;

    return this.waitForInFlight().then(
      () => {
        this.baseline = null;
        let released = false;
        return {
          release: () => {
            if (released) return;
            released = true;
            if (this.suspensionToken === token) this.suspensionToken = null;
          },
        };
      },
      (error: unknown) => {
        this.baseline = null;
        if (this.suspensionToken === token) this.suspensionToken = null;
        throw error;
      },
    );
  }

  private async updateActiveMotion(deltaMs: number): Promise<void> {
    const motion = this.transient;
    if (!motion || this.drag) return;
    if (!this.baseline) {
      const baseline = await this.port.getPosition();
      if (this.suspensionToken || this.transient !== motion || this.drag) return;
      this.baseline = baseline;
    }
    if (this.suspensionToken || this.transient !== motion || this.drag) return;
    motion.elapsedMs = Math.min(motion.durationMs, motion.elapsedMs + Math.max(0, deltaMs));
    if (motion.elapsedMs >= motion.durationMs) {
      this.transient = null;
      await this.port.setPosition(this.baseline!);
      return;
    }

    const progress = motion.elapsedMs / motion.durationMs;
    const offset = motion.kind === "shake"
      ? Math.sin(progress * Math.PI * 5) * motion.amplitude
      : -Math.sin(progress * Math.PI) * motion.amplitude;
    await this.port.setPosition({
      x: Math.round(this.baseline!.x + (motion.kind === "shake" ? offset : 0)),
      y: Math.round(this.baseline!.y + (motion.kind === "bounce" ? offset : 0)),
    });
  }

  private async beginActiveDrag(pointer: WindowPoint, dpr: number): Promise<void> {
    const window = this.baseline ?? await this.port.getPosition();
    if (this.suspensionToken) return;
    this.transient = null;
    this.baseline = { ...window };
    this.drag = { pointer: { ...pointer }, window: { ...window }, dpr: Math.max(1, dpr), current: { ...window } };
  }

  private startTransient(
    kind: TransientMotion["kind"],
    options: { amplitude: number; durationMs: number },
  ): void {
    if (this.suspensionToken || options.amplitude <= 0 || options.durationMs <= 0) return;
    this.transient = { kind, amplitude: options.amplitude, durationMs: options.durationMs, elapsedMs: 0 };
  }

  private track<T>(operation: Promise<T>): Promise<T> {
    this.inFlight.add(operation);
    void operation.then(
      () => { this.inFlight.delete(operation); },
      () => { this.inFlight.delete(operation); },
    );
    return operation;
  }

  private async waitForInFlight(): Promise<void> {
    const settled = await Promise.allSettled([...this.inFlight]);
    const failed = settled.find((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failed) throw failed.reason;
  }
}
