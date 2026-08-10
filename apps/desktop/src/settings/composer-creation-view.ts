import type { ComposerCandidateProjection, RecoveryReport } from "../creation/api";
import type { ComposerRecipe, CreationSnapshot } from "../creation/contracts";
import type { ComposerPackManifest } from "../creation/composer-pack";
import { validateRecipe } from "../creation/composer-pack";
import { ComposerState, type ComposerSelectionKind, type ComposerStep } from "../creation/composer-state";
import { parseMotionProfile } from "../runtime/animated-image-manifest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import type { CandidatePreviewController } from "./candidate-dynamic-preview";

type SaveState = "idle" | "saving" | "saved" | "unsaved";

export interface ComposerCreationApiPort {
  start(method: "composer"): Promise<CreationSnapshot>;
  draft(): Promise<CreationSnapshot | null>;
  snapshot(sessionId: string): Promise<CreationSnapshot>;
  composerSave(
    sessionId: string,
    recipe: ComposerRecipe,
    currentStep: string,
  ): Promise<CreationSnapshot>;
  composerCandidate(sessionId: string, pngB64: string): Promise<ComposerCandidateProjection>;
  recoverFinalization(): Promise<RecoveryReport>;
  setName(sessionId: string, displayName: string): Promise<CreationSnapshot>;
  abandon(sessionId: string): Promise<void>;
}

export interface ComposerCreationPorts {
  creation: ComposerCreationApiPort;
  loadPack(): Promise<ComposerPackManifest>;
  render(
    pack: ComposerPackManifest,
    recipe: ComposerRecipe,
    target?: HTMLCanvasElement,
  ): Promise<void>;
  exportPng(pack: ComposerPackManifest, recipe: ComposerRecipe): Promise<Blob>;
  blobToBase64(blob: Blob): Promise<string>;
  preview: Pick<CandidatePreviewController, "show" | "clear">;
  finalize(sessionId: string): Promise<PetSwitchResult>;
  confirm(message: string): boolean;
}

export interface ComposerCreationElements {
  canvas: HTMLCanvasElement;
  steps: HTMLElement;
  options: HTMLElement;
  saveStatus: HTMLElement;
  message: HTMLElement;
  previousButton: HTMLButtonElement;
  nextButton: HTMLButtonElement;
  candidateButton: HTMLButtonElement;
  candidatePreview: HTMLElement;
  nameInput: HTMLInputElement;
  finishButton: HTMLButtonElement;
  abandonButton: HTMLButtonElement;
}

const STEP_ORDER: readonly ComposerStep[] = [
  "body", "ears", "eyes", "muzzle", "tail", "coat", "name", "preview",
];

const NEXT_STEP: Record<ComposerSelectionKind, ComposerStep> = {
  body: "ears",
  ears: "eyes",
  eyes: "muzzle",
  muzzle: "tail",
  tail: "coat",
  color: "coat",
  pattern: "name",
};

interface ComposerMutationCoordinator {
  initialFlight: Promise<CreationSnapshot> | null;
  pendingInitialBody: string | null;
  sessionTails: Map<string, Promise<void>>;
  candidateFlights: Map<string, Promise<void>>;
  finalizeFlights: Map<string, Promise<PetSwitchResult>>;
  abandonFlights: Map<string, Promise<void>>;
  recoveryFlight: Promise<void> | null;
}

interface PreviewMutationCoordinator {
  tail: Promise<void>;
}

const MUTATION_COORDINATORS = new WeakMap<ComposerCreationApiPort, ComposerMutationCoordinator>();
const PREVIEW_COORDINATORS = new WeakMap<object, PreviewMutationCoordinator>();

export class ComposerCreationView {
  private packValue: ComposerPackManifest | null = null;
  private composer: ComposerState | null = null;
  private snapshotValue: CreationSnapshot | null = null;
  private stepValue: ComposerStep = "body";
  private persistence: SaveState = "idle";
  private message = "请选择身体底型开始组合。";
  private visit = 0;
  private active = false;
  private selectionRevision = 0;
  private candidateProjection: ComposerCandidateProjection | null = null;
  private dynamicReady = false;
  private previewCleared = true;
  private elements: ComposerCreationElements | null = null;
  private cleanupListeners: Array<() => void> = [];
  private readonly mutations: ComposerMutationCoordinator;

  constructor(private readonly ports: ComposerCreationPorts) {
    this.mutations = mutationCoordinatorFor(ports.creation);
  }

