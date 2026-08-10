import { describe, expect, it, vi } from "vitest";
import {
  WebViewCubismFrameworkLifetime,
  type CubismFrameworkPort,
} from "./cubism-framework-lifetime";

describe("WebViewCubismFrameworkLifetime", () => {
  it("reuses one Framework startup after an adapter lease is released", () => {
    let started = false;
    let initialized = false;
    let loggingRegistrations = 0;
    const framework: CubismFrameworkPort<{ logFunction: () => void }> = {
      isStarted: () => started,
      startUp: vi.fn(() => {
        loggingRegistrations += 1;
        if (loggingRegistrations > 1) {
          throw new Error("Unable to grow wasm table");
        }
        started = true;
        return true;
      }),
      isInitialized: () => initialized,
      initialize: vi.fn(() => {
        initialized = true;
      }),
    };
    const lifetime = new WebViewCubismFrameworkLifetime(framework, {
      logFunction: () => undefined,
    });

    lifetime.acquire().release();
    lifetime.acquire().release();

    expect(framework.startUp).toHaveBeenCalledOnce();
    expect(framework.initialize).toHaveBeenCalledOnce();
    expect(loggingRegistrations).toBe(1);
  });
});
