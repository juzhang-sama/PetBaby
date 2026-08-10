import { afterEach, describe, expect, it, vi } from "vitest";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { requestPetSwitch, type SwitchClientPorts } from "./pet-switch-client";

function clientPorts() {
  let listener: ((result: PetSwitchResult) => void) | undefined;
  const unlisten = vi.fn();
  const listen = vi.fn(async (next: (result: PetSwitchResult) => void) => {
    listener = next;
    return unlisten;
  });
  const emit = vi.fn(async () => undefined);
  return {
    emit,
    listen,
    ports: { emit, listen } satisfies SwitchClientPorts,
    result: (value: PetSwitchResult) => listener?.(value),
    unlisten,
  };
}

describe("requestPetSwitch", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("registers the result listener before it emits the request", async () => {
    const test = clientPorts();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    const pending = requestPetSwitch("pet-b", undefined, test.ports);
    await vi.waitFor(() => expect(test.emit).toHaveBeenCalledOnce());

    expect(test.listen.mock.invocationCallOrder[0]).toBeLessThan(test.emit.mock.invocationCallOrder[0]!);
    test.result({ ok: true, requestId: "request-1", petId: "pet-b" });
    await expect(pending).resolves.toEqual({ ok: true, requestId: "request-1", petId: "pet-b" });
  });

  it("filters results for other requests until the matching request completes", async () => {
    const test = clientPorts();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    const pending = requestPetSwitch("pet-b", "variant-1", test.ports);
    await vi.waitFor(() => expect(test.emit).toHaveBeenCalledOnce());
    test.result({ ok: true, requestId: "another-request", petId: "pet-c" });
    expect(test.unlisten).not.toHaveBeenCalled();
    test.result({ ok: false, requestId: "request-1", petId: "pet-b", code: "blank-frame", message: "blank" });

    await expect(pending).resolves.toMatchObject({ ok: false, code: "blank-frame" });
    expect(test.unlisten).toHaveBeenCalledOnce();
    expect(test.emit).toHaveBeenCalledWith({
      requestId: "request-1",
      petId: "pet-b",
      acceptedVariantId: "variant-1",
    });
  });

  it("returns a window-unavailable result after the desktop window does not respond", async () => {
    vi.useFakeTimers();
    const test = clientPorts();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    const pending = requestPetSwitch("pet-b", undefined, test.ports);
    await vi.runAllTimersAsync();

    await expect(pending).resolves.toMatchObject({
      ok: false,
      requestId: "request-1",
      petId: "pet-b",
      code: "pet-window-unavailable",
    });
    expect(test.unlisten).toHaveBeenCalledOnce();
  });

  it("settles safely when the matching callback fires before listen resolves", async () => {
    const unlisten = vi.fn();
    const listen = vi.fn((handler: (result: PetSwitchResult) => void) => {
      handler({ ok: true, requestId: "request-1", petId: "pet-b" });
      return Promise.resolve(unlisten);
    });
    const emit = vi.fn(async () => undefined);
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    await expect(requestPetSwitch("pet-b", undefined, { listen, emit })).resolves.toMatchObject({ ok: true });
    expect(unlisten).toHaveBeenCalledOnce();
    expect(emit).not.toHaveBeenCalled();
  });

  it("converts a listener registration failure into a window-unavailable result", async () => {
    const listen = vi.fn(async () => { throw new Error("pet window missing"); });
    const emit = vi.fn(async () => undefined);
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    await expect(requestPetSwitch("pet-b", undefined, { listen, emit })).resolves.toMatchObject({
      ok: false,
      code: "pet-window-unavailable",
      message: "pet window missing",
    });
    expect(emit).not.toHaveBeenCalled();
  });

  it("settles with a protocol result even when listener cleanup throws", async () => {
    const test = clientPorts();
    test.unlisten.mockImplementation(() => { throw new Error("cleanup failed"); });
    test.emit.mockRejectedValue(new Error("pet window missing"));
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("crypto", { randomUUID: () => "request-1" });

    await expect(requestPetSwitch("pet-b", undefined, test.ports)).resolves.toMatchObject({
      ok: false,
      code: "pet-window-unavailable",
      message: "pet window missing",
    });
    expect(test.unlisten).toHaveBeenCalledOnce();
  });
});