  async open(): Promise<void> {
    const visit = this.beginVisit();
    const pack = await this.ports.loadPack();
    if (!this.isCurrent(visit)) return;
    const draft = await this.readDraftAfterSharedMutations(visit);
    if (!this.isCurrent(visit)) return;
    this.packValue = pack;
    if (!draft) {
      this.resetDraftState();
      await this.renderCurrent(visit);
      return;
    }
    if (draft.method !== "composer") {
      this.resetDraftState();
      this.message = "已有其他创建草稿；请返回创建入口决定继续或放弃。";
      this.renderDom();
      return;
    }
    this.applySnapshot(draft);
    if (draft.recipe) {
      this.composer = ComposerState.fromRecipe(pack, draft.recipe);
      this.stepValue = parseStep(draft.currentStep, draft);
      this.persistence = "saved";
      this.message = "组合草稿已恢复，当前进度已保存。";
    } else {
      this.composer = null;
      this.stepValue = "body";
      this.persistence = "idle";
      this.message = "草稿尚未选择身体，请从身体底型继续。";
    }
    await this.renderCurrent(visit);
  }

  async restore(sessionId: string): Promise<void> {
    const visit = this.beginVisit();
    const pack = await this.ports.loadPack();
    await this.waitForSessionMutations(sessionId);
    let snapshot = await this.ports.creation.snapshot(sessionId);
    if (snapshot.status === "finalizing") {
      snapshot = await this.recoverFinalizingSession(sessionId);
    }
    if (!this.isCurrent(visit)) return;
    if (snapshot.method !== "composer") throw new Error("creation session is not a composer draft");
    this.packValue = pack;
    this.applySnapshot(snapshot);
    if (snapshot.recipe) {
      this.composer = ComposerState.fromRecipe(pack, snapshot.recipe);
      this.stepValue = parseStep(snapshot.currentStep, snapshot);
      this.persistence = "saved";
    } else {
      this.composer = null;
      this.stepValue = "body";
      this.persistence = "idle";
    }
    await this.renderCurrent(visit);
  }

  mount(elements: ComposerCreationElements): void {
    this.unmount();
    this.elements = elements;
    this.listen(elements.previousButton, "click", () => { void this.goRelative(-1); });
    this.listen(elements.nextButton, "click", () => { void this.goRelative(1); });
    this.listen(elements.candidateButton, "click", () => {
      void this.createCandidate(elements.candidatePreview).catch((error) => {
        this.message = `动态预览未准备好：${errorMessage(error)}`;
        this.renderDom();
      });
    });
    this.listen(elements.finishButton, "click", () => {
      void this.finish(elements.nameInput.value).catch((error) => {
        this.message = `完成失败：${errorMessage(error)}`;
        this.renderDom();
      });
    });
    this.listen(elements.abandonButton, "click", () => { void this.abandon(); });
    this.renderDom();
  }

  async selectBody(bodyId: string): Promise<void> {
    const pack = this.requirePack();
    if (isCandidateLocked(this.snapshotValue)) {
      throw new Error("candidateReady composer sessions cannot be edited");
    }
    if (this.snapshotValue?.status !== undefined && this.snapshotValue.status !== "draft") {
      throw new Error("only draft composer sessions can be edited");
    }
    if (this.snapshotValue && this.composer) {
      this.composer.select("body", bodyId);
      this.stepValue = "ears";
      await this.saveSelection(this.composer.recipe(), "ears");
      return;
    }

    this.mutations.pendingInitialBody = bodyId;
    const visit = this.visit;
    if (!this.mutations.initialFlight) {
      const existingSession = this.snapshotValue;
      this.mutations.initialFlight = (async () => {
        let session = existingSession;
        if (!session) session = await this.ports.creation.start("composer");
        if (session.method !== "composer") throw new Error("creation session is not a composer draft");
        let durable = session;
        while (this.mutations.pendingInitialBody) {
          const selected = this.mutations.pendingInitialBody;
          this.mutations.pendingInitialBody = null;
          const initial = ComposerState.start(pack, selected);
          durable = await enqueueSessionMutation(
            this.mutations,
            session.sessionId,
            () => this.ports.creation.composerSave(session.sessionId, initial.recipe(), "ears"),
          );
        }
        return durable;
      })().finally(() => {
        this.mutations.initialFlight = null;
      });
    }
    const durable = await this.mutations.initialFlight;
    if (!this.isCurrent(visit)) return;
    this.applySnapshot(durable);
    if (!durable.recipe) throw new Error("saved composer body did not return a recipe");
    this.composer = ComposerState.fromRecipe(pack, durable.recipe);
    this.stepValue = parseStep(durable.currentStep, durable);
    this.persistence = "saved";
    this.message = "已保存，可以安全关闭后继续。";
    await this.renderCurrent(visit);
  }

