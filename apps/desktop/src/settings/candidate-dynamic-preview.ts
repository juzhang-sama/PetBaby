import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import { AnimatedImageRenderer } from "../runtime/animated-image-renderer";
import type { PetRenderer } from "../runtime/pet-renderer";

export interface CandidatePreviewHandle {
  destroy(): void;
}

interface CandidateResizeObserver {
  observe(target: Element): void;
  disconnect(): void;
}

export interface CandidatePreviewPorts {
  createRenderer(root: HTMLElement): PetRenderer;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(id: number): void;
  createResizeObserver(callback: ResizeObserverCallback): CandidateResizeObserver;
  devicePixelRatio(): number;
}

const browserPorts: CandidatePreviewPorts = {
  createRenderer: (root) => new AnimatedImageRenderer(root),
  requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
  cancelAnimationFrame: (id) => window.cancelAnimationFrame(id),
  createResizeObserver: (callback) => new ResizeObserver(callback),
  devicePixelRatio: () => window.devicePixelRatio || 1,
};

export async function mountCandidateDynamicPreview(
  root: HTMLElement,
  imageUrl: string,
  profile: MotionProfileV1,
  ports: CandidatePreviewPorts = browserPorts,
): Promise<CandidatePreviewHandle> {
  const renderer = ports.createRenderer(root);
  try {
    await renderer.load({ kind: "animated-image", imageUrl, motionProfile: profile });
  } catch (error) {
    renderer.destroy();
    root.replaceChildren();
    throw error;
  }

  let destroyed = false;
  let frameId: number | undefined;
  let previousTimestamp: number | undefined;
  const resize = () => {
    if (destroyed) return;
    const bounds = root.getBoundingClientRect();
    renderer.resize({
      width: bounds.width,
      height: bounds.height,
      dpr: ports.devicePixelRatio(),
    });
  };
  const resizeObserver = ports.createResizeObserver(resize);
  const update = (timestamp: number) => {
    if (destroyed) return;
    if (previousTimestamp !== undefined) renderer.update(timestamp - previousTimestamp);
    previousTimestamp = timestamp;
    frameId = ports.requestAnimationFrame(update);
  };

  resize();
  resizeObserver.observe(root);
  renderer.setVisibility(true);
  renderer.playMotion("idle", { loop: true });
  frameId = ports.requestAnimationFrame(update);

  return {
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (frameId !== undefined) ports.cancelAnimationFrame(frameId);
      resizeObserver.disconnect();
      renderer.destroy();
      root.replaceChildren();
    },
  };
}

export class CandidatePreviewController {
  private current: CandidatePreviewHandle | null = null;
  private pending: Promise<void> | null = null;
  private revision = 0;

  constructor(private readonly ports: CandidatePreviewPorts = browserPorts) {}

  async show(root: HTMLElement, imageUrl: string, profile: MotionProfileV1): Promise<void> {
    const revision = ++this.revision;
    this.destroyCurrent();
    const mount = async () => {
      if (revision !== this.revision) return;
      const handle = await mountCandidateDynamicPreview(root, imageUrl, profile, this.ports);
      if (revision !== this.revision) {
        handle.destroy();
        return;
      }
      this.current = handle;
    };
    const showing = this.pending ? this.pending.then(mount) : mount();
    const queueTail = showing.catch(() => undefined);
    this.pending = queueTail;
    void queueTail.finally(() => {
      if (this.pending === queueTail) this.pending = null;
    });
    await showing;
  }

  clear(): void {
    this.revision += 1;
    this.destroyCurrent();
  }

  private destroyCurrent(): void {
    this.current?.destroy();
    this.current = null;
  }
}
