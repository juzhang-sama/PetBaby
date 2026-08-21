import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { wireWindowModeRuntime } from "./window-mode-runtime";

describe("window mode runtime handshake", () => {
  it("defers an early resume until the matching pause ACK is accepted", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    let acceptPause!: (accepted: boolean) => void;
    const pauseAck = new Promise<boolean>((resolve) => { acceptPause = resolve; });
    const ack = vi.fn()
      .mockImplementationOnce(() => pauseAck)
      .mockResolvedValueOnce(true);
    const resume = vi.fn();
    await wireWindowModeRuntime({
      listen: async (event, handler) => { handlers.set(event, handler); return () => undefined; },
      ready: async () => 1,
      ack,
      pause: vi.fn(),
      resume,
      abort: vi.fn(),
    });

    handlers.get("pet-runtime:pause")?.({ requestId: "deferred", cycle: 1, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("deferred", 1, "paused"));
    handlers.get("pet-runtime:resume")?.({
      requestId: "deferred",
      cycle: 1,
      phase: "resumed",
      effectiveVisible: false,
    });
    await Promise.resolve();
    expect(resume).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalledWith("deferred", 1, "resumed");

    acceptPause(true);
    await vi.waitFor(() => expect(resume).toHaveBeenCalledWith(false));
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("deferred", 1, "resumed"));
  });

  it("fails closed when deferred resume payloads conflict", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    let acceptPause!: (accepted: boolean) => void;
    const ack = vi.fn(() => new Promise<boolean>((resolve) => { acceptPause = resolve; }));
    const abort = vi.fn();
    const resume = vi.fn();
    const diagnose = vi.fn();
    await wireWindowModeRuntime({
      listen: async (event, handler) => { handlers.set(event, handler); return () => undefined; },
      ready: async () => 1,
      ack,
      pause: vi.fn(),
      resume,
      abort,
      diagnose,
    });

    handlers.get("pet-runtime:pause")?.({ requestId: "conflict", cycle: 1, phase: "paused" });
    handlers.get("pet-runtime:resume")?.({ requestId: "conflict", cycle: 1, phase: "resumed", effectiveVisible: true });
    handlers.get("pet-runtime:resume")?.({ requestId: "conflict", cycle: 1, phase: "resumed", effectiveVisible: true });
    handlers.get("pet-runtime:resume")?.({ requestId: "conflict", cycle: 1, phase: "resumed", effectiveVisible: false });

    expect(abort).toHaveBeenCalledOnce();
    expect(diagnose).toHaveBeenCalledWith(
      "window-mode-runtime-resume-conflict",
      expect.any(Error),
    );
    acceptPause(true);
    await Promise.resolve();
    await Promise.resolve();
    expect(resume).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalledWith("conflict", 1, "resumed");
  });

  it("lets a new Rust request recover after an old pause ACK remains unresolved", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    let rejectOld!: (accepted: boolean) => void;
    const ack = vi.fn()
      .mockImplementationOnce(() => new Promise<boolean>((resolve) => { rejectOld = resolve; }))
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(true);
    const pause = vi.fn();
    const resume = vi.fn();
    const abort = vi.fn();
    await wireWindowModeRuntime({
      listen: async (event, handler) => { handlers.set(event, handler); return () => undefined; },
      ready: async () => 1,
      ack,
      pause,
      resume,
      abort,
    });

    handlers.get("pet-runtime:pause")?.({ requestId: "stale", cycle: 1, phase: "paused" });
    handlers.get("pet-runtime:resume")?.({ requestId: "stale", cycle: 1, phase: "resumed", effectiveVisible: true });
    handlers.get("pet-runtime:pause")?.({ requestId: "recovery", cycle: 1, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("recovery", 1, "paused"));
    handlers.get("pet-runtime:resume")?.({ requestId: "recovery", cycle: 1, phase: "resumed", effectiveVisible: true });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("recovery", 1, "resumed"));

    rejectOld(false);
    await Promise.resolve();
    expect(abort).not.toHaveBeenCalled();
    expect(resume).toHaveBeenCalledWith(true);
  });

  it("registers both listeners before ready and ACKs only the matching ordered phases", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    const order: string[] = [];
    const pause = vi.fn(() => order.push("pause"));
    const resume = vi.fn((effectiveVisible: boolean) => order.push(`resume:${effectiveVisible}`));
    const abort = vi.fn(() => order.push("abort"));
    const ack = vi.fn(async (requestId: string, cycle: number, phase: string) => {
      order.push(`ack:${requestId}:${cycle}:${phase}`);
      return true;
    });
    const destroy = await wireWindowModeRuntime({
      listen: async (event, handler) => {
        order.push(`listen:${event}`);
        handlers.set(event, handler);
        return vi.fn<() => void>();
      },
      ready: async () => { order.push("ready"); return 1; },
      ack,
      pause,
      resume,
      abort,
    });

    expect(order.slice(0, 3)).toEqual([
      "listen:pet-runtime:pause",
      "listen:pet-runtime:resume",
      "ready",
    ]);
    handlers.get("pet-runtime:resume")?.({ requestId: "late-1", cycle: 1, phase: "resumed", effectiveVisible: true });
    handlers.get("pet-runtime:pause")?.({ requestId: "req-1", cycle: 1, phase: "paused" });
    handlers.get("pet-runtime:pause")?.({ requestId: "req-1", cycle: 1, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("req-1", 1, "paused"));
    await Promise.resolve();
    handlers.get("pet-runtime:resume")?.({ requestId: "other", cycle: 1, phase: "resumed", effectiveVisible: true });
    handlers.get("pet-runtime:resume")?.({ requestId: "req-1", cycle: 1, phase: "resumed", effectiveVisible: true });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("req-1", 1, "resumed"));
    await Promise.resolve();

    handlers.get("pet-runtime:pause")?.({ requestId: "req-1", cycle: 2, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("req-1", 2, "paused"));
    await Promise.resolve();
    expect(pause).toHaveBeenCalledTimes(2);
    handlers.get("pet-runtime:resume")?.({ requestId: "req-1", cycle: 2, phase: "resumed", effectiveVisible: false });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("req-1", 2, "resumed"));

    expect(resume).toHaveBeenNthCalledWith(1, true);
    expect(resume).toHaveBeenNthCalledWith(2, false);
    expect(ack).toHaveBeenCalledTimes(4);
    expect(abort).not.toHaveBeenCalled();
    destroy.destroy();
  });

  it("unlistens partial setup when the second listener or ready handshake fails", async () => {
    const firstUnlisten = vi.fn();
    await expect(wireWindowModeRuntime({
      listen: vi.fn()
        .mockResolvedValueOnce(firstUnlisten)
        .mockRejectedValueOnce(new Error("resume listener failed")),
      ready: vi.fn(),
      ack: vi.fn(),
      pause: vi.fn(),
      resume: vi.fn(),
      abort: vi.fn(),
    })).rejects.toThrow("resume listener failed");
    expect(firstUnlisten).toHaveBeenCalledOnce();

    const unlistenPause = vi.fn();
    const unlistenResume = vi.fn();
    await expect(wireWindowModeRuntime({
      listen: vi.fn()
        .mockResolvedValueOnce(unlistenPause)
        .mockResolvedValueOnce(unlistenResume),
      ready: async () => { throw new Error("ready failed"); },
      ack: vi.fn(),
      pause: vi.fn(),
      resume: vi.fn(),
      abort: vi.fn(),
    })).rejects.toThrow("ready failed");
    expect(unlistenPause).toHaveBeenCalledOnce();
    expect(unlistenResume).toHaveBeenCalledOnce();
  });

  it("tears down both listeners idempotently and ignores late events", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    const unlistenPause = vi.fn();
    const unlistenResume = vi.fn();
    const pause = vi.fn();
    const ack = vi.fn();
    const destroy = await wireWindowModeRuntime({
      listen: async (event, handler) => {
        handlers.set(event, handler);
        return event.endsWith("pause") ? unlistenPause : unlistenResume;
      },
      ready: async () => 1,
      ack,
      pause,
      resume: vi.fn(),
      abort: vi.fn(),
    });
    destroy.destroy();
    destroy.destroy();
    handlers.get("pet-runtime:pause")?.({ requestId: "late", cycle: 1, phase: "paused" });
    await Promise.resolve();
    expect(unlistenPause).toHaveBeenCalledOnce();
    expect(unlistenResume).toHaveBeenCalledOnce();
    expect(pause).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalled();
  });

  it("fails closed on false or rejected ACKs without deadlocking a newer cycle", async () => {
    const handlers = new Map<string, (payload: unknown) => void>();
    const pause = vi.fn();
    const resume = vi.fn();
    const abort = vi.fn();
    const ack = vi.fn()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true)
      .mockRejectedValueOnce(new Error("invoke failed"))
      .mockResolvedValueOnce(true);
    await wireWindowModeRuntime({
      listen: async (event, handler) => { handlers.set(event, handler); return () => undefined; },
      ready: async () => 1,
      ack,
      pause,
      resume,
      abort,
    });

    handlers.get("pet-runtime:pause")?.({ requestId: "retry", cycle: 1, phase: "paused" });
    handlers.get("pet-runtime:resume")?.({ requestId: "retry", cycle: 1, phase: "resumed", effectiveVisible: false });
    await vi.waitFor(() => expect(abort).toHaveBeenCalledTimes(1));
    expect(resume).not.toHaveBeenCalled();
    expect(ack).not.toHaveBeenCalledWith("retry", 1, "resumed");
    handlers.get("pet-runtime:pause")?.({ requestId: "retry", cycle: 2, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("retry", 2, "paused"));
    await Promise.resolve();
    handlers.get("pet-runtime:resume")?.({ requestId: "retry", cycle: 2, phase: "resumed", effectiveVisible: true });
    await vi.waitFor(() => expect(abort).toHaveBeenCalledTimes(2));
    handlers.get("pet-runtime:pause")?.({ requestId: "next", cycle: 1, phase: "paused" });
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("next", 1, "paused"));
    expect(pause).toHaveBeenCalledTimes(3);
  });

  it("keeps the Rust and TypeScript payload contracts cycle-versioned", () => {
    const rust = readFileSync(new URL("../../src-tauri/src/window_mode.rs", import.meta.url), "utf8");
    expect(rust).toMatch(/struct RuntimeTransitionPayload[\s\S]*request_id:[\s\S]*cycle:[\s\S]*phase:[\s\S]*effective_visible:/);
    expect(rust).toMatch(/pub fn runtime_ack\([\s\S]{0,180}request_id: &str,[\s\S]{0,80}cycle: u64,[\s\S]{0,80}phase: RuntimeAckPhase/);
  });
});
