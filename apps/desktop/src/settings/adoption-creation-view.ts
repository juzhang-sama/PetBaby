import type {
  AdoptionCatalogEntry,
  AdoptionTemplate,
  CreationSnapshot,
} from "../creation/contracts";
import type { RecoveryReport } from "../creation/api";
import { parseMotionProfile } from "../runtime/animated-image-manifest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import type { CandidatePreviewController } from "./candidate-dynamic-preview";
import type { CreationPageActivityPort } from "./creation-page-run";

export interface AdoptionCreationPorts {
  creation: {
    adoptionCatalog(): Promise<AdoptionCatalogEntry[]>;
    adoptionStart(templateId: string, displayName: string): Promise<CreationSnapshot>;
    snapshot(sessionId: string): Promise<CreationSnapshot>;
    recoverFinalization(): Promise<RecoveryReport>;
  };
  previewRoot: HTMLElement;
  preview: Pick<CandidatePreviewController, "show" | "clear">;
  assetUrl(templateId: string, relativePath: string): string;
  loadMotionProfile(url: string): Promise<unknown>;
  finalize(sessionId: string): Promise<PetSwitchResult>;
  switchPet(petId: string): Promise<PetSwitchResult>;
  refreshPets(): Promise<void>;
  onBusyChange(busy: boolean): void;
  activity?: CreationPageActivityPort;
  onBack?(): void;
}

export interface AdoptionCreationElements {
  root: HTMLElement;
  catalog: HTMLElement;
  selectedName: HTMLElement;
  selectedPersonality: HTMLElement;
  nameInput: HTMLInputElement;
  actionButton: HTMLButtonElement;
  refreshButton: HTMLButtonElement;
  backButton: HTMLButtonElement;
  status: HTMLElement;
}

export class AdoptionCreationView {
  private catalogValue: AdoptionCatalogEntry[] = [];
  private selectedId: string | null = null;
  private focusTemplateId: string | null = null;
  private pendingCatalogFocusId: string | null = null;
  private nameValue = "";
  private nameLockedValue = false;
  private status = "选择一只猫，先看看它呼吸微动的样子。";
  private active = false;
  private destroyed = false;
  private visit = 0;
  private selectionRevision = 0;
  private previewReady = false;
  private previewCleared = true;
  private busyValue = false;
  private activationFlight: Promise<void> | null = null;
  private refreshFlight: Promise<void> | null = null;
  private elements: AdoptionCreationElements | null = null;
  private cleanups: Array<() => void> = [];

  constructor(private readonly ports: AdoptionCreationPorts) {}

  entries(): AdoptionCatalogEntry[] {
    return this.catalogValue.map((item) => ({ ...item, template: { ...item.template } }));
  }

  entry(templateId: string): AdoptionCatalogEntry | undefined {
    const item = this.catalogValue.find((candidate) => candidate.template.templateId === templateId);
    return item ? { ...item, template: { ...item.template } } : undefined;
  }

  selectedTemplateId(): string | null {
    return this.selectedId;
  }

  dynamicReady(): boolean {
    return this.previewReady;
  }

  busy(): boolean {
    return this.busyValue;
  }

  statusText(): string {
    return this.status;
  }

  displayName(): string {
    return this.nameValue;
  }

  nameLocked(): boolean {
    return this.nameLockedValue;
  }

  async open(): Promise<void> {
    const pendingActivation = this.activationFlight;
    const visit = this.beginVisit();
    this.status = "正在读取本机认领目录…";
    this.render();
    if (pendingActivation) await pendingActivation.catch(() => undefined);
    if (!this.isCurrent(visit)) return;
    await this.refreshCatalog(visit);
    if (!this.isCurrent(visit)) return;
    this.status = "选择一只猫，先看看它呼吸微动的样子。";
    this.render();
  }

  refresh(): Promise<void> {
    if (this.refreshFlight) return this.refreshFlight;
    const operation = this.open();
    this.captureCatalogFocus();
    const tracked = operation.finally(() => {
      if (this.refreshFlight === tracked) this.refreshFlight = null;
      this.setBusy(false);
    });
    this.refreshFlight = tracked;
    this.setBusy(true);
    return tracked;
  }

