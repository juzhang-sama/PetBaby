import type { CreationSnapshot } from "./contracts";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";

export type CreationStep = "upload" | "generating" | "review" | "finalizing" | "complete";

export interface UploadCreationStore {
  start(): Promise<CreationSnapshot>;
  submit(sessionId: string, prompt: string, refPngB64: string): Promise<string>;
  snapshot(sessionId: string): Promise<CreationSnapshot>;
  setName(sessionId: string, name: string): Promise<CreationSnapshot>;
  finalize(sessionId: string): Promise<PetSwitchResult>;
  abandon(sessionId: string): Promise<void>;
}

export class CreationFlow {
  step: CreationStep = "upload";
  activationWarning: string | null = null;
  private currentSnapshot: CreationSnapshot | null = null;
  private photoBytes: Uint8Array | null = null;
  private currentJobId: string | null = null;

  constructor(private readonly store: UploadCreationStore) {}

  get sessionId(): string | null {
    return this.currentSnapshot?.sessionId ?? null;
  }

  get petId(): string | null {
    return this.currentSnapshot?.petId ?? null;
  }

  get jobId(): string | null {
    return this.currentJobId;
  }

  get displayName(): string | null {
    return this.currentSnapshot?.displayName ?? null;
  }

  get snapshot(): CreationSnapshot | null {
    return this.currentSnapshot;
  }

  setPhotoBytes(bytes: Uint8Array): void {
    this.photoBytes = bytes;
  }

  clearPhoto(): void {
    this.photoBytes = null;
  }

  async start(): Promise<CreationSnapshot> {
    const snapshot = await this.store.start();
    this.restore(snapshot);
    return snapshot;
  }

  restore(snapshot: CreationSnapshot): void {
    if (snapshot.method !== "upload") {
      throw new Error("当前草稿不是上传创建，请先返回对应创建入口处理");
    }
    if (snapshot.status === "abandoned") throw new Error("上传创建已放弃");
    this.currentSnapshot = snapshot;
    this.currentJobId = snapshot.jobId ?? snapshot.candidateId;
    this.step = stepFromSnapshot(snapshot);
  }

  async submitSingle(): Promise<string> {
    if (!this.photoBytes) throw new Error("photo required");
    const sessionId = this.sessionId;
    if (!sessionId) throw new Error("session id required");
    const jobId = await this.store.submit(
      sessionId,
      buildPrompt(),
      bytesToBase64(this.photoBytes),
    );
    this.currentJobId = jobId;
    this.step = "generating";
    return jobId;
  }

  async poll(): Promise<CreationSnapshot> {
    const sessionId = this.sessionId;
    if (!sessionId) throw new Error("session id required");
    const snapshot = await this.store.snapshot(sessionId);
    this.restore(snapshot);
    return snapshot;
  }

  async finish(displayName: string): Promise<PetSwitchResult> {
    const sessionId = this.sessionId;
    if (!sessionId) throw new Error("session id required");
    if (!displayName.trim()) throw new Error("请输入宠物名称");
    const saved = await this.store.setName(sessionId, displayName);
    this.restore(saved);
    this.step = "finalizing";
    let result: PetSwitchResult;
    try {
      result = await this.store.finalize(sessionId);
    } catch (error) {
      this.restore(saved);
      throw error;
    }
    if (!result.ok) {
      this.restore(saved);
      return result;
    }
    this.activationWarning = result.warning ?? null;
    this.step = "complete";
    return result;
  }

  async abandon(): Promise<void> {
    const sessionId = this.sessionId;
    if (!sessionId) return;
    await this.store.abandon(sessionId);
  }
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer);
  return Array.from(
    new Uint8Array(digest),
    (value) => value.toString(16).padStart(2, "0"),
  ).join("");
}

export function buildPrompt(): string {
  return (
    "Create a cute chibi cartoon style with a round face, unified soft outlines, "
    + "big expressive eyes, short rounded body, sitting upright. Subject: a cat. "
    + "Front view, facing the viewer directly, full body visible, "
    + "plain uniform light grey background, no text, no watermark. "
    + "High fidelity to the reference: keep the exact fur colors, markings, ear shape, "
    + "eye color and face proportions so the owner can recognise the pet. "
    + "Faithful face details: keep eye shape, eye colour and highlights, nose, whiskers, "
    + "mouth and face markings; calm natural expression."
  );
}

function stepFromSnapshot(snapshot: CreationSnapshot): CreationStep {
  if (snapshot.status === "completed") return "complete";
  if (snapshot.status === "finalizing") return "finalizing";
  if (
    snapshot.status === "candidateReady"
    || snapshot.lastStableStatus === "candidateReady"
    || snapshot.currentStep === "review"
  ) return "review";
  return snapshot.currentStep === "generating" ? "generating" : "upload";
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
