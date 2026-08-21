import {
  isPetDisplayScaleResult,
  MAX_DISPLAY_SCALE,
  MIN_DISPLAY_SCALE,
  type PetDisplayScaleResult,
} from "../runtime/contracts";

const DEFAULT_DEBOUNCE_MS = 100;
const SCALE_EPSILON = 1e-6;
const KEYBOARD_SCALE_KEYS = new Set([
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "End",
  "Home",
  "PageDown",
  "PageUp",
]);

export interface DisplaySizeControlElements {
  slider: HTMLInputElement;
  output: HTMLOutputElement;
  status: HTMLElement;
  error: HTMLElement;
  presets: readonly HTMLButtonElement[];
}

export interface DisplaySizeClock {
  setTimeout(callback: () => void, delayMs: number): number;
  clearTimeout(id: number): void;
}

export interface DisplaySizeControlOptions {
  initial: number;
  elements: DisplaySizeControlElements;
  request(displayScale: number): Promise<PetDisplayScaleResult>;
  clock?: DisplaySizeClock;
  debounceMs?: number;
}

export interface MountableDisplaySizeControl {
  mount(): void;
  destroy(): void;
}

export interface InitializeDisplaySizeControlOptions {
  loadInitial(): Promise<number>;
  createControl(initial: number): MountableDisplaySizeControl;
  onError(error: unknown): void;
}

export interface DisplaySizeControlLifecycle {
  ready: Promise<void>;
  destroy(): void;
}

export function initializeDisplaySizeControl(
  options: InitializeDisplaySizeControlOptions,
): DisplaySizeControlLifecycle {
  let tornDown = false;
  let control: MountableDisplaySizeControl | null = null;
  const ready = (async (): Promise<void> => {
    try {
      const initial = await options.loadInitial();
      if (tornDown) return;
      const nextControl = options.createControl(initial);
      if (tornDown) {
        nextControl.destroy();
        return;
      }
      control = nextControl;
      control.mount();
    } catch (error) {
      if (!tornDown) options.onError(error);
    }
  })();
  return {
    ready,
    destroy: () => {
      tornDown = true;
      const mountedControl = control;
      control = null;
      mountedControl?.destroy();
    },
  };
}

const browserClock: DisplaySizeClock = {
  setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
  clearTimeout: (id) => window.clearTimeout(id),
};

function isDisplayScale(value: number): boolean {
  return Number.isFinite(value)
    && value >= MIN_DISPLAY_SCALE
    && value <= MAX_DISPLAY_SCALE;
}

function sameScale(left: number, right: number): boolean {
  return Math.abs(left - right) <= SCALE_EPSILON;
}

export function nearestSelectableScale(scale: number): number {
  if (!Number.isFinite(scale)) throw new RangeError("display scale must be finite");
  const clamped = Math.min(MAX_DISPLAY_SCALE, Math.max(MIN_DISPLAY_SCALE, scale));
  return Number((Math.round(clamped * 20) / 20).toFixed(2));
}

function formatPercent(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  const message = String(error).trim();
  return message || "宠物窗口没有确认这次调整";
}

export class DisplaySizeControl {
  private readonly elements: DisplaySizeControlElements;
  private readonly request: DisplaySizeControlOptions["request"];
  private readonly clock: DisplaySizeClock;
  private readonly debounceMs: number;
  private confirmedScale: number;
  private desiredScale: number;
  private pendingScale: number | null = null;
  private pendingRevision = 0;
  private desiredRevision = 0;
  private handledRevision = 0;
  private debounceTimer: number | null = null;
  private mounted = false;
  private destroyed = false;

