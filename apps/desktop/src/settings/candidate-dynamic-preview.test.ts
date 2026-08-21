import { describe, expect, it, vi } from "vitest";
import { validMotionProfile } from "../runtime/animated-image-test-fixtures";
import type { PetRenderer } from "../runtime/pet-renderer";
import {
  CandidatePreviewController,
  mountCandidateDynamicPreview,
  type CandidatePreviewPorts,
} from "./candidate-dynamic-preview";

function fakeRenderer(): PetRenderer {
  return {
    load: vi.fn(async () => undefined),
    resize: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    setExpression: vi.fn(),
    setLookTarget: vi.fn(),
    setLipSync: vi.fn(),
    setCalibration: vi.fn(),
    hitTest: vi.fn(() => null),
    setVisibility: vi.fn(),
    update: vi.fn(),
    destroy: vi.fn(),
  };
}

function previewHarness(options: { reducedMotion?: boolean } = {}) {
  const callbacks = new Map<number, FrameRequestCallback>();
  const renderers: PetRenderer[] = [];
  let reducedMotion = options.reducedMotion ?? false;
  let reducedMotionListener: ((reduced: boolean) => void) | undefined;
  let nextFrame = 0;
  let resizeCallback: ResizeObserverCallback | undefined;
  const observer = { observe: vi.fn(), disconnect: vi.fn() };
  const requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
    const id = ++nextFrame;
    callbacks.set(id, callback);
    return id;
  });
  const cancelAnimationFrame = vi.fn((id: number) => callbacks.delete(id));
  const root = {
    replaceChildren: vi.fn(),
    getBoundingClientRect: vi.fn(() => ({ width: 320, height: 360 })),
  } as unknown as HTMLElement;
  const ports: CandidatePreviewPorts = {
    createRenderer: vi.fn(() => {
      const renderer = fakeRenderer();
      renderers.push(renderer);
      return renderer;
    }),
    requestAnimationFrame,
    cancelAnimationFrame,
    createResizeObserver: vi.fn((callback) => {
      resizeCallback = callback;
      return observer;
    }),
    devicePixelRatio: () => 2,
    prefersReducedMotion: () => reducedMotion,
    onReducedMotionChange: (listener: (reduced: boolean) => void) => {
      reducedMotionListener = listener;
      return () => { reducedMotionListener = undefined; };
    },
  };
  return {
    root,
    ports,
    renderers,
    observer,
    cancelAnimationFrame,
    get renderer() { return renderers[0]!; },
    fireFrame(timestamp: number) {
      const entry = callbacks.entries().next().value as [number, FrameRequestCallback] | undefined;
      if (!entry) throw new Error("no animation frame queued");
      callbacks.delete(entry[0]);
      entry[1](timestamp);
    },
    fireResize() {
      resizeCallback?.([], observer as unknown as ResizeObserver);
    },
    queuedFrames() {
      return callbacks.size;
    },
    setReducedMotion(value: boolean) {
      reducedMotion = value;
      reducedMotionListener?.(value);
    },
  };
}

