import { describe, expect, it, vi } from "vitest";
import type { AdoptionCatalogEntry, CreationSnapshot } from "../creation/contracts";
import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { AdoptionCreationView, type AdoptionCreationPorts } from "./adoption-creation-view";
import { CreationPageActivity } from "./creation-page-run";

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
    unavailableReason: null,
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
    unavailableReason: null,
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

interface FakeDocument {
  activeElement: FakeDomElement | null;
  createElement(tagName: string): FakeDomElement;
}

class FakeDomElement {
  private disabledValue = false;
  textContent = "";
  value = "";
  type = "";
  className = "";
  tabIndex = 0;
  src = "";
  alt = "";
  loading = "";
  dataset: Record<string, string> = {};
  children: FakeDomElement[] = [];
  parentElement: FakeDomElement | null = null;
  ownerDocument!: FakeDocument;
  readonly focus = vi.fn(() => { this.ownerDocument.activeElement = this; });
  private readonly attributes = new Map<string, string>();
  private readonly listeners = new Map<string, Set<EventListener>>();

  get disabled(): boolean { return this.disabledValue; }
  set disabled(value: boolean) {
    this.disabledValue = value;
    if (value && this.ownerDocument?.activeElement === this) this.ownerDocument.activeElement = null;
  }

  addEventListener(type: string, listener: EventListener): void {
    const listeners = this.listeners.get(type) ?? new Set<EventListener>();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: EventListener): void {
    this.listeners.get(type)?.delete(listener);
  }

  dispatch(type: string, event: Record<string, unknown>): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener({ preventDefault: vi.fn(), ...event } as unknown as Event);
    }
  }

  replaceChildren(...children: FakeDomElement[]): void {
    for (const child of [...this.children]) child.detach();
    this.append(...children);
  }
  append(...children: FakeDomElement[]): void {
    for (const child of children) {
      child.detach();
      child.parentElement = this;
      this.children.push(child);
    }
  }
  insertBefore(child: FakeDomElement, reference: FakeDomElement | null): void {
    if (reference === child) return;
    child.detach();
    child.parentElement = this;
    const index = reference ? this.children.indexOf(reference) : -1;
    if (index < 0) this.children.push(child);
    else this.children.splice(index, 0, child);
  }
  remove(): void { this.detach(); }
  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  getAttribute(name: string): string | null { return this.attributes.get(name) ?? null; }
  closest(selector: string): FakeDomElement | null {
    return selector === "[data-adoption-template]" && this.dataset.adoptionTemplate ? this : null;
  }

  contains(candidate: FakeDomElement): boolean {
    return candidate === this || this.children.some((child) => child.contains(candidate));
  }

  private detach(): void {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
    const active = this.ownerDocument.activeElement;
    if (active && this.contains(active)) this.ownerDocument.activeElement = null;
  }
}

function adoptionDomElements() {
  const document: FakeDocument = {
    activeElement: null,
    createElement: (_tagName: string) => {
      const element = new FakeDomElement();
      element.ownerDocument = document;
      return element;
    },
  };
  const make = () => {
    const element = new FakeDomElement();
    element.ownerDocument = document;
    return element;
  };
  const raw = {
    root: make(), catalog: make(), selectedName: make(), selectedPersonality: make(),
    nameInput: make(), actionButton: make(), refreshButton: make(), backButton: make(), status: make(),
  };
  vi.stubGlobal("Element", FakeDomElement);
  return {
    document,
    raw,
    typed: raw as unknown as import("./adoption-creation-view").AdoptionCreationElements,
  };
}

function adoptionPorts(options: {
  catalogs?: AdoptionCatalogEntry[][];
  start?: () => Promise<CreationSnapshot>;
  snapshot?: () => Promise<CreationSnapshot>;
  finalize?: (sessionId: string) => Promise<PetSwitchResult>;
  switchPet?: (petId: string) => Promise<PetSwitchResult>;
  loadMotionProfile?: (url: string) => Promise<unknown>;
  activity?: AdoptionCreationPorts["activity"];
  onBusyChange?: (busy: boolean) => void;
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
    switchPet: vi.fn(options.switchPet ?? (async (petId: string): Promise<PetSwitchResult> => ({
      ok: true as const, requestId: "switch-1", petId,
    }))),
    refreshPets: vi.fn(async () => undefined),
    onBack: vi.fn(),
    onBusyChange: vi.fn((busy: boolean) => options.onBusyChange?.(busy)),
    activity: options.activity,
  } satisfies AdoptionCreationPorts;
  return { ports, view: new AdoptionCreationView(ports) };
}

