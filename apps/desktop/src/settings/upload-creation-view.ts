import type { UploadJobRecord } from "../creation/api";
import type { CreationMethod, CreationSnapshot } from "../creation/contracts";
import { buildPrompt, sha256Hex } from "../creation/creation-flow";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { parseMotionProfile, type MotionProfileV1 } from "../runtime/animated-image-manifest";
import type { CandidatePreviewController } from "./candidate-dynamic-preview";
import { CreationPageRun } from "./creation-page-run";

export interface UploadCreationPorts {
  creation: {
    start(method: "upload" | "composer"): Promise<CreationSnapshot>;
    draft(): Promise<CreationSnapshot | null>;
    snapshot(sessionId: string): Promise<CreationSnapshot>;
    setName(sessionId: string, displayName: string): Promise<CreationSnapshot>;
    abandon(sessionId: string): Promise<void>;
    uploadStart(
      sessionId: string,
      prompt: string,
      refPngB64: string,
      refSha256: string,
    ): Promise<string>;
    uploadJobs(sessionId: string): Promise<UploadJobRecord[]>;
    uploadSource(sessionId: string): Promise<{ dataUrl: string; refSha256: string } | null>;
    recoverFinalization(): Promise<unknown>;
  };
  finalize(sessionId: string): Promise<PetSwitchResult>;
}

export type UploadCreationStep = "upload" | "generating" | "review" | "finalizing" | "complete";

export interface UploadCreationViewSnapshot {
  sessionId: string | null;
  method: CreationMethod | null;
  step: UploadCreationStep;
  creation: CreationSnapshot | null;
}

export interface UploadCreationElements {
  apiKeyInput: HTMLInputElement;
  saveKeyButton: HTMLButtonElement;
  keyStatus: HTMLElement;
  photoInput: HTMLInputElement;
  photoPreview: HTMLImageElement;
  status: HTMLElement;
  stepUpload: HTMLElement;
  stepGenerating: HTMLElement;
  stepReview: HTMLElement;
  stepComplete: HTMLElement;
  jobGrid: HTMLElement;
  candidateGrid: HTMLElement;
  nameInput: HTMLInputElement;
  nameError: HTMLElement;
  nextButton: HTMLButtonElement;
  cancelButton: HTMLButtonElement;
  retryButton: HTMLButtonElement;
  abandonButton: HTMLButtonElement;
  finishButton: HTMLButtonElement;
}

export function queryUploadCreationElements(root: Document): UploadCreationElements {
  const get = <T extends HTMLElement>(id: string): T => {
    const element = root.getElementById(id);
    if (!element) throw new Error(`missing element #${id}`);
    return element as T;
  };
  return {
    apiKeyInput: get<HTMLInputElement>("api-key"),
    saveKeyButton: get<HTMLButtonElement>("save-key"),
    keyStatus: get<HTMLElement>("key-status"),
    photoInput: get<HTMLInputElement>("photo-input"),
    photoPreview: get<HTMLImageElement>("photo-preview"),
    status: get<HTMLElement>("wizard-status"),
    stepUpload: get<HTMLElement>("step-upload"),
    stepGenerating: get<HTMLElement>("step-generating"),
    stepReview: get<HTMLElement>("step-review"),
    stepComplete: get<HTMLElement>("step-complete"),
    jobGrid: get<HTMLElement>("job-grid"),
    candidateGrid: get<HTMLElement>("candidate-grid"),
    nameInput: get<HTMLInputElement>("pet-name"),
    nameError: get<HTMLElement>("pet-name-error"),
    nextButton: get<HTMLButtonElement>("wizard-next"),
    cancelButton: get<HTMLButtonElement>("wizard-cancel"),
    retryButton: get<HTMLButtonElement>("review-retry"),
    abandonButton: get<HTMLButtonElement>("review-abandon"),
    finishButton: get<HTMLButtonElement>("review-accept"),
  };
}