  async select(kind: Exclude<ComposerSelectionKind, "body">, id: string): Promise<void> {
    if (this.snapshotValue?.status !== "draft") {
      throw new Error("only draft composer sessions can be edited");
    }
    const composer = this.requireComposer();
    composer.select(kind, id);
    this.stepValue = NEXT_STEP[kind];
    await this.saveSelection(composer.recipe(), this.stepValue);
  }

  async retrySave(): Promise<void> {
    const composer = this.requireComposer();
    await this.saveSelection(composer.recipe(), this.stepValue);
  }

  async createCandidate(root: HTMLElement): Promise<void> {
    const sessionId = this.requireSessionId();
    const existing = this.mutations.candidateFlights.get(sessionId);
    if (existing) return existing;
    const visit = this.visit;
    const flight = enqueueSessionMutation(this.mutations, sessionId, async (): Promise<void> => {
      if (!this.isCurrent(visit)) return;
      if (!this.canCreateCandidate()) throw new Error("组合草稿尚未保存");
      const pack = this.requirePack();
      const recipe = this.requireComposer().recipe();
      const blob = await this.ports.exportPng(pack, recipe);
      if (!this.isCurrent(visit)) return;
      const encoded = await this.ports.blobToBase64(blob);
      if (!this.isCurrent(visit)) return;
      const projection = await this.ports.creation.composerCandidate(sessionId, encoded);
      if (!this.isCurrent(visit)) return;
      this.applySnapshot(projection.snapshot);
      this.candidateProjection = projection;
      this.dynamicReady = false;
      try {
        const mounted = await this.showPreview(root, projection, visit);
        if (!mounted) return;
      } catch (error) {
        if (this.isCurrent(visit)) {
          this.message = "候选已保存，但动态预览未加载；请重试预览。";
          this.renderDom();
        }
        throw error;
      }
      if (!this.isCurrent(visit)) return;
      this.dynamicReady = true;
      this.stepValue = "preview";
      this.message = "动态预览已准备好，请命名并确认显示在桌面。";
      this.renderDom();
    });
    const tracked = flight.finally(() => {
      if (this.mutations.candidateFlights.get(sessionId) === tracked) {
        this.mutations.candidateFlights.delete(sessionId);
      }
    });
    this.mutations.candidateFlights.set(sessionId, tracked);
    return tracked;
  }

  async retryPreview(root: HTMLElement): Promise<void> {
    const projection = this.candidateProjection;
    if (!projection || !isCandidateLocked(this.snapshotValue)) {
      throw new Error("没有可重试的动态候选预览");
    }
    const visit = this.visit;
    if (!await this.showPreview(root, projection, visit)) return;
    this.dynamicReady = true;
    this.message = "动态预览已恢复。";
    this.renderDom();
  }

  async finish(displayName: string): Promise<PetSwitchResult> {
    const sessionId = this.requireSessionId();
    const existing = this.mutations.finalizeFlights.get(sessionId);
    if (existing) return existing;
    if (!this.canFinish()) throw new Error("动态候选尚未准备好");
    const visit = this.visit;
    const flight = enqueueSessionMutation(this.mutations, sessionId, async (): Promise<PetSwitchResult> => {
      const named = await this.ports.creation.setName(sessionId, displayName);
      if (this.isCurrent(visit)) this.applySnapshot(named);
      try {
        const result = await this.ports.finalize(sessionId);
        const durable = await this.ports.creation.snapshot(sessionId).catch(() => null);
        if (durable && this.isCurrent(visit)) this.applySnapshot(durable);
        if (result.ok && this.isCurrent(visit)) {
          this.dynamicReady = false;
        }
        return result;
      } catch (error) {
        const durable = await this.ports.creation.snapshot(sessionId).catch(() => null);
        if (durable && this.isCurrent(visit)) this.applySnapshot(durable);
        if (durable?.status === "completed") {
          return { ok: true as const, requestId: `recovered-${sessionId}`, petId: durable.petId };
        }
        throw error;
      }
    });
    const tracked = flight.finally(() => {
      if (this.mutations.finalizeFlights.get(sessionId) === tracked) {
        this.mutations.finalizeFlights.delete(sessionId);
      }
      this.renderDom();
    });
    this.mutations.finalizeFlights.set(sessionId, tracked);
    return tracked;
  }

