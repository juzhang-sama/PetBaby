import { describe, expect, it } from "vitest";
import type { CreationSnapshot } from "./contracts";
import {
  CreationFlow,
  sha256Hex,
  type UploadCreationStore,
} from "./creation-flow";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";

function creationSnapshot(overrides: Partial<CreationSnapshot> = {}): CreationSnapshot {
  return {
    sessionId: "session-1",
    petId: "pet-1",
    method: "upload",
    status: "draft",
    lastStableStatus: "draft",
    currentStep: "upload",
    displayName: null,
    jobId: null,
    jobStatus: null,
    candidateId: null,
    recipe: null,
    error: null,
    ...overrides,
  };
}

class FakeStore implements UploadCreationStore {
  started = 0;
  submitted: Array<{ sessionId: string; prompt: string; refPngB64: string }> = [];
  names: Array<{ sessionId: string; displayName: string }> = [];
  finalized: string[] = [];
  abandoned: string[] = [];
  current = creationSnapshot();
  finalResult: PetSwitchResult = { ok: true, requestId: "request-1", petId: "pet-1" };

  async start(): Promise<CreationSnapshot> {
    this.started += 1;
    return this.current;
  }

  async submit(sessionId: string, prompt: string, refPngB64: string): Promise<string> {
    this.submitted.push({ sessionId, prompt, refPngB64 });
    return "job-1";
  }

  async snapshot(): Promise<CreationSnapshot> {
    return this.current;
  }

  async setName(sessionId: string, displayName: string): Promise<CreationSnapshot> {
    this.names.push({ sessionId, displayName });
    this.current = { ...this.current, displayName: displayName.trim() };
    return this.current;
  }

  async finalize(sessionId: string): Promise<PetSwitchResult> {
    this.finalized.push(sessionId);
    return this.finalResult;
  }

  async abandon(sessionId: string): Promise<void> {
    this.abandoned.push(sessionId);
  }
}

describe("CreationFlow", () => {
  it("uses the durable session id as the root identity", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);

    await flow.start();
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await flow.submitSingle();

    expect(flow.sessionId).toBe("session-1");
    expect(store.submitted).toEqual([expect.objectContaining({ sessionId: "session-1" })]);
    expect(flow.step).toBe("generating");
  });

  it("restores only an upload session snapshot", () => {
    const flow = new CreationFlow(new FakeStore());
    flow.restore(creationSnapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      jobId: "job-1",
      candidateId: "job-1",
    }));

    expect(flow.step).toBe("review");
    expect(flow.jobId).toBe("job-1");
    expect(() => flow.restore(creationSnapshot({ method: "composer" }))).toThrow("上传");
  });

  it("saves the name through the backend before finalizing", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.restore(creationSnapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      jobId: "job-1",
      candidateId: "job-1",
    }));

    await expect(flow.finish("  团子  ")).resolves.toMatchObject({ ok: true });

    expect(store.names).toEqual([{ sessionId: "session-1", displayName: "  团子  " }]);
    expect(store.finalized).toEqual(["session-1"]);
    expect(flow.displayName).toBe("团子");
    expect(flow.step).toBe("complete");
  });

  it("does not complete when the finalizer returns a failure", async () => {
    const store = new FakeStore();
    store.finalResult = {
      ok: false,
      requestId: "request-1",
      petId: "pet-1",
      code: "blank-frame",
      message: "首帧为空",
    };
    const flow = new CreationFlow(store);
    store.current = creationSnapshot({
      status: "candidateReady",
      lastStableStatus: "candidateReady",
      currentStep: "review",
      jobId: "job-1",
      candidateId: "job-1",
    });
    flow.restore(store.current);

    await expect(flow.finish("团子")).resolves.toMatchObject({ ok: false });

    expect(flow.step).toBe("review");
  });

  it("computes the real SHA-256 digest", async () => {
    const bytes = new TextEncoder().encode("abc");
    await expect(sha256Hex(bytes)).resolves.toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });
});