  async select(templateId: string): Promise<void> {
    const selected = this.requireEntry(templateId);
    const visit = this.visit;
    if (!this.isCurrent(visit)) throw new Error("认领页面已离开，请重新打开后选择");
    const revision = ++this.selectionRevision;
    this.selectedId = templateId;
    this.nameValue = selected.template.defaultName;
    this.nameLockedValue = selected.retrySessionId !== null || selected.adoptedPetId !== null;
    this.previewReady = false;
    this.clearPreview();
    this.status = `正在加载${selected.template.defaultName}的动态预览…`;
    this.render();
    try {
      if (selected.retrySessionId) {
        const snapshot = await this.ports.creation.snapshot(selected.retrySessionId);
        if (!this.isSelectionCurrent(visit, revision, templateId)) return;
        this.nameValue = snapshot.displayName ?? selected.template.defaultName;
        this.render();
      }
      const profileUrl = this.assetUrl(selected.template, selected.template.motionProfilePath);
      const rawProfile = await this.ports.loadMotionProfile(profileUrl);
      if (!this.isSelectionCurrent(visit, revision, templateId)) return;
      const profile = parseMotionProfile(rawProfile);
      const bodyUrl = this.assetUrl(selected.template, selected.template.bodyPath);
      this.previewCleared = false;
      await this.ports.preview.show(this.ports.previewRoot, bodyUrl, profile);
      if (!this.isSelectionCurrent(visit, revision, templateId)) return;
      this.previewReady = true;
      this.status = `${selected.template.defaultName}的呼吸与微动预览已准备好。`;
      this.render();
    } catch (error) {
      if (this.isSelectionCurrent(visit, revision, templateId)) {
        this.previewReady = false;
        this.status = `动态预览未能加载：${errorMessage(error)}。请选择这张卡片重试。`;
        this.render();
      }
      throw error;
    }
  }

  activate(templateId: string, displayName?: string): Promise<void> {
    if (this.activationFlight) return this.activationFlight;
    const visit = this.visit;
    const entry = this.requireEntry(templateId);
    this.captureCatalogFocus();
    const execute = () => this.activateOnce(templateId, displayName, visit);
    const operation = this.ports.activity
      ? this.ports.activity.run({
        route: "adoption",
        kind: entry.adoptedPetId ? "switch" : "finalize",
        sessionId: entry.retrySessionId,
      }, execute)
      : execute();
    const tracked = operation.finally(() => {
      if (this.activationFlight === tracked) this.activationFlight = null;
      this.setBusy(false);
    });
    this.activationFlight = tracked;
    this.setBusy(true);
    return tracked;
  }

  leave(): void {
    if (!this.active && this.previewCleared) return;
    this.active = false;
    this.visit += 1;
    this.selectionRevision += 1;
    this.pendingCatalogFocusId = null;
    this.previewReady = false;
    this.clearPreviewOnce();
    this.setBusy(false);
    this.render();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.leave();
    this.unmount();
  }

  mount(elements: AdoptionCreationElements): void {
    this.unmount();
    this.destroyed = false;
    this.elements = elements;
    this.listen(elements.catalog, "click", (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const button = target.closest<HTMLButtonElement>("[data-adoption-template]");
      const templateId = button?.dataset.adoptionTemplate;
      if (!templateId || this.busyValue) return;
      this.focusTemplateId = templateId;
      void this.select(templateId).catch(() => undefined);
    });
    this.listen(elements.catalog, "keydown", (event) => this.moveCatalogFocus(event));
    this.listen(elements.nameInput, "input", () => {
      this.nameValue = elements.nameInput.value;
    });
    this.listen(elements.actionButton, "click", () => {
      if (!this.selectedId || this.busyValue) return;
      const visit = this.visit;
      void this.activate(this.selectedId, elements.nameInput.value).catch((error) => {
        if (!this.isCurrent(visit)) return;
        this.status = `操作未完成：${errorMessage(error)}。请保留当前页面并点击按钮重试。`;
        this.render();
      });
    });
    this.listen(elements.refreshButton, "click", () => {
      if (this.busyValue) return;
      const opening = this.refresh();
      const visit = this.visit;
      void opening.catch((error) => {
        if (!this.isCurrent(visit)) return;
        this.status = `认领目录未能刷新：${errorMessage(error)}。请重试。`;
        this.render();
      });
    });
    this.listen(elements.backButton, "click", () => {
      if (!this.busyValue) this.ports.onBack?.();
    });
    this.render();
  }