  async abandon(): Promise<void> {
    const sessionId = this.snapshotValue?.sessionId;
    if (!sessionId) return;
    const existing = this.mutations.abandonFlights.get(sessionId);
    if (existing) return existing;
    if (!this.ports.confirm("放弃当前组合草稿？已保存的组合进度将被删除。")) return;
    const visit = this.visit;
    const flight = enqueueSessionMutation(this.mutations, sessionId, async () => {
      await this.ports.creation.abandon(sessionId);
      if (!this.isCurrent(visit)) return;
      this.clearPreviewOnce();
      this.resetDraftState();
      this.renderDom();
    });
    const tracked = flight.finally(() => {
      if (this.mutations.abandonFlights.get(sessionId) === tracked) {
        this.mutations.abandonFlights.delete(sessionId);
      }
    });
    this.mutations.abandonFlights.set(sessionId, tracked);
    return tracked;
  }

  destroy(): void {
    if (!this.active && this.previewCleared) return;
    this.active = false;
    this.visit += 1;
    this.unmount();
    this.clearPreviewOnce();
  }

  recipe(): ComposerRecipe | null {
    return this.composer?.recipe() ?? null;
  }

  currentStep(): ComposerStep {
    return this.stepValue;
  }

  sessionId(): string | null {
    return this.snapshotValue?.sessionId ?? null;
  }

  saveState(): SaveState {
    return this.persistence;
  }

  statusText(): string {
    return this.message;
  }

  creationSnapshot(): CreationSnapshot | null {
    return this.snapshotValue ? { ...this.snapshotValue } : null;
  }

  canCreateCandidate(): boolean {
    return this.persistence === "saved"
      && this.composer !== null
      && (this.snapshotValue?.status === "draft" || isCandidateLocked(this.snapshotValue));
  }

  canFinish(): boolean {
    return this.dynamicReady && isCandidateLocked(this.snapshotValue);
  }

  private async saveSelection(recipe: ComposerRecipe, step: ComposerStep): Promise<void> {
    const sessionId = this.requireSessionId();
    const revision = ++this.selectionRevision;
    const visit = this.visit;
    this.persistence = "saving";
    this.message = "正在保存组合进度…";
    this.renderDom();
    const save = enqueueSessionMutation(
      this.mutations,
      sessionId,
      () => this.ports.creation.composerSave(sessionId, recipe, step),
    );
    try {
      const snapshot = await save;
      if (this.isCurrent(visit)
        && revision === this.selectionRevision
        && this.snapshotValue?.sessionId === sessionId) {
        this.applySnapshot(snapshot);
        this.persistence = "saved";
        this.message = "已保存，可以安全关闭后继续。";
        await this.renderCurrent(this.visit);
      }
    } catch (error) {
      if (this.isCurrent(visit)
        && revision === this.selectionRevision
        && this.snapshotValue?.sessionId === sessionId) {
        this.persistence = "unsaved";
        this.message = `未保存：${errorMessage(error)}。请重试后再关闭。`;
        this.renderDom();
      }
      throw error;
    }
  }

  private async goRelative(delta: -1 | 1): Promise<void> {
    const current = STEP_ORDER.indexOf(this.stepValue);
    const next = Math.max(0, Math.min(STEP_ORDER.length - 1, current + delta));
    this.stepValue = STEP_ORDER[next]!;
    if (this.composer && this.snapshotValue?.status === "draft") {
      await this.saveSelection(this.composer.recipe(), this.stepValue);
    } else {
      this.renderDom();
    }
  }

  private applySnapshot(snapshot: CreationSnapshot): void {
    if (snapshot.method !== "composer") throw new Error("snapshot does not belong to composer");
    this.snapshotValue = { ...snapshot, recipe: snapshot.recipe ? { ...snapshot.recipe } : null };
  }

  private resetDraftState(): void {
    this.snapshotValue = null;
    this.composer = null;
    this.stepValue = "body";
    this.persistence = "idle";
    this.candidateProjection = null;
    this.dynamicReady = false;
    this.message = "请选择身体底型开始组合。";
  }

