import { describe, expect, it, vi } from "vitest";
import type { PhotoAvatarSnapshot, PhotoAvatarUpload } from "../creation/api";
import type { CreationSnapshot } from "../creation/contracts";
import {
  PhotoAvatarCreationView,
  type PhotoAvatarCreationDomPorts,
  type PhotoAvatarCreationPorts,
} from "./photo-avatar-creation-view";
import type { PhotoAvatarPreviewHandle } from "./photo-avatar-pixel-preview";

class FakeElement {
  hidden = false;
  disabled = false;
  textContent = "";
  value = "";
  files: Array<{ name: string; type: string; arrayBuffer(): Promise<ArrayBuffer> }> | null = null;
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

  focus(): void {}
}

function photo(name: string, type: string, bytes = [1, 2, 3]) {
  return { name, type, arrayBuffer: async () => new Uint8Array(bytes).buffer };
}

function avatarSnapshot(overrides: Partial<PhotoAvatarSnapshot> = {}): PhotoAvatarSnapshot {
  return {
    sessionId: "session-1",
    revision: 1,
    step: "collecting",
    providerJobId: null,
    profile: null,
    attempts: {},
    errorCode: null,
    errorMessage: null,
    ...overrides,
  };
}

function previewReadySnapshot(): PhotoAvatarSnapshot {
  return avatarSnapshot({
    step: "previewReady",
    profile: {
      bodyModuleId: "body-rounded-v1",
      bodyModuleSource: "ai-completed",
      completionSummary: ["tail", "bodyType"],
    },
  });
}

function previewReadyPixelSnapshot(
  styleProfileId: "pixel-style-v1" | "pixel-style-v2-animation-ready",
): PhotoAvatarSnapshot {
  return avatarSnapshot({
    route: "pixel-v1",
    step: "previewReady",
    profile: {
      schemaVersion: 1,
      species: "cat",
      styleProfileId,
      traits: [],
      completionSummary: [],
    },
  });
}

function viewHarness(options: {
  consented?: boolean;
  snapshot?: PhotoAvatarSnapshot;
  status?: Array<PhotoAvatarSnapshot | null>;
  previewHandle?: PhotoAvatarPreviewHandle;
  preview?: () => Promise<void>;
  showPreview?: (sessionId: string) => Promise<PhotoAvatarPreviewHandle | void>;
} = {}) {
  const elements = {
    root: new FakeElement(), files: new FakeElement(), generate: new FakeElement(), generating: new FakeElement(),
    preview: new FakeElement(), live2d: new FakeElement(), completions: new FakeElement(), name: new FakeElement(),
    accept: new FakeElement(), regenerate: new FakeElement(), revision: new FakeElement(), revise: new FakeElement(),
    cancel: new FakeElement(), status: new FakeElement(), complete: new FakeElement(), done: new FakeElement(),
  };
  const snapshots = [...(options.status ?? [options.snapshot ?? avatarSnapshot()])];
  const api = {
    start: vi.fn(async () => ({ sessionId: "session-1" })),
    abandon: vi.fn(async () => undefined),
    photoAvatarConsent: vi.fn(async () => true),
    photoAvatarBegin: vi.fn(async () => avatarSnapshot({ step: "analyzeIdentity" })),
    photoAvatarStatus: vi.fn(async () => snapshots.length > 0 ? snapshots.shift()! : previewReadySnapshot()),
    photoAvatarCancel: vi.fn(async () => avatarSnapshot({ step: "cancelled" })),
    photoAvatarRegenerate: vi.fn(async () => avatarSnapshot({ step: "analyzeIdentity" })),
    photoAvatarRevise: vi.fn(async () => avatarSnapshot({ step: "analyzeIdentity" })),
    setName: vi.fn(async () => ({ displayName: "团子" } as CreationSnapshot)),
  };
  const dialog = { showThirdPartyConsent: vi.fn(async () => options.consented ?? true) };
  const preview = {
    show: vi.fn(async (_root: unknown, sessionId: string) => {
      if (options.showPreview) return options.showPreview(sessionId);
      await options.preview?.();
      return options.previewHandle;
    }),
    clear: vi.fn(),
  };
  let poll: (() => void) | null = null;
  const ports: PhotoAvatarCreationDomPorts = {
    elements: elements as unknown as PhotoAvatarCreationDomPorts["elements"],
    dialog,
    preview,
    setInterval: vi.fn((callback: () => void) => { poll = callback; return 1; }),
    clearInterval: vi.fn(),
    onCancel: vi.fn(),
  };
  const finalize = vi.fn(async () => ({ ok: true as const, requestId: "request-1", petId: "pet-1" }));
  const view = new PhotoAvatarCreationView({ api, finalize } satisfies PhotoAvatarCreationPorts, ports);
  return {
    view, api, dialog, preview, elements, ports, finalize,
    selectFiles(files: ReturnType<typeof photo>[]) { elements.files.files = files; elements.files.dispatch("change"); },
    clickGenerate() { elements.generate.dispatch("click"); },
    clickAccept() { elements.accept.dispatch("click"); },
    clickRegenerate() { elements.regenerate.dispatch("click"); },
    clickRevise() { elements.revise.dispatch("click"); },
    clickCancel() { elements.cancel.dispatch("click"); },
    async flush() { await new Promise((resolve) => setTimeout(resolve, 0)); await Promise.resolve(); },
    async poll() { poll?.(); await this.flush(); },
  };
}

