export const PET_RUNTIME_PAUSE = "pet-runtime:pause";
export const PET_RUNTIME_RESUME = "pet-runtime:resume";

export type WindowModeRuntimePhase = "paused" | "resumed";

interface WindowModeRuntimePauseEvent {
  requestId: string;
  cycle: number;
  phase: "paused";
}

interface WindowModeRuntimeResumeEvent {
  requestId: string;
  cycle: number;
  phase: "resumed";
  effectiveVisible: boolean;
}

export interface WindowModeRuntimePorts {
  listen(event: string, handler: (payload: unknown) => void): Promise<() => void>;
  ready(): Promise<number>;
  ack(requestId: string, cycle: number, phase: WindowModeRuntimePhase): Promise<boolean>;
  pause(): void;
  resume(effectiveVisible: boolean): void;
  abort(): void;
  diagnose?(stage: string, error: unknown): void;
}

export interface WindowModeRuntimeWiring {
  destroy(): void;
}

export async function wireWindowModeRuntime(ports: WindowModeRuntimePorts): Promise<WindowModeRuntimeWiring> {
  let destroyed = false;
  let active: {
    requestId: string;
    cycle: number;
    state: "ack-paused" | "paused" | "ack-resumed" | "failed";
    pendingResume?: WindowModeRuntimeResumeEvent;
  } | null = null;
  const completed = new Set<string>();

  const diagnose = (stage: string, error: unknown): void => {
    try { ports.diagnose?.(stage, error); } catch { /* diagnostics are observational */ }
  };
  const remember = (key: string): void => {
    completed.add(key);
    if (completed.size > 128) completed.delete(completed.values().next().value as string);
  };
  const failClosed = (stage: string, error: unknown): void => {
    try { ports.abort(); } catch (abortError) { diagnose("window-mode-runtime-abort", abortError); }
    if (active) {
      active.pendingResume = undefined;
      active.state = "failed";
    }
    diagnose(stage, error);
  };
  const acknowledge = (
    event: WindowModeRuntimePauseEvent | WindowModeRuntimeResumeEvent,
    key: string,
  ): void => {
    void ports.ack(event.requestId, event.cycle, event.phase)
      .then((accepted) => {
        if (destroyed) return;
        const matchesActive = !!active
          && active.requestId === event.requestId
          && active.cycle === event.cycle;
        if (accepted !== true) {
          if (matchesActive) {
            failClosed("window-mode-runtime-ack-rejected", new Error("runtime ACK was rejected"));
          }
          return;
        }
        remember(key);
        if (!matchesActive || !active) return;
        if (event.phase === "paused") {
          if (active.state !== "ack-paused") return;
          active.state = "paused";
          const pendingResume = active.pendingResume;
          active.pendingResume = undefined;
          if (pendingResume) receiveResume(pendingResume);
        } else if (active.state === "ack-resumed") {
          active = null;
        }
      })
      .catch((error) => {
        if (!destroyed
          && active?.requestId === event.requestId
          && active.cycle === event.cycle) {
          failClosed("window-mode-runtime-ack", error);
        }
      });
  };
  const receivePause = (candidate: unknown): void => {
    if (destroyed || !isWindowModeRuntimePauseEvent(candidate)) return;
    const key = `${candidate.requestId}:${candidate.cycle}:paused`;
    if (completed.has(key)) return;
    if (active) {
      const sameCycle = active.requestId === candidate.requestId && active.cycle === candidate.cycle;
      const rustAdvancedCycle = active.requestId === candidate.requestId
        && candidate.cycle > active.cycle
        && active.state === "ack-resumed";
      const rustClosedPriorCycle = active.state === "ack-paused" && !!active.pendingResume;
      if (sameCycle
        || (active.state !== "failed" && !rustAdvancedCycle && !rustClosedPriorCycle)) return;
    }
    active = { requestId: candidate.requestId, cycle: candidate.cycle, state: "ack-paused" };
    ports.pause();
    acknowledge(candidate, key);
  };
  const receiveResume = (candidate: unknown): void => {
    if (destroyed || !isWindowModeRuntimeResumeEvent(candidate)) return;
    const key = `${candidate.requestId}:${candidate.cycle}:resumed`;
    if (completed.has(key)) return;
    if (!active
      || active.requestId !== candidate.requestId
      || active.cycle !== candidate.cycle) {
      return;
    }
    if (active.state === "ack-paused") {
      if (!active.pendingResume) {
        active.pendingResume = candidate;
      } else if (active.pendingResume.effectiveVisible !== candidate.effectiveVisible) {
        failClosed(
          "window-mode-runtime-resume-conflict",
          new Error("runtime resume payload conflicts with the pending resume"),
        );
      }
      return;
    }
    if (active.state !== "paused") return;
    active.state = "ack-resumed";
    ports.resume(candidate.effectiveVisible);
    acknowledge(candidate, key);
  };

  let unlistenPause: (() => void) | undefined;
  let unlistenResume: (() => void) | undefined;
  try {
    unlistenPause = await ports.listen(PET_RUNTIME_PAUSE, receivePause);
    unlistenResume = await ports.listen(PET_RUNTIME_RESUME, receiveResume);
    await ports.ready();
  } catch (error) {
    try { unlistenResume?.(); } catch { /* best effort */ }
    try { unlistenPause?.(); } catch { /* best effort */ }
    throw error;
  }

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      if (active) active.pendingResume = undefined;
      active = null;
      try { unlistenResume?.(); } catch { /* best effort */ }
      try { unlistenPause?.(); } catch { /* best effort */ }
    },
  };
}

export function isWindowModeRuntimePauseEvent(value: unknown): value is WindowModeRuntimePauseEvent {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return Object.keys(record).length === 3
    && typeof record.requestId === "string"
    && /^[A-Za-z0-9_.:-]{1,128}$/.test(record.requestId)
    && Number.isSafeInteger(record.cycle) && (record.cycle as number) > 0
    && record.phase === "paused";
}

export function isWindowModeRuntimeResumeEvent(value: unknown): value is WindowModeRuntimeResumeEvent {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return Object.keys(record).length === 4
    && typeof record.requestId === "string"
    && /^[A-Za-z0-9_.:-]{1,128}$/.test(record.requestId)
    && Number.isSafeInteger(record.cycle) && (record.cycle as number) > 0
    && record.phase === "resumed"
    && typeof record.effectiveVisible === "boolean";
}
