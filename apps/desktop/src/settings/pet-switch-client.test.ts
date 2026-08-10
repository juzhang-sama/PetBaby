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
});