export interface CandidateDynamicAssets {
  schemaVersion: number;
  bodyUrl: string | null;
  motionProfile: unknown;
}

export interface UploadCreationDomPorts {
  elements: UploadCreationElements;
  createElement(tagName: string): HTMLElement;
  loadApiKey(): Promise<string | null>;
  saveApiKey(value: string): Promise<void>;
  loadCandidate(jobId: string): Promise<CandidateDynamicAssets>;
  preview: Pick<CandidatePreviewController, "show" | "clear">;
  setInterval(callback: () => void, delayMs: number): number;
  clearInterval(id: number): void;
  createObjectURL(file: Blob): string;
  revokeObjectURL(url: string): void;
  confirm(message: string): boolean;
  onCancel(): void;
  onAbandoned(): void;
}

export class UploadCreationView {
  private state: UploadCreationViewSnapshot = {
    sessionId: null,
    method: null,
    step: "upload",
    creation: null,
  };
  private finalizing: { sessionId: string; promise: Promise<PetSwitchResult> } | null = null;
  private abandoning: { sessionId: string; promise: Promise<void> } | null = null;
  private submitting: { sessionId: string; promise: Promise<string> } | null = null;
  private readonly run = new CreationPageRun();
  private visit = 0;
  private mounted = false;
  private dynamicReady = false;
  private pollTimer: number | null = null;
  private photoBytes: Uint8Array | null = null;
  private photoObjectUrl: string | null = null;
  private durableSourceLoaded = false;
  private photoRevision = 0;
  private readonly onNameInput = () => this.render();
  private readonly onFinishClick = (event: Event) => {
    event.preventDefault();
    void this.finishFromDom();
  };
  private readonly onPhotoChange = () => { void this.readSelectedPhoto(); };
  private readonly onNextClick = (event: Event) => {
    event.preventDefault();
    void this.submitFromDom();
  };
  private readonly onSaveKeyClick = (event: Event) => {
    event.preventDefault();
    void this.saveKeyFromDom();
  };
  private readonly onCancelClick = (event: Event) => {
    event.preventDefault();
    this.dom?.onCancel();
  };
  private readonly onRetryClick = (event: Event) => {
    event.preventDefault();
    void this.retryFromDom();
  };
  private readonly onAbandonClick = (event: Event) => {
    event.preventDefault();
    void this.abandonFromDom();
  };

  constructor(
    private readonly ports: UploadCreationPorts,
    private readonly dom?: UploadCreationDomPorts,
  ) {}

  snapshot(): UploadCreationViewSnapshot {
    return this.state;
  }

  mount(): void {
    if (!this.dom || this.mounted) return;
    this.mounted = true;
    this.dom.elements.nameInput.addEventListener("input", this.onNameInput);
    this.dom.elements.finishButton.addEventListener("click", this.onFinishClick);
    this.dom.elements.photoInput.addEventListener("change", this.onPhotoChange);
    this.dom.elements.nextButton.addEventListener("click", this.onNextClick);
    this.dom.elements.saveKeyButton.addEventListener("click", this.onSaveKeyClick);
    this.dom.elements.cancelButton.addEventListener("click", this.onCancelClick);
    this.dom.elements.retryButton.addEventListener("click", this.onRetryClick);
    this.dom.elements.abandonButton.addEventListener("click", this.onAbandonClick);
    this.render();
  }

  destroy(): void {
    if (!this.dom || !this.mounted) return;
    this.leave();
    this.dom.elements.nameInput.removeEventListener("input", this.onNameInput);
    this.dom.elements.finishButton.removeEventListener("click", this.onFinishClick);
    this.dom.elements.photoInput.removeEventListener("change", this.onPhotoChange);
    this.dom.elements.nextButton.removeEventListener("click", this.onNextClick);
    this.dom.elements.saveKeyButton.removeEventListener("click", this.onSaveKeyClick);
    this.dom.elements.cancelButton.removeEventListener("click", this.onCancelClick);
    this.dom.elements.retryButton.removeEventListener("click", this.onRetryClick);
    this.dom.elements.abandonButton.removeEventListener("click", this.onAbandonClick);
    this.mounted = false;
  }

