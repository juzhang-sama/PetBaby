import { describe, expect, it, vi } from "vitest";
import type { AdoptionCatalogEntry, CreationSnapshot } from "../creation/contracts";
import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { AdoptionCreationView, type AdoptionCreationPorts } from "./adoption-creation-view";

const profile: MotionProfileV1 = {
  profileVersion: 1,
  engineProfile: "life-v1",
  alphaBounds: { left: 0.1, top: 0.1, right: 0.9, bottom: 0.95 },
  breathZone: { left: 0.3, top: 0.55, right: 0.7, bottom: 0.85 },
  swayPivot: { x: 0.5, y: 0.78 },
};

function entry(overrides: Partial<AdoptionCatalogEntry> = {}): AdoptionCatalogEntry {
  return {
    template: {
      templateId: "cat-misty",
      templateVersion: 1,
      runtimeSchemaVersion: 3,
      defaultName: "雾团",
      personality: "安静陪伴，喜欢待在你身边。",
      thumbnailPath: "thumbnail.png",
      bodyPath: "body.png",
      motionProfilePath: "motion-profile.json",
      thumbnailSha256: "a".repeat(64),
      bodySha256: "b".repeat(64),
      motionProfileSha256: "c".repeat(64),
    },
    adoptedPetId: null,
    retrySessionId: null,
    ...overrides,
  };
}

function catalog(first: AdoptionCatalogEntry = entry()): AdoptionCatalogEntry[] {
  return [first, ...Array.from({ length: 7 }, (_, index) => entry({
    template: {
      ...first.template,
      templateId: `cat-${index + 2}`,
      defaultName: `猫咪${index + 2}`,
    },
    adoptedPetId: null,
    retrySessionId: null,
  }))];
}

function adoptionSnapshot(overrides: Partial<CreationSnapshot> = {}): CreationSnapshot {
  return {
    sessionId: "session-adoption",
    petId: "pet-adoption",
    method: "adoption",
    status: "candidateReady",
    lastStableStatus: "candidateReady",
    currentStep: "review",
    displayName: "雾团",
    jobId: null,
    jobStatus: null,
    candidateId: "candidate-adoption",
    recipe: null,
    error: null,
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => { resolve = onResolve; reject = onReject; });
  return { promise, resolve, reject };
}

function adoptionPorts(options: {
  catalogs?: AdoptionCatalogEntry[][];
  start?: () => Promise<CreationSnapshot>;
  snapshot?: () => Promise<CreationSnapshot>;
  finalize?: (sessionId: string) => Promise<PetSwitchResult>;
  loadMotionProfile?: (url: string) => Promise<unknown>;
} = {}) {
  const catalogs = options.catalogs ?? [catalog()];
  let catalogIndex = 0;
  const ports = {
    creation: {
      adoptionCatalog: vi.fn(async () => catalogs[Math.min(catalogIndex++, catalogs.length - 1)]!),
      adoptionStart: vi.fn(options.start ?? (async () => adoptionSnapshot())),
      snapshot: vi.fn(options.snapshot ?? (async () => adoptionSnapshot())),
      recoverFinalization: vi.fn(async () => ({
        completedSessionIds: [], retryableSessionIds: [], cleanedSessionIds: [], warnings: [],
      })),
    },
    previewRoot: {} as HTMLElement,
    preview: {
      show: vi.fn(async (
        _root: HTMLElement,
        _url: string,
        _profile: MotionProfileV1,
      ): Promise<void> => undefined),
      clear: vi.fn(),
    },
    assetUrl: vi.fn((templateId: string, relativePath: string) =>
      `/creation-content/adoption/${templateId}/${relativePath}`),
    loadMotionProfile: vi.fn(options.loadMotionProfile ?? (async () => profile)),
    finalize: vi.fn(options.finalize ?? (async (_sessionId: string): Promise<PetSwitchResult> => ({
      ok: true as const, requestId: "request-1", petId: "pet-adoption",
    }))),
    switchPet: vi.fn(async (petId: string) => ({ ok: true as const, requestId: "switch-1", petId })),
    refreshPets: vi.fn(async () => undefined),
    onBusyChange: vi.fn(),
  } satisfies AdoptionCreationPorts;
  return { ports, view: new AdoptionCreationView(ports) };
}

