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
    start: vi.fn<UploadCreationPorts["creation"]["start"]>(async () => snapshot()),
    draft: vi.fn<UploadCreationPorts["creation"]["draft"]>(async () => restored),
    snapshot: vi.fn<UploadCreationPorts["creation"]["snapshot"]>(async () => restored),
    setName: vi.fn<UploadCreationPorts["creation"]["setName"]>(async (_sessionId, displayName) => snapshot({
      ...restored,
      displayName: displayName.trim(),
    })),
    abandon: vi.fn<UploadCreationPorts["creation"]["abandon"]>(async () => undefined),
    uploadStart: vi.fn<UploadCreationPorts["creation"]["uploadStart"]>(async () => "job-1"),
    uploadJobs: vi.fn<UploadCreationPorts["creation"]["uploadJobs"]>(async () => []),
    uploadSource: vi.fn<UploadCreationPorts["creation"]["uploadSource"]>(async () => null),
    recoverFinalization: vi.fn<UploadCreationPorts["creation"]["recoverFinalization"]>(async () => ({})),
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
    expect(dom.elements.status.textContent).toContain("放弃当前创建并开始新会话");
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

  it("loads candidate assets strictly from the snapshot job id", async () => {
    const core = uploadPorts({ candidateReady: true });
    const ready = snapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      candidateId: "candidate-1",
      jobId: "job-1",
      jobStatus: "success",
    });
    core.creation.draft.mockResolvedValue(ready);
    core.creation.snapshot.mockResolvedValue(ready);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(dom.ports.loadCandidate).toHaveBeenCalledTimes(1);
    expect(dom.ports.loadCandidate).toHaveBeenCalledWith("job-1");
    expect(dom.ports.loadCandidate).not.toHaveBeenCalledWith("candidate-1");
  });

  it("reports a candidate without a job id and never guesses from candidate id", async () => {
    const core = uploadPorts({ candidateReady: true });
    const ready = snapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      candidateId: "candidate-1",
      jobId: null,
    });
    core.creation.draft.mockResolvedValue(ready);
    core.creation.snapshot.mockResolvedValue(ready);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(dom.ports.loadCandidate).not.toHaveBeenCalled();
    expect(dom.elements.status.textContent).toMatch(/job|任务/i);
    expect(dom.elements.finishButton.disabled).toBe(true);
  });

  it("keeps finalize, retry, and abandon mutually exclusive including the set-name window", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveName!: (value: CreationSnapshot) => void;
    core.creation.setName.mockImplementation(() => new Promise((resolve) => { resolveName = resolve; }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "Mimi";
    dom.elements.nameInput.dispatch("input");

    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.setName).toHaveBeenCalledOnce());
    expect(dom.elements.finishButton.disabled).toBe(true);
    expect(dom.elements.retryButton.disabled).toBe(true);
    expect(dom.elements.abandonButton.disabled).toBe(true);
    dom.elements.retryButton.dispatch("click");
    dom.elements.abandonButton.dispatch("click");
    expect(core.creation.abandon).not.toHaveBeenCalled();

    resolveName(snapshot({
      status: "candidateReady", lastStableStatus: "candidateReady", currentStep: "review",
      jobId: "job-1", candidateId: "candidate-1", displayName: "Mimi",
    }));
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());
  });

  it("keeps only the newest file when arrayBuffer results settle out of order", async () => {
    const core = uploadPorts();
    const dom = domPorts({ createObjectURL: vi.fn((file: Blob) => `blob:${(file as Blob & { id: string }).id}`) });
    let resolveA!: (value: ArrayBuffer) => void;
    let resolveB!: (value: ArrayBuffer) => void;
    const fileA = { id: "a", arrayBuffer: () => new Promise<ArrayBuffer>((resolve) => { resolveA = resolve; }) };
    const fileB = { id: "b", arrayBuffer: () => new Promise<ArrayBuffer>((resolve) => { resolveB = resolve; }) };
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();

    dom.elements.photoInput.files = [fileA];
    dom.elements.photoInput.dispatch("change");
    dom.elements.photoInput.files = [fileB];
    dom.elements.photoInput.dispatch("change");
    resolveB(new TextEncoder().encode("b").buffer);
    await vi.waitFor(() => expect(dom.elements.photoPreview.src).toBe("blob:b"));
    resolveA(new TextEncoder().encode("a").buffer);
    await Promise.resolve();

    expect(dom.ports.createObjectURL).toHaveBeenCalledTimes(1);
    expect(dom.elements.photoPreview.src).toBe("blob:b");
  });

  it("does not create a photo URL after leave or destroy and revokes committed URLs once", async () => {
    const core = uploadPorts();
    const dom = domPorts();
    let resolve!: (value: ArrayBuffer) => void;
    const pending = { arrayBuffer: () => new Promise<ArrayBuffer>((done) => { resolve = done; }) };
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.photoInput.files = [pending];
    dom.elements.photoInput.dispatch("change");
    view.destroy();
    resolve(new Uint8Array([1]).buffer);
    await Promise.resolve();
    expect(dom.ports.createObjectURL).not.toHaveBeenCalled();

    const dom2 = domPorts({ createObjectURL: vi.fn(() => "blob:kept") });
    const view2 = new UploadCreationView(core, dom2.ports);
    view2.mount();
    await view2.enter();
    dom2.elements.photoInput.files = [{ arrayBuffer: async () => new Uint8Array([2]).buffer }];
    dom2.elements.photoInput.dispatch("change");
    await vi.waitFor(() => expect(dom2.ports.createObjectURL).toHaveBeenCalledOnce());
    view2.leave();
    view2.destroy();
    expect(dom2.ports.revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(dom2.ports.revokeObjectURL).toHaveBeenCalledWith("blob:kept");
  });

  it("does not apply an older enter after a newer enter has restored another session", async () => {
    const core = uploadPorts();
    let resolveFirstDraft!: (value: CreationSnapshot | null) => void;
    core.creation.draft
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFirstDraft = resolve; }))
      .mockResolvedValueOnce(snapshot({ sessionId: "session-2", petId: "pet-2" }));
    core.creation.snapshot.mockImplementation(async (sessionId) => snapshot({ sessionId, petId: sessionId === "session-2" ? "pet-2" : "pet-1" }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    const first = view.enter();
    await vi.waitFor(() => expect(core.creation.draft).toHaveBeenCalledTimes(1));
    const second = view.enter();
    await second;
    resolveFirstDraft(snapshot({ sessionId: "session-1" }));
    await first;

    expect(view.snapshot().sessionId).toBe("session-2");
  });

  it("restores a durable source photo and reuses its exact bytes for retry", async () => {
    const core = uploadPorts();
    const failed = snapshot({ status: "retryableFailure", currentStep: "upload", error: "provider down" });
    core.creation.draft.mockResolvedValue(failed);
    core.creation.snapshot.mockResolvedValue(failed);
    core.creation.uploadSource.mockResolvedValue({
      dataUrl: "data:image/png;base64,YWJj",
      refSha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    });
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.apiKeyInput.value = "key";

    expect(dom.elements.photoPreview.src).toBe("data:image/png;base64,YWJj");
    dom.elements.nextButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.uploadStart).toHaveBeenCalledOnce());
    expect(core.creation.uploadStart.mock.calls[0]?.[2]).toBe("YWJj");
    expect(core.creation.uploadStart.mock.calls[0]?.[3]).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  });

  it("recovers a durable draft when retry abandon succeeds but starting the replacement is lost", async () => {
    const core = uploadPorts({ candidateReady: true });
    core.creation.start.mockRejectedValueOnce(new Error("response lost"));
    core.creation.draft.mockResolvedValueOnce(snapshot({
      status: "candidateReady", lastStableStatus: "candidateReady", currentStep: "review",
      jobId: "job-1", candidateId: "candidate-1",
    })).mockResolvedValueOnce(snapshot({ sessionId: "session-2", petId: "pet-2" }));
    core.creation.snapshot.mockImplementation(async (sessionId) => snapshot({ sessionId, petId: "pet-2" }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();

    dom.elements.retryButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.draft).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(view.snapshot().sessionId).toBe("session-2"));
    expect(dom.elements.nextButton.hidden).toBe(false);
  });

  it("does not overlap a retry mutation after leave and reenter until the old owner settles", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveAbandon!: () => void;
    core.creation.abandon
      .mockImplementationOnce(() => new Promise<void>((resolve) => { resolveAbandon = resolve; }))
      .mockResolvedValue(undefined);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.retryButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.abandon).toHaveBeenCalledOnce());

    view.leave();
    await view.enter();
    dom.elements.retryButton.dispatch("click");
    expect(core.creation.abandon).toHaveBeenCalledOnce();

    resolveAbandon();
    await vi.waitFor(() => expect(dom.elements.retryButton.disabled).toBe(false));
    dom.elements.retryButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.abandon).toHaveBeenCalledTimes(2));
  });

  it("does not call finalization when set-name settles after leave", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveName!: (value: CreationSnapshot) => void;
    core.creation.setName.mockImplementation(() => new Promise((resolve) => { resolveName = resolve; }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "Mimi";
    dom.elements.nameInput.dispatch("input");
    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.setName).toHaveBeenCalledOnce());

    view.leave();
    resolveName(snapshot({
      status: "candidateReady", lastStableStatus: "candidateReady", currentStep: "review",
      jobId: "job-1", candidateId: "candidate-1", displayName: "Mimi",
    }));
    await Promise.resolve();

    expect(core.finalize).not.toHaveBeenCalled();
    expect(dom.elements.stepComplete.hidden).toBe(true);
  });

  it("waits for an unsettled finalization across leave and reenter without starting a new draft", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveFinalize!: (value: Awaited<ReturnType<UploadCreationPorts["finalize"]>>) => void;
    core.finalize.mockImplementation(() => new Promise((resolve) => { resolveFinalize = resolve; }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "Mimi";
    dom.elements.nameInput.dispatch("input");
    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());

    core.creation.snapshot.mockResolvedValue(snapshot({
      status: "completed", lastStableStatus: "completed", currentStep: "completed",
      displayName: "Mimi", jobId: "job-1", candidateId: "candidate-1",
    }));
    const reentering = view.enter();
    await Promise.resolve();
    expect(core.creation.draft).toHaveBeenCalledTimes(1);
    resolveFinalize({ ok: true, requestId: "request-old", petId: "pet-1" });
    await reentering;

    expect(view.snapshot().sessionId).toBe("session-1");
    expect(view.snapshot().step).toBe("complete");
    expect(core.creation.start).not.toHaveBeenCalled();
    expect(dom.elements.stepComplete.hidden).toBe(false);
  });

  it("never starts a new draft when an unsettled finalization snapshot is temporarily unavailable", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveFinalize!: (value: Awaited<ReturnType<UploadCreationPorts["finalize"]>>) => void;
    core.finalize.mockImplementation(() => new Promise((resolve) => { resolveFinalize = resolve; }));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();
    dom.elements.nameInput.value = "Mimi";
    dom.elements.nameInput.dispatch("input");
    dom.elements.finishButton.dispatch("click");
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());
    core.creation.snapshot.mockRejectedValue(new Error("database busy"));
    core.creation.draft.mockResolvedValue(null);

    const reentering = view.enter();
    resolveFinalize({ ok: true, requestId: "request-old", petId: "pet-1" });
    await reentering;

    expect(core.creation.start).not.toHaveBeenCalled();
    expect(dom.elements.status.textContent).toContain("database busy");
  });

  it("reconciles a finalizing draft to completed on entry", async () => {
    const core = uploadPorts();
    const finalizing = snapshot({
      status: "finalizing", lastStableStatus: "candidateReady", currentStep: "finalizing",
      displayName: "Mimi", jobId: "job-1", candidateId: "candidate-1",
    });
    const completed = snapshot({
      status: "completed", lastStableStatus: "completed", currentStep: "completed",
      displayName: "Mimi", jobId: "job-1", candidateId: "candidate-1",
    });
    core.creation.draft.mockResolvedValue(finalizing);
    core.creation.snapshot.mockResolvedValueOnce(finalizing).mockResolvedValueOnce(completed);
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(core.creation.recoverFinalization).toHaveBeenCalledOnce();
    expect(view.snapshot().step).toBe("complete");
    expect(core.creation.start).not.toHaveBeenCalled();
  });

  it("reconciles an uncommitted finalizing draft back to review when retry returns false", async () => {
    const core = uploadPorts();
    const finalizing = snapshot({
      status: "finalizing", lastStableStatus: "candidateReady", currentStep: "finalizing",
      displayName: "Mimi", jobId: "job-1", candidateId: "candidate-1",
    });
    const ready = snapshot({
      status: "candidateReady", lastStableStatus: "candidateReady", currentStep: "review",
      displayName: "Mimi", jobId: "job-1", candidateId: "candidate-1",
    });
    core.creation.draft.mockResolvedValue(finalizing);
    core.creation.snapshot
      .mockResolvedValueOnce(finalizing)
      .mockResolvedValueOnce(ready)
      .mockResolvedValueOnce(ready);
    core.finalize.mockResolvedValue({
      ok: false, requestId: "request-2", petId: "pet-1", code: "pet-window-unavailable", message: "window down",
    });
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();

    await view.enter();

    expect(core.creation.recoverFinalization).toHaveBeenCalledOnce();
    expect(core.finalize).toHaveBeenCalledWith("session-1");
    expect(view.snapshot().step).toBe("review");
  });

  it("rejects abandon while direct finalization owns the same session", async () => {
    const core = uploadPorts({ candidateReady: true });
    let resolveFinalize!: (value: Awaited<ReturnType<UploadCreationPorts["finalize"]>>) => void;
    core.finalize.mockImplementation(() => new Promise((resolve) => { resolveFinalize = resolve; }));
    const view = new UploadCreationView(core);
    await view.restore("session-1");
    const finishing = view.finish("Mimi");
    await vi.waitFor(() => expect(core.finalize).toHaveBeenCalledOnce());

    await expect(view.abandon()).rejects.toThrow(/同时放弃/);
    expect(core.creation.abandon).not.toHaveBeenCalled();
    resolveFinalize({ ok: true, requestId: "request-1", petId: "pet-1" });
    await finishing;
  });

  it("keeps direct submit mutually exclusive with finalize and abandon", async () => {
    const core = uploadPorts();
    let resolveUpload!: (jobId: string) => void;
    core.creation.uploadStart.mockImplementation(() => new Promise((resolve) => { resolveUpload = resolve; }));
    const view = new UploadCreationView(core);
    await view.start();

    const submitting = view.submit(new TextEncoder().encode("abc"));
    await vi.waitFor(() => expect(core.creation.uploadStart).toHaveBeenCalledOnce());
    await expect(view.finish("Mimi")).rejects.toThrow(/正在提交/);
    await expect(view.abandon()).rejects.toThrow(/正在提交/);
    expect(core.creation.setName).not.toHaveBeenCalled();
    expect(core.creation.abandon).not.toHaveBeenCalled();
    resolveUpload("job-1");
    await submitting;
  });

  it("offers a real start-again action when retry cannot create or recover a draft", async () => {
    const core = uploadPorts({ candidateReady: true });
    core.creation.draft
      .mockResolvedValueOnce(snapshot({
        status: "candidateReady", lastStableStatus: "candidateReady", currentStep: "review",
        jobId: "job-1", candidateId: "candidate-1",
      }))
      .mockResolvedValue(null);
    core.creation.start.mockRejectedValue(new Error("disk unavailable"));
    const dom = domPorts();
    const view = new UploadCreationView(core, dom.ports);
    view.mount();
    await view.enter();

    dom.elements.retryButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.start).toHaveBeenCalledTimes(2));
    expect(view.snapshot().sessionId).toBeNull();
    expect(dom.elements.nextButton.hidden).toBe(false);
    expect(dom.elements.status.textContent).toContain("重新开始");

    dom.elements.nextButton.dispatch("click");
    await vi.waitFor(() => expect(core.creation.start).toHaveBeenCalledTimes(3));
    expect(dom.elements.status.textContent).toContain("重试");
  });
});