  async enter(): Promise<void> {
    const pendingFinalization = this.finalizing;
    const knownFinalizingSession = this.state.step === "finalizing" ? this.state.sessionId : null;
    if (!this.dom) throw new Error("上传创建页面未装配 DOM");
    this.leave();
    this.visit = this.run.enter("upload");
    const visit = this.visit;
    this.dynamicReady = false;
    this.dom.preview.clear();
    const key = await this.dom.loadApiKey().catch(() => null);
    if (!this.run.isCurrent(visit)) return;
    if (key) this.dom.elements.apiKeyInput.value = key;
    try {
      if (pendingFinalization) {
        await pendingFinalization.promise.catch(() => undefined);
        if (!this.run.isCurrent(visit)) return;
      }
      const unsettledSession = pendingFinalization?.sessionId ?? knownFinalizingSession;
      if (unsettledSession) {
        const settled = await this.ports.creation.snapshot(unsettledSession).catch((error) => {
          throw new Error(`暂时无法确认上次完成状态：${errorMessage(error)}`);
        });
        if (!this.run.isCurrent(visit)) return;
        this.applySnapshot(settled);
      } else {
        await this.open(visit);
      }
      if (this.state.step === "finalizing") await this.reconcileFinalizing(visit);
    } catch (error) {
      if (this.run.isCurrent(visit)) {
        this.setStatus(`${errorMessage(error)}。请返回对应入口继续，或先放弃现有草稿。`, true);
        this.render();
      }
      return;
    }
    if (!this.run.isCurrent(visit)) return;
    await this.restoreSourcePhoto(this.state.sessionId, visit);
    if (!this.run.isCurrent(visit)) return;
    this.dom.elements.nameInput.value = this.state.creation?.displayName ?? "";
    this.dom.elements.nameError.textContent = "";
    this.render();
    if (this.state.step === "upload" && this.state.creation?.error) {
      const next = this.photoBytes
        ? "原照片已从本机临时存储恢复，可直接重试；如需换照片，请先放弃并开始新会话。"
        : "照片未能恢复，请放弃当前创建并开始新会话后重新选择。";
      this.setStatus(`上次生成失败：${this.state.creation.error}。${next}`, true);
    }
    if (this.state.step === "review") await this.showCandidate(visit);
    if (this.state.step === "generating") this.startPolling(visit);
  }

  leave(): void {
    this.stopPolling();
    this.run.leave();
    this.visit = 0;
    this.dynamicReady = false;
    this.dom?.preview.clear();
    if (this.dom) {
      this.dom.elements.candidateGrid.replaceChildren();
      this.dom.elements.finishButton.disabled = true;
    }
    this.clearPhoto();
  }

  async start(visit?: number): Promise<CreationSnapshot> {
    const snapshot = await this.ports.creation.start("upload");
    if (this.canApplyVisit(visit)) this.applySnapshot(snapshot);
    return snapshot;
  }

  async open(visit?: number): Promise<CreationSnapshot> {
    const draft = await this.ports.creation.draft();
    if (!this.canApplyVisit(visit)) throw new StaleCreationOperation();
    if (!draft) return this.start(visit);
    if (draft.method !== "upload") {
      throw new Error("已有其他创建方式的草稿，请从对应入口继续或放弃后再上传");
    }
    return this.restore(draft.sessionId, visit);
  }

  async restore(sessionId: string, visit?: number): Promise<CreationSnapshot> {
    const snapshot = await this.ports.creation.snapshot(sessionId);
    if (this.canApplyVisit(visit)) this.applySnapshot(snapshot);
    return snapshot;
  }

