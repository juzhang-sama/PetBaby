import type { PhotoAvatarSnapshot, PhotoAvatarUpload } from "../creation/api";
import type { CreationSnapshot } from "../creation/contracts";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import type { PhotoAvatarPreviewHandle } from "./photo-avatar-pixel-preview";
import { mountPhotoAvatarPreview } from "./photo-avatar-pixel-preview";
import { photoAvatarStyleCopy } from "./photo-avatar-style-copy";

export type PhotoAvatarCreationStep =
  | "collecting"
  | "generating"
  | "preview"
  | "finalizing"
  | "complete"
  | "failed";

export interface PhotoAvatarViewState {
  sessionId: string | null;
  step: PhotoAvatarCreationStep;
  snapshot: PhotoAvatarSnapshot | null;
}

export interface PhotoAvatarCreationPorts {
  api: {
    start(method: "upload"): Promise<{ sessionId: string }>;
    abandon(sessionId: string): Promise<void>;
    photoAvatarConsent(accept: boolean): Promise<boolean>;
    photoAvatarBegin(sessionId: string, consentVersion: string, photos: PhotoAvatarUpload[]): Promise<PhotoAvatarSnapshot>;
    photoAvatarStatus(sessionId: string): Promise<PhotoAvatarSnapshot | null>;
    photoAvatarCancel(sessionId: string): Promise<PhotoAvatarSnapshot>;
    photoAvatarRegenerate(sessionId: string): Promise<PhotoAvatarSnapshot>;
    photoAvatarRevise(sessionId: string, instruction: string): Promise<PhotoAvatarSnapshot>;
    setName(sessionId: string, displayName: string): Promise<CreationSnapshot>;
  };
  finalize(sessionId: string): Promise<PetSwitchResult>;
}

export interface PhotoAvatarCreationElements {
  root: HTMLElement;
  files: HTMLInputElement;
  generate: HTMLButtonElement;
  generating: HTMLElement;
  preview: HTMLElement;
  live2d: HTMLElement;
  completions: HTMLElement;
  name: HTMLInputElement;
  accept: HTMLButtonElement;
  regenerate: HTMLButtonElement;
  revision: HTMLInputElement;
  revise: HTMLButtonElement;
  cancel: HTMLButtonElement;
  status: HTMLElement;
  complete: HTMLElement;
  done: HTMLButtonElement;
}

export function queryPhotoAvatarCreationElements(root: Document): PhotoAvatarCreationElements {
  const get = <T extends HTMLElement>(id: string): T => {
    const element = root.getElementById(id);
    if (!element) throw new Error(`missing element #${id}`);
    return element as T;
  };
  return {
    root: get("photo-avatar-workspace"),
    files: get("photo-avatar-files"),
    generate: get("photo-avatar-generate"),
    generating: get("photo-avatar-generating"),
    preview: get("photo-avatar-preview"),
    live2d: get("photo-avatar-live2d"),
    completions: get("photo-avatar-completions"),
    name: get("photo-avatar-name"),
    accept: get("photo-avatar-accept"),
    regenerate: get("photo-avatar-regenerate"),
    revision: get("photo-avatar-revision"),
    revise: get("photo-avatar-revise"),
    cancel: get("photo-avatar-cancel"),
    status: get("photo-avatar-status"),
    complete: get("photo-avatar-complete"),
    done: get("photo-avatar-done"),
  };
}

export interface PhotoAvatarCreationDomPorts {
  elements: PhotoAvatarCreationElements;
  dialog: { showThirdPartyConsent(): Promise<boolean> };
  preview: {
    show(root: HTMLElement, sessionId: string): Promise<PhotoAvatarPreviewHandle | void>;
    clear(): void;
  };
  setInterval(callback: () => void, delayMs: number): number;
  clearInterval(id: number): void;
  onCancel(): void;
}

