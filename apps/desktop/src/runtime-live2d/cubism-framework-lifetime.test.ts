import { describe, expect, it, vi } from "vitest";
import {
  WebViewCubismFrameworkLifetime,
  type CubismFrameworkPort,
} from "./cubism-framework-lifetime";

type FrameworkOptions = { logFunction: () => void };

function createFramework(failures: { startup?: number; initialize?: number } = {}) {
  let started = false;
  let initialized = false;
  let startupFailures = failures.startup ?? 0;
  let initializeFailures = failures.initialize ?? 0;
  const framework: CubismFrameworkPort<FrameworkOptions> = {
    isStarted: () => started,
    startUp: vi.fn(() => {
      if (startupFailures > 0) {
        startupFailures -= 1;
        return false;
      }
      started = true;
      return true;
    }),
    isInitialized: () => initialized,
    initialize: vi.fn(() => {
      if (initializeFailures > 0) {
        initializeFailures -= 1;
        throw new Error("initialize failed");
      }
      initialized = true;
    }),
    dispose: vi.fn(() => {
      initialized = false;
    }),
  };
  return framework;
}

function createLifetime(framework: CubismFrameworkPort<FrameworkOptions>) {
  return new WebViewCubismFrameworkLifetime(framework, {
    logFunction: () => undefined,
  });
}

describe("WebViewCubismFrameworkLifetime", () => {
  it("disposes only after the final concurrent lease and reinitializes without restarting", () => {
    const framework = createFramework();
    const lifetime = createLifetime(framework);

    const first = lifetime.acquire();
    const second = lifetime.acquire();

    first.release();
    expect(framework.dispose).not.toHaveBeenCalled();
    second.release();
    expect(framework.dispose).toHaveBeenCalledOnce();

    const third = lifetime.acquire();
    expect(framework.startUp).toHaveBeenCalledOnce();
    expect(framework.initialize).toHaveBeenCalledTimes(2);
    third.release();
    expect(framework.dispose).toHaveBeenCalledTimes(2);
  });

  it("makes repeated lease release idempotent", () => {
    const framework = createFramework();
    const lease = createLifetime(framework).acquire();

    lease.release();
    lease.release();

    expect(framework.dispose).toHaveBeenCalledOnce();
  });

  it("does not retain a lease when Framework startup fails", () => {
    const framework = createFramework({ startup: 1 });
    const lifetime = createLifetime(framework);

    expect(() => lifetime.acquire()).toThrow("Cubism Framework 启动失败");
    lifetime.acquire().release();

    expect(framework.startUp).toHaveBeenCalledTimes(2);
    expect(framework.initialize).toHaveBeenCalledOnce();
    expect(framework.dispose).toHaveBeenCalledOnce();
  });

  it("does not retain a lease when Framework initialization fails", () => {
    const framework = createFramework({ initialize: 1 });
    const lifetime = createLifetime(framework);

    expect(() => lifetime.acquire()).toThrow("initialize failed");
    lifetime.acquire().release();

    expect(framework.startUp).toHaveBeenCalledOnce();
    expect(framework.initialize).toHaveBeenCalledTimes(2);
    expect(framework.dispose).toHaveBeenCalledOnce();
  });
});
