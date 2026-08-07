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

export class WindowMotionController {
  private baseline: WindowPoint | null = null;
  private transient: TransientMotion | null = null;
  private drag: { pointer: WindowPoint; window: WindowPoint; dpr: number; current: WindowPoint } | null = null;

  constructor(private readonly port: WindowMotionPort) {}

  shake(options: { amplitude: number; durationMs: number }): void {
    this.startTransient("shake", options);
  }

  bounce(options: { amplitude: number; durationMs: number }): void {
    this.startTransient("bounce", options);
  }

  async update(deltaMs: number): Promise<void> {
    const motion = this.transient;
    if (!motion || this.drag) return;
    if (!this.baseline) this.baseline = await this.port.getPosition();
    motion.elapsedMs = Math.min(motion.durationMs, motion.elapsedMs + Math.max(0, deltaMs));
    if (motion.elapsedMs >= motion.durationMs) {
      this.transient = null;
      await this.port.setPosition(this.baseline);
      return;
    }

    const progress = motion.elapsedMs / motion.durationMs;
    const offset = motion.kind === "shake"
      ? Math.sin(progress * Math.PI * 5) * motion.amplitude
      : -Math.sin(progress * Math.PI) * motion.amplitude;
    await this.port.setPosition({
      x: Math.round(this.baseline.x + (motion.kind === "shake" ? offset : 0)),
      y: Math.round(this.baseline.y + (motion.kind === "bounce" ? offset : 0)),
    });
  }

  async beginDrag(pointer: WindowPoint, dpr = 1): Promise<void> {
    const window = this.baseline ?? await this.port.getPosition();
    this.transient = null;
    this.baseline = { ...window };
    this.drag = { pointer: { ...pointer }, window: { ...window }, dpr: Math.max(1, dpr), current: { ...window } };
  }

  async dragTo(pointer: WindowPoint): Promise<void> {
    if (!this.drag) return;
    const current = {
      x: Math.round(this.drag.window.x + (pointer.x - this.drag.pointer.x) * this.drag.dpr),
      y: Math.round(this.drag.window.y + (pointer.y - this.drag.pointer.y) * this.drag.dpr),
    };
    this.drag.current = current;
    await this.port.setPosition(current);
  }

  async endDrag(): Promise<void> {
    if (!this.drag) return;
    const finalPosition = this.drag.current;
    this.drag = null;
    this.baseline = { ...finalPosition };
    await this.port.persistPosition(finalPosition);
  }

  private startTransient(
    kind: TransientMotion["kind"],
    options: { amplitude: number; durationMs: number },
  ): void {
    if (options.amplitude <= 0 || options.durationMs <= 0) return;
    this.transient = { kind, amplitude: options.amplitude, durationMs: options.durationMs, elapsedMs: 0 };
  }
}