const CONSENT_VERSION = "photo-avatar-third-party-ai-lk888-no-delete-v2";
const FAILURE_MESSAGES: Record<string, string> = {
  auth: "图片生成服务认证失败。",
  quota: "图片生成服务额度不足。",
  contentPolicy: "图片生成请求未通过内容审核。",
  unsupported: "当前图片生成请求不受支持。",
  network: "网络连接失败，请稍后重试。",
  timeout: "图片生成超时，请稍后重试。",
  provider5xx: "图片生成服务暂时不可用，请稍后重试。",
  temporaryUnavailable: "图片生成服务暂时不可用，请稍后重试。",
  localStorage: "生成服务存储异常，请重启后端服务后重试。",
};

function safeInvalidInputMessage(message: string | null): string {
  const value = message?.trim();
  if (!value || value.length > 80 || /[\r\n]/u.test(value)) {
    return "生成请求无效，请检查照片后重试。";
  }
  const lowered = value.toLowerCase();
  const unsafe = [
    "http://", "https://", "api_key", "apikey", "authorization", "bearer",
    "token", "secret", "provider", "diagnostic", "stack", "trace",
  ];
  return unsafe.some((fragment) => lowered.includes(fragment))
    ? "生成请求无效，请检查照片后重试。"
    : value;
}

function failureMessage(errorCode: string | null, errorMessage: string | null): string {
  if (errorCode === "invalidInput") return safeInvalidInputMessage(errorMessage);
  return errorCode ? FAILURE_MESSAGES[errorCode] ?? "生成失败，请重试。" : "生成失败，请重试。";
}

const browserDomPorts = (): Omit<PhotoAvatarCreationDomPorts, "elements" | "dialog" | "onCancel"> => ({
  preview: {
    show: (root, sessionId) => mountPhotoAvatarPreview(root, sessionId),
    clear: () => undefined,
  },
  setInterval: (callback, delayMs) => window.setInterval(callback, delayMs),
  clearInterval: (id) => window.clearInterval(id),
});

export class PhotoAvatarCreationView {
  private state: PhotoAvatarViewState = { sessionId: null, step: "collecting", snapshot: null };
  private selected: File[] = [];
  private visit = 0;
  private pollId: number | null = null;
  private previewHandle: PhotoAvatarPreviewHandle | null = null;
  private previewMountKey: string | null = null;
  private selectionError: string | null = null;
  private mounted = false;
  private readonly onFiles = () => {
    this.selected = Array.from(this.dom.elements.files.files ?? []);
    this.selectionError = this.selected.length > 8 ? "请一次选择 1 至 8 张猫咪照片。" : null;
    this.render();
  };
  private readonly onGenerate = () => { void this.generate(); };
  private readonly onAccept = () => { void this.accept(); };
  private readonly onRegenerate = () => { void this.regenerate(); };
  private readonly onRevise = () => { void this.revise(); };
  private readonly onCancel = () => { void this.cancel(); };
  private readonly onDone = () => { this.dom.onCancel(); };

  constructor(
    private readonly ports: PhotoAvatarCreationPorts,
    private readonly dom: PhotoAvatarCreationDomPorts,
  ) {}

  async enter(sessionId?: string | null): Promise<void> {
    const visit = ++this.visit;
    this.bind();
    try {
      // sessionId === undefined：沿用视图当前 session（内部恢复）；
      // sessionId === null：调用方明确要求全新会话，绝不 fallback 到残留的旧 session，
      // 否则会出现"安装成功后再次进入仍停留在上次完成界面、生成按钮不可点"的问题。
      const existingId = sessionId === undefined ? this.state.sessionId : sessionId;
      if (existingId === null) return await this.startFreshSession(visit);
      const id = existingId;
      if (!this.current(visit)) return;
      this.state = { ...this.state, sessionId: id };
      const hasRun = await this.refresh(visit);
      if (!this.current(visit)) return;
      if (
        this.state.snapshot?.step === "failed"
        || this.state.snapshot?.step === "cancelled"
        || this.state.snapshot?.step === "cleanupPending"
      ) {
        await this.ports.api.abandon(id);
        if (!this.current(visit)) return;
        await this.startFreshSession(visit);
        return;
      }
      if (hasRun && this.shouldPoll()) this.startPolling(visit);
    } catch (error) {
      if (this.current(visit)) this.fail(error);
    }
  }