  private requirePack(): ComposerPackManifest {
    if (!this.packValue) throw new Error("composer pack is not loaded");
    return this.packValue;
  }

  private requireComposer(): ComposerState {
    if (!this.composer) throw new Error("select a body before other composer parts");
    return this.composer;
  }

  private requireSessionId(): string {
    const sessionId = this.snapshotValue?.sessionId;
    if (!sessionId) throw new Error("composer session is not started");
    return sessionId;
  }

  private isCurrent(visit: number): boolean {
    return this.active && visit === this.visit;
  }

  private clearPreviewOnce(): void {
    if (this.previewCleared) return;
    this.previewCleared = true;
    this.dynamicReady = false;
    void enqueuePreviewMutation(this.ports.preview, () => {
      this.ports.preview.clear();
    });
  }

  private beginVisit(): number {
    if (this.active) this.clearPreviewOnce();
    const visit = ++this.visit;
    this.active = true;
    this.previewCleared = false;
    return visit;
  }

  private async readDraftAfterSharedMutations(visit: number): Promise<CreationSnapshot | null> {
    while (true) {
      const initial = this.mutations.initialFlight;
      if (initial) await initial.catch(() => undefined);
      const finalizations = [...this.mutations.finalizeFlights.values()];
      if (finalizations.length > 0) {
        await Promise.all(finalizations.map((flight) => flight.catch(() => undefined)));
        if (!this.isCurrent(visit)) return null;
      }
      const draft = await this.ports.creation.draft();
      if (!this.isCurrent(visit)) return draft;
      if (this.mutations.initialFlight && this.mutations.initialFlight !== initial) continue;
      if (!draft) return null;
      await this.waitForSessionMutations(draft.sessionId);
      let restored = await this.ports.creation.snapshot(draft.sessionId);
      if (restored.status === "finalizing") {
        restored = await this.recoverFinalizingSession(restored.sessionId);
      }
      if (isTerminal(restored)) return null;
      return restored;
    }
  }

  private async waitForSessionMutations(sessionId: string): Promise<void> {
    while (true) {
      const tail = this.mutations.sessionTails.get(sessionId);
      if (!tail) return;
      await tail;
      if (this.mutations.sessionTails.get(sessionId) === tail) return;
    }
  }

  private showPreview(
    root: HTMLElement,
    projection: ComposerCandidateProjection,
    visit: number,
  ): Promise<boolean> {
    const profile = parseMotionProfile(projection.motionProfile);
    return enqueuePreviewMutation(this.ports.preview, async () => {
      if (!this.isCurrent(visit)) return false;
      await this.ports.preview.show(root, projection.bodyUrl, profile);
      return this.isCurrent(visit);
    });
  }

  private async recoverFinalizingSession(sessionId: string): Promise<CreationSnapshot> {
    if (!this.mutations.recoveryFlight) {
      this.mutations.recoveryFlight = enqueueSessionMutation(
        this.mutations,
        sessionId,
        async () => { await this.ports.creation.recoverFinalization(); },
      ).finally(() => {
        this.mutations.recoveryFlight = null;
      });
    }
    await this.mutations.recoveryFlight;
    const restored = await this.ports.creation.snapshot(sessionId);
    if (restored.status === "finalizing") {
      throw new Error("完成恢复后仍处于 finalizing，请稍后重试");
    }
    return restored;
  }

  private async renderCurrent(visit: number): Promise<void> {
    const recipe = this.composer?.recipe();
    if (recipe) {
      await this.ports.render(this.requirePack(), recipe, this.elements?.canvas);
      if (!this.isCurrent(visit)) return;
    }
    this.renderDom();
  }

  private renderDom(): void {
    const dom = this.elements;
    if (!dom) return;
    dom.saveStatus.textContent = saveStateText(this.persistence);
    dom.saveStatus.dataset.state = this.persistence;
    dom.message.textContent = this.message;
    dom.previousButton.disabled = STEP_ORDER.indexOf(this.stepValue) <= 0;
    dom.nextButton.disabled = this.persistence === "unsaved"
      || STEP_ORDER.indexOf(this.stepValue) >= STEP_ORDER.length - 1;
    dom.candidateButton.disabled = !this.canCreateCandidate();
    dom.finishButton.disabled = !this.canFinish();
    this.renderSteps(dom.steps);
    this.renderOptions(dom.options);
  }

