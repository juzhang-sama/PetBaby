import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import type { PetProfile, PetProfileClient } from "../pets/pet-profile-contract";
import { PetProfileEditor, type PetProfileEditorElements } from "./pet-profile-editor";

function profile(overrides: Partial<PetProfile> = {}): PetProfile {
  return {
    schemaVersion: 1,
    petId: "pet-a",
    displayName: "小白",
    gender: "unknown",
    birthDate: null,
    editable: true,
    updatedAt: "2026-08-12T08:00:00Z",
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => { resolve = onResolve; reject = onReject; });
  return { promise, reject, resolve };
}

class FakeElement {
  hidden = true;
  disabled = false;
  value = "";
  textContent = "";
  parentElement: FakeElement | null = null;
  readonly focus = vi.fn();
  readonly attributes = new Map<string, string>();
  private readonly listeners = new Map<string, Set<(event: FakeEvent) => void>>();

  append(child: FakeElement): void {
    child.parentElement = this;
  }

  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  removeAttribute(name: string): void { this.attributes.delete(name); }
  addEventListener(type: string, listener: (event: FakeEvent) => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }
  dispatch(type: string, event: FakeEvent = new FakeEvent()): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

class FakeEvent {
  key = "";
  readonly preventDefault = vi.fn();
  constructor(key = "") { this.key = key; }
}

function mount(
  clientOverrides: Partial<PetProfileClient> = {},
  refreshCatalog = vi.fn(async () => true),
) {
  const elements = {
    root: new FakeElement(),
    form: new FakeElement(),
    name: new FakeElement(),
    gender: new FakeElement(),
    birthDate: new FakeElement(),
    cancel: new FakeElement(),
    save: new FakeElement(),
    loading: new FakeElement(),
    error: new FakeElement(),
  } as unknown as PetProfileEditorElements;
  const client: PetProfileClient = {
    get: vi.fn(async (petId) => profile({ petId })),
    update: vi.fn(async ({ petId, value }) => profile({
      petId,
      displayName: value.displayName.trim(),
      gender: value.gender,
      birthDate: value.birthDate,
    })),
    ...clientOverrides,
  };
  const setStatus = vi.fn();
  const editor = new PetProfileEditor(client, { elements, refreshCatalog, setStatus });
  const row = new FakeElement();
  const opener = new FakeElement();
  return { client, editor, elements, opener, refreshCatalog, row, setStatus };
}

describe("PetProfileEditor", () => {
  it("assembles an accessible inline card that remains usable at narrow widths", () => {
    const html = readFileSync(new URL("../../settings.html", import.meta.url), "utf8");
    const source = readFileSync(new URL("../settings.ts", import.meta.url), "utf8");
    const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

    expect(html).toContain('id="pet-profile-editor"');
    expect(html).toContain('aria-labelledby="pet-profile-editor-title"');
    expect(html).toContain('for="pet-profile-name"');
    expect(html).toContain('for="pet-profile-gender"');
    expect(html).toContain('for="pet-profile-birth-date"');
    expect(html).toContain('id="pet-profile-error"');
    expect(html).toContain('role="alert"');
    expect(html).toContain('aria-live="polite"');
    expect(source).toContain("new PetProfileEditor(");
    expect(source).toContain("profileEditor.reconcileAnchor(");
    expect(source).toContain("refreshCatalog: () => renderList(),");
    expect(css).toContain(".pet-profile-card");
    expect(css).toContain(".pet-profile-fields");
    expect(css).toMatch(
      /@media \(max-width: 520px\)[\s\S]*\.settings-shell \.pet-profile-card \{[^}]*flex: 0 0 auto;/,
    );
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
  });

  it("opens a user profile and saves the canonical result before refreshing and closing", async () => {
    const test = mount();
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    test.elements.name.value = " 米米 ";
    test.elements.gender.value = "female";
    test.elements.birthDate.value = "2024-02-29";

    await test.editor.save();

    expect(test.client.update).toHaveBeenCalledWith({
      petId: "pet-a",
      value: { displayName: " 米米 ", gender: "female", birthDate: "2024-02-29" },
    });
    expect(test.elements.name.value).toBe("米米");
    expect(test.refreshCatalog).toHaveBeenCalledOnce();
    expect(test.elements.root.hidden).toBe(true);
    expect(test.setStatus).toHaveBeenCalledWith("资料已保存", "info");
  });

  it("normalizes an empty date to null", async () => {
    const test = mount();
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    test.elements.birthDate.value = "";

    await test.editor.save();

    expect(test.client.update).toHaveBeenCalledWith(expect.objectContaining({
      value: expect.objectContaining({ birthDate: null }),
    }));
  });

  it("refuses a backend profile that is not editable even when an action was rendered", async () => {
    const update = vi.fn();
    const test = mount({ get: vi.fn(async () => profile({ editable: false })), update });

    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);

    expect(test.elements.root.hidden).toBe(true);
    expect(update).not.toHaveBeenCalled();
    expect(test.setStatus).toHaveBeenCalledWith("这只宠物的资料不可编辑。", "error");
    expect(test.opener.focus).toHaveBeenCalledOnce();
  });

  it("keeps one editor and ignores a stale load after switching pets", async () => {
    const first = deferred<PetProfile>();
    const get = vi.fn((petId: string) => petId === "pet-a"
      ? first.promise
      : Promise.resolve(profile({ petId: "pet-b", displayName: "豆豆" })));
    const test = mount({ get });
    const firstRow = new FakeElement();
    const secondRow = new FakeElement();
    const secondOpener = new FakeElement();

    const oldOpen = test.editor.open("pet-a", firstRow as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    await test.editor.open("pet-b", secondRow as unknown as HTMLElement, secondOpener as unknown as HTMLButtonElement);
    first.resolve(profile({ petId: "pet-a", displayName: "旧响应" }));
    await oldOpen;

    expect(test.editor.editingPetId).toBe("pet-b");
    expect(test.elements.root.parentElement).toBe(secondRow);
    expect(test.elements.name.value).toBe("豆豆");
  });

  it("does not let a stale load failure clear the current pet", async () => {
    const first = deferred<PetProfile>();
    const get = vi.fn((petId: string) => petId === "pet-a"
      ? first.promise
      : Promise.resolve(profile({ petId: "pet-b", displayName: "豆豆" })));
    const test = mount({ get });
    const oldOpen = test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);

    await test.editor.open("pet-b", new FakeElement() as unknown as HTMLElement, new FakeElement() as unknown as HTMLButtonElement);
    first.reject(new Error("旧请求失败"));
    await oldOpen;

    expect(test.editor.editingPetId).toBe("pet-b");
    expect(test.elements.name.value).toBe("豆豆");
    expect(test.elements.name.disabled).toBe(false);
    expect(test.elements.error.textContent).toBe("");
  });

  it("treats a repeated open for the same loaded pet as focus, not another request", async () => {
    const test = mount();
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);

    expect(test.client.get).toHaveBeenCalledOnce();
    expect(test.elements.name.focus).toHaveBeenCalledTimes(2);
  });

