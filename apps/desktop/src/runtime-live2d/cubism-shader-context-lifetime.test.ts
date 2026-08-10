import { describe, expect, it, vi } from "vitest";
import {
  WebViewCubismShaderContextLifetime,
  type CubismShaderContextManager,
} from "./cubism-shader-context-lifetime";

interface TestContext {
  isContextLost?(): boolean;
}

function createManager() {
  const shaders = new Map<TestContext, { release(): void }>();
  const manager: CubismShaderContextManager<TestContext> = {
    getShader: (context) => shaders.get(context),
    deleteShader: vi.fn((context) => shaders.delete(context)),
  };
  return { manager, shaders };
}

describe("WebViewCubismShaderContextLifetime", () => {
  it("releases a shared context only after its final owner and can acquire it again", () => {
    const { manager, shaders } = createManager();
    const context = {};
    const firstShader = { release: vi.fn() };
    shaders.set(context, firstShader);
    const lifetime = new WebViewCubismShaderContextLifetime(() => manager);
    const first = lifetime.acquire(context);
    const second = lifetime.acquire(context);

    first.release();
    expect(firstShader.release).not.toHaveBeenCalled();
    expect(shaders.get(context)).toBe(firstShader);

    second.release();
    second.release();
    expect(firstShader.release).toHaveBeenCalledOnce();
    expect(shaders.has(context)).toBe(false);

    const replacement = { release: vi.fn() };
    shaders.set(context, replacement);
    lifetime.acquire(context).release();
    expect(replacement.release).toHaveBeenCalledOnce();
    expect(shaders.has(context)).toBe(false);
  });

  it("tracks different contexts independently", () => {
    const { manager, shaders } = createManager();
    const firstContext = {};
    const secondContext = {};
    const firstShader = { release: vi.fn() };
    const secondShader = { release: vi.fn() };
    shaders.set(firstContext, firstShader);
    shaders.set(secondContext, secondShader);
    const lifetime = new WebViewCubismShaderContextLifetime(() => manager);
    const first = lifetime.acquire(firstContext);
    const second = lifetime.acquire(secondContext);

    first.release();
    expect(firstShader.release).toHaveBeenCalledOnce();
    expect(shaders.has(firstContext)).toBe(false);
    expect(secondShader.release).not.toHaveBeenCalled();
    expect(shaders.has(secondContext)).toBe(true);

    second.release();
    expect(secondShader.release).toHaveBeenCalledOnce();
    expect(shaders.has(secondContext)).toBe(false);
  });

  it("deletes the context registration when shader release fails", () => {
    const { manager, shaders } = createManager();
    const context = {};
    const failure = new Error("shader release failed");
    shaders.set(context, { release: () => { throw failure; } });
    const diagnose = vi.fn();
    const lease = new WebViewCubismShaderContextLifetime(() => manager, diagnose).acquire(context);

    expect(() => lease.release()).not.toThrow();

    expect(shaders.has(context)).toBe(false);
    expect(diagnose).toHaveBeenCalledWith(failure);
  });

  it("does not release shader programs from a lost context", () => {
    const { manager, shaders } = createManager();
    const context = { isContextLost: () => true };
    const shader = { release: vi.fn() };
    shaders.set(context, shader);

    new WebViewCubismShaderContextLifetime(() => manager).acquire(context).release();

    expect(shader.release).not.toHaveBeenCalled();
    expect(shaders.has(context)).toBe(false);
  });

  it("invalidates a shared context without changing its owner count", () => {
    const { manager, shaders } = createManager();
    const context = {};
    const invalidShader = { release: vi.fn() };
    shaders.set(context, invalidShader);
    const lifetime = new WebViewCubismShaderContextLifetime(() => manager);
    const first = lifetime.acquire(context);
    const second = lifetime.acquire(context);

    lifetime.invalidate(context);
    lifetime.invalidate(context);

    expect(invalidShader.release).not.toHaveBeenCalled();
    expect(shaders.has(context)).toBe(false);

    const replacement = { release: vi.fn() };
    shaders.set(context, replacement);
    first.release();
    expect(replacement.release).not.toHaveBeenCalled();
    expect(shaders.get(context)).toBe(replacement);

    second.release();
    expect(replacement.release).toHaveBeenCalledOnce();
    expect(shaders.has(context)).toBe(false);
  });
});
