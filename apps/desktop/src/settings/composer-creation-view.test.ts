import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import type { ComposerRecipe, CreationSnapshot } from "../creation/contracts";
import { parseComposerPack } from "../creation/composer-pack";
import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import {
  ComposerCreationView,
  composerOptionLabel,
  type ComposerCreationPorts,
  type ComposerCreationElements,
} from "./composer-creation-view";
import type { CreationActivityOwner, CreationPageActivityPort } from "./creation-page-run";

class FakeElement {
  disabled = false;
  textContent = "";
  value = "";
  className = "";
  title = "";
  tabIndex = 0;
  src = "";
  alt = "";
  loading = "";
  dataset: Record<string, string> = {};
  style: Record<string, string> = {};
  children: FakeElement[] = [];
  private readonly attributes = new Map<string, string>();
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
    for (const listener of this.listeners.get(type) ?? []) listener({ preventDefault() {} } as Event);
  }

  replaceChildren(...children: FakeElement[]): void { this.children = children; }
  append(...children: FakeElement[]): void { this.children.push(...children); }
  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  removeAttribute(name: string): void { this.attributes.delete(name); }
  getAttribute(name: string): string | null { return this.attributes.get(name) ?? null; }
}

function composerElements(): { raw: Record<keyof ComposerCreationElements, FakeElement>; typed: ComposerCreationElements } {
  const raw = {
    canvas: new FakeElement(), steps: new FakeElement(), options: new FakeElement(),
    saveStatus: new FakeElement(), message: new FakeElement(), previousButton: new FakeElement(),
    nextButton: new FakeElement(), candidateButton: new FakeElement(), candidatePreview: new FakeElement(),
    nameInput: new FakeElement(), finishButton: new FakeElement(), abandonButton: new FakeElement(),
  };
  return { raw, typed: raw as unknown as ComposerCreationElements };
}

function stubComposerDocument(): void {
  vi.stubGlobal("document", {
    createElement: vi.fn(() => new FakeElement()),
  });
}

function pack() {
  const path = fileURLToPath(new URL("../../public/creation-content/composer/cat-cute-v1/manifest.json", import.meta.url));
  return parseComposerPack(JSON.parse(readFileSync(path, "utf8")));
}

function snapshot(overrides: Partial<CreationSnapshot> = {}): CreationSnapshot {
  return {
    sessionId: "session-composer",
    petId: "pet-composer",
    method: "composer",
    status: "draft",
    lastStableStatus: "draft",
    currentStep: "ears",
    displayName: null,
    jobId: null,
    jobStatus: null,
    candidateId: null,
    recipe: null,
    error: null,
    ...overrides,
  };
}

function motionProfile(): MotionProfileV1 {
  return {
    profileVersion: 1,
    engineProfile: "life-v1",
    alphaBounds: { left: 0.1, top: 0.05, right: 0.9, bottom: 0.98 },
    breathZone: { left: 0.26, top: 0.5, right: 0.76, bottom: 0.9 },
    swayPivot: { x: 0.5, y: 0.76 },
  };
}

function composerPorts(options: { draft?: CreationSnapshot | null } = {}) {
  let durable = options.draft ?? null;
  const recoverFinalization = vi.fn(async () => ({
    completedSessionIds: [] as string[],
    retryableSessionIds: [] as string[],
    cleanedSessionIds: [] as string[],
    warnings: [] as string[],
  }));
  const creation: ComposerCreationPorts["creation"] = {
    start: vi.fn(async () => {
      durable = snapshot({ currentStep: "composer" });
      return durable;
    }),
    draft: vi.fn(async () => durable),
    snapshot: vi.fn(async () => {
      if (!durable) throw new Error("missing session");
      return durable;
    }),
    composerSave: vi.fn(async (_sessionId: string, recipe: ComposerRecipe, currentStep: string) => {
      if (!durable) throw new Error("missing session");
      durable = { ...durable, recipe: { ...recipe }, currentStep };
      return durable;
    }),
    composerCandidate: vi.fn(async () => {
      if (!durable) throw new Error("missing session");
      durable = durable.status === "retryableFailure"
        ? { ...durable, currentStep: "review", candidateId: "candidate-1" }
        : { ...durable, status: "candidateReady", currentStep: "review", candidateId: "candidate-1" };
      return {
        snapshot: durable,
        bodyUrl: "data:image/png;base64,candidate",
        motionProfile: motionProfile(),
      };
    }),
    setName: vi.fn(async (_sessionId: string, displayName: string) => {
      if (!durable) throw new Error("missing session");
      durable = { ...durable, displayName };
      return durable;
    }),
    abandon: vi.fn(async () => undefined),
    recoverFinalization,
  };
  const ports: ComposerCreationPorts = {
    creation,
    loadPack: vi.fn(async () => pack()),
    render: vi.fn(async () => undefined),
    exportPng: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
    blobToBase64: vi.fn(async () => "encoded-png"),
    assetAvailable: vi.fn(async () => true),
    preview: { show: vi.fn(async () => undefined), clear: vi.fn() },
    finalize: vi.fn(async (): Promise<PetSwitchResult> => {
      if (durable) durable = {
        ...durable,
        status: "completed",
        lastStableStatus: "completed",
        currentStep: "completed",
      };
      return { ok: true, requestId: "request-1", petId: "pet-composer" };
    }),
    confirm: vi.fn(() => true),
  };
  return {
    ports,
    creation,
    recoverFinalization,
    durable: () => durable,
    setDurable: (value: CreationSnapshot | null) => { durable = value; },
  };
}

