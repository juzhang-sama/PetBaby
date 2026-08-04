import { invoke } from "@tauri-apps/api/core";

export type CreationStep = "upload" | "traits" | "generating" | "review" | "confirm";

export interface JobUpdate {
  jobId: string;
  status: string;
  error: string | null;
}

export interface CreationStore {
  genStart(petId: string, prompt: string, refPngB64: string): Promise<string>;
  genList(petId: string): Promise<JobUpdate[]>;
  accept(variantId: string): Promise<void>;
  compile(petId: string): Promise<{ manifestPath: string; degraded: boolean }>;
}

class TauriStore implements CreationStore {
  private readonly petId: string;

  constructor(petId: string) {
    this.petId = petId;
  }

  async genStart(_petId: string, prompt: string, refPngB64: string): Promise<string> {
    return invoke("gen_start", {
      petId: this.petId,
      prompt,
      refPngB64,
      refSha256: "sha-placeholder",
    });
  }

  async genList(_petId: string): Promise<JobUpdate[]> {
    const jobs = await invoke<Array<Record<string, unknown>>>("gen_list", {
      petId: this.petId,
    });
    return jobs.map((job) => ({
      jobId: String(job.jobId),
      status: String(job.status),
      error: job.error ? String(job.error) : null,
    }));
  }

  async accept(_variantId: string): Promise<void> {
    // variant acceptance is recorded during compile in M4 Task 6
  }

  async compile(_petId: string): Promise<{ manifestPath: string; degraded: boolean }> {
    return invoke("asset_compile", { petId: this.petId });
  }
}

export class CreationFlow {
  step: CreationStep = "upload";
  private species = "cat";
  private photoBytes: Uint8Array | null = null;
  private jobIds: string[] = [];
  private selectedVariant: string | null = null;
  private compiled = false;
  private readonly store: CreationStore;

  constructor(store?: CreationStore) {
    this.store = store ?? new TauriStore("pet-1");
  }

  setSpecies(species: "cat" | "dog"): void {
    this.species = species;
  }

  setPhotoBytes(bytes: Uint8Array): void {
    this.photoBytes = bytes;
  }

  advance(): void {
    const order: CreationStep[] = ["upload", "traits", "generating", "review", "confirm"];
    const index = order.indexOf(this.step);
    if (index >= 0 && index < order.length - 1) {
      this.step = order[index + 1]!;
    }
  }

  async submitBatch(count: number): Promise<void> {
    if (!this.photoBytes) throw new Error("photo required");
    const b64 = bytesToBase64(this.photoBytes);
    const prompt = buildPrompt(this.species);
    for (let i = 0; i < count; i += 1) {
      const jobId = await this.store.genStart("pet-1", prompt, b64);
      this.jobIds.push(jobId);
    }
    this.step = "generating";
  }

  async poll(): Promise<boolean> {
    const jobs = await this.store.genList("pet-1");
    const pending = jobs.filter((job) => job.status === "pending" || job.status === "running");
    if (pending.length === 0 && jobs.length > 0) {
      this.step = "review";
      return true;
    }
    return false;
  }

  accept(variantId: string): void {
    this.selectedVariant = variantId;
    void this.store.accept(variantId);
  }

  async compile(): Promise<{ manifestPath: string; degraded: boolean }> {
    if (!this.selectedVariant) throw new Error("no variant selected");
    const result = await this.store.compile("pet-1");
    this.compiled = true;
    this.step = "confirm";
    return result;
  }

  get isCompiled(): boolean {
    return this.compiled;
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
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}