  private async activateOnce(
    templateId: string,
    displayName: string | undefined,
    visit: number,
  ): Promise<void> {
    const selected = this.requireEntry(templateId);
    if (selected.adoptedPetId) {
      const result = await this.ports.switchPet(selected.adoptedPetId);
      if (!result.ok) {
        await this.refreshCatalog(visit);
        throw new Error(result.message);
      }
      await this.ports.refreshPets();
      if (this.isCurrent(visit)) {
        this.status = `已切换到${selected.template.defaultName}。`;
        this.render();
      }
      return;
    }

    let sessionId = selected.retrySessionId;
    if (sessionId) {
      const snapshot = await this.ports.creation.snapshot(sessionId);
      const reconciled = await this.reconcileSnapshot(snapshot);
      if (reconciled === "completed") {
        await this.projectSuccess(templateId, visit);
        return;
      }
      if (reconciled === "needsStart") sessionId = null;
    }

    if (!sessionId) {
      const name = normalizeDisplayName(displayName ?? (this.nameValue || selected.template.defaultName));
      try {
        const snapshot = await this.ports.creation.adoptionStart(templateId, name);
        sessionId = snapshot.sessionId;
      } catch (startError) {
        await this.refreshCatalog(visit);
        const durable = this.requireEntry(templateId);
        if (durable.adoptedPetId) {
          await this.projectSuccess(templateId, visit);
          return;
        }
        sessionId = durable.retrySessionId;
        if (!sessionId) throw startError;
        const snapshot = await this.ports.creation.snapshot(sessionId);
        const reconciled = await this.reconcileSnapshot(snapshot);
        if (reconciled === "completed") {
          await this.projectSuccess(templateId, visit);
          return;
        }
        if (reconciled === "needsStart") throw startError;
      }
    }

    await this.finalizeAndProject(templateId, sessionId, visit);
  }

  private async reconcileSnapshot(
    snapshot: CreationSnapshot,
  ): Promise<"finalizable" | "completed" | "needsStart"> {
    if (snapshot.status === "completed") return "completed";
    if (snapshot.status === "finalizing") {
      await this.ports.creation.recoverFinalization();
      const recovered = await this.ports.creation.snapshot(snapshot.sessionId);
      if (recovered.status === "completed") return "completed";
      if (recovered.status === "finalizing") {
        throw new Error("认领仍在完成，请稍后重试，系统不会重复创建");
      }
      if (isFinalizable(recovered)) return "finalizable";
      return "needsStart";
    }
    return isFinalizable(snapshot) ? "finalizable" : "needsStart";
  }

  private async finalizeAndProject(
    templateId: string,
    sessionId: string,
    visit: number,
  ): Promise<void> {
    let result: PetSwitchResult;
    try {
      result = await this.ports.finalize(sessionId);
    } catch (error) {
      const durable = await this.ports.creation.snapshot(sessionId).catch(() => null);
      if (durable?.status === "completed") {
        await this.projectSuccess(templateId, visit);
        return;
      }
      await this.refreshCatalog(visit);
      throw error;
    }
    if (!result.ok) {
      await this.refreshCatalog(visit);
      throw new Error(result.message);
    }
    await this.projectSuccess(templateId, visit);
  }

  private async projectSuccess(templateId: string, visit: number): Promise<void> {
    await this.ports.refreshPets();
    await this.refreshCatalog(visit);
    if (!this.isCurrent(visit)) return;
    const projected = this.requireEntry(templateId);
    if (!projected.adoptedPetId) {
      throw new Error("认领完成状态尚未同步，请刷新目录重试");
    }
    if (this.isCurrent(visit)) {
      this.status = `${projected.template.defaultName}已认领并显示在桌面。`;
      this.render();
    }
  }

  private async refreshCatalog(visit: number): Promise<void> {
    const entries = await this.ports.creation.adoptionCatalog();
    validateCatalog(entries);
    if (!this.isCurrent(visit)) return;
    this.catalogValue = entries.map((item) => ({ ...item, template: { ...item.template } }));
    if (this.selectedId && !this.catalogValue.some((item) => item.template.templateId === this.selectedId)) {
      this.selectedId = null;
      this.nameValue = "";
      this.nameLockedValue = false;
      this.previewReady = false;
      this.clearPreviewOnce();
    } else if (this.selectedId) {
      const selected = this.catalogValue.find((item) => item.template.templateId === this.selectedId)!;
      this.nameLockedValue = selected.retrySessionId !== null || selected.adoptedPetId !== null;
      if (!this.nameLockedValue) this.nameValue = selected.template.defaultName;
    }
    this.render();
  }

  private beginVisit(): number {
    this.active = true;
    this.destroyed = false;
    this.visit += 1;
    this.selectionRevision += 1;
    this.pendingCatalogFocusId = null;
    this.selectedId = null;
    this.nameValue = "";
    this.nameLockedValue = false;
    this.previewReady = false;
    this.clearPreviewOnce();
    return this.visit;
  }

  private isCurrent(visit: number): boolean {
    return this.active && this.visit === visit;
  }