describe("PhotoAvatarCreationView", () => {
  it("keeps a newly started upload session collecting until the first begin succeeds", async () => {
    const h = viewHarness();

    await h.view.enter();

    expect(h.api.start).toHaveBeenCalledWith("upload");
    expect(h.api.photoAvatarStatus).not.toHaveBeenCalled();
    expect(h.ports.setInterval).not.toHaveBeenCalled();
    expect(h.view.snapshot()).toMatchObject({ sessionId: "session-1", step: "collecting" });

    h.selectFiles([photo("face.jpg", "image/jpeg")]);
    expect(h.elements.generate.disabled).toBe(false);
    h.clickGenerate();
    await vi.waitFor(() => expect(h.api.photoAvatarBegin).toHaveBeenCalledOnce());

    expect(h.ports.setInterval).toHaveBeenCalledOnce();
  });

  it("rebuilds a fresh session when an explicit draft has disappeared from status", async () => {
    const h = viewHarness({ status: [null] });

    await h.view.enter("durable-session");

    expect(h.api.start).toHaveBeenCalledWith("upload");
    expect(h.api.photoAvatarStatus).toHaveBeenCalledWith("durable-session");
    expect(h.ports.setInterval).not.toHaveBeenCalled();
    expect(h.view.snapshot()).toEqual({
      sessionId: "session-1",
      step: "collecting",
      snapshot: null,
    });

    h.selectFiles([photo("face.jpg", "image/jpeg")]);
    expect(h.elements.generate.disabled).toBe(false);
  });

  it("stops polling and rebuilds when a generating run disappears from status", async () => {
    const h = viewHarness({ status: [avatarSnapshot({ step: "analyzeIdentity" }), null] });

    await h.view.enter("session-1");
    await h.poll();

    expect(h.ports.clearInterval).toHaveBeenCalledWith(1);
    expect(h.api.start).toHaveBeenCalledWith("upload");
    expect(h.view.snapshot()).toEqual({
      sessionId: "session-1",
      step: "collecting",
      snapshot: null,
    });
  });

  it("destroys the mounted preview and rebuilds when a preview run disappears from status", async () => {
    const handle = { evidence: null, destroy: vi.fn() };
    const h = viewHarness({ status: [previewReadySnapshot(), null], previewHandle: handle });

    await h.view.enter("session-1");
    await h.view.enter("session-1");

    expect(handle.destroy).toHaveBeenCalledOnce();
    expect(h.preview.clear).toHaveBeenCalledOnce();
    expect(h.api.start).toHaveBeenCalledWith("upload");
    expect(h.view.snapshot()).toEqual({
      sessionId: "session-1",
      step: "collecting",
      snapshot: null,
    });
  });

  it("requires v2 consent then submits all selected photos in one begin call", async () => {
    const h = viewHarness({ consented: true });
    await h.view.enter();
    h.selectFiles([photo("face.jpg", "image/jpeg"), photo("body.png", "image/png")]);
    h.clickGenerate();
    await vi.waitFor(() => expect(h.api.photoAvatarBegin).toHaveBeenCalledOnce());

    expect(h.dialog.showThirdPartyConsent).toHaveBeenCalledOnce();
    expect(h.api.photoAvatarConsent).toHaveBeenCalledWith(true);
    expect(h.api.photoAvatarBegin).toHaveBeenCalledOnce();
    expect(h.api.photoAvatarBegin).toHaveBeenCalledWith(
      "session-1", "photo-avatar-third-party-ai-lk888-no-delete-v2", expect.arrayContaining([
        expect.objectContaining({ bytesB64: "AQID" }), expect.objectContaining({ bytesB64: "AQID" }),
      ]),
    );
  });

  it("allows one photo, keeps selection order, and disables generation for no photos", async () => {
    const h = viewHarness();
    await h.view.enter();
    expect(h.elements.generate.disabled).toBe(true);
    h.selectFiles([photo("second.png", "image/png", [2]), photo("first.jpg", "image/jpeg", [1])]);
    expect(h.elements.generate.disabled).toBe(false);
    h.clickGenerate();
    await vi.waitFor(() => expect(h.api.photoAvatarBegin).toHaveBeenCalledOnce());

    const photos = (h.api.photoAvatarBegin.mock.calls[0] as unknown as [string, string, PhotoAvatarUpload[]])[2];
    expect(photos.map((item) => item.bytesB64)).toEqual(["Ag==", "AQ=="]);
  });

  it("rejects a selection over eight photos before consent or provider submission", async () => {
    const h = viewHarness();
    await h.view.enter();
    h.selectFiles(Array.from({ length: 9 }, (_, index) => photo(`${index}.png`, "image/png")));
    h.clickGenerate();
    await h.flush();

    expect(h.elements.generate.disabled).toBe(true);
    expect(h.elements.status.textContent).toContain("1 至 8");
    expect(h.dialog.showThirdPartyConsent).not.toHaveBeenCalled();
    expect(h.api.photoAvatarBegin).not.toHaveBeenCalled();
  });

  it("does not start a provider request when third-party consent is declined", async () => {
    const h = viewHarness({ consented: false });
    await h.view.enter();
    h.selectFiles([photo("face.jpg", "image/jpeg")]);
    h.clickGenerate();
    await h.flush();

    expect(h.api.photoAvatarConsent).toHaveBeenCalledWith(false);
    expect(h.api.photoAvatarBegin).not.toHaveBeenCalled();
    expect(h.view.snapshot().step).toBe("collecting");
  });

  it("shows only a complete live2d preview and all three final actions", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");

    expect(h.preview.show).toHaveBeenCalledOnce();
    expect(h.elements.accept.hidden).toBe(false);
    expect(h.elements.regenerate.hidden).toBe(false);
    expect(h.elements.revise.hidden).toBe(false);
    expect(h.elements.completions.textContent).toContain("tail");
    expect(h.elements.completions.textContent).toContain("AI 补全");
  });

  it("describes v2 as an animation-ready simplified pixel identity", async () => {
    const h = viewHarness({
      snapshot: previewReadyPixelSnapshot("pixel-style-v2-animation-ready"),
    });

    await h.view.enter("session-1");

    expect(h.elements.completions.textContent).toContain("动画优先简约像素形象");
  });

  it("shows the exact invalidInput message reached during the current visit and never offers a static success fallback", async () => {
    const h = viewHarness({ status: [
      avatarSnapshot({ step: "analyzeIdentity" }),
      avatarSnapshot({ step: "failed", errorCode: "invalidInput", errorMessage: "照片必须是 PNG 或 JPEG" }),
    ] });
    await h.view.enter("session-1");
    await h.poll();

    expect(h.elements.status.textContent).toBe("照片必须是 PNG 或 JPEG");
    expect(h.elements.accept.hidden).toBe(true);
    expect(h.preview.show).not.toHaveBeenCalled();
  });

  it("keeps a failure reached during the current visit visible and allows regeneration", async () => {
    const h = viewHarness({
      status: [
        avatarSnapshot({ step: "analyzeIdentity" }),
        avatarSnapshot({
          step: "failed",
          errorCode: "invalidInput",
          errorMessage: "生成图片不符合像素素材要求，请重试。",
        }),
      ],
    });

    await h.view.enter("session-1");
    await h.poll();

    expect(h.elements.status.textContent).toBe("生成图片不符合像素素材要求，请重试。");
    expect(h.elements.regenerate.hidden).toBe(false);
    h.clickRegenerate();
    await h.flush();
    expect(h.api.photoAvatarRegenerate).toHaveBeenCalledWith("session-1");
  });

  it.each([
    ["network", "网络连接失败，请稍后重试。"],
    ["timeout", "图片生成超时，请稍后重试。"],
  ] as const)("shows a fixed safe message for %s failures", async (errorCode, expected) => {
    const h = viewHarness({ status: [
      avatarSnapshot({ step: "analyzeIdentity" }),
      avatarSnapshot({
        step: "failed",
        errorCode,
        errorMessage: "https://private.example?api_key=secret diagnostic=provider",
      }),
    ] });

    await h.view.enter("session-1");
    await h.poll();

    expect(h.elements.status.textContent).toBe(expected);
    expect(h.elements.status.textContent).not.toContain("private.example");
    expect(h.elements.status.textContent).not.toContain("secret");
  });

  it("uses a generic fallback when failed state has no trusted error code", async () => {
    const h = viewHarness({ status: [
      avatarSnapshot({ step: "analyzeIdentity" }),
      avatarSnapshot({
        step: "failed",
        errorCode: null,
        errorMessage: "Authorization: Bearer private-token provider=https://private.example",
      }),
    ] });

    await h.view.enter("session-1");
    await h.poll();

    expect(h.elements.status.textContent).toBe("生成失败，请重试。");
  });

  it("abandons a restored terminal upload draft and opens a fresh collecting session", async () => {
    const h = viewHarness({ status: [avatarSnapshot({ sessionId: "old-session", step: "failed" })] });

    await h.view.enter("old-session");

    expect(h.api.abandon).toHaveBeenCalledWith("old-session");
    expect(h.api.start).toHaveBeenCalledWith("upload");
    expect(h.view.snapshot()).toEqual({
      sessionId: "session-1",
      step: "collecting",
      snapshot: null,
    });
  });

  it("restarts polling when regeneration returns to generating", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");
    vi.mocked(h.ports.setInterval).mockClear();

    h.clickRegenerate();
    await h.flush();

    expect(h.view.snapshot().step).toBe("generating");
    expect(h.ports.setInterval).toHaveBeenCalledOnce();
  });

  it("runs the runtime inspection, then refreshes to previewReady before allowing acceptance", async () => {
    const h = viewHarness({ status: [
      avatarSnapshot({ step: "runtimeCheckPending" }), previewReadySnapshot(),
    ] });
    await h.view.enter("session-1");
    await h.flush();

    expect(h.preview.show).toHaveBeenCalledOnce();
    expect(h.elements.accept.hidden).toBe(false);
    expect(h.elements.status.textContent).toBe("像素宠物预览已通过运行时检查，请确认后安装。");
  });

  it("keeps one runtime-check renderer while repeated pending polls overlap", async () => {
    let finishPreview!: () => void;
    const previewPending = new Promise<void>((resolve) => { finishPreview = resolve; });
    const pending = avatarSnapshot({ step: "runtimeCheckPending" });
    const h = viewHarness({
      status: [pending, pending, pending],
      preview: async () => previewPending,
    });

    await h.view.enter("session-1");
    await h.flush();
    await h.poll();
    await h.poll();

    expect(h.preview.show).toHaveBeenCalledOnce();
    expect(h.preview.clear).not.toHaveBeenCalled();
    finishPreview();
    await h.flush();
  });

  it("replaces the mounted preview when entering a different session and revision", async () => {
    const sessionA = previewReadySnapshot();
    const sessionB = avatarSnapshot({
      ...previewReadySnapshot(),
      sessionId: "session-2",
      revision: 2,
    });
    const h = viewHarness({ status: [sessionA, sessionB] });

    await h.view.enter("session-1");
    await h.view.enter("session-2");

    expect(h.preview.clear).toHaveBeenCalledOnce();
    expect(h.preview.show).toHaveBeenNthCalledWith(1, h.elements.live2d, "session-1");
    expect(h.preview.show).toHaveBeenNthCalledWith(2, h.elements.live2d, "session-2");
  });

  it("destroys only a late old-revision handle after the new renderer mounts", async () => {
    let resolveOld!: (handle: PhotoAvatarPreviewHandle) => void;
    const oldHandle = { evidence: null, destroy: vi.fn() };
    const newHandle = { evidence: null, destroy: vi.fn() };
    const oldMount = new Promise<PhotoAvatarPreviewHandle>((resolve) => { resolveOld = resolve; });
    let previewCalls = 0;
    const h = viewHarness({
      status: [
        previewReadySnapshot(),
        avatarSnapshot({ ...previewReadySnapshot(), revision: 2 }),
      ],
      showPreview: async () => {
        previewCalls += 1;
        return previewCalls === 1 ? oldMount : newHandle;
      },
    });

    await h.view.enter("session-1");
    await h.flush();
    await h.view.enter("session-1");
    await h.flush();
    resolveOld(oldHandle);
    await h.flush();

    expect(h.preview.show).toHaveBeenCalledTimes(2);
    expect(oldHandle.destroy).toHaveBeenCalledOnce();
    expect(newHandle.destroy).not.toHaveBeenCalled();
  });

  it("keeps acceptance hidden when the runtime inspection fails", async () => {
    const h = viewHarness({ snapshot: avatarSnapshot({ step: "runtimeCheckPending" }), preview: async () => { throw new Error("motionPixelDeltaMissing"); } });
    await h.view.enter("session-1");

    expect(h.elements.accept.hidden).toBe(true);
    expect(h.view.snapshot().step).toBe("failed");
    expect(h.finalize).not.toHaveBeenCalled();
  });

  it("installs only after previewReady and an explicit accept click", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");

    expect(h.elements.accept.hidden).toBe(false);
    expect(h.finalize).not.toHaveBeenCalled();
    h.elements.name.value = "团子";
    h.clickAccept();
    await h.flush();
    expect(h.finalize).toHaveBeenCalledWith("session-1");
  });

  it("prompts for a pet name before finalizing when the name is empty", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");

    h.clickAccept();
    await h.flush();
    expect(h.api.setName).not.toHaveBeenCalled();
    expect(h.finalize).not.toHaveBeenCalled();
    expect(h.elements.status.textContent).toContain("名字");
  });

  it("accepts, regenerates, validates a local revision, and cancels through photo-avatar commands", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");
    h.elements.name.value = "团子";
    h.clickAccept();
    await h.flush();
    expect(h.finalize).toHaveBeenCalledWith("session-1");
    expect(h.view.snapshot().step).toBe("complete");

    const r = viewHarness({ snapshot: previewReadySnapshot() });
    await r.view.enter("session-1");
    r.clickRevise();
    await r.flush();
    expect(r.api.photoAvatarRevise).not.toHaveBeenCalled();
    expect(r.elements.status.textContent).toContain("填写");
    r.elements.revision.value = "耳朵更圆一些";
    r.clickRevise();
    await r.flush();
    expect(r.api.photoAvatarRevise).toHaveBeenCalledWith("session-1", "耳朵更圆一些");
    const g = viewHarness({ snapshot: previewReadySnapshot() });
    await g.view.enter("session-1");
    g.clickRegenerate();
    await g.flush();
    expect(g.api.photoAvatarRegenerate).toHaveBeenCalledOnce();
    expect(g.preview.clear).toHaveBeenCalledOnce();
    r.clickCancel();
    await r.flush();
    expect(r.api.photoAvatarCancel).toHaveBeenCalledWith("session-1");
    expect(r.api.abandon).toHaveBeenCalledWith("session-1");
    expect(r.ports.onCancel).toHaveBeenCalledOnce();
  });

  it("shows a done button only after completion and returns home when clicked", async () => {
    const h = viewHarness({ snapshot: previewReadySnapshot() });
    await h.view.enter("session-1");

    expect(h.elements.done.hidden).toBe(true);

    h.elements.name.value = "团子";
    h.clickAccept();
    await h.flush();

    expect(h.view.snapshot().step).toBe("complete");
    expect(h.elements.done.hidden).toBe(false);
    h.elements.done.dispatch("click");
    expect(h.ports.onCancel).toHaveBeenCalledOnce();
    expect(h.api.abandon).not.toHaveBeenCalled();
  });

  it("stops polling and isolates late status updates after leave without cancelling background work", async () => {
    let resolveStatus!: (value: PhotoAvatarSnapshot) => void;
    const h = viewHarness({ status: [avatarSnapshot({ step: "analyzeIdentity" }), new Promise<PhotoAvatarSnapshot>((resolve) => { resolveStatus = resolve; }) as unknown as PhotoAvatarSnapshot] });
    await h.view.enter("session-1");
    const entering = h.poll();
    await h.flush();
    h.view.leave();
    resolveStatus(previewReadySnapshot());
    await entering;

    expect(h.api.photoAvatarCancel).not.toHaveBeenCalled();
    expect(h.ports.clearInterval).toHaveBeenCalledWith(1);
    expect(h.preview.show).not.toHaveBeenCalled();
  });
});