  async submit(bytes: Uint8Array, visit?: number): Promise<string> {
    const sessionId = this.state.sessionId;
    if (!sessionId) throw new Error("请先开始上传创建");
    if (this.submitting?.sessionId === sessionId) return this.submitting.promise;
    if (this.finalizing?.sessionId === sessionId || this.abandoning?.sessionId === sessionId) {
      throw new Error("当前创建正在执行其他操作，请稍候");
    }
    const submitting = (async () => {
      const hash = await sha256Hex(bytes);
      if (!this.canApplySession(sessionId, visit)) throw new StaleCreationOperation();
      const jobId = await this.ports.creation.uploadStart(
        sessionId,
        buildPrompt(),
        bytesToBase64(bytes),
        hash,
      );
      if (!this.canApplySession(sessionId, visit)) return jobId;
      this.durableSourceLoaded = true;
      const creation = this.state.creation
        ? { ...this.state.creation, currentStep: "generating", jobId, jobStatus: "pending" }
        : null;
      this.state = { ...this.state, step: "generating", creation };
      return jobId;
    })();
    this.submitting = { sessionId, promise: submitting };
    try {
      return await submitting;
    } finally {
      if (this.submitting?.promise === submitting) this.submitting = null;
    }
  }

  async finish(displayName: string): Promise<PetSwitchResult> {
    if (!displayName.trim()) throw new Error("请输入宠物名称");
    const sessionId = this.state.sessionId;
    if (!sessionId) throw new Error("请先开始上传创建");
    if (this.finalizing?.sessionId === sessionId) return this.finalizing.promise;
    if (this.abandoning?.sessionId === sessionId) {
      throw new Error("当前创建正在放弃，不能同时完成创建");
    }
    if (this.submitting?.sessionId === sessionId) {
      throw new Error("照片正在提交，不能同时完成创建");
    }
    const visit = this.mounted ? this.visit : undefined;
    const finishing = (async () => {
      const saved = await this.ports.creation.setName(sessionId, displayName);
      if (!this.canApplySession(sessionId, visit)) throw new StaleCreationOperation();
      this.applySnapshot(saved);
      this.state = { ...this.state, step: "finalizing" };
      try {
        const result = await this.ports.finalize(sessionId);
        if (this.canApplySession(sessionId, visit)) {
          this.state = { ...this.state, step: result.ok ? "complete" : "review" };
        }
        return result;
      } catch (error) {
        if (this.canApplySession(sessionId, visit)) this.state = { ...this.state, step: "review" };
        throw error;
      }
    })();
    this.finalizing = { sessionId, promise: finishing };
    try {
      return await finishing;
    } finally {
      if (this.finalizing?.promise === finishing) this.finalizing = null;
    }
  }

  async abandon(): Promise<void> {
    const sessionId = this.state.sessionId;
    if (!sessionId) return;
    if (this.abandoning?.sessionId === sessionId) return this.abandoning.promise;
    if (this.finalizing?.sessionId === sessionId) {
      throw new Error("当前创建正在完成，不能同时放弃");
    }
    if (this.submitting?.sessionId === sessionId) {
      throw new Error("照片正在提交，不能同时放弃");
    }
    const visit = this.mounted ? this.visit : undefined;
    const abandoning = (async () => {
      await this.ports.creation.abandon(sessionId);
      if (this.canApplySession(sessionId, visit)) {
        this.state = {
          sessionId: null,
          method: null,
          step: "upload",
          creation: null,
        };
      }
    })();
    this.abandoning = { sessionId, promise: abandoning };
    try {
      await abandoning;
    } finally {
      if (this.abandoning?.promise === abandoning) this.abandoning = null;
    }
  }

  private applySnapshot(snapshot: CreationSnapshot): void {
    this.state = {
      sessionId: snapshot.sessionId,
      method: snapshot.method,
      step: stepFromSnapshot(snapshot),
      creation: snapshot,
    };
  }

  private canApplyVisit(visit?: number): boolean {
    return visit === undefined || this.run.isCurrent(visit);
  }

