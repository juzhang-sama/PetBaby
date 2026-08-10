import { existsSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

const hasLocalCubismSdk = existsSync(new URL(
  "../../.vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework.ts",
  import.meta.url,
));

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
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

  it.skipIf(!hasLocalCubismSdk)("releases only the destroyed adapter's WebGL shader context", async () => {
    vi.stubGlobal("Live2DCubismCore", {
      Logging: {
        csmSetLogFunction: vi.fn(),
        csmGetLogFunction: vi.fn(() => undefined),
      },
      Memory: {
        initializeAmountOfMemory: vi.fn(),
      },
      Version: {
        csmGetVersion: vi.fn(() => 0x05000000),
      },
    });

    const { createCubismAdapter } = await import("@cubism-runtime");
    const { CubismShaderManager_WebGL } = await import("@cubism-framework/rendering/cubismshader_webgl");
    const firstGl = {} as WebGLRenderingContext;
    const secondGl = {} as WebGLRenderingContext;
    const first = createCubismAdapter();
    const second = createCubismAdapter();
    await first.initialize({ getContext: vi.fn(() => firstGl) } as unknown as HTMLCanvasElement);
    await second.initialize({ getContext: vi.fn(() => secondGl) } as unknown as HTMLCanvasElement);
    const shaders = CubismShaderManager_WebGL.getInstance();
    const shaderMap = (shaders as unknown as {
      _shaderMap: Map<WebGLRenderingContext, { release(): void }>;
    })._shaderMap;
    const releaseFailure = new Error("shader release failed");
    const firstShader = { release: vi.fn(() => { throw releaseFailure; }) };
    const secondShader = { release: vi.fn() };
    shaderMap.set(firstGl, firstShader);
    shaderMap.set(secondGl, secondShader);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);

    expect(() => first.destroy()).not.toThrow();

    expect(shaders.getShader(firstGl)).toBeUndefined();
    expect(shaders.getShader(secondGl)).toBeDefined();
    expect(firstShader.release).toHaveBeenCalledOnce();
    expect(secondShader.release).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledWith("Cubism shader context 释放失败", releaseFailure);

    shaderMap.set(firstGl, { release: vi.fn() });
    expect(shaders.getShader(firstGl)).toBeDefined();
    second.destroy();
  });

  it.skipIf(!hasLocalCubismSdk)("keeps a shared WebGL context until the final stale adapter is destroyed", async () => {
    vi.stubGlobal("Live2DCubismCore", {
      Logging: {
        csmSetLogFunction: vi.fn(),
        csmGetLogFunction: vi.fn(() => undefined),
      },
      Memory: {
        initializeAmountOfMemory: vi.fn(),
      },
      Version: {
        csmGetVersion: vi.fn(() => 0x05000000),
      },
    });

    const { createCubismAdapter } = await import("@cubism-runtime");
    const { CubismShaderManager_WebGL } = await import("@cubism-framework/rendering/cubismshader_webgl");
    const gl = { isContextLost: vi.fn(() => false) } as unknown as WebGLRenderingContext;
    const canvas = { getContext: vi.fn(() => gl) } as unknown as HTMLCanvasElement;
    const first = createCubismAdapter();
    const second = createCubismAdapter();
    await first.initialize(canvas);
    await second.initialize(canvas);

    const shaders = CubismShaderManager_WebGL.getInstance();
    const shaderMap = (shaders as unknown as {
      _shaderMap: Map<WebGLRenderingContext, { release(): void }>;
    })._shaderMap;
    const events: string[] = [];
    shaderMap.set(gl, { release: () => { events.push("shader"); } });
    (first as unknown as { model: { releaseWithTextures(): void } }).model = {
      releaseWithTextures: () => {
        events.push("first-model");
        throw new Error("first model release failed");
      },
    };
    (second as unknown as { model: { releaseWithTextures(): void } }).model = {
      releaseWithTextures: () => { events.push("second-model"); },
    };
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(CubismShaderManager_WebGL, "deleteInstance").mockImplementation(() => {
      events.push("framework");
    });

    expect(() => first.destroy()).not.toThrow();
    expect(events).toEqual(["first-model"]);
    expect(shaders.getShader(gl)).toBeDefined();

    expect(() => second.destroy()).not.toThrow();
    expect(events).toEqual(["first-model", "second-model", "shader", "framework"]);
    expect(shaders.getShader(gl)).toBeUndefined();
  });
});