describe("ComposerCreationView", () => {
  it("runs option saves through the injected page activity owner", async () => {
    stubComposerDocument();
    const owners: CreationActivityOwner[] = [];
    const activity: CreationPageActivityPort = {
      run: async <T>(owner: CreationActivityOwner, operation: () => Promise<T>) => {
        owners.push(owner);
        return operation();
      },
    };
    const test = composerPorts();
    test.ports.activity = activity;
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);

    elements.raw.options.children[0]!.dispatch("click");
    await vi.waitFor(() => expect(test.creation.composerSave).toHaveBeenCalledOnce());

    expect(owners[0]).toMatchObject({ route: "composer", kind: "save" });
    vi.unstubAllGlobals();
  });

  it("starts only after the first body selection and autosaves every valid selection", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    expect(test.creation.start).not.toHaveBeenCalled();

    await view.selectBody("body-round");
    await view.select("ears", "ears-folded");

    expect(test.creation.start).toHaveBeenCalledTimes(1);
    expect(test.creation.composerSave).toHaveBeenCalledTimes(2);
    expect(test.creation.composerSave).toHaveBeenLastCalledWith(
      "session-composer",
      expect.objectContaining({ bodyId: "body-round", earsId: "ears-folded" }),
      "eyes",
    );
    expect(view.saveState()).toBe("saved");
  });

  it("restores the same durable recipe and current step in a new view instance", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.select("ears", "ears-folded");

    const second = new ComposerCreationView(test.ports);
    await second.open();
    expect(second.recipe()?.earsId).toBe("ears-folded");
    expect(second.currentStep()).toBe("eyes");
    expect(second.sessionId()).toBe("session-composer");
  });

  it("restores an explicit composer snapshot without showing the empty-body prompt", async () => {
    const manifest = pack();
    const body = manifest.bodies[0]!;
    const recipe = {
      recipeVersion: 1, packId: manifest.packId, packVersion: manifest.packVersion,
      layerContractVersion: manifest.layerContractVersion, bodyId: body.id,
      ...body.defaults,
    };
    const test = composerPorts({ draft: snapshot({ recipe, currentStep: "ears" }) });
    const view = new ComposerCreationView(test.ports);

    await view.restore("session-composer");

    expect(view.currentStep()).toBe("ears");
    expect(view.recipe()?.bodyId).toBe(body.id);
    expect(view.statusText()).toContain("已恢复");
    expect(view.statusText()).not.toContain("请选择身体");
  });

  it("coalesces concurrent first body selections into one composer session", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();

    await Promise.all([
      view.selectBody("body-round"),
      view.selectBody("body-round"),
    ]);

    expect(test.creation.start).toHaveBeenCalledTimes(1);
    expect(test.creation.composerSave).toHaveBeenCalledTimes(1);
  });

  it("keeps the started session and unsaved local recipe when the first save fails", async () => {
    const test = composerPorts();
    vi.mocked(test.creation.composerSave).mockRejectedValueOnce(new Error("disk busy"));
    const view = new ComposerCreationView(test.ports);
    await view.open();

    await expect(view.selectBody("body-round")).rejects.toThrow("disk busy");

    expect(view.sessionId()).toBe("session-composer");
    expect(view.recipe()?.bodyId).toBe("body-round");
    expect(view.saveState()).toBe("unsaved");
    await view.retrySave();
    expect(test.creation.start).toHaveBeenCalledTimes(1);
    expect(test.creation.composerSave).toHaveBeenCalledTimes(2);
    expect(view.saveState()).toBe("saved");
  });

  it("serializes different selections so a slow old save cannot overwrite the latest recipe", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    let releaseFirst!: () => void;
    const originalSave = test.creation.composerSave;
    vi.mocked(test.creation.composerSave).mockImplementationOnce(async (...args) => {
      await new Promise<void>((resolve) => { releaseFirst = resolve; });
      return originalSave(...args);
    });

    const first = view.select("ears", "ears-folded");
    const second = view.select("ears", "ears-tufted");
    await vi.waitFor(() => expect(releaseFirst).toBeTypeOf("function"));
    releaseFirst();
    await Promise.all([first, second]);

    expect(test.durable()?.recipe?.earsId).toBe("ears-tufted");
    expect(view.recipe()?.earsId).toBe("ears-tufted");
  });

  it("keeps a failed local selection dirty and says 未保存 until retry succeeds", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    vi.mocked(test.creation.composerSave).mockRejectedValueOnce(new Error("disk full"));

    await expect(view.select("ears", "ears-folded")).rejects.toThrow("disk full");
    expect(view.recipe()?.earsId).toBe("ears-folded");
    expect(view.saveState()).toBe("unsaved");
    expect(view.statusText()).toContain("未保存");
    expect(view.canCreateCandidate()).toBe(false);

    await view.retrySave();
    expect(view.saveState()).toBe("saved");
    expect(view.canCreateCandidate()).toBe(true);
  });

  it("ignores a late restore from an old visit", async () => {
    const test = composerPorts();
    let resolveDraft!: (value: CreationSnapshot | null) => void;
    vi.mocked(test.creation.draft).mockImplementationOnce(() => new Promise((resolve) => {
      resolveDraft = resolve;
    }));
    const view = new ComposerCreationView(test.ports);
    const oldOpen = view.open();
    await vi.waitFor(() => expect(test.creation.draft).toHaveBeenCalledTimes(1));
    view.destroy();
    await view.open();
    resolveDraft(snapshot({ recipe: null, currentStep: "ears" }));
    await oldOpen;

    expect(view.sessionId()).toBeNull();
  });

  it("exports once, stores a trusted candidate, mounts dynamic preview, names and finalizes", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    const root = {} as HTMLElement;

    await view.createCandidate(root);
    const result = await view.finish("团子");

    expect(test.ports.exportPng).toHaveBeenCalledTimes(1);
    expect(test.creation.composerCandidate).toHaveBeenCalledWith("session-composer", "encoded-png");
    expect(test.ports.preview.show).toHaveBeenCalledWith(
      root,
      "data:image/png;base64,candidate",
      motionProfile(),
    );
    expect(test.creation.setName).toHaveBeenCalledWith("session-composer", "团子");
    expect(test.ports.finalize).toHaveBeenCalledWith("session-composer");
    expect(result.ok).toBe(true);
  });

  it("keeps candidateReady when dynamic mount fails and retries preview without rewriting candidate", async () => {
    const test = composerPorts();
    vi.mocked(test.ports.preview.show).mockRejectedValueOnce(new Error("renderer unavailable"));
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    const root = {} as HTMLElement;

    await expect(view.createCandidate(root)).rejects.toThrow("renderer unavailable");
    expect(view.creationSnapshot()?.status).toBe("candidateReady");
    expect(view.canFinish()).toBe(false);
    await view.retryPreview(root);

    expect(test.creation.composerCandidate).toHaveBeenCalledTimes(1);
    expect(test.ports.preview.show).toHaveBeenCalledTimes(2);
    expect(view.canFinish()).toBe(true);
  });

  it("reopens candidateReady at preview and idempotently restores its trusted dynamic projection", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    first.destroy();

    const reopened = new ComposerCreationView(test.ports);
    await reopened.open();
    expect(reopened.currentStep()).toBe("preview");
    expect(reopened.canCreateCandidate()).toBe(true);

    const root = {} as HTMLElement;
    await reopened.createCandidate(root);
    expect(test.creation.composerCandidate).toHaveBeenCalledTimes(2);
    expect(test.ports.preview.show).toHaveBeenLastCalledWith(
      root,
      "data:image/png;base64,candidate",
      motionProfile(),
    );
    expect(reopened.canFinish()).toBe(true);
  });

  it("does not offer backward editing after a composer candidate is locked", async () => {
    stubComposerDocument();
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    await view.createCandidate({} as HTMLElement);
    const elements = composerElements();
    view.mount(elements.typed);

    expect(view.currentStep()).toBe("preview");
    expect(elements.raw.previousButton.disabled).toBe(true);
    elements.raw.previousButton.dispatch("click");
    expect(view.currentStep()).toBe("preview");
    vi.unstubAllGlobals();
  });

  it("keeps backward modification available while the composer is still a draft", async () => {
    stubComposerDocument();
    const manifest = pack();
    const body = manifest.bodies[0]!;
    const recipe = {
      recipeVersion: 1, packId: manifest.packId, packVersion: manifest.packVersion,
      layerContractVersion: manifest.layerContractVersion, bodyId: body.id,
      ...body.defaults,
    };
    const test = composerPorts({ draft: snapshot({ recipe, currentStep: "eyes" }) });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);

    expect(elements.raw.previousButton.disabled).toBe(false);
    elements.raw.previousButton.dispatch("click");
    await vi.waitFor(() => expect(test.creation.composerSave)
      .toHaveBeenLastCalledWith("session-composer", recipe, "ears"));
    await vi.waitFor(() => expect(view.saveState()).toBe("saved"));
    expect(view.currentStep()).toBe("ears");
    vi.unstubAllGlobals();
  });

  it("keeps review on finalize false and reconciles a completed response-loss snapshot", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    await view.createCandidate({} as HTMLElement);
    vi.mocked(test.ports.finalize).mockResolvedValueOnce({
      ok: false,
      requestId: "request-false",
      petId: "pet-composer",
      code: "pet-window-unavailable",
      message: "window unavailable",
    });
    expect((await view.finish("团子")).ok).toBe(false);
    expect(view.creationSnapshot()?.status).toBe("candidateReady");

    vi.mocked(test.ports.finalize).mockRejectedValueOnce(new Error("response lost"));
    vi.mocked(test.creation.snapshot).mockResolvedValueOnce(snapshot({
      status: "completed",
      lastStableStatus: "completed",
      currentStep: "completed",
      candidateId: "candidate-1",
    }));
    expect((await view.finish("团子")).ok).toBe(true);
  });

  it("keeps one finalization owner while destroy and reenter wait for its durable result", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    let resolveFinalize!: () => void;
    vi.mocked(test.ports.finalize).mockImplementationOnce(() => {
      test.setDurable(snapshot({
        status: "finalizing",
        lastStableStatus: "candidateReady",
        currentStep: "finalizing",
        candidateId: "candidate-1",
        recipe: test.durable()?.recipe ?? null,
      }));
      return new Promise<PetSwitchResult>((resolve) => {
        resolveFinalize = () => {
          test.setDurable(snapshot({
            status: "completed",
            lastStableStatus: "completed",
            currentStep: "completed",
            candidateId: "candidate-1",
            recipe: test.durable()?.recipe ?? null,
          }));
          resolve({ ok: true, requestId: "request-finish", petId: "pet-composer" });
        };
      });
    });

    const finishing = first.finish("团子");
    await vi.waitFor(() => expect(resolveFinalize).toBeTypeOf("function"));
    first.destroy();
    const reopened = new ComposerCreationView(test.ports);
    let opened = false;
    const opening = reopened.open().then(() => { opened = true; });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(opened).toBe(false);
    expect(test.creation.draft).toHaveBeenCalledTimes(1);
    resolveFinalize();
    await Promise.all([finishing, opening]);

    expect(test.ports.finalize).toHaveBeenCalledTimes(1);
    expect(reopened.sessionId()).toBeNull();
  });

  it("recovers a durable finalizing session before restoring the composer view", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    test.setDurable(snapshot({
      status: "finalizing",
      lastStableStatus: "candidateReady",
      currentStep: "finalizing",
      candidateId: "candidate-1",
      recipe: test.durable()?.recipe ?? null,
    }));
    vi.mocked(test.recoverFinalization).mockImplementationOnce(async () => {
      test.setDurable(snapshot({
        status: "retryableFailure",
        lastStableStatus: "candidateReady",
        currentStep: "review",
        candidateId: "candidate-1",
        recipe: test.durable()?.recipe ?? null,
      }));
      return {
        completedSessionIds: [], retryableSessionIds: ["session-composer"],
        cleanedSessionIds: ["session-composer"], warnings: [],
      };
    });

    const reopened = new ComposerCreationView(test.ports);
    await reopened.open();

    expect(test.recoverFinalization).toHaveBeenCalledTimes(1);
    expect(reopened.creationSnapshot()?.status).toBe("retryableFailure");
    expect(reopened.currentStep()).toBe("preview");
  });

  it("restores retryable candidateReady through the trusted projection and retries finish", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    test.setDurable(snapshot({
      status: "retryableFailure",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      candidateId: "candidate-1",
      recipe: test.durable()?.recipe ?? null,
    }));
    first.destroy();

    const reopened = new ComposerCreationView(test.ports);
    await reopened.open();
    await reopened.createCandidate({} as HTMLElement);

    expect(reopened.creationSnapshot()?.status).toBe("retryableFailure");
    expect(reopened.canFinish()).toBe(true);
    await reopened.finish("团子");
    expect(reopened.creationSnapshot()?.status).toBe("completed");
  });

  it("reads a durable candidate projection without re-exporting PNG after reopen", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    first.destroy();
    vi.mocked(test.ports.exportPng).mockClear();
    vi.mocked(test.ports.blobToBase64).mockClear();
    vi.mocked(test.creation.composerCandidate).mockClear();

    const reopened = new ComposerCreationView(test.ports);
    await reopened.open();
    await reopened.createCandidate({} as HTMLElement);

    expect(test.ports.exportPng).not.toHaveBeenCalled();
    expect(test.ports.blobToBase64).not.toHaveBeenCalled();
    expect(test.creation.composerCandidate).toHaveBeenCalledWith("session-composer");
    expect(reopened.canFinish()).toBe(true);
  });

  it("recovers finalizing through restore(sessionId), not only through draft open", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    await first.createCandidate({} as HTMLElement);
    test.setDurable(snapshot({
      status: "finalizing",
      lastStableStatus: "candidateReady",
      currentStep: "finalizing",
      candidateId: "candidate-1",
      recipe: test.durable()?.recipe ?? null,
    }));
    vi.mocked(test.recoverFinalization).mockImplementationOnce(async () => {
      test.setDurable(snapshot({
        status: "retryableFailure",
        lastStableStatus: "candidateReady",
        currentStep: "review",
        candidateId: "candidate-1",
        recipe: test.durable()?.recipe ?? null,
      }));
      return {
        completedSessionIds: [], retryableSessionIds: ["session-composer"],
        cleanedSessionIds: ["session-composer"], warnings: [],
      };
    });

    const restored = new ComposerCreationView(test.ports);
    await restored.restore("session-composer");

    expect(test.recoverFinalization).toHaveBeenCalledTimes(1);
    expect(restored.creationSnapshot()?.status).toBe("retryableFailure");
    expect(restored.currentStep()).toBe("preview");
  });

  it("destroys preview exactly once and ignores a late candidate result", async () => {
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    let resolveCandidate!: (value: Awaited<ReturnType<typeof test.creation.composerCandidate>>) => void;
    vi.mocked(test.creation.composerCandidate).mockImplementationOnce(() => new Promise((resolve) => {
      resolveCandidate = resolve;
    }));
    const pending = view.createCandidate({} as HTMLElement);
    await vi.waitFor(() => expect(resolveCandidate).toBeTypeOf("function"));
    view.destroy();
    view.destroy();
    resolveCandidate({
      snapshot: snapshot({ status: "candidateReady", currentStep: "review", candidateId: "candidate-1" }),
      bodyUrl: "data:image/png;base64,candidate",
      motionProfile: motionProfile(),
    });
    await pending;

    expect(test.ports.preview.clear).toHaveBeenCalledTimes(1);
    expect(test.ports.preview.show).not.toHaveBeenCalled();
  });

  it("clears a preview that finishes mounting after its visit was destroyed", async () => {
    const test = composerPorts();
    const events: string[] = [];
    let resolveShow!: () => void;
    vi.mocked(test.ports.preview.show).mockImplementationOnce(() => new Promise<void>((resolve) => {
      resolveShow = () => {
        events.push("show-complete");
        resolve();
      };
    }));
    vi.mocked(test.ports.preview.clear).mockImplementation(() => { events.push("clear"); });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");

    const pending = view.createCandidate({} as HTMLElement);
    await vi.waitFor(() => expect(resolveShow).toBeTypeOf("function"));
    view.destroy();
    resolveShow();
    await pending;
    await vi.waitFor(() => expect(events.at(-1)).toBe("clear"));
  });

  it("shares the session save owner across view instances before restoring durable state", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    let releaseSave!: () => void;
    const originalSave = test.creation.composerSave;
    vi.mocked(test.creation.composerSave).mockImplementationOnce(async (...args) => {
      await new Promise<void>((resolve) => { releaseSave = resolve; });
      return originalSave(...args);
    });

    const oldSelection = first.select("ears", "ears-folded");
    await vi.waitFor(() => expect(releaseSave).toBeTypeOf("function"));
    first.destroy();
    const reopened = new ComposerCreationView(test.ports);
    const opening = reopened.open();
    await vi.waitFor(() => expect(test.creation.draft).toHaveBeenCalledTimes(2));
    releaseSave();
    await Promise.all([oldSelection, opening]);

    expect(reopened.recipe()?.earsId).toBe("ears-folded");
    expect(reopened.currentStep()).toBe("eyes");
  });

  it("lets an initial body mutation finish under the shared owner after destroy and reenter", async () => {
    const test = composerPorts();
    let resolveStart!: () => void;
    vi.mocked(test.creation.start).mockImplementationOnce(() => new Promise<CreationSnapshot>((resolve) => {
      resolveStart = () => {
        const started = snapshot({ currentStep: "composer" });
        test.setDurable(started);
        resolve(started);
      };
    }));
    const first = new ComposerCreationView(test.ports);
    await first.open();
    const initialSelection = first.selectBody("body-round");
    await vi.waitFor(() => expect(resolveStart).toBeTypeOf("function"));
    first.destroy();

    const reopened = new ComposerCreationView(test.ports);
    const opening = reopened.open();
    resolveStart();
    await Promise.all([initialSelection, opening]);

    expect(test.creation.start).toHaveBeenCalledTimes(1);
    expect(test.creation.composerSave).toHaveBeenCalledTimes(1);
    expect(reopened.recipe()?.bodyId).toBe("body-round");
    expect(reopened.currentStep()).toBe("ears");
  });

  it("serializes abandon behind an in-flight candidate for the same session", async () => {
    const test = composerPorts();
    const events: string[] = [];
    const originalCandidate = test.creation.composerCandidate;
    let resolveCandidate!: () => void;
    vi.mocked(test.creation.composerCandidate).mockImplementationOnce((...args) => {
      events.push("candidate-start");
      return new Promise((resolve, reject) => {
        resolveCandidate = () => {
          events.push("candidate-complete");
          originalCandidate(...args).then(resolve, reject);
        };
      });
    });
    vi.mocked(test.creation.abandon).mockImplementationOnce(async () => {
      events.push("abandon");
    });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");

    const candidate = view.createCandidate({} as HTMLElement);
    await vi.waitFor(() => expect(resolveCandidate).toBeTypeOf("function"));
    const abandoning = view.abandon();
    await Promise.resolve();
    expect(test.creation.abandon).not.toHaveBeenCalled();
    resolveCandidate();
    await Promise.all([candidate, abandoning]);

    expect(events).toEqual(["candidate-start", "candidate-complete", "abandon"]);
    expect(view.sessionId()).toBeNull();
  });

  it("does not restore an abandoned session when abandon settles across destroy and reentry", async () => {
    const test = composerPorts();
    const first = new ComposerCreationView(test.ports);
    await first.open();
    await first.selectBody("body-round");
    let resolveAbandon!: () => void;
    vi.mocked(test.creation.abandon).mockImplementationOnce(() => new Promise<void>((resolve) => {
      resolveAbandon = () => {
        test.setDurable(snapshot({
          status: "abandoned",
          lastStableStatus: "abandoned",
          currentStep: "abandoned",
          recipe: null,
        }));
        resolve();
      };
    }));

    const abandoning = first.abandon();
    await vi.waitFor(() => expect(resolveAbandon).toBeTypeOf("function"));
    first.destroy();
    const reopened = new ComposerCreationView(test.ports);
    const opening = reopened.open();
    resolveAbandon();
    await Promise.all([abandoning, opening]);

    expect(reopened.sessionId()).toBeNull();
    expect(reopened.recipe()).toBeNull();
  });

  it("catches abandon listener rejection and renders a retryable error without unhandled rejection", async () => {
    stubComposerDocument();
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    vi.mocked(test.creation.abandon).mockRejectedValueOnce(new Error("database busy"));
    const elements = composerElements();
    view.mount(elements.typed);
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => { unhandled.push(reason); };
    process.on("unhandledRejection", onUnhandled);

    elements.raw.abandonButton.dispatch("click");
    await new Promise((resolve) => setTimeout(resolve, 10));
    process.off("unhandledRejection", onUnhandled);

    expect(unhandled).toEqual([]);
    expect(elements.raw.message.textContent).toContain("database busy");
    expect(view.sessionId()).toBe("session-composer");
    vi.unstubAllGlobals();
  });

  it("catches previous and step save rejections without leaking unhandled promises", async () => {
    stubComposerDocument();
    const test = composerPorts();
    const view = new ComposerCreationView(test.ports);
    await view.open();
    await view.selectBody("body-round");
    const elements = composerElements();
    view.mount(elements.typed);
    vi.mocked(test.creation.composerSave)
      .mockRejectedValueOnce(new Error("previous save busy"))
      .mockRejectedValueOnce(new Error("step save busy"));
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => { unhandled.push(reason); };
    process.on("unhandledRejection", onUnhandled);

    elements.raw.previousButton.dispatch("click");
    await new Promise((resolve) => setTimeout(resolve, 10));
    const step = elements.raw.steps.children[2]!;
    step.dispatch("click");
    await new Promise((resolve) => setTimeout(resolve, 10));
    process.off("unhandledRejection", onUnhandled);

    expect(unhandled).toEqual([]);
    expect(elements.raw.message.textContent).toContain("step save busy");
    vi.unstubAllGlobals();
  });

  it("preflights broken assets, keeps unavailable choices focusable, and preserves a healthy default body", async () => {
    stubComposerDocument();
    const test = composerPorts();
    Object.assign(test.ports, {
      assetAvailable: vi.fn(async (path: string) => path !== "parts/ears/ears-round.png"),
    });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);
    const round = elements.raw.options.children[0]!;
    const slim = elements.raw.options.children[1]!;

    expect(round.disabled).toBe(false);
    expect(round.getAttribute("aria-disabled")).toBe("true");
    expect(round.getAttribute("aria-description")).toContain("素材");
    expect(round.tabIndex).toBe(0);
    expect(slim.getAttribute("aria-disabled")).not.toBe("true");
    round.dispatch("click");
    await Promise.resolve();
    expect(test.creation.start).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("blocks composer entry with an explicit message when no complete default body remains", async () => {
    stubComposerDocument();
    const test = composerPorts();
    const brokenDefaults = new Set([
      "parts/ears/ears-round.png",
      "parts/ears/ears-pointed.png",
      "parts/ears/ears-tufted.png",
    ]);
    Object.assign(test.ports, {
      assetAvailable: vi.fn(async (path: string) => !brokenDefaults.has(path)),
    });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);

    expect(elements.raw.options.children).toHaveLength(3);
    expect(elements.raw.options.children.every((button) =>
      button.getAttribute("aria-disabled") === "true")).toBe(true);
    expect(elements.raw.message.textContent).toContain("没有可用");
    vi.unstubAllGlobals();
  });

  it("reprobes a temporarily unavailable asset after destroy and reopen", async () => {
    stubComposerDocument();
    const test = composerPorts();
    let healthy = false;
    const assetAvailable = vi.fn(async () => healthy);
    Object.assign(test.ports, { assetAvailable });
    const first = new ComposerCreationView(test.ports);
    await first.open();
    const firstElements = composerElements();
    first.mount(firstElements.typed);
    expect(firstElements.raw.options.children[0]!.getAttribute("aria-disabled")).toBe("true");

    first.destroy();
    healthy = true;
    const reopened = new ComposerCreationView(test.ports);
    await reopened.open();
    const reopenedElements = composerElements();
    reopened.mount(reopenedElements.typed);

    expect(reopenedElements.raw.options.children[0]!.getAttribute("aria-disabled")).toBe("false");
    expect(assetAvailable).toHaveBeenCalledTimes(2 * composerAssetCount(pack()));
    vi.unstubAllGlobals();
  });

  it("disables candidate generation while the current recipe contains an unhealthy asset", async () => {
    stubComposerDocument();
    const manifest = pack();
    const body = manifest.bodies[0]!;
    const recipe = {
      recipeVersion: 1, packId: manifest.packId, packVersion: manifest.packVersion,
      layerContractVersion: manifest.layerContractVersion, bodyId: body.id,
      ...body.defaults,
    };
    const test = composerPorts({ draft: snapshot({ recipe, currentStep: "name" }) });
    Object.assign(test.ports, {
      assetAvailable: vi.fn(async (path: string) => path !== "parts/ears/ears-round.png"),
    });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);

    expect(view.canCreateCandidate()).toBe(false);
    expect(elements.raw.candidateButton.disabled).toBe(true);
    vi.unstubAllGlobals();
  });

  it("renders visual swatches for colors and the no-pattern choice", async () => {
    stubComposerDocument();
    const manifest = pack();
    const body = manifest.bodies[0]!;
    const recipe = {
      recipeVersion: 1, packId: manifest.packId, packVersion: manifest.packVersion,
      layerContractVersion: manifest.layerContractVersion, bodyId: body.id,
      ...body.defaults,
    };
    const test = composerPorts({ draft: snapshot({ recipe, currentStep: "coat" }) });
    const view = new ComposerCreationView(test.ports);
    await view.open();
    const elements = composerElements();
    view.mount(elements.typed);

    expect(elements.raw.options.children.some((button) =>
      button.children.some((child) => child.className === "composer-color-swatch"))).toBe(true);
    expect(elements.raw.options.children.some((button) =>
      button.children.some((child) => child.className === "composer-pattern-none"))).toBe(true);
    vi.unstubAllGlobals();
  });

  it("uses one preview canvas contract with accessible option and step controls", () => {
    const html = readFileSync(fileURLToPath(new URL("../../settings.html", import.meta.url)), "utf8");
    const css = readFileSync(fileURLToPath(new URL("../styles.css", import.meta.url)), "utf8");
    expect(html.match(/<canvas[^>]+data-composer-canvas/g)).toHaveLength(1);
    expect(html).toContain("aria-live=\"polite\"");
    expect(html).toContain("data-composer-steps");
    expect(css).toContain(":focus-visible");
    expect(css).toContain("prefers-reduced-motion");
    expect(css).toContain("@media (max-width:");
  });

  it("uses truthful naming guidance and readable localized full-canvas part thumbnails", () => {
    const html = readFileSync(fileURLToPath(new URL("../../settings.html", import.meta.url)), "utf8");
    const css = readFileSync(fileURLToPath(new URL("../styles.css", import.meta.url)), "utf8");
    expect(html).not.toMatch(/data-composer-name[^>]+maxlength=/);
    expect(html).toContain("1–20 个字符");

    const manifest = pack();
    const officialIds = [
      ...manifest.bodies, ...manifest.ears, ...manifest.eyes, ...manifest.muzzles,
      ...manifest.tails, ...manifest.colors, ...manifest.patterns,
    ].map((item) => item.id);
    for (const id of officialIds) {
      expect(composerOptionLabel(id), id).toMatch(/[\u3400-\u9fff]/u);
    }
    expect(composerOptionLabel("future-part")).toBe("未知选项");
    expect(css).toContain(".composer-option-thumbnail");
    expect(css).toContain("data-composer-kind=\"ears\"");
    expect(css).toContain("transform: scale(");
  });
});

function composerAssetCount(manifest: ReturnType<typeof pack>): number {
  const paths = new Set<string>();
  const add = (value?: string | null) => { if (value) paths.add(value); };
  for (const part of [...manifest.bodies, ...manifest.ears, ...manifest.muzzles, ...manifest.tails]) {
    add(part.image); add(part.colorMask); add(part.patternMask);
  }
  for (const eyes of manifest.eyes) {
    add(eyes.openImage); add(eyes.closedImage); add(eyes.colorMask); add(eyes.patternMask);
  }
  for (const pattern of manifest.patterns) add(pattern.image);
  return paths.size;
}