  private canApplySession(sessionId: string, visit?: number): boolean {
    return this.canApplyVisit(visit) && this.state.sessionId === sessionId;
  }

  private async reconcileFinalizing(visit: number): Promise<void> {
    const sessionId = this.state.sessionId;
    if (!sessionId || !this.run.isCurrent(visit)) return;
    const token = this.run.begin(visit, "finalize", sessionId);
    if (!token) return;
    try {
      await this.ports.creation.recoverFinalization();
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      let snapshot = await this.ports.creation.snapshot(sessionId);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      this.applySnapshot(snapshot);
      if (this.state.step === "review") {
        try {
          await this.ports.finalize(sessionId);
        } catch {
          // The durable snapshot below decides whether recovery completed or returned to review.
        }
        if (!this.run.shouldApply(token, this.state.sessionId)) return;
        snapshot = await this.ports.creation.snapshot(sessionId);
        if (!this.run.shouldApply(token, this.state.sessionId)) return;
        this.applySnapshot(snapshot);
      }
    } finally {
      this.run.settle(token);
    }
  }

  private async showCandidate(visit: number): Promise<void> {
    if (!this.dom || !this.run.isCurrent(visit)) return;
    const sessionId = this.state.sessionId;
    const jobId = this.state.creation?.jobId;
    if (!sessionId || !jobId) {
      this.setStatus("候选记录缺少生成任务 jobId，请重新生成。", true);
      this.render();
      return;
    }
    const token = this.run.begin(visit, "preview", sessionId);
    if (!token) return;
    try {
      const assets = await this.dom.loadCandidate(jobId);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      if (assets.schemaVersion !== 3 || !assets.bodyUrl) {
        throw new Error("候选缺少 v3 动态图片配置");
      }
      const profile: MotionProfileV1 = parseMotionProfile(assets.motionProfile);
      const root = this.dom.createElement("div");
      root.className = "candidate-preview";
      root.setAttribute("role", "img");
      root.setAttribute("aria-label", "会呼吸微动的宠物候选");
      this.dom.elements.candidateGrid.replaceChildren(root);
      await this.dom.preview.show(root, assets.bodyUrl, profile);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      this.dynamicReady = true;
      this.setStatus("动态候选已准备好，请为它取名。", false);
      this.render();
    } catch (error) {
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      this.dynamicReady = false;
      this.dom.preview.clear();
      this.dom.elements.candidateGrid.replaceChildren();
      this.setStatus(`动态预览不可用：${errorMessage(error)}。请选择“重新生成”再试。`, true);
      this.render();
    } finally {
      this.run.settle(token);
    }
  }

  private startPolling(visit: number): void {
    if (!this.dom || !this.run.isCurrent(visit)) return;
    this.stopPolling();
    this.pollTimer = this.dom.setInterval(() => {
      void this.pollOnce(visit);
    }, 4_000);
  }

  private stopPolling(): void {
    if (!this.dom || this.pollTimer === null) return;
    this.dom.clearInterval(this.pollTimer);
    this.pollTimer = null;
  }

  private async pollOnce(visit: number): Promise<void> {
    if (!this.dom || !this.run.isCurrent(visit)) return;
    const sessionId = this.state.sessionId;
    if (!sessionId) return;
    const token = this.run.begin(visit, "poll", sessionId);
    if (!token) return;
    try {
      const snapshot = await this.ports.creation.snapshot(sessionId);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      this.applySnapshot(snapshot);
      this.render();
      if (this.state.step === "review") {
        this.stopPolling();
        await this.showCandidate(visit);
      } else if (this.state.step === "upload" && snapshot.error) {
        this.stopPolling();
        this.setStatus(`生成失败：${snapshot.error}。请重新选择照片后重试。`, true);
      }
    } catch (error) {
      if (this.run.shouldApply(token, this.state.sessionId)) {
        this.setStatus(`查询生成进度失败：${errorMessage(error)}。创建记录已保留，将自动重试。`, true);
      }
    } finally {
      this.run.settle(token);
    }
  }