  private renderSteps(root: HTMLElement): void {
    root.replaceChildren(...STEP_ORDER.map((step, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "composer-step";
      button.textContent = `${String(index + 1).padStart(2, "0")} ${stepLabel(step)}`;
      if (step === this.stepValue) button.setAttribute("aria-current", "step");
      button.disabled = !this.composer || this.snapshotValue?.status !== "draft";
      button.addEventListener("click", () => {
        this.stepValue = step;
        void this.saveSelection(this.requireComposer().recipe(), step);
      });
      return button;
    }));
  }

  private renderOptions(root: HTMLElement): void {
    const pack = this.packValue;
    if (!pack || this.stepValue === "name" || this.stepValue === "preview") {
      root.replaceChildren();
      return;
    }
    const bodyId = this.composer?.recipe().bodyId;
    const recipe = this.composer?.recipe();
    const options: Array<{
      kind: ComposerSelectionKind;
      id: string;
      label: string;
      image?: string;
      selected: boolean;
      disabled: boolean;
      reason?: string;
    }> = [];
    if (this.stepValue === "body") {
      for (const body of pack.bodies) options.push({
        kind: "body", id: body.id, label: composerOptionLabel(body.id), image: body.image,
        selected: recipe?.bodyId === body.id, disabled: false,
      });
    } else if (this.stepValue === "coat") {
      for (const color of pack.colors) options.push({
        kind: "color", id: color.id, label: composerOptionLabel(color.id),
        selected: recipe?.colorId === color.id, disabled: false,
      });
      for (const pattern of pack.patterns) options.push({
        kind: "pattern", id: pattern.id, label: composerOptionLabel(pattern.id), image: pattern.image ?? undefined,
        selected: recipe?.patternId === pattern.id, disabled: false,
      });
    } else {
      const collection = this.stepValue === "ears" ? pack.ears
        : this.stepValue === "eyes" ? pack.eyes
          : this.stepValue === "muzzle" ? pack.muzzles : pack.tails;
      for (const item of collection) {
        const compatible = bodyId !== undefined && item.compatibleBodyIds.includes(bodyId);
        const field = `${this.stepValue}Id` as "earsId" | "eyesId" | "muzzleId" | "tailId";
        options.push({
          kind: this.stepValue,
          id: item.id,
          label: composerOptionLabel(item.id),
          image: "openImage" in item ? item.openImage : item.image,
          selected: recipe?.[field] === item.id,
          disabled: !compatible,
          reason: compatible ? undefined : "与当前身体底型不兼容",
        });
      }
    }
    root.replaceChildren(...options.map((option) => this.optionButton(option)));
  }

  private optionButton(option: {
    kind: ComposerSelectionKind;
    id: string;
    label: string;
    image?: string;
    selected: boolean;
    disabled: boolean;
    reason?: string;
  }): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "composer-option";
    button.setAttribute("aria-pressed", String(option.selected));
    button.disabled = option.disabled;
    if (option.reason) {
      button.title = option.reason;
      button.setAttribute("aria-description", option.reason);
    }
    if (option.image) {
      const thumbnail = document.createElement("span");
      thumbnail.className = "composer-option-thumbnail";
      thumbnail.dataset.composerKind = option.kind;
      thumbnail.setAttribute("aria-hidden", "true");
      const image = document.createElement("img");
      image.src = `/creation-content/composer/cat-cute-v1/${option.image}`;
      image.alt = "";
      image.loading = "lazy";
      thumbnail.append(image);
      button.append(thumbnail);
    }
    const label = document.createElement("span");
    label.className = "composer-option-label";
    label.textContent = option.label;
    button.append(label);
    button.addEventListener("click", () => {
      const action = option.kind === "body"
        ? this.selectBody(option.id)
        : this.select(option.kind, option.id);
      void action.catch((error) => {
        this.message = errorMessage(error);
        this.renderDom();
      });
    });
    return button;
  }

  private listen(target: EventTarget, type: string, listener: EventListener): void {
    target.addEventListener(type, listener);
    this.cleanupListeners.push(() => target.removeEventListener(type, listener));
  }

  private unmount(): void {
    for (const cleanup of this.cleanupListeners.splice(0)) cleanup();
    this.elements = null;
  }
}

