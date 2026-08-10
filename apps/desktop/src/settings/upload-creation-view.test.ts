import { describe, expect, it, vi } from "vitest";
import type { CreationSnapshot } from "../creation/contracts";
import { validMotionProfile } from "../runtime/animated-image-test-fixtures";
import {
  UploadCreationView,
  type UploadCreationDomPorts,
  type UploadCreationPorts,
} from "./upload-creation-view";

class FakeElement {
  hidden = false;
  disabled = false;
  textContent = "";
  value = "";
  src = "";
  files: Array<{ arrayBuffer(): Promise<ArrayBuffer> }> | null = null;
  style = { display: "" };
  classList = { toggle: vi.fn() };
  children: unknown[] = [];
  private readonly listeners = new Map<string, Set<EventListener>>();

  addEventListener(type: string, listener: EventListener): void {
    const listeners = this.listeners.get(type) ?? new Set<EventListener>();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: EventListener): void {
    this.listeners.get(type)?.delete(listener);
  }

  dispatch(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener({ preventDefault() {} } as Event);
    }
  }

  replaceChildren(...children: unknown[]): void {
    this.children = children;
  }

  append(...children: unknown[]): void {
    this.children.push(...children);
  }

  setAttribute(): void {}
  removeAttribute(): void {}

  listenerCount(): number {
    return Array.from(this.listeners.values()).reduce((sum, listeners) => sum + listeners.size, 0);
  }
}

function snapshot(overrides: Partial<CreationSnapshot> = {}): CreationSnapshot {
  return {
    sessionId: "session-1",
    petId: "pet-1",
    method: "upload",
    status: "draft",
    lastStableStatus: "draft",
    currentStep: "upload",
    displayName: null,
    jobId: null,
    jobStatus: null,
    candidateId: null,
    recipe: null,
    error: null,
    ...overrides,
  };
}

function uploadPorts(options: { candidateReady?: boolean; method?: CreationSnapshot["method"] } = {}) {
  const restored = snapshot({
    method: options.method ?? "upload",
    ...(options.candidateReady ? {
    status: "candidateReady",
    lastStableStatus: "candidateReady",
    currentStep: "review",
    jobId: "job-1",
    jobStatus: "success",
    candidateId: "job-1",
    } : {}),
  });
  const creation = {
    start: vi.fn(async () => snapshot()),
    draft: vi.fn(async () => restored),
    snapshot: vi.fn(async () => restored),
    setName: vi.fn(async (_sessionId: string, displayName: string) => snapshot({
      ...restored,
      displayName: displayName.trim(),
    })),
    abandon: vi.fn(async () => undefined),
    uploadStart: vi.fn(async () => "job-1"),
    uploadJobs: vi.fn(async () => []),
  };
  const finalize = vi.fn<UploadCreationPorts["finalize"]>(async () => ({
    ok: true as const,
    requestId: "request-1",
    petId: "pet-1",
  }));
  return {
    creation,
    finalize,
  } satisfies UploadCreationPorts;
}

function domPorts(overrides: Partial<UploadCreationDomPorts> = {}) {
  const elements = {
    apiKeyInput: new FakeElement(),
    saveKeyButton: new FakeElement(),
    keyStatus: new FakeElement(),
    photoInput: new FakeElement(),
    photoPreview: new FakeElement(),
    status: new FakeElement(),
    stepUpload: new FakeElement(),
    stepGenerating: new FakeElement(),
    stepReview: new FakeElement(),
    stepComplete: new FakeElement(),
    jobGrid: new FakeElement(),
    candidateGrid: new FakeElement(),
    nameInput: new FakeElement(),
    nameError: new FakeElement(),
    nextButton: new FakeElement(),
    cancelButton: new FakeElement(),
    retryButton: new FakeElement(),
    abandonButton: new FakeElement(),
    finishButton: new FakeElement(),
  };
  const preview = { show: vi.fn(async () => undefined), clear: vi.fn() };
  const ports: UploadCreationDomPorts = {
    elements: elements as unknown as UploadCreationDomPorts["elements"],
    createElement: vi.fn(() => new FakeElement() as unknown as HTMLElement),
    loadApiKey: vi.fn(async () => "saved-key"),
    saveApiKey: vi.fn(async () => undefined),
    loadCandidate: vi.fn(async () => ({
      schemaVersion: 3 as const,
      bodyUrl: "data:image/png;base64,AA==",
      motionProfile: validMotionProfile(),
    })),
    preview,
    setInterval: vi.fn(() => 1),
    clearInterval: vi.fn(),
    createObjectURL: vi.fn(() => "blob:photo"),
    revokeObjectURL: vi.fn(),
    confirm: vi.fn(() => true),
    onCancel: vi.fn(),
    onAbandoned: vi.fn(),
    ...overrides,
  };
  return { ports, elements, preview };
}