  it("clears the previous pet immediately and keeps controls locked when the next load fails", async () => {
    const next = deferred<PetProfile>();
    const get = vi.fn((petId: string) => petId === "pet-a"
      ? Promise.resolve(profile({ petId, displayName: "小白", gender: "female", birthDate: "2024-02-29" }))
      : next.promise);
    const test = mount({ get });
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    const nextRow = new FakeElement();
    const nextOpener = new FakeElement();

    const pending = test.editor.open("pet-b", nextRow as unknown as HTMLElement, nextOpener as unknown as HTMLButtonElement);
    expect(test.elements.name.value).toBe("");
    expect(test.elements.gender.value).toBe("");
    expect(test.elements.birthDate.value).toBe("");
    expect(test.elements.name.disabled).toBe(true);
    expect(test.elements.gender.disabled).toBe(true);
    expect(test.elements.birthDate.disabled).toBe(true);
    expect(test.elements.save.disabled).toBe(true);
    next.reject(new Error("读取失败"));
    await pending;

    expect(test.editor.editingPetId).toBe("pet-b");
    expect(test.elements.name.value).toBe("");
    expect(test.elements.name.disabled).toBe(true);
    expect(test.elements.error.textContent).toContain("读取失败");
    await test.editor.save();
    expect(test.client.update).not.toHaveBeenCalled();
  });

