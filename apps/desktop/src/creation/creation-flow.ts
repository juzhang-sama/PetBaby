import { invoke } from "@tauri-apps/api/core";
import type { CreationResume } from "../pets/pet-catalog-contract";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { requestPetSwitch } from "../settings/pet-switch-client";

export type CreationStep = "upload" | "generating" | "review" | "confirm" | "complete";

export interface CreationStore {
  genStart(petId: string, prompt: string, refPngB64: string): Promise<string>;
  resume(petId: string): Promise<CreationResume>;
  compile(petId: string, variantId: string): Promise<{ manifestPath: string; degraded: boolean }>;
  switchPet(petId: string, acceptedVariantId: string): Promise<PetSwitchResult>;
}

class TauriStore implements CreationStore {
  async genStart(petId: string, prompt: string, refPngB64: string): Promise<string> {
    return invoke("gen_start", {
      petId,
      prompt,
      refPngB64,
      refSha256: "sha-placeholder",
    });
  }

  async resume(petId: string): Promise<CreationResume> {
    return invoke("pet_creation_resume", { petId });
  }

  async compile(petId: string, variantId: string): Promise<{ manifestPath: string; degraded: boolean }> {
    const cutoutPath = await invoke<string>("gen_cutout_path", { jobId: variantId });
    return invoke("asset_compile", { petId, variantId, cutoutPath });
  }

  switchPet(petId: string, acceptedVariantId: string): Promise<PetSwitchResult> {
    return requestPetSwitch(petId, { acceptedVariantId });
  }
}

export class CreationFlow {
  step: CreationStep = "upload";
  activationWarning: string | null = null;
  private species: "cat" | "dog" = "cat";
  private currentPetId: string | null = null;
  private photoBytes: Uint8Array | null = null;
  private currentJobId: string | null = null;
  private currentVariantId: string | null = null;
  private readonly store: CreationStore;

  constructor(store?: CreationStore) {
    this.store = store ?? new TauriStore();
  }

  get petId(): string | null {
    return this.currentPetId;
  }

  get jobId(): string | null {
    return this.currentJobId;
  }

  get variantId(): string | null {
    return this.currentVariantId;
  }

  setPetId(petId: string): void {
    this.currentPetId = petId;
  }

  setSpecies(species: "cat" | "dog"): void {
    this.species = species;
  }

  setPhotoBytes(bytes: Uint8Array): void {
    this.photoBytes = bytes;
  }

  clearPhoto(): void {
    this.photoBytes = null;
  }

  restore(snapshot: CreationResume): void {
    this.currentPetId = snapshot.petId;
    this.currentJobId = snapshot.jobId;
    this.currentVariantId = snapshot.variantId;
    switch (snapshot.status) {
      case "generating":
        this.step = "generating";
        break;
      case "generationFailed":
        this.step = "upload";
        break;
      case "awaitingConfirm":
      case "compileRetryable":
        this.step = "review";
        break;
      case "awaitingActivation":
        this.step = "confirm";
        break;
      case "ready":
        this.step = "complete";
        break;
      case "corrupt":
        throw new Error("corrupt pet is not resumable");
    }
  }

  async submitSingle(): Promise<void> {
    if (!this.photoBytes) throw new Error("photo required");
    if (!this.currentPetId) throw new Error("pet id required");
    this.currentJobId = await this.store.genStart(
      this.currentPetId,
      buildPrompt(this.species),
      bytesToBase64(this.photoBytes),
    );
    this.currentVariantId = this.currentJobId;
    this.step = "generating";
  }

  async poll(): Promise<CreationResume> {
    if (!this.currentPetId) throw new Error("pet id required");
    const snapshot = await this.store.resume(this.currentPetId);
    this.restore(snapshot);
    return snapshot;
  }

  async compileCandidate(): Promise<{ manifestPath: string; degraded: boolean }> {
    if (!this.currentPetId || !this.currentVariantId) throw new Error("candidate required");
    try {
      const result = await this.store.compile(this.currentPetId, this.currentVariantId);
      this.step = "confirm";
      return result;
    } catch (error) {
      this.step = "review";
      throw error;
    }
  }

  async activateCandidate(): Promise<string | undefined> {
    if (!this.currentPetId || !this.currentVariantId) throw new Error("candidate required");
    const result = await this.store.switchPet(this.currentPetId, this.currentVariantId);
    if (!result.ok) throw new Error(`${result.code}: ${result.message}`);
    this.activationWarning = result.warning ?? null;
    this.step = "complete";
    return result.warning;
  }
}

export function buildPrompt(species: string): string {
  return (
    `Create a cute chibi cartoon style with a round face, unified soft outlines, ` +
    `big expressive eyes, short rounded body, sitting upright. Subject: a ${species}. ` +
    `Front view, facing the viewer directly, full body visible, ` +
    `plain uniform light grey background, no text, no watermark. ` +
    `High fidelity to the reference: keep the exact fur colors, markings, ear shape, ` +
    `eye color and face proportions so the owner can recognise the pet. ` +
    `Faithful face details: keep eye shape, eye colour and highlights, nose, whiskers, ` +
    `mouth and face markings; calm natural expression.`
  );
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