describe("UploadCreationView", () => {
  it("starts upload through a durable creation session", async () => {
    const ports = uploadPorts();
    const view = new UploadCreationView(ports);

    await view.start();

    expect(ports.creation.start).toHaveBeenCalledWith("upload");
    expect(view.snapshot().step).toBe("upload");
  });

  it("requires a valid name before finalization", async () => {
    const ports = uploadPorts({ candidateReady: true });
    const view = new UploadCreationView(ports);
    await view.restore("session-1");

    await expect(view.finish("\n")).rejects.toThrow("名称");

    expect(ports.finalize).not.toHaveBeenCalled();
  });

  it("restores the durable upload draft through its session snapshot", async () => {
    const ports = uploadPorts({ candidateReady: true });
    const view = new UploadCreationView(ports);

    await view.open();

    expect(ports.creation.draft).toHaveBeenCalledOnce();
    expect(ports.creation.snapshot).toHaveBeenCalledWith("session-1");
    expect(ports.creation.start).not.toHaveBeenCalled();
    expect(view.snapshot().step).toBe("review");
  });

  it("leaves a draft from another creation method for its own entry", async () => {
    const ports = uploadPorts({ method: "composer" });
    const view = new UploadCreationView(ports);

    await expect(view.open()).rejects.toThrow("其他创建方式");

    expect(ports.creation.snapshot).not.toHaveBeenCalled();
    expect(ports.creation.start).not.toHaveBeenCalled();
  });

  it("submits the photo with its real SHA-256 digest", async () => {
    const ports = uploadPorts();
    const view = new UploadCreationView(ports);
    await view.start();

    await view.submit(new TextEncoder().encode("abc"));

    expect(ports.creation.uploadStart).toHaveBeenCalledWith(
      "session-1",
      expect.stringContaining("Subject: a cat"),
      "YWJj",
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(view.snapshot().step).toBe("generating");
  });

  it("coalesces a double click into one finalization", async () => {
    const ports = uploadPorts({ candidateReady: true });
    let resolveFinalize!: (result: Awaited<ReturnType<typeof ports.finalize>>) => void;
    ports.finalize.mockImplementation(() => new Promise((resolve) => {
      resolveFinalize = resolve;
    }));
    const view = new UploadCreationView(ports);
    await view.restore("session-1");

    const first = view.finish("团子");
    const second = view.finish("团子");
    await vi.waitFor(() => expect(ports.finalize).toHaveBeenCalledOnce());
    resolveFinalize({ ok: true, requestId: "request-1", petId: "pet-1" });
    await Promise.all([first, second]);

    expect(ports.creation.setName).toHaveBeenCalledOnce();
    expect(view.snapshot().step).toBe("complete");
  });

  it("keeps review available when finalization returns false", async () => {
    const ports = uploadPorts({ candidateReady: true });
    ports.finalize.mockResolvedValue({
      ok: false,
      requestId: "request-1",
      petId: "pet-1",
      code: "blank-frame",
      message: "首帧为空",
    });
    const view = new UploadCreationView(ports);
    await view.restore("session-1");

    await expect(view.finish("团子")).resolves.toMatchObject({ ok: false });

    expect(view.snapshot().step).toBe("review");
  });

  it("abandons the durable session idempotently", async () => {
    const ports = uploadPorts({ candidateReady: true });
    const view = new UploadCreationView(ports);
    await view.restore("session-1");

    await Promise.all([view.abandon(), view.abandon()]);
    await view.abandon();

    expect(ports.creation.abandon).toHaveBeenCalledOnce();
    expect(ports.creation.abandon).toHaveBeenCalledWith("session-1");
    expect(view.snapshot().sessionId).toBeNull();
  });

  it("enables finalization only after a v3 body and motion profile mount dynamically", async () => {
    const core = uploadPorts({ candidateReady: true });
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();
    dom.elements.nameInput.value = "团子";
    dom.elements.nameInput.dispatch("input");

    expect(dom.ports.loadCandidate).toHaveBeenCalledWith("job-1");
    expect(dom.preview.show).toHaveBeenCalledOnce();
    expect(dom.elements.finishButton.disabled).toBe(false);
  });

  it("wires the final action once and renders success only after an ok result", async () => {
    const core = uploadPorts({ candidateReady: true });
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "团子";
    dom.elements.nameInput.dispatch("input");

    dom.elements.finishButton.dispatch("click");
    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());

    expect(dom.elements.stepComplete.hidden).toBe(false);
    expect(dom.elements.status.textContent).toContain("出现在桌面");
  });

  it("ignores a snapshot poll that settles after leaving the visit", async () => {
    const core = uploadPorts();
    const generating = snapshot({
      currentStep: "generating",
      jobId: "job-1",
      jobStatus: "running",
    });
    core.creation.draft.mockResolvedValue(generating);
    core.creation.snapshot.mockResolvedValueOnce(generating);
    let tick: (() => void) | undefined;
    const dom = domPorts({
      setInterval: vi.fn((callback) => {
        tick = callback;
        return 7;
      }),
    });
    let resolvePoll!: (value: CreationSnapshot) => void;
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    expect((dom.elements.jobGrid.children[0] as FakeElement).textContent).toContain("生成中");
    core.creation.snapshot.mockImplementationOnce(() => new Promise((resolve) => {
      resolvePoll = resolve;
    }));

    tick!();
    await vi.waitFor(() => expect(core.creation.snapshot).toHaveBeenCalledTimes(2));
    view.leave();
    resolvePoll(snapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      jobId: "job-1",
      candidateId: "job-1",
    }));
    await Promise.resolve();
    await Promise.resolve();

    expect(view.snapshot().step).toBe("generating");
    expect(dom.preview.show).not.toHaveBeenCalled();
    expect(dom.ports.clearInterval).toHaveBeenCalledWith(7);
  });

  it("destroys a pending dynamic preview when leaving and never revives its DOM", async () => {
    const core = uploadPorts({ candidateReady: true });
    let finishPreview!: () => void;
    const preview = {
      show: vi.fn(() => new Promise<void>((resolve) => {
        finishPreview = resolve;
      })),
      clear: vi.fn(),
    };
    const dom = domPorts({ preview });
    dom.elements.nameInput.value = "团子";
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    const entering = view.enter();
    await vi.waitFor(() => expect(preview.show).toHaveBeenCalledOnce());
    view.leave();
    finishPreview();
    await entering;

    expect(preview.clear).toHaveBeenCalled();
    expect(dom.elements.candidateGrid.children).toEqual([]);
    expect(dom.elements.finishButton.disabled).toBe(true);
  });

  it("mounts listeners once and removes them idempotently", () => {
    const core = uploadPorts();
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);

    view.mount();
    const firstCount = Object.values(dom.elements).reduce(
      (sum, element) => sum + element.listenerCount(),
      0,
    );
    view.mount();
    const secondCount = Object.values(dom.elements).reduce(
      (sum, element) => sum + element.listenerCount(),
      0,
    );
    view.destroy();
    view.destroy();

    expect(firstCount).toBeGreaterThan(0);
    expect(secondCount).toBe(firstCount);
    expect(Object.values(dom.elements).reduce(
      (sum, element) => sum + element.listenerCount(),
      0,
    )).toBe(0);
  });

  it("wires one photo submission and automatically saves the API key", async () => {
    const core = uploadPorts();
    const dom = domPorts();
    dom.elements.photoInput.files = [{
      arrayBuffer: async () => new TextEncoder().encode("abc").buffer,
    }];
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.apiKeyInput.value = " key-1 ";

    dom.elements.photoInput.dispatch("change");
    await vi.waitFor(() => expect(dom.ports.createObjectURL).toHaveBeenCalledOnce());
    dom.elements.nextButton.dispatch("click");
    dom.elements.nextButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.uploadStart).toHaveBeenCalledOnce());

    expect(dom.ports.saveApiKey).toHaveBeenCalledWith("key-1");
    expect(view.snapshot().step).toBe("generating");
  });

  it("keeps the final action disabled when the candidate lacks v3 dynamic capability", async () => {
    const core = uploadPorts({ candidateReady: true });
    const dom = domPorts({
      loadCandidate: vi.fn(async () => ({
        schemaVersion: 2,
        bodyUrl: "data:image/png;base64,AA==",
        motionProfile: validMotionProfile(),
      })),
    });
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();
    dom.elements.nameInput.value = "团子";
    dom.elements.nameInput.dispatch("input");

    expect(dom.elements.finishButton.disabled).toBe(true);
    expect(dom.preview.show).not.toHaveBeenCalled();
    expect(dom.elements.status.textContent).toContain("重新生成");
    dom.elements.finishButton.dispatch("click");
    await Promise.resolve();
    expect(core.finalize).not.toHaveBeenCalled();
  });

  it("abandons a rejected candidate before starting a fresh retry session", async () => {
    const core = uploadPorts({ candidateReady: true });
    core.creation.start.mockResolvedValue(snapshot({ sessionId: "session-2", petId: "pet-2" }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "团子";
    dom.elements.nameInput.dispatch("input");

    dom.elements.retryButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.start).toHaveBeenCalledWith("upload"));

    expect(core.creation.abandon).toHaveBeenCalledWith("session-1");
    expect(view.snapshot().sessionId).toBe("session-2");
    expect(view.snapshot().step).toBe("upload");
    expect(dom.elements.status.textContent).toContain("重新选择");
  });

  it("wires abandon idempotently from the review action", async () => {
    const core = uploadPorts({ candidateReady: true });
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();

    dom.elements.abandonButton.dispatch("click");
    dom.elements.abandonButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.abandon).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(dom.ports.onAbandoned).toHaveBeenCalledOnce());
  });

  it("does not render a finalization result after leaving the visit", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveFinalize!: (value: Awaited<ReturnType<UploadCreationPorts["finalize"]>>) => void;
    core.finalize.mockImplementation(() => new Promise((resolve) => {
      resolveFinalize = resolve;
    }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "团子";
    dom.elements.nameInput.dispatch("input");

    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());
    view.leave();
    resolveFinalize({ ok: true, requestId: "request-1", petId: "pet-1" });
    await Promise.resolve();
    await Promise.resolve();

    expect(dom.elements.stepComplete.hidden).toBe(true);
    expect(dom.elements.status.textContent).not.toContain("出现在桌面");
  });

  it("restores a generation failure with an actionable retry message", async () => {
    const core = uploadPorts();
    const failed = snapshot({
      status: "retryableFailure",
      currentStep: "upload",
      error: "第三方服务暂不可用",
    });
    core.creation.draft.mockResolvedValue(failed);
    core.creation.snapshot.mockResolvedValue(failed);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(view.snapshot().step).toBe("upload");
    expect(dom.elements.status.textContent).toContain("重新选择照片");
    expect(dom.elements.status.textContent).toContain("第三方服务暂不可用");
  });

  it("restores the backend-normalized name with a ready candidate", async () => {
    const core = uploadPorts({ candidateReady: true });
    const named = snapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      jobId: "job-1",
      candidateId: "job-1",
      displayName: "团子",
    });
    core.creation.draft.mockResolvedValue(named);
    core.creation.snapshot.mockResolvedValue(named);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(dom.elements.nameInput.value).toBe("团子");
    expect(dom.elements.finishButton.disabled).toBe(false);
  });
});
