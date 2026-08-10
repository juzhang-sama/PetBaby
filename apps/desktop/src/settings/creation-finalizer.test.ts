import { describe, expect, it, vi } from "vitest";
import type { CreationSnapshot } from "../creation/contracts";
import type { PetSwitchResult } from "../runtime/pet-switch-protocol";
import { finalizeCreation, type FinalizerPorts, type PreparedCreation } from "./creation-finalizer";

function harness(input: {
  alreadyCompleted?: boolean;
  switchResult?: PetSwitchResult;
  switchError?: Error;
} = {}) {
  const calls: string[] = [];
  const prepared: PreparedCreation = {
    requestId: "ignored-backend-value",
    sessionId: "session-1",
    petId: "pet-1",
    variantId: "variant-1",
    alreadyCompleted: input.alreadyCompleted ?? false,
  };
  const prepare = vi.fn(async (_sessionId: string, _requestId: string) => {
    calls.push("prepare");
    return prepared;
  });
  const switchPet = vi.fn(async (_petId: string, _options: Parameters<FinalizerPorts["switchPet"]>[1]) => {
    calls.push("switch");
    if (input.switchError) throw input.switchError;
    const success: PetSwitchResult = {
      ok: true,
      requestId: "request-1",
      petId: "pet-1",
    };
    return input.switchResult ?? success;
  });
  const abort = vi.fn(async () => {
    calls.push("abort");
    return { sessionId: "session-1" } as CreationSnapshot;
  });
  const cancel = vi.fn(async () => {
    calls.push("cancel");
  });
  return {
    calls,
    ports: { prepare, switchPet, abort, cancel } satisfies FinalizerPorts,
    prepare,
    switchPet,
    abort,
    cancel,
  };
}

describe("finalizeCreation", () => {
  it("carries one request owner from prepare through the creation switch", async () => {
    const test = harness();
    vi.spyOn(crypto, "randomUUID").mockReturnValue("request-1" as ReturnType<typeof crypto.randomUUID>);

    await expect(finalizeCreation("session-1", test.ports)).resolves.toMatchObject({ ok: true });

    expect(test.prepare).toHaveBeenCalledWith("session-1", "request-1");
    expect(test.switchPet).toHaveBeenCalledWith("pet-1", {
      requestId: "request-1",
      acceptedVariantId: "variant-1",
      creationSessionId: "session-1",
    });
    expect(test.calls).toEqual(["prepare", "switch"]);
  });

  it("returns an already completed creation without switching or touching the gate", async () => {
    const test = harness({ alreadyCompleted: true });
    vi.spyOn(crypto, "randomUUID").mockReturnValue("request-completed" as ReturnType<typeof crypto.randomUUID>);

    await expect(finalizeCreation("session-1", test.ports)).resolves.toEqual({
      ok: true,
      requestId: "request-completed",
      petId: "pet-1",
    });

    expect(test.switchPet).not.toHaveBeenCalled();
    expect(test.abort).not.toHaveBeenCalled();
    expect(test.cancel).not.toHaveBeenCalled();
  });

  it("aborts before cancelling when the desktop reports a creation switch failure", async () => {
    const test = harness({
      switchResult: {
        ok: false,
        requestId: "request-failed",
        petId: "pet-1",
        code: "pet-window-unavailable",
        message: "桌面宠物窗口没有响应",
      },
    });
    vi.spyOn(crypto, "randomUUID").mockReturnValue("request-failed" as ReturnType<typeof crypto.randomUUID>);

    await expect(finalizeCreation("session-1", test.ports)).resolves.toMatchObject({ ok: false });

    expect(test.abort).toHaveBeenCalledWith("session-1", "桌面宠物窗口没有响应");
    expect(test.cancel).toHaveBeenCalledWith("request-failed");
    expect(test.calls).toEqual(["prepare", "switch", "abort", "cancel"]);
  });

  it("uses the same abort and cancel fallback when the desktop window never responds", async () => {
    const test = harness({ switchError: new Error("desktop request timed out") });
    vi.spyOn(crypto, "randomUUID").mockReturnValue("request-timeout" as ReturnType<typeof crypto.randomUUID>);

    await expect(finalizeCreation("session-1", test.ports)).resolves.toMatchObject({
      ok: false,
      code: "pet-window-unavailable",
      message: "desktop request timed out",
    });

    expect(test.abort).toHaveBeenCalledWith("session-1", "desktop request timed out");
    expect(test.cancel).toHaveBeenCalledWith("request-timeout");
    expect(test.calls).toEqual(["prepare", "switch", "abort", "cancel"]);
  });

  it("does not abort a committed success that only carries a cleanup warning", async () => {
    const test = harness({
      switchResult: {
        ok: true,
        requestId: "request-warning",
        petId: "pet-1",
        warning: "old runtime cleanup failed",
      },
    });
    vi.spyOn(crypto, "randomUUID").mockReturnValue("request-warning" as ReturnType<typeof crypto.randomUUID>);

    await expect(finalizeCreation("session-1", test.ports)).resolves.toMatchObject({
      ok: true,
      warning: "old runtime cleanup failed",
    });

    expect(test.abort).not.toHaveBeenCalled();
    expect(test.cancel).not.toHaveBeenCalled();
  });
});
