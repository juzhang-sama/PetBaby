import { describe, expect, it, vi } from "vitest";
import { createCreationApi } from "./api";

describe("creationApi", () => {
  it("invokes creation_start with the selected method", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.start("upload");

    expect(invoke).toHaveBeenCalledWith("creation_start", { method: "upload" });
  });

  it("invokes creation_draft without an args object", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const api = createCreationApi(invoke);

    await api.draft();

    expect(invoke).toHaveBeenCalledWith("creation_draft");
  });

  it("invokes creation_snapshot with a camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.snapshot("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_snapshot", {
      sessionId: "session-1",
    });
  });

  it("invokes creation_set_name with camelCase arguments", async () => {
    const invoke = vi.fn().mockResolvedValue({ displayName: "团子" });
    const api = createCreationApi(invoke);

    await api.setName("session-1", "团子");

    expect(invoke).toHaveBeenCalledWith("creation_set_name", {
      sessionId: "session-1",
      displayName: "团子",
    });
  });

  it("invokes creation_abandon with a camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const api = createCreationApi(invoke);

    await api.abandon("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_abandon", {
      sessionId: "session-1",
    });
  });

  it("starts upload generation with camelCase session ownership arguments", async () => {
    const invoke = vi.fn().mockResolvedValue("job-1");
    const api = createCreationApi(invoke);

    await api.uploadStart("session-1", "fluffy cat", "cG5n", "sha-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_start", {
      sessionId: "session-1",
      prompt: "fluffy cat",
      refPngB64: "cG5n",
      refSha256: "sha-1",
    });
  });

  it("lists upload jobs by camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue([]);
    const api = createCreationApi(invoke);

    await api.uploadJobs("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_jobs", {
      sessionId: "session-1",
    });
  });

  it("loads a durable upload source by camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const api = createCreationApi(invoke);

    await api.uploadSource("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_source", {
      sessionId: "session-1",
    });
  });
});