  it("retries the same pet after a failed load instead of leaving a dead editor", async () => {
    const get = vi.fn()
      .mockRejectedValueOnce(new Error("暂时失败"))
      .mockResolvedValueOnce(profile({ petId: "pet-a", displayName: "重试成功" }));
    const test = mount({ get });

    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);

    expect(get).toHaveBeenCalledTimes(2);
    expect(test.elements.name.value).toBe("重试成功");
    expect(test.elements.name.disabled).toBe(false);
  });

  it("disables the form while saving and preserves input on failure", async () => {
    const saving = deferred<PetProfile>();
    const test = mount({ update: vi.fn(() => saving.promise) });
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    test.elements.name.value = "保留我";

    const pending = test.editor.save();
    expect(test.elements.name.disabled).toBe(true);
    expect(test.elements.gender.disabled).toBe(true);
    expect(test.elements.birthDate.disabled).toBe(true);
    expect(test.elements.save.disabled).toBe(true);
    expect(test.elements.cancel.disabled).toBe(false);
    saving.reject(new Error("<img src=x onerror=alert(1)>") );
    await pending;

    expect(test.elements.name.value).toBe("保留我");
    expect(test.elements.name.disabled).toBe(false);
    expect(test.elements.error.textContent).toContain("<img src=x onerror=alert(1)>");
    expect((test.elements.error as unknown as FakeElement).attributes.get("role")).toBe("alert");
  });

  it("cancels with Escape and restores focus to the edit trigger", async () => {
    const test = mount();
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    const event = new FakeEvent("Escape");

    (test.elements.root as unknown as FakeElement).dispatch("keydown", event);

    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(test.elements.root.hidden).toBe(true);
    expect(test.opener.focus).toHaveBeenCalledOnce();
  });

  it("can cancel while loading and ignores the late response", async () => {
    const loading = deferred<PetProfile>();
    const test = mount({ get: vi.fn(() => loading.promise) });

    const pending = test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    expect(test.elements.cancel.disabled).toBe(false);
    expect(test.elements.cancel.focus).toHaveBeenCalledOnce();
    (test.elements.cancel as unknown as FakeElement).dispatch("click");
    expect(test.elements.root.hidden).toBe(true);
    expect(test.opener.focus).toHaveBeenCalledOnce();
    loading.resolve(profile({ displayName: "迟到资料" }));
    await pending;

    expect(test.editor.editingPetId).toBeNull();
    expect(test.elements.root.hidden).toBe(true);
    expect(test.elements.name.value).not.toBe("迟到资料");
  });

  it("handles Escape while loading and restores the trigger", async () => {
    const loading = deferred<PetProfile>();
    const test = mount({ get: vi.fn(() => loading.promise) });
    const pending = test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    const event = new FakeEvent("Escape");

    expect(test.elements.cancel.focus).toHaveBeenCalledOnce();
    (test.elements.root as unknown as FakeElement).dispatch("keydown", event);
    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(test.elements.root.hidden).toBe(true);
    expect(test.opener.focus).toHaveBeenCalledOnce();
    loading.reject(new Error("迟到失败"));
    await pending;

    expect(test.elements.root.hidden).toBe(true);
    expect(test.elements.error.textContent).toBe("");
  });

  it("keeps canonical values and reports saved-but-not-refreshed without a success claim", async () => {
    const refreshCatalog = vi.fn(async () => false);
    const test = mount({}, refreshCatalog);
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    test.elements.name.value = " 新名字 ";

    await test.editor.save();

    expect(test.elements.root.hidden).toBe(false);
    expect(test.elements.name.value).toBe("新名字");
    expect(test.elements.name.disabled).toBe(false);
    expect(test.elements.error.textContent).toBe("资料已保存，但列表刷新失败，请稍后重试。");
    expect(test.setStatus).not.toHaveBeenCalledWith("资料已保存", "info");
  });

  it("does not let a stale save affect a newly opened editor", async () => {
    const saving = deferred<PetProfile>();
    const update = vi.fn(() => saving.promise);
    const test = mount({ update });
    await test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    const oldSave = test.editor.save();
    const nextRow = new FakeElement();
    const nextOpener = new FakeElement();
    await test.editor.open("pet-b", nextRow as unknown as HTMLElement, nextOpener as unknown as HTMLButtonElement);
    saving.resolve(profile({ petId: "pet-a", displayName: "旧保存" }));
    await oldSave;

    expect(test.editor.editingPetId).toBe("pet-b");
    expect(test.elements.name.value).toBe("小白");
    expect(test.refreshCatalog).not.toHaveBeenCalled();
    expect(test.setStatus).not.toHaveBeenCalledWith("资料已保存", "info");
  });

  it("reattaches after catalog refresh and safely closes when the edited row disappears", async () => {
    const loading = deferred<PetProfile>();
    const test = mount({ get: vi.fn(() => loading.promise) });
    const pending = test.editor.open("pet-a", test.row as unknown as HTMLElement, test.opener as unknown as HTMLButtonElement);
    const refreshedRow = new FakeElement();
    const refreshedOpener = new FakeElement();

    test.editor.reconcileAnchor(refreshedRow as unknown as HTMLElement, refreshedOpener as unknown as HTMLButtonElement);
    expect(test.elements.root.parentElement).toBe(refreshedRow);
    test.editor.reconcileAnchor(null, null);
    loading.resolve(profile({ displayName: "迟到响应" }));
    await pending;

    expect(test.editor.editingPetId).toBeNull();
    expect(test.elements.root.hidden).toBe(true);
    expect(test.elements.name.value).not.toBe("迟到响应");
  });
});
