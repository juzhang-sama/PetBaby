import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import type { PetDisplayScaleResult } from "../runtime/contracts";
import {
  DisplaySizeControl,
  initializeDisplaySizeControl,
  nearestSelectableScale,
  type DisplaySizeControlElements,
} from "./display-size-control";

type FakeEvent = { key?: string; currentTarget?: FakeElement };
type Listener = (event: FakeEvent) => void;

class FakeElement {
  value = "";
  textContent = "";
  disabled = false;
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  private listeners = new Map<string, Set<Listener>>();

  addEventListener(type: string, listener: Listener): void {
    const listeners = this.listeners.get(type) ?? new Set<Listener>();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  dispatch(type: string, event: FakeEvent = {}): void {
    event.currentTarget = this;
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  listenerCount(type?: string): number {
    if (type) return this.listeners.get(type)?.size ?? 0;
    return [...this.listeners.values()].reduce((total, listeners) => total + listeners.size, 0);
  }
}

class FakeClock {
  private nextId = 1;
  private now = 0;
  private callbacks = new Map<number, { due: number; callback: () => void }>();
  readonly delays: number[] = [];

  setTimeout = (callback: () => void, delayMs: number): number => {
    const id = this.nextId++;
    this.delays.push(delayMs);
    this.callbacks.set(id, { due: this.now + delayMs, callback });
    return id;
  };

  clearTimeout = (id: number): void => {
    this.callbacks.delete(id);
  };

  advance(milliseconds: number): void {
    const target = this.now + milliseconds;
    while (true) {
      const next = [...this.callbacks.entries()]
        .filter(([, timer]) => timer.due <= target)
        .sort((left, right) => left[1].due - right[1].due || left[0] - right[0])[0];
      if (!next) break;
      const [id, timer] = next;
      this.callbacks.delete(id);
      this.now = timer.due;
      timer.callback();
    }
    this.now = target;
  }

  flush(): void {
    const latestDue = Math.max(this.now, ...[...this.callbacks.values()].map(({ due }) => due));
    this.advance(latestDue - this.now);
  }

  get pendingCount(): number { return this.callbacks.size; }
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const success = (requested: number, actual = requested): PetDisplayScaleResult => ({
  requestId: `request-${requested}`,
  requestedDisplayScale: requested,
  ok: true,
  displayScale: actual,
  rect: { x: 10, y: 20, width: 420 * actual, height: 520 * actual },
});

const failure = (requested: number, message = "resize failed"): PetDisplayScaleResult => ({
  requestId: `request-${requested}`,
  requestedDisplayScale: requested,
  ok: false,
  message,
});

function mount(options: {
  initial?: number;
  request?: (scale: number) => Promise<PetDisplayScaleResult>;
} = {}) {
  const slider = new FakeElement();
  const output = new FakeElement();
  const status = new FakeElement();
  const error = new FakeElement();
  const presets = [0.75, 1, 1.25].map((scale) => {
    const button = new FakeElement();
    button.dataset.displayScale = String(scale);
    return button;
  });
  const clock = new FakeClock();
  const sentScales: number[] = [];
  const requests: Deferred<PetDisplayScaleResult>[] = [];
  const request = options.request ?? ((scale: number) => {
    sentScales.push(scale);
    const next = deferred<PetDisplayScaleResult>();
    requests.push(next);
    return next.promise;
  });
  const elements = {
    slider,
    output,
    status,
    error,
    presets,
  } as unknown as DisplaySizeControlElements;
  const control = new DisplaySizeControl({
    initial: options.initial ?? 1,
    elements,
    request,
    clock,
  });
  control.mount();
  return {
    control, slider, output, status, error, presets, clock, sentScales, requests,
    input(scale: number) {
      slider.value = String(scale);
      slider.dispatch("input");
    },
    flush() { clock.flush(); },
    async settle() { await Promise.resolve(); await Promise.resolve(); },
  };
}

describe("DisplaySizeControl", () => {
  it.each([
    [0.49, 0.5],
    [0.5, 0.5],
    [0.725, 0.75],
    [1.025, 1.05],
    [1.075, 1.1],
    [1.175, 1.2],
    [1.275, 1.3],
    [1.325, 1.35],
    [1.425, 1.45],
    [1.075 - 1e-12, 1.05],
    [1.075 + 1e-12, 1.1],
    [1.5, 1.5],
    [1.51, 1.5],
  ])("normalizes actual scale %s to selectable range value %s", (actual, selectable) => {
    expect(nearestSelectableScale(actual)).toBe(selectable);
  });

  it("renders the persisted v2 scale as the initial confirmed value", () => {
    const view = mount({ initial: 1.15 });

    expect(view.slider.value).toBe("1.15");
    expect(view.output.textContent).toBe("实际 115%");
    expect(view.presets[0]?.disabled).toBe(false);
    expect(view.sentScales).toEqual([]);
  });

  it("separates an off-step initial actual value from its nearest slider position", () => {
    const view = mount({ initial: 1.1875 });

    expect(view.slider.value).toBe("1.2");
    expect(view.output.textContent).toBe("实际 119%");
    expect(view.slider.attributes.get("aria-valuetext")).toBe("实际 119%，滑杆档位 120%");
    expect(view.sentScales).toEqual([]);
  });

  it("previews immediately and trails rapid input by 100ms with one request", () => {
    const view = mount();

    view.input(1.2);
    expect(view.output.textContent).toBe("选择 120%");
    expect(view.slider.attributes.get("aria-valuetext")).toBe("选择档位 120%");
    view.input(1.25);
    expect(view.clock.pendingCount).toBe(1);
    expect(view.clock.delays).toEqual([100, 100]);
    expect(view.sentScales).toEqual([]);
    view.clock.advance(99);
    expect(view.sentScales).toEqual([]);
    view.clock.advance(1);

    expect(view.sentScales).toEqual([1.25]);
    expect(view.status.textContent).toContain("125%");
  });

  it.each(["change", "pointerup"])("flushes the final value immediately on %s", (event) => {
    const view = mount();
    view.input(1.2);

    view.slider.dispatch(event);

    expect(view.sentScales).toEqual([1.2]);
    expect(view.clock.pendingCount).toBe(0);
  });

  it("flushes keyboard adjustments immediately", () => {
    const view = mount();
    view.slider.value = "0.95";
    view.slider.dispatch("input");

    view.slider.dispatch("keyup", { key: "ArrowLeft" });

    expect(view.sentScales).toEqual([0.95]);
    expect(view.clock.pendingCount).toBe(0);
  });

  it("keeps at most one request pending and coalesces later input to the latest value", async () => {
    const view = mount();
    view.input(1.1);
    view.flush();
    view.input(1.2);
    view.input(1.3);
    view.flush();

    expect(view.sentScales).toEqual([1.1]);
    view.requests[0]?.resolve(success(1.1));
    await view.settle();

    expect(view.sentScales).toEqual([1.1, 1.3]);
    expect(view.requests).toHaveLength(2);
  });

  it("confirms and displays the actual ACK scale even when it is off step", async () => {
    const view = mount();
    view.input(1.2);
    view.flush();
    view.requests[0]?.resolve(success(1.2, 1.1875));

    await view.settle();

    expect(view.slider.value).toBe("1.2");
    expect(view.output.textContent).toBe("实际 119%");
    expect(view.slider.attributes.get("aria-valuetext")).toBe("实际 119%，滑杆档位 120%");
    expect(view.status.textContent).toContain("119%");
    expect(view.error.textContent).toBe("");
    expect(view.sentScales).toEqual([1.2]);
  });

  it("restores an off-step confirmed actual value and exposes a retryable alert on failure", async () => {
    const view = mount({ initial: 1.1875 });
    view.input(1.25);
    view.flush();
    view.requests[0]?.resolve(failure(1.25));

    await view.settle();

    expect(view.slider.value).toBe("1.2");
    expect(view.output.textContent).toBe("实际 119%");
    expect(view.slider.attributes.get("aria-valuetext")).toBe("实际 119%，滑杆档位 120%");
    expect(view.error.textContent).toContain("resize failed");
    expect(view.error.textContent).toContain("重试");
    expect(view.slider.disabled).toBe(false);
  });

  it("continues with the newest desired value after an older request fails", async () => {
    const view = mount();
    view.input(1.1);
    view.flush();
    view.input(1.35);
    view.requests[0]?.resolve(failure(1.1));

    await view.settle();

    expect(view.slider.value).toBe("1.35");
    expect(view.sentScales).toEqual([1.1, 1.35]);
  });

  it("routes presets through the same queue while a request is pending", async () => {
    const view = mount();
    view.presets[2]?.dispatch("click");
    expect(view.sentScales).toEqual([1.25]);
    expect(view.output.textContent).toBe("选择 125%");

    view.presets[0]?.dispatch("click");
    expect(view.sentScales).toEqual([1.25]);
    view.requests[0]?.resolve(success(1.25));
    await view.settle();

    expect(view.sentScales).toEqual([1.25, 0.75]);
  });

  it("does not request the confirmed value or duplicate a queued value", async () => {
    const view = mount();
    view.input(1);
    view.flush();
    expect(view.sentScales).toEqual([]);

    view.input(1.2);
    view.flush();
    view.input(1.2);
    view.flush();
    expect(view.sentScales).toEqual([1.2]);
    view.requests[0]?.resolve(success(1.2));
    await view.settle();
    expect(view.sentScales).toEqual([1.2]);
  });

  it("ignores out-of-range and NaN DOM values without sending them", () => {
    const view = mount({ initial: 1.15 });
    for (const value of ["0.49", "1.51", "NaN"]) {
      view.slider.value = value;
      view.slider.dispatch("input");
      view.flush();
      expect(view.slider.value).toBe("1.15");
      expect(view.output.textContent).toBe("实际 115%");
    }
    expect(view.sentScales).toEqual([]);
  });

  it("rejects invalid initial state instead of silently assuming 100%", () => {
    const slider = new FakeElement();
    const elements = {
      slider,
      output: new FakeElement(),
      status: new FakeElement(),
      error: new FakeElement(),
      presets: [],
    } as unknown as DisplaySizeControlElements;

    expect(() => new DisplaySizeControl({
      initial: Number.NaN,
      elements,
      request: vi.fn(),
    })).toThrow(RangeError);
  });

  it("clears a queued timer and every listener when destroyed before debounce", () => {
    const view = mount();
    view.input(1.2);
    expect(view.clock.pendingCount).toBe(1);

    view.control.destroy();
    view.clock.advance(1_000);
    view.slider.value = "1.3";
    for (const event of ["input", "change", "pointerup", "keyup"]) {
      view.slider.dispatch(event, event === "keyup" ? { key: "ArrowRight" } : {});
    }
    for (const preset of view.presets) preset.dispatch("click");

    expect(view.sentScales).toEqual([]);
    expect(view.clock.pendingCount).toBe(0);
    expect(view.slider.listenerCount()).toBe(0);
    expect(view.presets.map((preset) => preset.listenerCount())).toEqual([0, 0, 0]);
  });

  it("does not mutate the DOM or continue the queue when a pending result arrives after destroy", async () => {
    const view = mount();
    view.input(1.2);
    view.flush();
    view.control.destroy();
    const before = {
      slider: view.slider.value,
      output: view.output.textContent,
      status: view.status.textContent,
    };
    view.requests[0]?.resolve(success(1.2));
    await view.settle();

    expect({
      slider: view.slider.value,
      output: view.output.textContent,
      status: view.status.textContent,
    }).toEqual(before);
    expect(view.sentScales).toEqual([1.2]);
  });

  it("treats a rejected bridge request like a failure and allows retry", async () => {
    const request = vi.fn()
      .mockRejectedValueOnce(new Error("request timed out"))
      .mockResolvedValueOnce(success(1.25));
    const view = mount({ request });
    view.input(1.25);
    view.flush();
    await view.settle();
    expect(view.slider.value).toBe("1");
    expect(view.error.textContent).toContain("request timed out");

    view.presets[2]?.dispatch("click");
    await view.settle();
    expect(request).toHaveBeenCalledTimes(2);
    expect(view.slider.value).toBe("1.25");
  });
});

describe("display size settings assembly", () => {
  it("does not mount after page teardown wins a deferred preference load", async () => {
    const loaded = deferred<number>();
    const control = { mount: vi.fn(), destroy: vi.fn() };
    const createControl = vi.fn(() => control);
    const lifecycle = initializeDisplaySizeControl({
      loadInitial: () => loaded.promise,
      createControl,
      onError: vi.fn(),
    });

    lifecycle.destroy();
    loaded.resolve(1.25);
    await lifecycle.ready;

    expect(createControl).not.toHaveBeenCalled();
    expect(control.mount).not.toHaveBeenCalled();
    expect(control.destroy).not.toHaveBeenCalled();
  });

  it("destroys an already-mounted control during page teardown", async () => {
    const control = { mount: vi.fn(), destroy: vi.fn() };
    const lifecycle = initializeDisplaySizeControl({
      loadInitial: async () => 1.25,
      createControl: vi.fn(() => control),
      onError: vi.fn(),
    });
    await lifecycle.ready;

    lifecycle.destroy();

    expect(control.mount).toHaveBeenCalledOnce();
    expect(control.destroy).toHaveBeenCalledOnce();
  });

  it("provides an accessible percentage scale and three practical presets", () => {
    const html = readFileSync(new URL("../../settings.html", import.meta.url), "utf8");
    const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

    expect(html).toContain('id="display-size-slider"');
    expect(html).toContain('min="0.5" max="1.5" step="0.05"');
    const outputTag = html.match(/<output\b[^>]*\bid="display-size-output"[^>]*>/)?.[0];
    const outputAttributes = Object.fromEntries(
      [...(outputTag ?? "").matchAll(/([\w-]+)="([^"]*)"/g)].map((match) => [match[1], match[2]]),
    );
    const sliderLabel = html.match(/<label\b[^>]*\bfor="display-size-slider"[^>]*>([^<]+)<\/label>/);
    expect(outputAttributes).toEqual({ id: "display-size-output", for: "display-size-slider" });
    expect(sliderLabel?.[1]).toBe("桌面宠物显示比例");
    expect(html.match(/data-display-scale=/g)).toHaveLength(3);
    expect(html).toContain('data-display-scale="0.75"');
    expect(html).toContain('data-display-scale="1"');
    expect(html).toContain('data-display-scale="1.25"');
    expect(html).toContain('id="display-size-status" role="status" aria-live="polite"');
    expect(html).toContain('id="display-size-error" role="alert"');
    expect(css).toContain(".display-size-scale");
    expect(css).toContain(".display-size-presets");
    for (const position of [0, 25, 50, 75, 100]) {
      expect(html).toContain(`data-position="${position}"`);
      expect(css).toContain(`.display-size-ticks span[data-position="${position}"]`);
    }
    expect(css).toContain("@media (max-width: 520px)");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
  });
});