  private async finishFromDom(): Promise<void> {
    if (!this.dom) return;
    if (this.state.step !== "review" || !this.dynamicReady) return;
    const sessionId = this.state.sessionId;
    const visit = this.visit;
    if (!sessionId) return;
    const token = this.run.begin(visit, "finalize", sessionId);
    if (!token) return;
    this.render();
    this.dom.elements.nameError.textContent = "";
    try {
      const result = await this.finish(this.dom.elements.nameInput.value);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      if (result.ok) {
        this.dom.preview.clear();
        this.dynamicReady = false;
        this.setStatus(result.warning ?? "宠物已出现在桌面。", false);
      } else {
        this.setStatus(`未能放到桌面：${result.message}。动态候选仍保留，请重试。`, true);
      }
      this.render();
    } catch (error) {
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      const message = errorMessage(error);
      if (message.includes("名称")) this.dom.elements.nameError.textContent = message;
      this.setStatus(`${message}。请检查名称或桌面宠物窗口后重试。`, true);
      this.render();
    } finally {
      this.run.settle(token);
      if (this.run.shouldApply(token, this.state.sessionId)) this.render();
    }
  }

  private async readSelectedPhoto(): Promise<void> {
    if (!this.dom) return;
    if (this.durableSourceLoaded) {
      this.setStatus("如需换照片，请先放弃当前创建并开始新会话。", true);
      return;
    }
    const file = this.dom.elements.photoInput.files?.[0];
    if (!file) return;
    const revision = ++this.photoRevision;
    const visit = this.visit;
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (!this.mounted || revision !== this.photoRevision || !this.run.isCurrent(visit)) return;
    this.photoBytes = bytes;
    if (this.photoObjectUrl) this.dom.revokeObjectURL(this.photoObjectUrl);
    this.photoObjectUrl = this.dom.createObjectURL(file);
    this.dom.elements.photoPreview.src = this.photoObjectUrl;
    this.dom.elements.photoPreview.hidden = false;
    this.setStatus("照片已选择，可以开始生成。", false);
  }

  private clearPhoto(): void {
    this.photoRevision += 1;
    this.photoBytes = null;
    this.durableSourceLoaded = false;
    if (!this.dom) return;
    if (this.photoObjectUrl) this.dom.revokeObjectURL(this.photoObjectUrl);
    this.photoObjectUrl = null;
    this.dom.elements.photoInput.value = "";
    this.dom.elements.photoPreview.removeAttribute("src");
    this.dom.elements.photoPreview.hidden = true;
  }

  private async restoreSourcePhoto(sessionId: string | null, visit: number): Promise<void> {
    if (!this.dom || !sessionId || !this.run.isCurrent(visit)) return;
    const revision = ++this.photoRevision;
    const source = await this.ports.creation.uploadSource(sessionId).catch(() => null);
    if (!source || revision !== this.photoRevision || !this.run.isCurrent(visit)) return;
    this.photoBytes = dataUrlBytes(source.dataUrl);
    this.durableSourceLoaded = true;
    if (this.photoObjectUrl) this.dom.revokeObjectURL(this.photoObjectUrl);
    this.photoObjectUrl = null;
    this.dom.elements.photoPreview.src = source.dataUrl;
    this.dom.elements.photoPreview.hidden = false;
  }

  private async saveKeyFromDom(): Promise<void> {
    if (!this.dom) return;
    const key = this.dom.elements.apiKeyInput.value.trim();
    if (!key) {
      this.dom.elements.keyStatus.textContent = "请输入 API Key 后再保存。";
      return;
    }
    try {
      await this.dom.saveApiKey(key);
      this.dom.elements.keyStatus.textContent = "API Key 已保存在本机。";
    } catch (error) {
      this.dom.elements.keyStatus.textContent = `保存失败：${errorMessage(error)}。请检查后重试。`;
    }
  }