  private isSelectionCurrent(visit: number, revision: number, templateId: string): boolean {
    return this.isCurrent(visit) && revision === this.selectionRevision && this.selectedId === templateId;
  }

  private requireEntry(templateId: string): AdoptionCatalogEntry {
    const selected = this.catalogValue.find((item) => item.template.templateId === templateId);
    if (!selected) throw new Error(`认领目录中没有模板：${templateId}`);
    return selected;
  }

  private assetUrl(template: AdoptionTemplate, relativePath: string): string {
    return this.ports.assetUrl(template.templateId, relativePath);
  }

  private setBusy(value: boolean): void {
    if (this.busyValue === value) return;
    this.busyValue = value;
    this.ports.onBusyChange(value);
    this.render();
  }

  private captureCatalogFocus(): void {
    const catalog = this.elements?.catalog;
    if (!catalog) return;
    const activeElement = catalog.ownerDocument.activeElement;
    const activeButton = activeElement instanceof Element
      ? activeElement.closest<HTMLButtonElement>("[data-adoption-template]")
      : null;
    if (activeButton && catalog.contains(activeButton)) {
      this.pendingCatalogFocusId = activeButton.dataset.adoptionTemplate ?? null;
    }
  }

  private clearPreview(): void {
    this.ports.preview.clear();
    this.previewCleared = true;
  }

  private clearPreviewOnce(): void {
    if (this.previewCleared) return;
    this.clearPreview();
  }

  private render(): void {
    const dom = this.elements;
    if (!dom) return;
    dom.root.setAttribute("aria-busy", String(this.busyValue));
    dom.status.textContent = this.status;
    dom.refreshButton.disabled = this.busyValue;
    dom.backButton.disabled = this.busyValue;
    const selected = this.selectedId ? this.catalogValue.find(
      (item) => item.template.templateId === this.selectedId,
    ) : undefined;
    dom.selectedName.textContent = selected?.template.defaultName ?? "还没有选择猫咪";
    dom.selectedPersonality.textContent = selected?.template.personality
      ?? "从左侧目录选择一张卡片，动态预览会出现在这里。";
    if (dom.nameInput.value !== this.nameValue) dom.nameInput.value = this.nameValue;
    dom.nameInput.disabled = this.busyValue || !selected || this.nameLockedValue;
    dom.nameInput.setAttribute("aria-disabled", String(dom.nameInput.disabled));
    dom.actionButton.textContent = selected ? actionLabel(selected) : "先选择一只猫";
    dom.actionButton.disabled = this.busyValue || !selected || !this.previewReady;
    dom.actionButton.setAttribute("aria-disabled", String(dom.actionButton.disabled));
    this.renderCatalog(dom.catalog);
  }

  private renderCatalog(root: HTMLElement): void {
    const document = root.ownerDocument;
    const activeElement = document.activeElement;
    const activeButton = activeElement instanceof Element
      ? activeElement.closest<HTMLButtonElement>("[data-adoption-template]")
      : null;
    if (activeButton && root.contains(activeButton)) {
      this.pendingCatalogFocusId = activeButton.dataset.adoptionTemplate ?? null;
    }
    const existing = new Map(Array.from(root.children, (child) => {
      const button = child as HTMLButtonElement;
      return [button.dataset.adoptionTemplate ?? "", button] as const;
    }));
    const buttons = this.catalogValue.map((entry) => {
      const templateId = entry.template.templateId;
      const button = existing.get(templateId) ?? document.createElement("button");
      button.type = "button";
      button.className = "adoption-card";
      button.dataset.adoptionTemplate = templateId;
      button.setAttribute("role", "option");
      button.setAttribute("aria-selected", String(templateId === this.selectedId));
      button.disabled = this.busyValue;
      button.setAttribute("aria-disabled", String(button.disabled));
      const image = (button.children[0] as HTMLImageElement | undefined) ?? document.createElement("img");
      image.src = this.assetUrl(entry.template, entry.template.thumbnailPath);
      image.alt = `${entry.template.defaultName}缩略图`;
      image.loading = "lazy";
      const copy = (button.children[1] as HTMLElement | undefined) ?? document.createElement("span");
      copy.className = "adoption-card-copy";
      const name = (copy.children[0] as HTMLElement | undefined) ?? document.createElement("strong");
      name.textContent = entry.template.defaultName;
      const personality = (copy.children[1] as HTMLElement | undefined) ?? document.createElement("span");
      personality.textContent = entry.template.personality;
      const state = (copy.children[2] as HTMLElement | undefined) ?? document.createElement("span");
      state.className = "adoption-card-state";
      state.textContent = actionLabel(entry);
      if (copy.children.length === 0) copy.append(name, personality, state);
      if (button.children.length === 0) button.append(image, copy);
      return button;
    });
    const desiredIds = new Set(buttons.map((button) => button.dataset.adoptionTemplate));
    const structureChanged = buttons.length !== root.children.length || buttons.some(
      (button, index) => root.children[index] !== button,
    );
    if (structureChanged) {
      for (const [templateId, button] of existing) {
        if (!desiredIds.has(templateId)) button.remove();
      }
      for (const [index, button] of buttons.entries()) {
        const current = root.children[index] ?? null;
        if (current !== button) root.insertBefore(button, current);
      }
    }
    if (!this.focusTemplateId || !buttons.some(
      (button) => button.dataset.adoptionTemplate === this.focusTemplateId,
    )) {
      this.focusTemplateId = this.selectedId ?? buttons[0]?.dataset.adoptionTemplate ?? null;
    }
    for (const button of buttons) {
      button.tabIndex = button.dataset.adoptionTemplate === this.focusTemplateId ? 0 : -1;
    }
    if (!this.busyValue && this.pendingCatalogFocusId) {
      const canRestoreFocus = this.active && (
        !document.activeElement
        || document.activeElement === document.body
        || root.contains(document.activeElement)
      );
      const focusedId = desiredIds.has(this.pendingCatalogFocusId)
        ? this.pendingCatalogFocusId
        : this.focusTemplateId;
      const target = buttons.find((button) => button.dataset.adoptionTemplate === focusedId);
      if (canRestoreFocus && target && document.activeElement !== target) target.focus();
      this.pendingCatalogFocusId = null;
    }
  }