describe("candidate dynamic preview", () => {
  it("runs the shared animated renderer until the preview is destroyed", async () => {
    const test = previewHarness();
    const handle = await mountCandidateDynamicPreview(
      test.root,
      "data:image/png;base64,AA==",
      validMotionProfile(),
      test.ports,
    );

    expect(test.renderer.load).toHaveBeenCalledWith(expect.objectContaining({
      kind: "animated-image",
      imageUrl: "data:image/png;base64,AA==",
      motionProfile: validMotionProfile(),
    }));
    expect(test.renderer.resize).toHaveBeenCalledWith({ width: 320, height: 360, dpr: 2 });
    expect(test.renderer.setVisibility).toHaveBeenCalledWith(true);
    expect(test.renderer.playMotion).toHaveBeenCalledWith("idle", { loop: true });

    test.fireFrame(16);
    expect(test.renderer.update).not.toHaveBeenCalled();
    expect(test.queuedFrames()).toBe(1);
    test.fireFrame(32);
    expect(test.renderer.update).toHaveBeenCalledWith(16);
    expect(test.queuedFrames()).toBe(1);

    handle.destroy();
    expect(test.cancelAnimationFrame).toHaveBeenCalledOnce();
    expect(test.observer.disconnect).toHaveBeenCalledOnce();
    expect(test.renderer.destroy).toHaveBeenCalledOnce();
    expect(test.root.replaceChildren).toHaveBeenLastCalledWith();
    expect(test.queuedFrames()).toBe(0);
  });

  it("resizes from the current container bounds and device pixel ratio", async () => {
    const test = previewHarness();
    await mountCandidateDynamicPreview(test.root, "image", validMotionProfile(), test.ports);
    vi.mocked(test.root.getBoundingClientRect).mockReturnValue({
      width: 480,
      height: 270,
    } as DOMRect);

    test.fireResize();

    expect(test.renderer.resize).toHaveBeenLastCalledWith({ width: 480, height: 270, dpr: 2 });
    expect(test.observer.observe).toHaveBeenCalledWith(test.root);
  });

  it("destroys idempotently", async () => {
    const test = previewHarness();
    const handle = await mountCandidateDynamicPreview(test.root, "image", validMotionProfile(), test.ports);

    handle.destroy();
    handle.destroy();

    expect(test.cancelAnimationFrame).toHaveBeenCalledOnce();
    expect(test.observer.disconnect).toHaveBeenCalledOnce();
    expect(test.renderer.destroy).toHaveBeenCalledOnce();
  });

  it("does not start idle animation or RAF under reduced motion and follows preference changes", async () => {
    const test = previewHarness({ reducedMotion: true });
    const handle = await mountCandidateDynamicPreview(
      test.root,
      "image",
      validMotionProfile(),
      test.ports,
    );

    expect(test.renderer.playMotion).not.toHaveBeenCalled();
    expect(test.queuedFrames()).toBe(0);

    test.setReducedMotion(false);
    expect(test.renderer.playMotion).toHaveBeenCalledWith("idle", { loop: true });
    expect(test.queuedFrames()).toBe(1);

    test.setReducedMotion(true);
    expect(test.queuedFrames()).toBe(0);
    handle.destroy();
  });

  it("destroys the previous candidate before mounting a replacement", async () => {
    const test = previewHarness();
    const controller = new CandidatePreviewController(test.ports);
    await controller.show(test.root, "first", validMotionProfile());
    await controller.show(test.root, "second", validMotionProfile());

    expect(test.renderers[0]!.destroy).toHaveBeenCalledOnce();
    expect(test.renderers[1]!.destroy).not.toHaveBeenCalled();
  });

  it("does not revive a pending preview after the controller is cleared", async () => {
    const test = previewHarness();
    let finishLoad!: () => void;
    vi.mocked(test.ports.createRenderer).mockImplementation(() => {
      const renderer = fakeRenderer();
      vi.mocked(renderer.load).mockImplementation(() => new Promise<void>((resolve) => {
        finishLoad = resolve;
      }));
      test.renderers.push(renderer);
      return renderer;
    });
    const controller = new CandidatePreviewController(test.ports);

    const showing = controller.show(test.root, "pending", validMotionProfile());
    controller.clear();
    finishLoad();
    await showing;

    expect(test.renderer.destroy).toHaveBeenCalledOnce();
    expect(test.queuedFrames()).toBe(0);
    controller.clear();
    expect(test.renderer.destroy).toHaveBeenCalledOnce();
  });

  it("finishes stale cleanup before creating a replacement renderer", async () => {
    const test = previewHarness();
    let finishFirstLoad!: () => void;
    vi.mocked(test.ports.createRenderer).mockImplementation(() => {
      const renderer = fakeRenderer();
      if (test.renderers.length === 0) {
        vi.mocked(renderer.load).mockImplementation(() => new Promise<void>((resolve) => {
          finishFirstLoad = resolve;
        }));
      }
      test.renderers.push(renderer);
      return renderer;
    });
    const controller = new CandidatePreviewController(test.ports);

    const first = controller.show(test.root, "first", validMotionProfile());
    const second = controller.show(test.root, "second", validMotionProfile());

    expect(test.renderers).toHaveLength(1);
    finishFirstLoad();
    await Promise.all([first, second]);

    expect(test.renderers).toHaveLength(2);
    expect(vi.mocked(test.root.replaceChildren).mock.invocationCallOrder.at(-1)).toBeLessThan(
      vi.mocked(test.ports.createRenderer).mock.invocationCallOrder[1]!,
    );
    expect(test.renderers[1]!.destroy).not.toHaveBeenCalled();
  });
});