  constructor(options: DisplaySizeControlOptions) {
    if (!isDisplayScale(options.initial)) {
      throw new RangeError(
        `initial display scale must be between ${MIN_DISPLAY_SCALE} and ${MAX_DISPLAY_SCALE}`,
      );
    }
    const debounceMs = options.debounceMs ?? DEFAULT_DEBOUNCE_MS;
    if (!Number.isFinite(debounceMs) || debounceMs < 0) {
      throw new RangeError("debounceMs must be a finite non-negative number");
    }
    this.elements = options.elements;
    this.request = options.request;
    this.clock = options.clock ?? browserClock;
    this.debounceMs = debounceMs;
    this.confirmedScale = options.initial;
    this.desiredScale = nearestSelectableScale(options.initial);
  }

  mount(): void {
    if (this.mounted || this.destroyed) return;
    this.mounted = true;
    this.elements.slider.addEventListener("input", this.onInput);
    this.elements.slider.addEventListener("change", this.onImmediateCommit);
    this.elements.slider.addEventListener("pointerup", this.onImmediateCommit);
    this.elements.slider.addEventListener("keyup", this.onKeyUp);
    for (const preset of this.elements.presets) {
      preset.addEventListener("click", this.onPreset);
      preset.disabled = false;
    }
    this.elements.slider.disabled = false;
    this.renderConfirmed(this.confirmedScale);
    this.elements.status.textContent = "";
    this.elements.error.textContent = "";
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.clearDebounce();
    if (!this.mounted) return;
    this.mounted = false;
    this.elements.slider.removeEventListener("input", this.onInput);
    this.elements.slider.removeEventListener("change", this.onImmediateCommit);
    this.elements.slider.removeEventListener("pointerup", this.onImmediateCommit);
    this.elements.slider.removeEventListener("keyup", this.onKeyUp);
    for (const preset of this.elements.presets) {
      preset.removeEventListener("click", this.onPreset);
    }
  }

  private readonly onInput = (): void => {
    this.readSlider(false);
  };

  private readonly onImmediateCommit = (): void => {
    this.readSlider(true);
  };

  private readonly onKeyUp = (event: KeyboardEvent): void => {
    if (KEYBOARD_SCALE_KEYS.has(event.key)) this.readSlider(true);
  };

  private readonly onPreset = (event: Event): void => {
    const preset = event.currentTarget as HTMLButtonElement | null;
    const value = Number(preset?.dataset.displayScale);
    if (!isDisplayScale(value)) return;
    this.acceptDesired(value);
    this.flush();
  };

  private readSlider(immediate: boolean): void {
    if (this.destroyed) return;
    const value = Number(this.elements.slider.value);
    if (!isDisplayScale(value)) {
      this.clearDebounce();
      this.desiredScale = nearestSelectableScale(this.confirmedScale);
      this.renderConfirmed(this.confirmedScale);
      return;
    }
    this.acceptDesired(nearestSelectableScale(value));
    if (immediate) this.flush();
    else this.schedule();
  }

  private acceptDesired(value: number): void {
    this.desiredScale = value;
    this.desiredRevision += 1;
    this.elements.error.textContent = "";
    this.renderPreview(value);
  }

  private schedule(): void {
    this.clearDebounce();
    if (this.pendingScale === null && sameScale(this.desiredScale, this.confirmedScale)) {
      this.handledRevision = this.desiredRevision;
      this.renderConfirmed(this.confirmedScale);
      return;
    }
    this.debounceTimer = this.clock.setTimeout(() => {
      this.debounceTimer = null;
      this.sendDesired();
    }, this.debounceMs);
  }

  private flush(): void {
    this.clearDebounce();
    this.sendDesired();
  }

  private clearDebounce(): void {
    if (this.debounceTimer === null) return;
    this.clock.clearTimeout(this.debounceTimer);
    this.debounceTimer = null;
  }