  private moveCatalogFocus(event: Event): void {
    const keyboard = event as KeyboardEvent;
    if (!["ArrowDown", "ArrowRight", "ArrowUp", "ArrowLeft", "Home", "End"].includes(keyboard.key)) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const button = target.closest<HTMLButtonElement>("[data-adoption-template]");
    const currentId = button?.dataset.adoptionTemplate;
    if (!currentId || !this.elements) return;
    const ids = this.catalogValue.map((entry) => entry.template.templateId);
    const current = ids.indexOf(currentId);
    if (current < 0) return;
    let next = current;
    if (keyboard.key === "Home") next = 0;
    else if (keyboard.key === "End") next = ids.length - 1;
    else if (keyboard.key === "ArrowDown" || keyboard.key === "ArrowRight") {
      next = Math.min(ids.length - 1, current + 1);
    } else {
      next = Math.max(0, current - 1);
    }
    event.preventDefault();
    this.focusTemplateId = ids[next]!;
    const buttons = Array.from(this.elements.catalog.children) as HTMLButtonElement[];
    for (const candidate of buttons) {
      candidate.tabIndex = candidate.dataset.adoptionTemplate === this.focusTemplateId ? 0 : -1;
    }
    buttons.find((candidate) => candidate.dataset.adoptionTemplate === this.focusTemplateId)?.focus();
  }

  private listen(target: EventTarget, type: string, listener: EventListener): void {
    target.addEventListener(type, listener);
    this.cleanups.push(() => target.removeEventListener(type, listener));
  }

  private unmount(): void {
    for (const cleanup of this.cleanups.splice(0)) cleanup();
    this.elements = null;
  }
}

export function normalizeDisplayName(value: string): string {
  const normalized = value.trim();
  if ([...normalized].some((character) => {
    const code = character.codePointAt(0)!;
    return code <= 0x1f || code === 0x7f || code === 0x2028 || code === 0x2029;
  })) {
    throw new Error("名称不能包含控制字符或换行");
  }
  const count = typeof Intl.Segmenter === "function"
    ? [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(normalized)].length
    : [...normalized].length;
  if (count < 1 || count > 20) throw new Error("名称请输入 1–20 个完整字符");
  return normalized;
}

function validateCatalog(entries: AdoptionCatalogEntry[]): void {
  if (entries.length !== 8) throw new Error(`认领目录必须包含 8 只猫，当前为 ${entries.length} 只`);
  const ids = entries.map((item) => item.template.templateId);
  if (new Set(ids).size !== ids.length) throw new Error("认领目录包含重复模板，无法安全显示");
}

function isFinalizable(snapshot: CreationSnapshot): boolean {
  return snapshot.status === "candidateReady"
    || (snapshot.status === "retryableFailure" && snapshot.lastStableStatus === "candidateReady");
}

function actionLabel(entry: AdoptionCatalogEntry): string {
  if (entry.adoptedPetId) return "切换到它";
  if (entry.retrySessionId) return "重试认领";
  return "认领";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