  private async submitFromDom(): Promise<void> {
    if (!this.dom) return;
    const visit = this.visit;
    if (!this.state.sessionId) {
      try {
        await this.recoverUploadDraft(visit);
      } catch (error) {
        if (this.run.isCurrent(visit)) {
          this.setStatus(`暂时无法新建上传草稿：${errorMessage(error)}。请点击“上传照片，生成候选”重试。`, true);
          this.render();
        }
        return;
      }
    }
    const sessionId = this.state.sessionId;
    if (!sessionId || !this.run.isCurrent(visit)) {
      this.setStatus("暂时无法开始上传草稿，请点击“开始生成”重试。", true);
      return;
    }
    if (!this.photoBytes) {
      this.setStatus("请先选择一张清晰的猫咪照片。", true);
      return;
    }
    const key = this.dom.elements.apiKeyInput.value.trim();
    if (!key) {
      this.setStatus("请先填写 API Key；它只保存在本机。", true);
      return;
    }
    const token = this.run.begin(visit, "submit", sessionId);
    if (!token) return;
    this.dom.elements.nextButton.disabled = true;
    try {
      await this.dom.saveApiKey(key);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      await this.submit(this.photoBytes, visit);
      if (!this.run.shouldApply(token, this.state.sessionId)) return;
      this.setStatus("照片已安全提交，正在生成会呼吸微动的候选。", false);
      this.render();
      this.startPolling(visit);
    } catch (error) {
      if (this.run.shouldApply(token, this.state.sessionId)) {
        this.setStatus(`提交生成失败：${errorMessage(error)}。照片和草稿仍在，可直接重试。`, true);
      }
    } finally {
      this.run.settle(token);
      if (this.run.shouldApply(token, this.state.sessionId)) {
        this.dom.elements.nextButton.disabled = false;
        this.render();
      }
    }
  }

  private async retryFromDom(): Promise<void> {
    if (!this.dom) return;
    const sessionId = this.state.sessionId;
    const visit = this.visit;
    if (!sessionId || !this.run.isCurrent(visit)) return;
    const token = this.run.begin(visit, "retry", sessionId);
    if (!token) return;
    this.render();
    const retainedPhoto = this.photoBytes;
    const retainedPreview = this.dom.elements.photoPreview.src;
    try {
      await this.abandon();
      if (!this.run.isCurrent(visit)) return;
      await this.start(visit);
      if (!this.run.isCurrent(visit)) return;
      this.stopPolling();
      this.dom.preview.clear();
      this.dom.elements.candidateGrid.replaceChildren();
      this.dom.elements.nameInput.value = "";
      this.dom.elements.nameError.textContent = "";
      this.dynamicReady = false;
      this.photoBytes = retainedPhoto;
      this.durableSourceLoaded = false;
      if (retainedPhoto && retainedPreview) {
        this.dom.elements.photoPreview.src = retainedPreview;
        this.dom.elements.photoPreview.hidden = false;
      }
      this.setStatus(retainedPhoto
        ? "旧候选已清理，原照片仍可直接重新生成。"
        : "旧候选已清理。请重新选择一张清晰照片，再重新生成。", false);
      this.render();
    } catch (error) {
      if (this.run.isCurrent(visit)) {
        try {
          await this.recoverUploadDraft(visit);
          this.setStatus(`准备重新生成未确认：${errorMessage(error)}。已重新同步本地草稿，可继续操作。`, true);
        } catch (recoveryError) {
          this.setStatus(
            `准备重新生成失败：${errorMessage(error)}；${errorMessage(recoveryError)}。请点击“上传照片，生成候选”重新开始。`,
            true,
          );
        }
      }
    } finally {
      this.run.settle(token);
      if (this.run.isCurrent(visit)) {
        this.render();
      } else if (this.mounted) {
        this.render();
      }
    }
  }

