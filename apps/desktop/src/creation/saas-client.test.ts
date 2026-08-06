import { describe, expect, it } from "vitest";
import { SaasClient, sniffImageMime } from "./saas-client";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function bytesResponse(bytes: Uint8Array, status = 200): Response {
  return new Response(new Blob([new Uint8Array(bytes)]), { status });
}

describe("sniffImageMime", () => {
  it("detects PNG and JPEG magic bytes", () => {
    expect(sniffImageMime(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]))).toBe("image/png");
    expect(sniffImageMime(new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBe("image/jpeg");
  });

  it("returns null for unknown bytes", () => {
    expect(sniffImageMime(new Uint8Array([1, 2, 3]))).toBeNull();
  });
});

describe("SaasClient", () => {
  it("creates a generation with multipart photo and species", async () => {
    let capturedUrl = "";
    let capturedBody: FormData | undefined;
    const client = new SaasClient("http://x.test", async (url, init) => {
      capturedUrl = String(url);
      capturedBody = init?.body as FormData;
      return jsonResponse({ jobId: "job-1", status: "queued" }, 202);
    });

    const photo = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
    const result = await client.createGeneration(photo, "cat");

    expect(result).toEqual({ jobId: "job-1", status: "queued" });
    expect(capturedUrl).toBe("http://x.test/api/v1/generations");
    expect(capturedBody?.get("species")).toBe("cat");
    expect(capturedBody?.get("photos")).toBeInstanceOf(Blob);
    expect(capturedBody?.get("style")).toBe("cartoon");
    expect(capturedBody?.get("count")).toBe("1");
  });

  it("uploads up to three reference photos under one field", async () => {
    let capturedBody: FormData | undefined;
    const client = new SaasClient("http://x.test", async (_url, init) => {
      capturedBody = init?.body as FormData;
      return jsonResponse({ jobId: "job-3", status: "queued" }, 202);
    });
    const photo = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]);
    const result = await client.createGeneration([photo, photo, photo], "cat");
    expect(result.jobId).toBe("job-3");
    expect(capturedBody?.getAll("photos")).toHaveLength(3);
  });

  it("sends the requested 3D style", async () => {
    let capturedBody: FormData | undefined;
    const client = new SaasClient("http://x.test", async (_url, init) => {
      capturedBody = init?.body as FormData;
      return jsonResponse({ jobId: "job-4", status: "queued" }, 202);
    });
    await client.createGeneration(new Uint8Array([1]), "cat", undefined, "3d");
    expect(capturedBody?.get("style")).toBe("3d");
  });

  it("requests multiple candidates and keeps all job ids", async () => {
    let capturedBody: FormData | undefined;
    const client = new SaasClient("http://x.test", async (_url, init) => {
      capturedBody = init?.body as FormData;
      return jsonResponse(
        { jobIds: ["job-a", "job-b"], jobId: "job-a", status: "queued" },
        202,
      );
    });
    const result = await client.createGeneration(
      new Uint8Array([1]),
      "cat",
      undefined,
      "cartoon",
      2,
    );
    expect(capturedBody?.get("count")).toBe("2");
    expect(result.jobIds).toEqual(["job-a", "job-b"]);
  });

  it("throws when creation fails", async () => {
    const client = new SaasClient("http://x.test", async () => jsonResponse({ detail: "boom" }, 422));
    await expect(client.createGeneration(new Uint8Array([1]), "cat")).rejects.toThrow(/422/);
  });

  it("reports a friendly message when the backend is unreachable", async () => {
    const client = new SaasClient("http://x.test", async () => {
      throw new TypeError("Failed to fetch");
    });
    await expect(client.createGeneration(new Uint8Array([1]), "cat")).rejects.toThrow(
      /无法连接生成服务/,
    );
  });

  it("sends bearer token when configured", async () => {
    let capturedHeaders: HeadersInit | undefined;
    const client = new SaasClient(
      "http://x.test",
      async (_url, init) => {
        capturedHeaders = init?.headers;
        return jsonResponse({ jobId: "job-1", status: "queued" }, 202);
      },
      "secret",
    );
    await client.createGeneration(new Uint8Array([1]), "cat");
    expect(new Headers(capturedHeaders ?? {}).get("Authorization")).toBe("Bearer secret");
  });

  it("creates a guided generation without photo and with traits", async () => {
    let capturedBody: FormData | undefined;
    const client = new SaasClient("http://x.test", async (_url, init) => {
      capturedBody = init?.body as FormData;
      return jsonResponse({ jobId: "job-2", status: "queued" }, 202);
    });
    const result = await client.createGeneration(null, "cat", {
      body: "round",
      fur: "short",
      color: "orange",
      pattern: "striped",
      face: "round face with big eyes",
      accessory: "red bow",
    });
    expect(result.jobId).toBe("job-2");
    expect(capturedBody?.get("species")).toBe("cat");
    expect(capturedBody?.getAll("photos")).toHaveLength(0);
    expect(capturedBody?.get("traits")).toContain("orange");
  });

  it("calls fetch with the global receiver (no Illegal invocation)", async () => {
    function fetchLike(
      this: unknown,
      _input: RequestInfo | URL,
      _init?: RequestInit,
    ): Promise<Response> {
      if (this !== globalThis) {
        return Promise.reject(new TypeError("Illegal invocation"));
      }
      return Promise.resolve(jsonResponse({ jobId: "job-1", status: "queued" }, 202));
    }
    const client = new SaasClient("http://x.test", fetchLike as typeof fetch);
    const result = await client.createGeneration(new Uint8Array([1]), "cat");
    expect(result.jobId).toBe("job-1");
  });

  it("reads generation status", async () => {
    const client = new SaasClient("http://x.test", async (url) => {
      expect(String(url)).toContain("/api/v1/generations/job-1");
      return jsonResponse({
        jobId: "job-1",
        species: "cat",
        status: "completed",
        error: null,
        resultAvailable: true,
      });
    });

    const status = await client.getGeneration("job-1");
    expect(status.status).toBe("completed");
    expect(status.resultAvailable).toBe(true);
  });

  it("downloads the result as bytes", async () => {
    const client = new SaasClient("http://x.test", async () => bytesResponse(new Uint8Array([1, 2, 3])));
    const bytes = await client.downloadResult("job-1");
    expect(Array.from(bytes)).toEqual([1, 2, 3]);
  });

  it("deletes a generation", async () => {
    let method = "";
    const client = new SaasClient("http://x.test", async (_url, init) => {
      method = init?.method ?? "GET";
      return new Response(null, { status: 204 });
    });
    await client.deleteGeneration("job-1");
    expect(method).toBe("DELETE");
  });

  it("analyzes feature landmarks for a result image", async () => {
    let capturedUrl = "";
    const box = { x: 0.2, y: 0.3, width: 0.1, height: 0.08 };
    const landmarks = {
      leftEye: box,
      rightEye: box,
      leftEar: box,
      rightEar: box,
      tail: box,
    };
    const client = new SaasClient("http://x.test", async (url) => {
      capturedUrl = String(url);
      return jsonResponse({ landmarks }, 200);
    });
    const result = await client.analyzeLandmarks(new Uint8Array([1]), "cat");
    expect(result).toEqual(landmarks);
    expect(capturedUrl).toContain("/api/v1/landmarks");
  });

  it("returns null when landmarks are unavailable", async () => {
    const client = new SaasClient(
      "http://x.test",
      async () => jsonResponse({ landmarks: null }, 200),
    );
    expect(await client.analyzeLandmarks(new Uint8Array([1]), "cat")).toBeNull();
  });
});
