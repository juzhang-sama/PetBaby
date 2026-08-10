import { existsSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

const hasLocalCubismSdk = existsSync(new URL(
  "../../.vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework.ts",
  import.meta.url,
));
const cubismShaderModuleSpecifier = "@cubism-framework/rendering/cubismshader_webgl";

function createCanvas(gl: WebGLRenderingContext): HTMLCanvasElement {
  return {
    getContext: vi.fn(() => gl),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  } as unknown as HTMLCanvasElement;
}

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
    const canvas = createCanvas({} as WebGLRenderingContext);

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
    const { CubismShaderManager_WebGL } = await import(
      /* @vite-ignore */ cubismShaderModuleSpecifier
    );
    const firstGl = {} as WebGLRenderingContext;
    const secondGl = {} as WebGLRenderingContext;
    const first = createCubismAdapter();
    const second = createCubismAdapter();
    await first.initialize(createCanvas(firstGl));
    await second.initialize(createCanvas(secondGl));
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
    const { CubismShaderManager_WebGL } = await import(
      /* @vite-ignore */ cubismShaderModuleSpecifier
    );
    const gl = { isContextLost: vi.fn(() => false) } as unknown as WebGLRenderingContext;
    const canvas = createCanvas(gl);
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

  it.skipIf(!hasLocalCubismSdk)("invalidates shaders on context loss and removes its listener on destroy", async () => {
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
    const { CubismShaderManager_WebGL } = await import(
      /* @vite-ignore */ cubismShaderModuleSpecifier
    );
    const gl = { isContextLost: vi.fn(() => false) } as unknown as WebGLRenderingContext;
    const listeners = new Map<string, EventListenerOrEventListenerObject>();
    const canvas = {
      getContext: vi.fn(() => gl),
      addEventListener: vi.fn((type: string, listener: EventListenerOrEventListenerObject) => {
        listeners.set(type, listener);
      }),
      removeEventListener: vi.fn((type: string, listener: EventListenerOrEventListenerObject) => {
        if (listeners.get(type) === listener) listeners.delete(type);
      }),
    } as unknown as HTMLCanvasElement;
    const adapter = createCubismAdapter();
    await adapter.initialize(canvas);
    const shaders = CubismShaderManager_WebGL.getInstance();
    const shaderMap = (shaders as unknown as {
      _shaderMap: Map<WebGLRenderingContext, { release(): void }>;
    })._shaderMap;
    const invalidShader = { release: vi.fn() };
    shaderMap.set(gl, invalidShader);

    const listener = listeners.get("webglcontextlost");
    expect(listener).toBeDefined();
    const event = { preventDefault: vi.fn() } as unknown as Event;
    if (typeof listener === "function") listener(event);
    else listener?.handleEvent(event);

    expect(invalidShader.release).not.toHaveBeenCalled();
    expect(shaders.getShader(gl)).toBeUndefined();

    const replacement = { release: vi.fn() };
    shaderMap.set(gl, replacement);
    adapter.destroy();

    expect(replacement.release).toHaveBeenCalledOnce();
    expect(canvas.removeEventListener).toHaveBeenCalledWith("webglcontextlost", listener);
    expect(listeners.has("webglcontextlost")).toBe(false);
  });
});