  private async recoverUploadDraft(visit: number): Promise<void> {
    if (!this.run.isCurrent(visit)) return;
    const draft = await this.ports.creation.draft().catch(() => null);
    if (!this.run.isCurrent(visit)) return;
    if (draft) {
      if (draft.method !== "upload") throw new Error("已有其他创建方式的草稿");
      await this.restore(draft.sessionId, visit);
      await this.restoreSourcePhoto(draft.sessionId, visit);
      return;
    }
    await this.start(visit);
  }

  private async abandonFromDom(): Promise<void> {
    if (!this.dom) return;
    const sessionId = this.state.sessionId;
    const visit = this.visit;
    if (!sessionId || !this.run.isCurrent(visit)) return;
    if (!this.dom.confirm("确定放弃这次创建吗？本地草稿和生成任务会被清理。")) return;
    const token = this.run.begin(visit, "abandon", sessionId);
    if (!token) return;
    this.render();
    try {
      await this.abandon();
      if (!this.run.isCurrent(visit)) return;
      this.dom.preview.clear();
      this.dom.onAbandoned();
    } catch (error) {
      if (this.run.isCurrent(visit)) {
        this.setStatus(`放弃创建失败：${errorMessage(error)}。草稿仍保留，可再次尝试。`, true);
      }
    } finally {
      this.run.settle(token);
      if (this.run.isCurrent(visit)) this.render();
    }
  }

  private render(): void {
    if (!this.dom) return;
    const { elements } = this.dom;
    elements.stepUpload.hidden = this.state.step !== "upload";
    elements.stepGenerating.hidden = this.state.step !== "generating";
    elements.stepReview.hidden = this.state.step !== "review" && this.state.step !== "finalizing";
    elements.stepComplete.hidden = this.state.step !== "complete";
    elements.nextButton.hidden = this.state.step !== "upload";
    elements.cancelButton.hidden = this.state.step !== "generating";
    const mutating = this.run.isMutating(this.state.sessionId);
    elements.nextButton.disabled = mutating;
    elements.retryButton.disabled = mutating;
    elements.abandonButton.disabled = mutating;
    elements.finishButton.disabled = this.state.step !== "review"
      || !this.dynamicReady
      || !elements.nameInput.value.trim()
      || mutating;
    if (this.state.step === "generating") {
      const status = this.state.creation?.jobStatus ?? "pending";
      const labels: Record<string, string> = {
        pending: "正在排队…",
        running: "生成中…",
        success: "生成完成，正在准备动态预览…",
        failed: "生成失败",
      };
      const card = this.dom.createElement("div");
      card.className = `job-card ${status === "success" ? "success" : status === "failed" ? "failed" : ""}`;
      card.textContent = labels[status] ?? `生成状态：${status}`;
      elements.jobGrid.replaceChildren(card);
    } else {
      elements.jobGrid.replaceChildren();
    }
  }

  private setStatus(message: string, error: boolean): void {
    if (!this.dom) return;
    this.dom.elements.status.textContent = message;
    this.dom.elements.status.classList.toggle("error", error);
  }
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function stepFromSnapshot(snapshot: CreationSnapshot): UploadCreationStep {
  if (snapshot.status === "completed") return "complete";
  if (snapshot.status === "finalizing") return "finalizing";
  if (snapshot.status === "candidateReady" || snapshot.lastStableStatus === "candidateReady") {
    return "review";
  }
  return snapshot.currentStep === "generating" ? "generating" : "upload";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function dataUrlBytes(dataUrl: string): Uint8Array {
  const match = /^data:[^;,]+;base64,([A-Za-z0-9+/=]+)$/.exec(dataUrl);
  if (!match) throw new Error("持久照片格式无效");
  const binary = atob(match[1]!);
  return Uint8Array.from(binary, (value) => value.charCodeAt(0));
}

class StaleCreationOperation extends Error {
  constructor() {
    super("创建页面已切换");
  }
}