function mutationCoordinatorFor(creation: ComposerCreationApiPort): ComposerMutationCoordinator {
  const existing = MUTATION_COORDINATORS.get(creation);
  if (existing) return existing;
  const created: ComposerMutationCoordinator = {
    initialFlight: null,
    pendingInitialBody: null,
    sessionTails: new Map(),
    candidateFlights: new Map(),
    finalizeFlights: new Map(),
    abandonFlights: new Map(),
    recoveryFlight: null,
  };
  MUTATION_COORDINATORS.set(creation, created);
  return created;
}

function enqueueSessionMutation<T>(
  coordinator: ComposerMutationCoordinator,
  sessionId: string,
  operation: () => Promise<T>,
): Promise<T> {
  const prior = coordinator.sessionTails.get(sessionId) ?? Promise.resolve();
  const result = prior.then(operation, operation);
  const tail = result.then(() => undefined, () => undefined);
  coordinator.sessionTails.set(sessionId, tail);
  void tail.finally(() => {
    if (coordinator.sessionTails.get(sessionId) === tail) {
      coordinator.sessionTails.delete(sessionId);
    }
  });
  return result;
}

function enqueuePreviewMutation<T>(
  preview: object,
  operation: () => T | Promise<T>,
): Promise<T> {
  let coordinator = PREVIEW_COORDINATORS.get(preview);
  if (!coordinator) {
    coordinator = { tail: Promise.resolve() };
    PREVIEW_COORDINATORS.set(preview, coordinator);
  }
  const result = coordinator.tail.then(operation, operation);
  coordinator.tail = result.then(() => undefined, () => undefined);
  return result;
}

function isCandidateLocked(snapshot: CreationSnapshot | null): boolean {
  return snapshot?.status === "candidateReady"
    || (snapshot?.status === "retryableFailure" && snapshot.lastStableStatus === "candidateReady");
}

function isTerminal(snapshot: CreationSnapshot): boolean {
  return snapshot.status === "completed" || snapshot.status === "abandoned";
}

function parseStep(value: string, snapshot?: CreationSnapshot): ComposerStep {
  if (snapshot && isCandidateLocked(snapshot)) return "preview";
  if (value === "review") return "preview";
  return STEP_ORDER.includes(value as ComposerStep) ? value as ComposerStep : "body";
}

function saveStateText(value: SaveState): string {
  if (value === "saving") return "保存中";
  if (value === "saved") return "已保存";
  if (value === "unsaved") return "未保存";
  return "尚未开始";
}

function stepLabel(step: ComposerStep): string {
  return {
    body: "身体", ears: "耳朵", eyes: "眼睛", muzzle: "鼻嘴",
    tail: "尾巴", coat: "毛色花纹", name: "命名", preview: "动态预览",
  }[step];
}

const COMPOSER_OPTION_LABELS: Readonly<Record<string, string>> = {
  "body-round": "圆润体型",
  "body-slim": "修长体型",
  "body-fluffy": "蓬松体型",
  "ears-round": "圆耳",
  "ears-pointed": "尖耳",
  "ears-folded": "折耳",
  "ears-tufted": "簇毛耳",
  "eyes-amber": "琥珀眼",
  "eyes-blue": "蓝眼",
  "eyes-green": "绿眼",
  "eyes-gold": "金色眼",
  "eyes-violet": "紫罗兰眼",
  "muzzle-gentle": "温柔嘴型",
  "muzzle-smile": "微笑嘴型",
  "muzzle-curious": "好奇嘴型",
  "muzzle-sleepy": "困倦嘴型",
  "tail-curl": "卷尾",
  "tail-straight": "直尾",
  "tail-plume": "蓬松尾",
  "tail-short": "短尾",
  "color-cream": "奶油色",
  "color-orange": "橘色",
  "color-gray": "灰色",
  "color-black": "黑色",
  "color-white": "白色",
  "color-brown": "棕色",
  "pattern-none": "纯色",
  "pattern-tabby": "虎斑",
  "pattern-tuxedo": "燕尾服花纹",
  "pattern-calico": "三花",
  "pattern-spots": "斑点",
};

export function composerOptionLabel(id: string): string {
  return COMPOSER_OPTION_LABELS[id] ?? "未知选项";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function assertComposerRecipe(pack: ComposerPackManifest, recipe: ComposerRecipe): void {
  const errors = validateRecipe(pack, recipe);
  if (errors.length > 0) throw new Error(`invalid composer recipe: ${errors.join("; ")}`);
}
