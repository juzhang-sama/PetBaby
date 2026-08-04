import { describe, expect, it } from "vitest";
import { CreationFlow } from "./creation-flow";
import type { JobUpdate } from "./creation-flow";

class FakeStore {
  started: Array<{ petId: string; prompt: string; refPngB64: string }> = [];
  jobs: JobUpdate[] = [];

  async genStart(petId: string, prompt: string, refPngB64: string): Promise<string> {
    this.started.push({ petId, prompt, refPngB64 });
    const jobId = `job-${this.started.length}`;
    this.jobs.push({ jobId, status: "pending", error: null });
    return jobId;
  }

  async genList(): Promise<JobUpdate[]> {
    return this.jobs;
  }

  async accept(_variantId: string): Promise<void> {}
  async compile(): Promise<{ manifestPath: string; degraded: boolean }> {
    return { manifestPath: "/tmp/manifest.json", degraded: false };
  }
}

describe("CreationFlow", () => {
  it("tracks steps through the creation wizard", () => {
    const flow = new CreationFlow(new FakeStore());
    expect(flow.step).toBe("upload");
    flow.setSpecies("cat");
    flow.advance();
    expect(flow.step).toBe("traits");
    flow.advance();
    expect(flow.step).toBe("generating");
  });

  it("starts four jobs for a batch", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.setSpecies("cat");
    flow.setPhotoBytes(new Uint8Array([0x89, 0x50, 0x4e, 0x47]));
    await flow.submitBatch(4);
    expect(store.started.length).toBe(4);
    expect(store.started[0]!.petId).toBe("pet-1");
    expect(flow.step).toBe("generating");
  });

  it("moves to review when all jobs finish", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.setSpecies("dog");
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await flow.submitBatch(2);
    store.jobs = store.jobs.map((job) => ({ ...job, status: "success" }));
    const done = await flow.poll();
    expect(done).toBe(true);
    expect(flow.step).toBe("review");
  });

  it("keeps generating while jobs are pending", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.setSpecies("cat");
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await flow.submitBatch(2);
    store.jobs[0] = { ...store.jobs[0]!, status: "running" };
    const done = await flow.poll();
    expect(done).toBe(false);
    expect(flow.step).toBe("generating");
  });

  it("accepts a candidate and compiles", async () => {
    const store = new FakeStore();
    const flow = new CreationFlow(store);
    flow.setSpecies("cat");
    flow.setPhotoBytes(new Uint8Array([1, 2, 3]));
    await flow.submitBatch(1);
    store.jobs[0] = { ...store.jobs[0]!, status: "success" };
    await flow.poll();
    flow.accept("variant-1");
    const result = await flow.compile();
    expect(result.manifestPath).toContain("manifest.json");
    expect(flow.step).toBe("confirm");
  });
});