function blurCatalogFocusOnBusy(dom: ReturnType<typeof adoptionDomElements>, busy: boolean): void {
  const active = dom.document.activeElement;
  if (busy && active && dom.raw.catalog.contains(active)) dom.document.activeElement = null;
}

describe("AdoptionCreationView", () => {
  it("disables only an unavailable template while keeping healthy templates interactive", async () => {
    const unavailable = {
      ...entry(),
      unavailableReason: "素材校验失败",
    } as AdoptionCatalogEntry;
    const test = adoptionPorts({ catalogs: [catalog(unavailable)] });
    const dom = adoptionDomElements();
    test.view.mount(dom.typed);

    await test.view.open();

    expect(dom.raw.catalog.children[0]?.disabled).toBe(true);
    expect(dom.raw.catalog.children[1]?.disabled).toBe(false);
    await expect(test.view.select("cat-misty")).rejects.toThrow(/素材校验失败/);
    expect(test.ports.loadMotionProfile).not.toHaveBeenCalled();

    await test.view.select("cat-2");
    expect(test.ports.preview.show).toHaveBeenCalledOnce();
  });

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
    expect(test.ports.onBack).toHaveBeenCalledOnce();
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

  it("keeps exactly one adoption card in the keyboard tab order", async () => {
    const test = adoptionPorts();
    const dom = adoptionDomElements();
    test.view.mount(dom.typed);

    await test.view.open();

    expect(dom.raw.catalog.children.map((button) => button.tabIndex)).toEqual([
      0, -1, -1, -1, -1, -1, -1, -1,
    ]);
  });

  it("preserves catalog nodes and supports End Arrow Home roving focus", async () => {
    const test = adoptionPorts({ catalogs: [catalog(), catalog()] });
    const dom = adoptionDomElements();
    test.view.mount(dom.typed);
    await test.view.open();
    const firstNode = dom.raw.catalog.children[0]!;
    const lastNode = dom.raw.catalog.children[7]!;

    dom.raw.catalog.dispatch("keydown", { target: firstNode, key: "End" });
    expect(lastNode.focus).toHaveBeenCalledOnce();
    dom.raw.catalog.dispatch("keydown", { target: lastNode, key: "Home" });
    expect(firstNode.focus).toHaveBeenCalledOnce();

    await test.view.refresh();
    expect(dom.raw.catalog.children[0]).toBe(firstNode);
  });

  it("keeps focus on the selected card while selection and preview settle render", async () => {
    const loadingProfile = deferred<unknown>();
    const test = adoptionPorts({ loadMotionProfile: () => loadingProfile.promise });
    const dom = adoptionDomElements();
    test.view.mount(dom.typed);
    await test.view.open();
    const firstNode = dom.raw.catalog.children[0]!;
    firstNode.focus();

    dom.raw.catalog.dispatch("click", { target: firstNode });
    const activeAfterSelection = dom.document.activeElement;
    loadingProfile.resolve(profile);
    await vi.waitFor(() => expect(test.ports.preview.show).toHaveBeenCalledOnce());

    expect(activeAfterSelection).toBe(firstNode);
    expect(dom.document.activeElement).toBe(firstNode);
  });

  it("restores catalog focus across reorder and falls back when the focused card is removed", async () => {
    const initial = catalog();
    const reordered = [initial[1]!, initial[0]!, ...initial.slice(2)];
    const replacement = entry({
      template: {
        ...initial[0]!.template,
        templateId: "cat-9",
        defaultName: "猫咪9",
      },
    });
    const withoutMisty = [...reordered.filter(
      (item) => item.template.templateId !== "cat-misty",
    ), replacement];
    const dom = adoptionDomElements();
    const test = adoptionPorts({
      catalogs: [initial, reordered, withoutMisty],
      onBusyChange: (busy) => blurCatalogFocusOnBusy(dom, busy),
    });
    test.view.mount(dom.typed);
    await test.view.open();
    const firstNode = dom.raw.catalog.children[0]!;
    const catTwoNode = dom.raw.catalog.children[1]!;

    firstNode.focus();
    dom.raw.catalog.dispatch("keydown", { target: firstNode, key: "ArrowRight" });
    await test.view.refresh();
    expect(dom.raw.catalog.children[0]).toBe(catTwoNode);
    expect(dom.document.activeElement).toBe(catTwoNode);

    const mistyNode = dom.raw.catalog.children[1]!;
    dom.raw.catalog.dispatch("keydown", { target: catTwoNode, key: "ArrowRight" });
    expect(dom.document.activeElement).toBe(mistyNode);
    await test.view.refresh();

    expect(dom.document.activeElement).toBe(dom.raw.catalog.children[0]);
    expect(dom.raw.catalog.children[0]!.tabIndex).toBe(0);
    expect(dom.raw.catalog.children.filter((button) => button.tabIndex === 0)).toHaveLength(1);
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

  it("captures catalog focus before shared activity clears it and restores after activation", async () => {
    const dom = adoptionDomElements();
    const blurOnBusy = (busy: boolean) => blurCatalogFocusOnBusy(dom, busy);
    const test = adoptionPorts({
      catalogs: [catalog(entry({ adoptedPetId: "pet-existing" }))],
      activity: new CreationPageActivity(blurOnBusy),
      onBusyChange: blurOnBusy,
    });
    test.view.mount(dom.typed);
    await test.view.open();
    const focusedCard = dom.raw.catalog.children[0]!;
    focusedCard.focus();

    await test.view.activate("cat-misty");

    expect(dom.document.activeElement).toBe(focusedCard);
  });

  it("restores catalog focus when activation throws after global busy cleared it", async () => {
    const dom = adoptionDomElements();
    const blurOnBusy = (busy: boolean) => blurCatalogFocusOnBusy(dom, busy);
    const test = adoptionPorts({
      catalogs: [catalog(entry({ adoptedPetId: "pet-existing" })), catalog(entry({ adoptedPetId: "pet-existing" }))],
      switchPet: async (): Promise<PetSwitchResult> => ({
        ok: false,
        requestId: "switch-failed",
        petId: "pet-existing",
        code: "pet-window-unavailable",
        message: "宠物窗口没有响应",
      }),
      activity: new CreationPageActivity(blurOnBusy),
      onBusyChange: blurOnBusy,
    });
    test.view.mount(dom.typed);
    await test.view.open();
    const focusedCard = dom.raw.catalog.children[0]!;
    focusedCard.focus();

    await expect(test.view.activate("cat-misty")).rejects.toThrow(/没有响应/);

    expect(dom.document.activeElement).toBe(focusedCard);
  });

  it("does not replace focus that was outside the adoption catalog", async () => {
    const dom = adoptionDomElements();
    const blurOnBusy = (busy: boolean) => blurCatalogFocusOnBusy(dom, busy);
    const test = adoptionPorts({
      catalogs: [catalog(entry({ adoptedPetId: "pet-existing" }))],
      activity: new CreationPageActivity(blurOnBusy),
      onBusyChange: blurOnBusy,
    });
    test.view.mount(dom.typed);
    await test.view.open();
    const external = dom.document.createElement("button");
    external.focus();

    await test.view.activate("cat-misty");

    expect(dom.document.activeElement).toBe(external);
  });

  it("does not steal focus moved outside the catalog while activation is pending", async () => {
    const switching = deferred<PetSwitchResult>();
    const dom = adoptionDomElements();
    const blurOnBusy = (busy: boolean) => blurCatalogFocusOnBusy(dom, busy);
    const test = adoptionPorts({
      catalogs: [catalog(entry({ adoptedPetId: "pet-existing" }))],
      switchPet: () => switching.promise,
      activity: new CreationPageActivity(blurOnBusy),
      onBusyChange: blurOnBusy,
    });
    test.view.mount(dom.typed);
    await test.view.open();
    dom.raw.catalog.children[0]!.focus();
    const activation = test.view.activate("cat-misty");
    await vi.waitFor(() => expect(test.ports.switchPet).toHaveBeenCalledOnce());
    const external = dom.document.createElement("button");
    external.focus();

    switching.resolve({ ok: true, requestId: "switch-1", petId: "pet-existing" });
    await activation;

    expect(dom.document.activeElement).toBe(external);
  });

  it.each(["leave", "destroy"] as const)(
    "does not restore hidden adoption focus after %s",
    async (exit) => {
      const switching = deferred<PetSwitchResult>();
      const dom = adoptionDomElements();
      const blurOnBusy = (busy: boolean) => blurCatalogFocusOnBusy(dom, busy);
      const test = adoptionPorts({
        catalogs: [catalog(entry({ adoptedPetId: "pet-existing" }))],
        switchPet: () => switching.promise,
        activity: new CreationPageActivity(blurOnBusy),
        onBusyChange: blurOnBusy,
      });
      test.view.mount(dom.typed);
      await test.view.open();
      const focusedCard = dom.raw.catalog.children[0]!;
      focusedCard.focus();
      const activation = test.view.activate("cat-misty");
      await vi.waitFor(() => expect(test.ports.switchPet).toHaveBeenCalledOnce());

      test.view[exit]();
      const external = dom.document.createElement("button");
      external.focus();
      switching.resolve({ ok: true, requestId: "switch-1", petId: "pet-existing" });
      await activation;

      expect(dom.document.activeElement).toBe(external);
    },
  );

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

  it("does not start a second adoption while recovery remains finalizing", async () => {
    const finalizing = adoptionSnapshot({ status: "finalizing" });
    const test = adoptionPorts({
      catalogs: [catalog(entry({ retrySessionId: "session-adoption" }))],
      snapshot: async () => finalizing,
    });
    await test.view.open();

    await expect(test.view.activate("cat-misty")).rejects.toThrow(/仍在完成|稍后重试/);

    expect(test.ports.creation.recoverFinalization).toHaveBeenCalledTimes(1);
    expect(test.ports.creation.adoptionStart).not.toHaveBeenCalled();
    expect(test.ports.finalize).not.toHaveBeenCalled();
  });

  it("restores and locks the durable custom name for a retry session", async () => {
    const retry = entry({ retrySessionId: "session-adoption" });
    const test = adoptionPorts({
      catalogs: [catalog(retry)],
      snapshot: async () => adoptionSnapshot({ displayName: "小雾的名字" }),
    });
    await test.view.open();

    await test.view.select("cat-misty");

    expect(test.view.displayName()).toBe("小雾的名字");
    expect(test.view.nameLocked()).toBe(true);
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

  it("refreshes durable catalog after an adopted switch fails", async () => {
    const adopted = entry({ adoptedPetId: "pet-existing" });
    const test = adoptionPorts({ catalogs: [catalog(adopted), catalog()] });
    test.ports.switchPet.mockImplementation(async (): Promise<PetSwitchResult> => ({
      ok: false,
      requestId: "switch-failed",
      petId: "pet-existing",
      code: "target-not-found",
      message: "宠物已被删除",
    }));
    await test.view.open();

    await expect(test.view.activate("cat-misty")).rejects.toThrow(/宠物已被删除/);

    expect(test.ports.creation.adoptionCatalog).toHaveBeenCalledTimes(2);
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

  it("returns to the parent view after a newly adopted pet is projected", async () => {
    const test = adoptionPorts({
      catalogs: [catalog(), catalog(entry({ adoptedPetId: "pet-adoption" }))],
    });
    await test.view.open();

    await test.view.activate("cat-misty");

    expect(test.ports.onBack).toHaveBeenCalledOnce();
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
      children: [] as unknown[],
      ownerDocument: {},
      setAttribute: vi.fn(),
      replaceChildren: vi.fn(),
      append: vi.fn(),
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

    expect(added.map((spy) => spy.mock.calls.length)).toEqual([4, 2, 2, 2, 2]);
    expect(removed.map((spy) => spy.mock.calls.length)).toEqual([4, 2, 2, 2, 2]);
  });
});
