import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { createServer } from "vite";

describe("Cubism runtime lifecycle test portability", () => {
  it("transforms without a local Cubism Framework alias", async () => {
    const root = fileURLToPath(new URL("../../", import.meta.url));
    const unavailableRuntime = fileURLToPath(new URL("./cubism-runtime-unavailable.ts", import.meta.url));
    const server = await createServer({
      root,
      configFile: false,
      logLevel: "silent",
      appType: "custom",
      server: { middlewareMode: true },
      resolve: { alias: { "@cubism-runtime": unavailableRuntime } },
    });

    try {
      const result = await server.transformRequest(
        "/src/runtime-live2d/cubism-runtime-lifecycle.test.ts",
        { ssr: true },
      );
      expect(result).not.toBeNull();
      expect(result?.code).not.toContain(
        '__vite_ssr_dynamic_import__("@cubism-framework/rendering/cubismshader_webgl")',
      );
    } finally {
      await server.close();
    }
  });
});
