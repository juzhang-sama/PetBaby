import {
  canonicalPetCalibration,
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "../runtime/pet-calibration";

export type PetCalibrationRuntimeAction = "preview" | "restore" | "feedback" | "commit";

export interface PetCalibrationControlPorts {
  load(petId: string): Promise<unknown>;
  save(petId: string, value: PetCalibrationV1): Promise<unknown>;
  runtime(
    petId: string,
    action: PetCalibrationRuntimeAction,
    value: PetCalibrationV1,
  ): Promise<void>;
}

export interface PetCalibrationControlElements {
  root: HTMLElement;
  petName: HTMLElement;
  breath: HTMLInputElement;
  breathOutput: HTMLOutputElement;
  feedback: HTMLInputElement;
  feedbackOutput: HTMLOutputElement;
  reset: HTMLButtonElement;
  feedbackTest: HTMLButtonElement;
  cancel: HTMLButtonElement;
  save: HTMLButtonElement;
  status: HTMLElement;
  error: HTMLElement;
}

interface CalibrationClock {
  setTimeout(callback: () => void, delayMs: number): number;
  clearTimeout(id: number): void;
}

export interface PetCalibrationControlOptions {
  elements: PetCalibrationControlElements;
  ports: PetCalibrationControlPorts;
  clock?: CalibrationClock;
}

const browserClock: CalibrationClock = {
  setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
  clearTimeout: (id) => window.clearTimeout(id),
};

const PREVIEW_DEBOUNCE_MS = 100;

export interface CalibrationCatalogEntry {
  petId: string;
  displayName: string;
  isCurrent: boolean;
}

interface CalibrationTarget {
  open(petId: string, petName: string): Promise<boolean>;
  closeCurrent(message: string): void;
  updatePetName(petName: string): void;
}

export class PetCalibrationCatalogCoordinator {
  private readonly target: CalibrationTarget;
  private targetPetId: string | null = null;
  private revision = 0;

  constructor(target: CalibrationTarget) {
    this.target = target;
  }

  async reconcile(entries: readonly CalibrationCatalogEntry[]): Promise<void> {
    const revision = ++this.revision;
    const current = entries.find((entry) => entry.isCurrent);
    if (!current) {
      this.targetPetId = null;
      this.target.closeCurrent("没有可校准的当前宠物");
      return;
    }
    this.target.updatePetName(current.displayName);
    if (current.petId === this.targetPetId) return;
    const opened = await this.target.open(current.petId, current.displayName);
    if (revision !== this.revision) return;
    this.targetPetId = opened ? current.petId : null;
  }

  unavailable(message: string): void {
    this.revision += 1;
    this.targetPetId = null;
    this.target.closeCurrent(message);
  }
}

export class PetCalibrationControl {
  private readonly elements: PetCalibrationControlElements;
  private readonly ports: PetCalibrationControlPorts;
  private readonly clock: CalibrationClock;
  private petId: string | null = null;
  private saved: PetCalibrationV1 = { ...DEFAULT_PET_CALIBRATION };
  private draft: PetCalibrationV1 = { ...DEFAULT_PET_CALIBRATION };
  private revision = 0;
  private previewRevision = 0;
  private previewTimer: number | undefined;
  private runtimeTail: Promise<void> = Promise.resolve();
  private runtimeQueueRevision = 0;
  private mounted = false;
  private destroyed = false;
  private busy = false;
  private available = false;
  private hasSavedSnapshot = false;
  private runtimePreviewActive = false;
  private saveTransaction: Promise<void> | null = null;
  private pendingCanonicalCommit: PetCalibrationV1 | null = null;

  private readonly onInput = (): void => {
    if (this.destroyed || this.busy || !this.available || !this.petId) return;
    try {
      this.draft = this.readDraft();
      this.previewRevision += 1;
      this.renderValue(this.draft);
      this.elements.status.textContent = "尚未保存 · 正在准备预览";
      this.elements.error.textContent = "";
      this.schedulePreview(this.revision, this.previewRevision);
    } catch (error) {
      this.renderValue(this.draft);
      this.elements.error.textContent = `参数无效：${errorMessage(error)}`;
    }
  };

  private readonly onReset = (): void => { void this.reset(); };
  private readonly onFeedback = (): void => { void this.previewFeedback(); };
  private readonly onCancel = (): void => { void this.cancel(); };
  private readonly onSave = (): void => { void this.save(); };

  constructor(options: PetCalibrationControlOptions) {
    this.elements = options.elements;
    this.ports = options.ports;
    this.clock = options.clock ?? browserClock;
  }

  mount(): void {
    if (this.mounted || this.destroyed) return;
    this.mounted = true;
    for (const slider of this.sliders()) slider.addEventListener("input", this.onInput);
    this.elements.reset.addEventListener("click", this.onReset);
    this.elements.feedbackTest.addEventListener("click", this.onFeedback);
    this.elements.cancel.addEventListener("click", this.onCancel);
    this.elements.save.addEventListener("click", this.onSave);
    this.setDisabled(true);
  }

  async open(petId: string, petName = petId): Promise<boolean> {
    if (this.destroyed) return false;
    const previousPetId = this.petId;
    const previousSaved = this.saved;
    const previousHasSavedSnapshot = this.hasSavedSnapshot;
    const currentRevision = ++this.revision;
    this.previewRevision += 1;
    this.clearPreviewTimer();
    this.cancelQueuedRuntime();
    this.pendingCanonicalCommit = null;
    this.petId = petId;
    this.available = false;
    this.hasSavedSnapshot = false;
    this.runtimePreviewActive = false;
    this.elements.root.hidden = false;
    this.elements.petName.textContent = petName;
    this.elements.status.textContent = "正在读取当前宠物的校准参数…";
    this.elements.error.textContent = "";
    this.setBusy(true);

    if (previousPetId && previousPetId !== petId && previousHasSavedSnapshot) {
      void this.sendRuntimeNow(previousPetId, "restore", previousSaved).catch(() => {
        // The runtime can already have switched pets. The old request is still issued as required.
      });
    }

    try {
      const loaded = canonicalPetCalibration(await this.ports.load(petId));
      if (!this.isCurrent(currentRevision, petId)) return false;
      this.saved = loaded;
      this.draft = { ...loaded };
      this.hasSavedSnapshot = true;
      this.available = true;
      this.renderValue(this.draft);
      this.elements.status.textContent = "拖动滑杆可实时预览，保存后才会长期生效。";
      this.elements.error.textContent = "";
      return true;
    } catch (error) {
      if (!this.isCurrent(currentRevision, petId)) return false;
      this.available = false;
      this.hasSavedSnapshot = false;
      this.renderUnavailable();
      this.elements.status.textContent = "";
      this.elements.error.textContent = `无法读取当前宠物的校准参数：${errorMessage(error)}`;
      return false;
    } finally {
      if (this.isCurrent(currentRevision, petId)) this.setBusy(false);
    }
  }

  updatePetName(petName: string): void {
    if (!this.destroyed) this.elements.petName.textContent = petName;
  }

  hasActiveCalibration(): boolean {
    return !this.destroyed && this.petId !== null && this.hasSavedSnapshot;
  }

  needsRestoreBeforeClose(): boolean {
    return this.hasActiveCalibration()
      && (this.runtimePreviewActive || this.pendingCanonicalCommit !== null);
  }

  freezeForClose(): void {
    if (this.destroyed) return;
    this.previewRevision += 1;
    this.clearPreviewTimer();
    this.cancelQueuedRuntime();
    this.available = false;
    this.busy = true;
    this.elements.root.setAttribute("aria-busy", "true");
    this.setDisabled(true);
    this.elements.status.textContent = "正在恢复已保存参数并关闭设置…";
    this.elements.error.textContent = "";
  }

  unfreezeAfterCloseFailure(): void {
    if (this.destroyed) return;
    this.available = this.petId !== null && this.hasSavedSnapshot;
    this.busy = this.saveTransaction !== null;
    this.elements.root.setAttribute("aria-busy", String(this.busy));
    this.setDisabled(this.busy || !this.available);
    this.elements.status.textContent = this.busy
      ? "保存仍在进行；完成后可再次关闭。"
      : "关闭未完成，可以继续调整或重试关闭。";
  }

  async settleForClose(): Promise<void> {
    await (this.saveTransaction ?? Promise.resolve());
  }

  async restoreBeforeClose(): Promise<void> {
    const petId = this.petId;
    if (!petId || !this.hasSavedSnapshot) return;
    if (this.pendingCanonicalCommit) {
      const canonical = this.pendingCanonicalCommit;
      await this.sendRuntimeNow(petId, "commit", canonical);
      this.finishSaved(canonical);
      return;
    }
    await this.sendRuntimeNow(petId, "restore", this.saved);
    this.runtimePreviewActive = false;
  }

  finalizeClose(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.revision += 1;
    this.previewRevision += 1;
    this.clearPreviewTimer();
    this.cancelQueuedRuntime();
    for (const slider of this.sliders()) slider.removeEventListener("input", this.onInput);
    this.elements.reset.removeEventListener("click", this.onReset);
    this.elements.feedbackTest.removeEventListener("click", this.onFeedback);
    this.elements.cancel.removeEventListener("click", this.onCancel);
    this.elements.save.removeEventListener("click", this.onSave);
    this.petId = null;
    this.available = false;
    this.hasSavedSnapshot = false;
    this.runtimePreviewActive = false;
    this.pendingCanonicalCommit = null;
  }

  closeCurrent(message: string): void {
    if (this.destroyed) return;
    const petId = this.petId;
    const saved = this.saved;
    const shouldRestore = this.hasSavedSnapshot;
    this.revision += 1;
    this.previewRevision += 1;
    this.clearPreviewTimer();
    this.cancelQueuedRuntime();
    this.petId = null;
    this.available = false;
    this.hasSavedSnapshot = false;
    this.runtimePreviewActive = false;
    this.pendingCanonicalCommit = null;
    this.busy = false;
    this.elements.root.setAttribute("aria-busy", "false");
    this.elements.petName.textContent = "当前宠物";
    this.renderUnavailable();
    this.setDisabled(true);
    this.elements.status.textContent = "";
    this.elements.error.textContent = message;
    if (petId && shouldRestore) {
      void this.sendRuntimeNow(petId, "restore", saved).catch(() => undefined);
    }
  }

  async reset(): Promise<void> {
    const context = this.editableContext();
    if (!context || this.busy) return;
    this.clearPreviewTimer();
    const actionRevision = ++this.previewRevision;
    this.cancelQueuedRuntime();
    this.draft = {
      ...DEFAULT_PET_CALIBRATION,
      blinkIntervalScale: this.draft.blinkIntervalScale,
    };
    this.renderValue(this.draft);
    this.elements.status.textContent = "已预览默认值；尚未保存。";
    this.elements.error.textContent = "";
    try {
      this.runtimePreviewActive = true;
      await this.enqueueRuntime(context.petId, "preview", this.draft);
    } catch (error) {
      await this.handleRuntimeFailure(context, actionRevision, error, "默认值预览失败");
    }
  }

  async previewFeedback(): Promise<void> {
    const context = this.editableContext();
    if (!context || this.busy) return;
    this.clearPreviewTimer();
    const actionRevision = ++this.previewRevision;
    this.cancelQueuedRuntime();
    try {
      this.draft = this.readDraft();
      this.renderValue(this.draft);
      this.runtimePreviewActive = true;
      await this.enqueueRuntime(context.petId, "feedback", this.draft);
      if (this.isCurrent(context.revision, context.petId)
        && actionRevision === this.previewRevision) {
        this.elements.status.textContent = "已发送一次点击反馈预览；尚未保存。";
        this.elements.error.textContent = "";
      }
    } catch (error) {
      await this.handleRuntimeFailure(context, actionRevision, error, "点击反馈预览失败");
    }
  }

  async cancel(): Promise<void> {
    const context = this.editableContext();
    if (!context) return;
    this.clearPreviewTimer();
    const actionRevision = ++this.previewRevision;
    this.cancelQueuedRuntime();
    this.draft = { ...this.saved };
    this.renderValue(this.draft);
    try {
      await this.sendRuntimeNow(context.petId, "restore", this.saved);
      if (this.isCurrent(context.revision, context.petId)
        && actionRevision === this.previewRevision) {
        this.runtimePreviewActive = false;
        this.elements.status.textContent = "已取消更改并恢复已保存值。";
        this.elements.error.textContent = "";
      }
    } catch (error) {
      if (this.isCurrent(context.revision, context.petId)
        && actionRevision === this.previewRevision) {
        this.elements.status.textContent = "已在设置中恢复已保存值。";
        this.elements.error.textContent = `桌面宠物恢复失败：${errorMessage(error)}`;
      }
    }
  }

  save(): Promise<void> {
    const context = this.editableContext();
    if (!context || this.busy || this.saveTransaction) return Promise.resolve();
    const transaction = this.performSave(context);
    this.saveTransaction = transaction;
    return transaction.finally(() => {
      if (this.saveTransaction === transaction) this.saveTransaction = null;
    });
  }

  private async performSave(context: { revision: number; petId: string }): Promise<void> {
    this.clearPreviewTimer();
    this.previewRevision += 1;
    this.cancelQueuedRuntime();
    try {
      this.draft = this.readDraft();
    } catch (error) {
      this.elements.error.textContent = `参数无效：${errorMessage(error)}`;
      return;
    }
    const valueToSave = { ...this.draft };
    const oldSaved = { ...this.saved };
    this.setBusy(true);
    this.elements.status.textContent = "正在保存…";
    this.elements.error.textContent = "";

    let canonical: PetCalibrationV1;
    try {
      canonical = canonicalPetCalibration(await this.ports.save(context.petId, valueToSave));
    } catch (error) {
      if (!this.isCurrent(context.revision, context.petId)) return;
      this.saved = oldSaved;
      this.draft = { ...oldSaved };
      this.renderValue(this.draft);
      try {
        await this.sendRuntimeNow(context.petId, "restore", oldSaved);
        this.runtimePreviewActive = false;
      } catch { /* Report save truth first. */ }
      if (!this.isCurrent(context.revision, context.petId)) return;
      this.elements.status.textContent = "已恢复保存前的参数。";
      this.elements.error.textContent = `保存失败：${errorMessage(error)}`;
      this.setBusy(false);
      return;
    }

    if (!this.isCurrent(context.revision, context.petId)) return;
    this.pendingCanonicalCommit = canonical;
    try {
      await this.sendRuntimeNow(context.petId, "commit", canonical);
      if (!this.isCurrent(context.revision, context.petId)) return;
      this.finishSaved(canonical);
      return;
    } catch (firstCommitError) {
      if (!this.isCurrent(context.revision, context.petId)) return;
      await this.reconcileSavedCommit(context, canonical, firstCommitError);
    } finally {
      if (this.isCurrent(context.revision, context.petId)) this.setBusy(false);
    }
  }

  destroy(): void {
    if (this.destroyed) return;
    const petId = this.petId;
    const saved = this.saved;
    const shouldRestore = this.hasSavedSnapshot;
    this.finalizeClose();
    if (petId && shouldRestore) {
      void this.sendRuntimeNow(petId, "restore", saved).catch(() => undefined);
    }
  }

  private async reconcileSavedCommit(
    context: { revision: number; petId: string },
    saveResult: PetCalibrationV1,
    firstCommitError: unknown,
  ): Promise<void> {
    let persisted = saveResult;
    let reloadError: unknown;
    try {
      persisted = canonicalPetCalibration(await this.ports.load(context.petId));
    } catch (error) {
      reloadError = error;
    }
    if (!this.isCurrent(context.revision, context.petId)) return;
    this.saved = persisted;
    this.draft = { ...persisted };
    this.pendingCanonicalCommit = persisted;
    this.renderValue(this.draft);
    try {
      await this.sendRuntimeNow(context.petId, "commit", persisted);
      if (!this.isCurrent(context.revision, context.petId)) return;
      this.finishSaved(persisted);
      if (reloadError) {
        this.elements.error.textContent = `已使用保存返回值完成同步；重新确认存储值失败：${errorMessage(reloadError)}`;
      }
    } catch (retryError) {
      if (!this.isCurrent(context.revision, context.petId)) return;
      this.elements.status.textContent = "参数已保存到本机。";
      this.elements.error.textContent = `桌面预览尚未同步：${errorMessage(retryError)}。重新打开设置或显示宠物后再试。`;
      if (reloadError) {
        this.elements.error.textContent += ` 存储复核也失败：${errorMessage(reloadError)}。`;
      } else if (firstCommitError !== retryError) {
        // Both failures are intentionally collapsed into one actionable message.
      }
    }
  }

  private finishSaved(value: PetCalibrationV1): void {
    this.saved = value;
    this.draft = { ...value };
    this.renderValue(this.draft);
    this.runtimePreviewActive = false;
    this.pendingCanonicalCommit = null;
    this.elements.status.textContent = "已保存，并同步到桌面宠物。";
    this.elements.error.textContent = "";
  }

  private schedulePreview(revision: number, previewRevision: number): void {
    this.clearPreviewTimer();
    this.previewTimer = this.clock.setTimeout(() => {
      this.previewTimer = undefined;
      if (!this.isCurrent(revision) || previewRevision !== this.previewRevision) return;
      const petId = this.petId;
      if (!petId) return;
      const value = { ...this.draft };
      this.runtimePreviewActive = true;
      void this.enqueueRuntime(petId, "preview", value).then(() => {
        if (!this.isCurrent(revision, petId) || previewRevision !== this.previewRevision) return;
        this.elements.status.textContent = "实时预览中 · 尚未保存";
        this.elements.error.textContent = "";
      }, (error: unknown) => {
        void this.handleRuntimeFailure({ revision, petId }, previewRevision, error, "实时预览失败");
      });
    }, PREVIEW_DEBOUNCE_MS);
  }

  private async handleRuntimeFailure(
    context: { revision: number; petId: string },
    previewRevision: number,
    error: unknown,
    label: string,
  ): Promise<void> {
    if (!this.isCurrent(context.revision, context.petId) || previewRevision !== this.previewRevision) return;
    this.draft = { ...this.saved };
    this.renderValue(this.draft);
    try { await this.sendRuntimeNow(context.petId, "restore", this.saved); } catch { /* Original error is primary. */ }
    if (!this.isCurrent(context.revision, context.petId) || previewRevision !== this.previewRevision) return;
    this.runtimePreviewActive = false;
    this.elements.status.textContent = "已恢复已保存值。";
    this.elements.error.textContent = `${label}：${errorMessage(error)}`;
  }

  private enqueueRuntime(
    petId: string,
    action: PetCalibrationRuntimeAction,
    value: PetCalibrationV1,
  ): Promise<void> {
    const canonical = canonicalPetCalibration(value);
    const queueRevision = this.runtimeQueueRevision;
    const request = this.runtimeTail.then(() => {
      if (queueRevision !== this.runtimeQueueRevision) return;
      return this.ports.runtime(petId, action, canonical);
    });
    this.runtimeTail = request.catch(() => undefined);
    return request;
  }

  private cancelQueuedRuntime(): void {
    this.runtimeQueueRevision += 1;
    this.runtimeTail = Promise.resolve();
  }

  private sendRuntimeNow(
    petId: string,
    action: PetCalibrationRuntimeAction,
    value: PetCalibrationV1,
  ): Promise<void> {
    return this.ports.runtime(petId, action, canonicalPetCalibration(value));
  }

  private editableContext(): { revision: number; petId: string } | null {
    return this.petId && this.available && this.hasSavedSnapshot
      ? { revision: this.revision, petId: this.petId }
      : null;
  }

  private isCurrent(revision: number, petId?: string): boolean {
    return !this.destroyed
      && revision === this.revision
      && (petId === undefined || petId === this.petId);
  }

  private readDraft(): PetCalibrationV1 {
    return canonicalPetCalibration({
      schemaVersion: 1,
      breathAmplitudePercent: Number(this.elements.breath.value),
      blinkIntervalScale: this.draft.blinkIntervalScale,
      feedbackStrength: Number(this.elements.feedback.value),
    });
  }

  private renderValue(value: PetCalibrationV1): void {
    this.elements.breath.value = String(value.breathAmplitudePercent);
    this.elements.feedback.value = String(value.feedbackStrength);
    this.elements.breathOutput.textContent = `${formatNumber(value.breathAmplitudePercent)}%`;
    this.elements.feedbackOutput.textContent = `${Math.round(value.feedbackStrength * 100)}%`;
    this.elements.breath.setAttribute(
      "aria-valuetext",
      `呼吸幅度 ${formatNumber(value.breathAmplitudePercent)}%`,
    );
    this.elements.feedback.setAttribute(
      "aria-valuetext",
      `点击反馈强度 ${Math.round(value.feedbackStrength * 100)}%`,
    );
  }

  private renderUnavailable(): void {
    for (const output of [
      this.elements.breathOutput,
      this.elements.feedbackOutput,
    ]) output.textContent = "不可用";
    for (const slider of this.sliders()) slider.setAttribute("aria-valuetext", "不可用");
  }

  private setBusy(busy: boolean): void {
    this.busy = busy;
    this.elements.root.setAttribute("aria-busy", String(busy));
    this.setDisabled(busy || !this.available);
  }

  private setDisabled(disabled: boolean): void {
    for (const element of [
      ...this.sliders(),
      this.elements.reset,
      this.elements.feedbackTest,
      this.elements.cancel,
      this.elements.save,
    ]) element.disabled = disabled;
  }

  private sliders(): HTMLInputElement[] {
    return [this.elements.breath, this.elements.feedback];
  }

  private clearPreviewTimer(): void {
    if (this.previewTimer === undefined) return;
    this.clock.clearTimeout(this.previewTimer);
    this.previewTimer = undefined;
  }
}

function formatNumber(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(2)));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