describe("AdoptionCreationView", () => {
  it("previews a template dynamically before adoption", async () => {
    const test = adoptionPorts();
    await test.view.open();

    await test.view.select("cat-misty");

    expect(test.ports.preview.show).toHaveBeenCalledWith(
      expect.anything(),
      expect.stringContaining("cat-misty/body.png"),
      profile,
    );
  });

  it("switches an adopted template instead of creating a duplicate", async () => {
    const test = adoptionPorts({ catalogs: [catalog(entry({ adoptedPetId: "pet-existing" }))] });
    await test.view.open();

    await test.view.activate("cat-misty");

    expect(test.ports.switchPet).toHaveBeenCalledWith("pet-existing");
    expect(test.ports.creation.adoptionStart).not.toHaveBeenCalled();
    expect(test.ports.refreshPets).toHaveBeenCalledTimes(1);
  });

  it("requires the fixed eight-entry trusted catalog", async () => {
    const test = adoptionPorts({ catalogs: [[entry()]] });
    await expect(test.view.open()).rejects.toThrow(/8/);
    expect(test.view.entries()).toEqual([]);
  });

  it("keeps the latest selection when A and B profile loads settle in reverse order", async () => {
    const a = deferred<unknown>();
    const b = deferred<unknown>();
    const entries = catalog();
    const test = adoptionPorts({
      catalogs: [entries],
      loadMotionProfile: (url) => url.includes("cat-misty") ? a.promise : b.promise,
    });
    await test.view.open();

    const selectingA = test.view.select("cat-misty");
    const selectingB = test.view.select("cat-2");
    b.resolve(profile);
    await selectingB;
    a.resolve(profile);
    await selectingA;

    expect(test.ports.preview.show).toHaveBeenCalledTimes(1);
    expect(test.ports.preview.show.mock.calls[0]![1]).toContain("cat-2/body.png");
    expect(test.view.selectedTemplateId()).toBe("cat-2");
  });

  it("invalidates a pending preview when leaving", async () => {
    const pending = deferred<void>();
    const test = adoptionPorts();
    test.ports.preview.show.mockImplementation(async () => pending.promise);
    await test.view.open();
    const selecting = test.view.select("cat-misty");
    await Promise.resolve();

    test.view.leave();
    pending.resolve();
    await selecting;

    expect(test.view.dynamicReady()).toBe(false);
    expect(test.ports.preview.clear).toHaveBeenCalled();
  });

  it("coalesces double activation into one start and finalize", async () => {
    const pending = deferred<CreationSnapshot>();
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(entry({ adoptedPetId: "pet-adoption" }))],
      start: () => pending.promise,
    });
    await test.view.open();

    const first = test.view.activate("cat-misty", "雾团");
    const second = test.view.activate("cat-misty", "雾团");
    expect(test.view.busy()).toBe(true);
    expect(test.ports.onBusyChange).toHaveBeenLastCalledWith(true);
    pending.resolve(adoptionSnapshot());
    await Promise.all([first, second]);

    expect(test.ports.creation.adoptionStart).toHaveBeenCalledTimes(1);
    expect(test.ports.finalize).toHaveBeenCalledTimes(1);
    expect(test.view.busy()).toBe(false);
    expect(test.ports.onBusyChange).toHaveBeenLastCalledWith(false);
  });

  it("coalesces catalog refresh and exposes one shared busy interval", async () => {
    const pending = deferred<AdoptionCatalogEntry[]>();
    const test = adoptionPorts();
    test.ports.creation.adoptionCatalog.mockImplementation(async () => pending.promise);

    const first = test.view.refresh();
    const second = test.view.refresh();
    expect(test.view.busy()).toBe(true);
    pending.resolve(catalog());
    await Promise.all([first, second]);

    expect(test.ports.creation.adoptionCatalog).toHaveBeenCalledTimes(1);
    expect(test.ports.onBusyChange.mock.calls.map(([busy]) => busy)).toEqual([true, false]);
  });

  it("reconciles a lost start response through the durable retry session", async () => {
    const retry = entry({ retrySessionId: "session-adoption" });
    const adopted = entry({ adoptedPetId: "pet-adoption" });
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(retry), catalog(adopted)],
      start: async () => { throw new Error("response lost"); },
    });
    await test.view.open();

    await test.view.activate("cat-misty", "雾团");

    expect(test.ports.creation.snapshot).toHaveBeenCalledWith("session-adoption");
    expect(test.ports.finalize).toHaveBeenCalledWith("session-adoption");
    expect(test.view.entry("cat-misty")?.adoptedPetId).toBe("pet-adoption");
  });

  it("recovers a finalizing retry before projecting adopted state", async () => {
    const finalizing = adoptionSnapshot({ status: "finalizing" });
    const completed = adoptionSnapshot({ status: "completed", lastStableStatus: "candidateReady" });
    let snapshots = 0;
    const test = adoptionPorts({
      catalogs: [catalog(entry({ retrySessionId: "session-adoption" })), catalog(entry({ adoptedPetId: "pet-adoption" }))],
      snapshot: async () => snapshots++ === 0 ? finalizing : completed,
    });
    await test.view.open();

    await test.view.activate("cat-misty");

    expect(test.ports.creation.recoverFinalization).toHaveBeenCalledTimes(1);
    expect(test.ports.finalize).not.toHaveBeenCalled();
    expect(test.view.entry("cat-misty")?.adoptedPetId).toBe("pet-adoption");
  });

  it("refreshes durable catalog after finalize failure and preserves retry", async () => {
    const retry = entry({ retrySessionId: "session-adoption" });
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(retry)],
      finalize: async (): Promise<PetSwitchResult> => ({
        ok: false, requestId: "request-1", petId: "pet-adoption",
        code: "pet-window-unavailable", message: "宠物窗口没有响应",
      }),
    });
    await test.view.open();

    await expect(test.view.activate("cat-misty")).rejects.toThrow(/宠物窗口没有响应/);

    expect(test.view.entry("cat-misty")?.retrySessionId).toBe("session-adoption");
    expect(test.view.entry("cat-misty")?.adoptedPetId).toBeNull();
  });

  it("shows success only after catalog reprojection and refreshes my pets", async () => {
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(entry({ adoptedPetId: "pet-adoption" }))],
    });
    await test.view.open();

    await test.view.activate("cat-misty", "  雾团  ");

    expect(test.ports.creation.adoptionStart).toHaveBeenCalledWith("cat-misty", "雾团");
    expect(test.ports.refreshPets).toHaveBeenCalledTimes(1);
    expect(test.view.statusText()).toMatch(/已认领/);
  });

  it("rejects invalid names before starting adoption", async () => {
    const test = adoptionPorts();
    await test.view.open();
    await expect(test.view.activate("cat-misty", "猫".repeat(21))).rejects.toThrow(/1.*20/);
    expect(test.ports.creation.adoptionStart).not.toHaveBeenCalled();
  });

  it("keeps preview failures retryable without static fallback", async () => {
    const test = adoptionPorts({ loadMotionProfile: async () => { throw new Error("profile damaged"); } });
    await test.view.open();
    await expect(test.view.select("cat-misty")).rejects.toThrow(/profile damaged/);
    expect(test.ports.preview.show).not.toHaveBeenCalled();
    expect(test.view.dynamicReady()).toBe(false);
  });

  it("cleans the renderer exactly once when destroy is repeated", async () => {
    const test = adoptionPorts();
    await test.view.open();
    await test.view.select("cat-misty");
    const before = test.ports.preview.clear.mock.calls.length;

    test.view.destroy();
    test.view.destroy();

    expect(test.ports.preview.clear).toHaveBeenCalledTimes(before + 1);
    expect(test.view.busy()).toBe(false);
  });

  it("does not project a finalize result into a page that has been left", async () => {
    const finalizing = deferred<PetSwitchResult>();
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(entry({ adoptedPetId: "pet-adoption" }))],
      finalize: async () => finalizing.promise,
    });
    await test.view.open();
    const activating = test.view.activate("cat-misty");

    test.view.leave();
    finalizing.resolve({ ok: true, requestId: "request-late", petId: "pet-adoption" });
    await activating;

    expect(test.view.statusText()).not.toMatch(/已认领并显示/);
    expect(test.view.busy()).toBe(false);
  });

  it("removes every DOM listener exactly once per mount lifecycle", () => {
    const makeElement = () => Object.assign(new EventTarget(), {
      textContent: "",
      value: "",
      disabled: false,
      ownerDocument: {},
      setAttribute: vi.fn(),
      replaceChildren: vi.fn(),
    });
    const elements = {
      root: makeElement(),
      catalog: makeElement(),
      selectedName: makeElement(),
      selectedPersonality: makeElement(),
      nameInput: makeElement(),
      actionButton: makeElement(),
      refreshButton: makeElement(),
      backButton: makeElement(),
      status: makeElement(),
    } as unknown as import("./adoption-creation-view").AdoptionCreationElements;
    const eventTargets = [
      elements.catalog,
      elements.nameInput,
      elements.actionButton,
      elements.refreshButton,
      elements.backButton,
    ];
    const added = eventTargets.map((target) => vi.spyOn(target, "addEventListener"));
    const removed = eventTargets.map((target) => vi.spyOn(target, "removeEventListener"));
    const test = adoptionPorts();

    test.view.mount(elements);
    test.view.mount(elements);
    test.view.destroy();
    test.view.destroy();

    expect(added.map((spy) => spy.mock.calls.length)).toEqual([2, 2, 2, 2, 2]);
    expect(removed.map((spy) => spy.mock.calls.length)).toEqual([2, 2, 2, 2, 2]);
  });
});
