import { describe, expect, it } from "vitest";
import { CreationFlow, type CreationStore } from "./creation-flow";
import type { CreationResume } from "../pets/pet-catalog-contract";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";

class FakeStore implements CreationStore {
  started: Array<{ petId: string; prompt: string; refPngB64: string }> = [];
  snapshot: CreationResume = {
    petId: "pet-1", status: "generating", jobId: "job-1", variantId: "job-1", error: null,
  };
  switchResult: PetSwitchResult = { ok: true, requestId: "request-1", petId: "pet-1" };
  compiled: Array<{ petId: string; variantId: string }> = [];
  switched: Array<{ petId: string; variantId: string }> = [];

  constructor(options?: { switchResult?: PetSwitchResult }) {
    if (options?.switchResult) this.switchResult = options.switchResult;
  }

  async genStart(petId: string, prompt: string, refPngB64: string): Promise<string> {
    this.started.push({ petId, prompt, refPngB64 });
    return `job-${this.started.length}`;
  }

  async resume(): Promise<CreationResume> {
    return this.snapshot;
  }

  async compile(petId: string, variantId: string): Promise<{ manifestPath: string; degraded: boolean }> {
    this.compiled.push({ petId, variantId });
    return { manifestPath: "/tmp/manifest.json", degraded: false };
  }

  async switchPet(petId: string, acceptedVariantId: string): Promise<PetSwitchResult> {
    this.switched.push({ petId, variantId: acceptedVariantId });
    return this.switchResult;
  }
}

function restoredAwaitingActivation(store = new FakeStore()): CreationFlow {
  const flow = new CreationFlow(store);
  flow.restore({
    petId: "pet-1", status: "awaitingActivation", jobId: "job-1", variantId: "job-1", error: null,
  });
  return flow;
}

describe("CreationFlow", () => {
  it("submits exactly one candidate job", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.setPetId("pet-1");
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await flow.submitSingle();
    expect(store.started).toHaveLength(1);
    expect(flow.step).toBe("generating");
    expect(flow.variantId).toBe("job-1");
  });

  it("requires a pet created or restored by the caller before submission", async () => {
    const flow = new CreationFlow(new FakeStore());
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await expect(flow.submitSingle()).rejects.toThrow("pet id required");
  });

  it.each([
    ["generating", "generating"],
    ["generationFailed", "upload"],
    ["awaitingConfirm", "review"],
    ["compileRetryable", "review"],
    ["awaitingActivation", "confirm"],
  ] as const)("restores %s to %s", (status, step) => {
    const flow = new CreationFlow(new FakeStore());
    flow.restore({ petId: "pet-1", status, jobId: "job-1", variantId: "job-1", error: null });
    expect(flow.step).toBe(step);
    expect(flow.petId).toBe("pet-1");
  });

  it("restores ready creation as complete", () => {
    const flow = new CreationFlow(new FakeStore());
    flow.restore({ petId: "pet-1", status: "ready", jobId: "job-1", variantId: "job-1", error: null });
    expect(flow.step).toBe("complete");
  });

  it("refuses to resume a corrupt creation", () => {
    const flow = new CreationFlow(new FakeStore());
    expect(() => flow.restore({
      petId: "pet-1", status: "corrupt", jobId: null, variantId: null, error: "manifest damaged",
    })).toThrow("corrupt pet is not resumable");
  });

  it("uses the latest resume snapshot when polling", async () => {
    const store = new FakeStore();
    store.snapshot = { petId: "pet-1", status: "awaitingConfirm", jobId: "job-1", variantId: "job-1", error: null };
    const flow = new CreationFlow(store);
    flow.restore({ petId: "pet-1", status: "generating", jobId: "job-1", variantId: "job-1", error: null });
    await flow.poll();
    expect(flow.step).toBe("review");
  });

  it("keeps review available when compilation fails", async () => {
    const store = new FakeStore();
    store.compile = async () => { throw new Error("compiler unavailable"); };
    const flow = new CreationFlow(store);
    flow.restore({ petId: "pet-1", status: "awaitingConfirm", jobId: "job-1", variantId: "job-1", error: null });
    await expect(flow.compileCandidate()).rejects.toThrow("compiler unavailable");
    expect(flow.step).toBe("review");
  });

  it("only completes after a successful desktop switch", async () => {
    const store = new FakeStore();
    const flow = restoredAwaitingActivation(store);
    await flow.activateCandidate();
    expect(store.switched).toEqual([{ petId: "pet-1", variantId: "job-1" }]);
    expect(flow.step).toBe("complete");
  });

  it("compiles and activates the accepted candidate with its original pet and variant ids", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.restore({
      petId: "pet-original",
      status: "awaitingConfirm",
      jobId: "job-original",
      variantId: "variant-original",
      error: null,
    });

    await flow.compileCandidate();
    await flow.activateCandidate();

    expect(store.compiled).toEqual([{ petId: "pet-original", variantId: "variant-original" }]);
    expect(store.switched).toEqual([{ petId: "pet-original", variantId: "variant-original" }]);
  });

  it("does not finish when desktop switching fails", async () => {
    const store = new FakeStore({ switchResult: {
      ok: false, requestId: "request-1", petId: "pet-1", code: "blank-frame", message: "first frame empty",
    } });
    const flow = restoredAwaitingActivation(store);
    await expect(flow.activateCandidate()).rejects.toThrow("blank-frame");
    expect(flow.step).toBe("confirm");
  });
});