  leave(): void {
    this.visit += 1;
    this.stopPolling();
    this.clearPreview();
    this.unbind();
  }

  snapshot(): Readonly<PhotoAvatarViewState> {
    return this.state;
  }

  private async generate(): Promise<void> {
    if (this.selectionError || this.selected.length === 0 || this.state.sessionId === null || this.state.step !== "collecting") return;
    const visit = this.visit;
    const accepted = await this.dom.dialog.showThirdPartyConsent();
    if (!this.current(visit)) return;
    await this.ports.api.photoAvatarConsent(accepted);
    if (!accepted || !this.current(visit)) return;
    try {
      const snapshot = await this.ports.api.photoAvatarBegin(
        this.state.sessionId,
        CONSENT_VERSION,
        await Promise.all(this.selected.map(toUpload)),
      );
      this.apply(snapshot, visit);
      if (this.current(visit) && stepFor(snapshot) === "generating") this.startPolling(visit);
    } catch (error) {
      if (this.current(visit)) this.fail(error);
    }
  }

  private async regenerate(): Promise<void> {
    const { sessionId } = this.state;
    if (!sessionId || (this.state.step !== "preview" && this.state.step !== "failed")) return;
    const visit = this.visit;
    try {
      const snapshot = await this.ports.api.photoAvatarRegenerate(sessionId);
      this.apply(snapshot, visit);
      if (this.current(visit) && stepFor(snapshot) === "generating") this.startPolling(visit);
    }
    catch (error) { if (this.current(visit)) this.fail(error); }
  }

  private async revise(): Promise<void> {
    const { sessionId } = this.state;
    const instruction = this.dom.elements.revision.value.trim();
    if (!sessionId || this.state.step !== "preview") return;
    if (!instruction) {
      this.setStatus("请填写需要修改的位置。");
      return;
    }
    const visit = this.visit;
    try { this.apply(await this.ports.api.photoAvatarRevise(sessionId, instruction), visit); }
    catch (error) { if (this.current(visit)) this.fail(error); }
  }

  private async accept(): Promise<void> {
    const { sessionId } = this.state;
    if (
      !sessionId
      || this.state.step !== "preview"
      || this.state.snapshot?.step !== "previewReady"
    ) return;
    const name = this.dom.elements.name.value.trim();
    if (!name) {
      this.setStatus("请先给宠物起个名字。");
      this.dom.elements.name.focus();
      return;
    }
    const visit = this.visit;
    this.state = { ...this.state, step: "finalizing" };
    this.render();
    try {
      await this.ports.api.setName(sessionId, name);
      if (!this.current(visit)) return;
      const result = await this.ports.finalize(sessionId);
      if (!this.current(visit)) return;
      if (!result.ok) {
        this.state = { ...this.state, step: "preview" };
        this.render();
        // render() 只会在 preview 步骤显示"请确认后安装"，这里用真实失败原因覆盖，
        // 避免用户误以为还在等待确认。
        this.setStatus(result.message);
        return;
      }
      this.state = { ...this.state, step: "complete" };
      this.render();
      this.setStatus(result.warning ?? "照片分身已安装。");
    } catch (error) {
      if (this.current(visit)) this.fail(error);
    }
  }

  private async cancel(): Promise<void> {
    const { sessionId } = this.state;
    if (!sessionId) return;
    const visit = this.visit;
    try {
      await this.ports.api.photoAvatarCancel(sessionId);
      if (!this.current(visit)) return;
      await this.ports.api.abandon(sessionId);
      if (!this.current(visit)) return;
      this.stopPolling();
      this.resetSelection();
      this.dom.onCancel();
    } catch (error) {
      if (this.current(visit)) this.fail(error);
    }
  }

