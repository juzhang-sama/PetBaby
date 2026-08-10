import { describe, expect, it, vi } from "vitest";
import {
  BUILTIN_LIVE2D_PET,
  createBuiltinPetTransport,
  selectStartupPetSource,
} from "./startup-pet";

describe("startup pet source", () => {
  it("uses the approved built-in Live2D pet when no installed pet is active", () => {
    expect(selectStartupPetSource(null)).toEqual({
      kind: "builtin",
      ...BUILTIN_LIVE2D_PET,
    });
  });

  it("keeps an explicitly active installed pet", () => {
    expect(selectStartupPetSource("pet-user-1")).toEqual({
      kind: "installed",
      petId: "pet-user-1",
    });
  });

  it("treats the persisted built-in id as a built-in source", () => {
    expect(selectStartupPetSource("pet-live2d-v1")).toMatchObject({
      kind: "builtin",
      petId: "pet-live2d-v1",
    });
  });
});

describe("built-in pet transport", () => {
  it("reads the manifest and files from the built-in package directory", async () => {
    const manifest = { schemaVersion: 2 };
    const texture = Uint8Array.from([1, 2, 3]);
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("manifest.json")) {
        return new Response(JSON.stringify(manifest), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      return new Response(texture, { status: 200 });
    });
    const transport = createBuiltinPetTransport({
      manifestUrl: "/builtin-pets/pet-live2d-v1/manifest.json",
      origin: "http://127.0.0.1:1420",
      fetcher,
    });

    await expect(transport.readManifest("pet-live2d-v1")).resolves.toEqual(manifest);
    await expect(transport.readFile(
      "pet-live2d-v1",
      "pet-live2d-v1.2048/texture_00.png",
    )).resolves.toEqual(texture);
    expect(fetcher).toHaveBeenNthCalledWith(
      1,
      "http://127.0.0.1:1420/builtin-pets/pet-live2d-v1/manifest.json",
    );
    expect(fetcher).toHaveBeenNthCalledWith(
      2,
      "http://127.0.0.1:1420/builtin-pets/pet-live2d-v1/pet-live2d-v1.2048/texture_00.png",
    );
  });

  it("reports a missing built-in resource instead of returning corrupt bytes", async () => {
    const transport = createBuiltinPetTransport({
      manifestUrl: "/builtin-pets/pet-live2d-v1/manifest.json",
      origin: "http://127.0.0.1:1420",
      fetcher: vi.fn(async () => new Response("missing", { status: 404 })),
    });

    await expect(transport.readFile("pet-live2d-v1", "preview.png"))
      .rejects.toThrow("built-in pet resource failed (404)");
  });
});