  private sendDesired(): void {
    if (this.destroyed || this.pendingScale !== null) return;
    if (this.desiredRevision <= this.handledRevision) return;
    const requestedScale = this.desiredScale;
    if (sameScale(requestedScale, this.confirmedScale)) {
      this.handledRevision = this.desiredRevision;
      this.renderConfirmed(this.confirmedScale);
      return;
    }
    this.pendingScale = requestedScale;
    this.pendingRevision = this.desiredRevision;
    this.elements.status.textContent = `正在调整到 ${formatPercent(requestedScale)}…`;

    let pending: Promise<PetDisplayScaleResult>;
    try {
      pending = this.request(requestedScale);
    } catch (error) {
      this.finishFailure(requestedScale, error);
      return;
    }
    void pending.then(
      (result) => this.finishResult(requestedScale, result),
      (error: unknown) => this.finishFailure(requestedScale, error),
    );
  }

  private finishResult(requestedScale: number, result: PetDisplayScaleResult): void {
    if (this.destroyed || this.pendingScale === null) return;
    if (!isPetDisplayScaleResult(result) || result.requestedDisplayScale !== requestedScale) {
      this.finishFailure(requestedScale, new Error("宠物窗口返回了无效的尺寸确认"));
      return;
    }
    if (!result.ok) {
      this.finishFailure(requestedScale, new Error(result.message));
      return;
    }

    this.pendingScale = null;
    this.confirmedScale = result.displayScale;
    const hasNewerDesired = this.desiredRevision > this.pendingRevision
      && !sameScale(this.desiredScale, requestedScale);
    this.handledRevision = Math.max(this.handledRevision, this.pendingRevision);
    if (!hasNewerDesired) {
      this.handledRevision = this.desiredRevision;
      this.desiredScale = nearestSelectableScale(result.displayScale);
    }
    this.elements.status.textContent = `已调整为 ${formatPercent(result.displayScale)}`;
    this.elements.error.textContent = "";
    if (hasNewerDesired) this.renderPreview(this.desiredScale);
    else this.renderConfirmed(result.displayScale);
    this.continueWithLatest();
  }

  private finishFailure(requestedScale: number, error: unknown): void {
    if (this.destroyed || this.pendingScale === null) return;
    this.pendingScale = null;
    const hasNewerDesired = this.desiredRevision > this.pendingRevision
      && !sameScale(this.desiredScale, requestedScale);
    this.handledRevision = Math.max(this.handledRevision, this.pendingRevision);
    if (!hasNewerDesired) {
      this.handledRevision = this.desiredRevision;
      this.desiredScale = nearestSelectableScale(this.confirmedScale);
    }
    this.elements.status.textContent = "";
    this.elements.error.textContent = `调整失败：${errorMessage(error)}。请重试。`;
    if (hasNewerDesired) this.renderPreview(this.desiredScale);
    else this.renderConfirmed(this.confirmedScale);
    this.continueWithLatest();
  }

  private continueWithLatest(): void {
    if (this.destroyed || this.desiredRevision <= this.handledRevision) return;
    if (sameScale(this.desiredScale, this.confirmedScale)) {
      this.handledRevision = this.desiredRevision;
      this.renderConfirmed(this.confirmedScale);
      return;
    }
    this.clearDebounce();
    this.sendDesired();
  }

  private renderConfirmed(actualScale: number): void {
    const selectableScale = nearestSelectableScale(actualScale);
    this.renderSlider(selectableScale);
    this.elements.output.textContent = `实际 ${formatPercent(actualScale)}`;
    this.elements.slider.setAttribute(
      "aria-valuetext",
      `实际 ${formatPercent(actualScale)}，滑杆档位 ${formatPercent(selectableScale)}`,
    );
  }

  private renderPreview(selectableScale: number): void {
    this.renderSlider(selectableScale);
    this.elements.output.textContent = `选择 ${formatPercent(selectableScale)}`;
    this.elements.slider.setAttribute("aria-valuetext", `选择档位 ${formatPercent(selectableScale)}`);
  }

  private renderSlider(scale: number): void {
    if (this.destroyed) return;
    this.elements.slider.value = String(scale);
    for (const preset of this.elements.presets) {
      const presetScale = Number(preset.dataset.displayScale);
      preset.setAttribute("aria-pressed", String(
        isDisplayScale(presetScale) && sameScale(scale, presetScale),
      ));
    }
  }
}