  private async startFreshSession(visit: number): Promise<void> {
    const { sessionId } = await this.ports.api.start("upload");
    if (!this.current(visit)) return;
    this.stopPolling();
    this.clearPreview();
    this.resetSelection();
    this.state = { sessionId, step: "collecting", snapshot: null };
    this.setStatus("");
    this.render();
  }

  private resetSelection(): void {
    this.selected = [];
    this.selectionError = null;
    this.dom.elements.files.value = "";
  }

  private async refresh(visit: number): Promise<boolean> {
    const { sessionId } = this.state;
    if (!sessionId) return false;
    const snapshot = await this.ports.api.photoAvatarStatus(sessionId);
    if (snapshot === null) {
      if (this.current(visit)) {
        // session 在 Rust 端已不存在（可能被 abandon/重启清理）。
        // 必须重建新会话，否则后续"生成"会用废弃的 sessionId 报
        // "photo avatar session does not exist"。startFreshSession 内部
        // 会 stopPolling + clearPreview + 重置选择。
        await this.startFreshSession(visit);
      }
      return false;
    }
    this.apply(snapshot, visit);
    return true;
  }

  private apply(snapshot: PhotoAvatarSnapshot, visit: number): void {
    if (!this.current(visit)) return;
    if (snapshot.step !== "runtimeCheckPending" && snapshot.step !== "previewReady") {
      this.clearPreview();
    }
    this.state = { sessionId: snapshot.sessionId, snapshot, step: stepFor(snapshot) };
    if (!this.shouldPoll()) this.stopPolling();
    this.render();
    if (snapshot.step === "runtimeCheckPending") void this.mountRuntimeCheck(visit, snapshot.sessionId, snapshot.revision);
    if (snapshot.step === "previewReady") void this.mountPreview(visit, snapshot.sessionId, snapshot.revision);
  }

  private shouldPoll(): boolean {
    return this.state.step === "generating"
      && this.state.snapshot?.step !== "runtimeCheckPending";
  }

  private async mountRuntimeCheck(visit: number, sessionId: string, revision: number): Promise<void> {
    try {
      await this.mountPreview(visit, sessionId, revision);
      if (this.current(visit)) await this.refresh(visit);
    } catch (error) {
      if (this.current(visit)) this.fail(error);
    }
  }

  private async mountPreview(visit: number, sessionId: string, revision: number): Promise<void> {
    const mountKey = `${sessionId}:${revision}`;
    if (!this.current(visit) || this.previewMountKey === mountKey) return;
    if (this.previewMountKey !== null || this.previewHandle !== null) this.clearPreview();
    this.previewMountKey = mountKey;
    try {
      const handle = await this.dom.preview.show(this.dom.elements.live2d, sessionId);
      if (!this.current(visit) || this.previewMountKey !== mountKey) {
        handle?.destroy();
        return;
      }
      this.previewHandle = handle ?? null;
    } catch (error) {
      if (this.previewMountKey === mountKey) this.previewMountKey = null;
      if (this.current(visit)) this.fail(error);
      throw error;
    }
  }

  private startPolling(visit: number): void {
    this.stopPolling();
    this.pollId = this.dom.setInterval(() => {
      void this.refresh(visit).catch((error) => { if (this.current(visit)) this.fail(error); });
    }, 1_000);
  }

  private stopPolling(): void {
    if (this.pollId !== null) this.dom.clearInterval(this.pollId);
    this.pollId = null;
  }

  private clearPreview(): void {
    this.previewHandle?.destroy();
    this.previewHandle = null;
    this.previewMountKey = null;
    this.dom.preview.clear();
  }

  private fail(error: unknown): void {
    this.state = { ...this.state, step: "failed" };
    this.setStatus(error instanceof Error ? error.message : String(error));
    this.render();
  }

