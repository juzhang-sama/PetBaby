import { existsSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

const hasLocalCubismSdk = existsSync(new URL(
  "../../.vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework.ts",
  import.meta.url,
));

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

describe("official Cubism runtime lifecycle", () => {
  it.skipIf(!hasLocalCubismSdk)("initializes again after the previous adapter is destroyed", async () => {
    let loggingRegistrations = 0;
    const initializeAmountOfMemory = vi.fn();
    vi.stubGlobal("Live2DCubismCore", {
      Logging: {
        csmSetLogFunction: vi.fn(() => {
          loggingRegistrations += 1;
          if (loggingRegistrations > 1) {
            throw new Error(
              "Unable to grow wasm table. Use a higher value for RESERVED_FUNCTION_POINTERS or set ALLOW_TABLE_GROWTH.",
            );
          }
        }),
        csmGetLogFunction: vi.fn(() => undefined),
      },
      Memory: {
        initializeAmountOfMemory,
      },
      Version: {
        csmGetVersion: vi.fn(() => 0x05000000),
      },
    });

    const { createCubismAdapter } = await import("@cubism-runtime");
    const canvas = {
      getContext: vi.fn(() => ({})),
    } as unknown as HTMLCanvasElement;

    const first = createCubismAdapter();
    await first.initialize(canvas);
    first.destroy();

    const second = createCubismAdapter();
    await expect(second.initialize(canvas)).resolves.toBeUndefined();
    second.destroy();

    expect(loggingRegistrations).toBe(1);
    expect(initializeAmountOfMemory).toHaveBeenCalledTimes(2);
  });
});
