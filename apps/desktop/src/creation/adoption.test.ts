import { describe, expect, it } from "vitest";
import { loadBuiltinPets, loadBuiltinPng } from "./adoption";

describe("loadBuiltinPets", () => {
  it("loads and parses the built-in pet list", async () => {
    const client = async () => new Response(JSON.stringify({
      pets: [
        { id: "cat-1", name: "奶油橘猫", species: "cat", preview: "/builtin-pets/cat-1.png" },
      ],
    }), { status: 200 });
    const pets = await loadBuiltinPets(client as unknown as typeof fetch);
    expect(pets).toHaveLength(1);
    expect(pets[0]?.id).toBe("cat-1");
    expect(pets[0]?.species).toBe("cat");
  });

  it("throws when the list cannot be loaded", async () => {
    const client = async () => new Response("{}", { status: 404 });
    await expect(loadBuiltinPets(client as unknown as typeof fetch)).rejects.toThrow(/404/);
  });
});

describe("loadBuiltinPng", () => {
  it("loads a built-in png as bytes", async () => {
    const client = async () => new Response(new Blob([new Uint8Array([1, 2, 3])]));
    const bytes = await loadBuiltinPng("/builtin-pets/cat-1.png", client as unknown as typeof fetch);
    expect(Array.from(bytes)).toEqual([1, 2, 3]);
  });
});