  private render(): void {
    const { elements } = this.dom;
    const preview = this.state.step === "preview";
    const canRegenerate = preview || this.state.step === "failed";
    elements.generate.disabled = this.selected.length === 0 || this.selectionError !== null || this.state.step !== "collecting";
    elements.generating.hidden = this.state.step !== "generating";
    elements.preview.hidden = !preview;
    elements.accept.hidden = !preview;
    elements.regenerate.hidden = !canRegenerate;
    elements.revise.hidden = !preview;
    elements.complete.hidden = this.state.step !== "complete";
    elements.done.hidden = this.state.step !== "complete";
    elements.cancel.hidden = this.state.step === "collecting" || this.state.step === "complete";
    if (this.state.snapshot?.profile) {
      elements.completions.textContent = completionText(this.state.snapshot.profile);
    }
    if (this.state.step === "preview" && this.state.snapshot?.step === "previewReady") {
      this.setStatus("像素宠物预览已通过运行时检查，请确认后安装。");
    }
    if (this.state.step === "generating") this.setStatus("正在生成完整像素宠物，请稍候。");
    if (this.state.step === "finalizing") this.setStatus("正在安装照片分身。");
    if (this.state.snapshot?.step === "failed") {
      this.setStatus(failureMessage(
        this.state.snapshot.errorCode,
        this.state.snapshot.errorMessage,
      ));
    }
    if (this.selectionError) this.setStatus(this.selectionError);
  }

  private setStatus(message: string): void { this.dom.elements.status.textContent = message; }
  private current(visit: number): boolean { return visit === this.visit; }

  private bind(): void {
    if (this.mounted) return;
    this.mounted = true;
    const { elements } = this.dom;
    elements.files.addEventListener("change", this.onFiles);
    elements.generate.addEventListener("click", this.onGenerate);
    elements.accept.addEventListener("click", this.onAccept);
    elements.regenerate.addEventListener("click", this.onRegenerate);
    elements.revise.addEventListener("click", this.onRevise);
    elements.cancel.addEventListener("click", this.onCancel);
    elements.done.addEventListener("click", this.onDone);
  }

  private unbind(): void {
    if (!this.mounted) return;
    this.mounted = false;
    const { elements } = this.dom;
    elements.files.removeEventListener("change", this.onFiles);
    elements.generate.removeEventListener("click", this.onGenerate);
    elements.accept.removeEventListener("click", this.onAccept);
    elements.regenerate.removeEventListener("click", this.onRegenerate);
    elements.revise.removeEventListener("click", this.onRevise);
    elements.cancel.removeEventListener("click", this.onCancel);
    elements.done.removeEventListener("click", this.onDone);
  }
}

function stepFor(snapshot: PhotoAvatarSnapshot): PhotoAvatarCreationStep {
  if (snapshot.step === "previewReady") return "preview";
  if (snapshot.step === "failed" || snapshot.step === "cleanupPending") return "failed";
  if (snapshot.step === "collecting" || snapshot.step === "cancelled") return "collecting";
  if (snapshot.step === "completed") return "complete";
  return "generating";
}

async function toUpload(file: File): Promise<PhotoAvatarUpload> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  return { bytesB64: bytesToBase64(bytes), sha256: await sha256Hex(bytes) };
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer);
  return Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
}

function completionText(profile: unknown): string {
  if (!profile || typeof profile !== "object") return "";
  const value = profile as { completionSummary?: unknown; styleProfileId?: unknown };
  const completions = Array.isArray(value.completionSummary) ? value.completionSummary.join("、") : "";
  const style = photoAvatarStyleCopy(value.styleProfileId);
  return [completions && `AI 补全：${completions}`, style].filter(Boolean).join("；");
}

export function createPhotoAvatarCreationDomPorts(
  documentRoot: Document,
  dialog: PhotoAvatarCreationDomPorts["dialog"],
  onCancel: () => void,
): PhotoAvatarCreationDomPorts {
  return { elements: queryPhotoAvatarCreationElements(documentRoot), dialog, onCancel, ...browserDomPorts() };
}
