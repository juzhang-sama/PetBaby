import { describe, expect, it, vi } from "vitest";
import { createConfiguredCubismAdapter } from "./cubism-adapter";

describe("configured Cubism adapter", () => {
  it("loads Cubism Core before evaluating the Framework runtime", async () => {
    const calls: string[] = [];
    const adapter = { initialize: vi.fn(), loadModel: vi.fn(), resize: vi.fn(), update: vi.fn(), draw: vi.fn(), destroy: vi.fn() };

    const result = await createConfiguredCubismAdapter({
      loadCore: async () => { calls.push("core"); },
      loadRuntime: async () => {
        calls.push("framework");
        return { createCubismAdapter: () => adapter };
      },
    });

    expect(calls).toEqual(["core", "framework"]);
    expect(result).toBe(adapter);
  });
});
