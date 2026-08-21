export interface ScreenRect { left: number; top: number; right: number; bottom: number }

export function classifyFullscreen(window: ScreenRect, monitor: ScreenRect, tolerance = 2): boolean {
  return Math.abs(window.left - monitor.left) <= tolerance
    && Math.abs(window.top - monitor.top) <= tolerance
    && Math.abs(window.right - monitor.right) <= tolerance
    && Math.abs(window.bottom - monitor.bottom) <= tolerance;
}

export interface FullscreenSnapshot {
  isFullscreen: boolean;
  foregroundHwnd: number | null;
  monitorRect: ScreenRect | null;
  reason: "foreground-covers-monitor" | "fullscreen-on-other-monitor" | "not-fullscreen" | "own-window" | "no-foreground" | "desktop-foreground";
}

const reasons = new Set<FullscreenSnapshot["reason"]>([
  "foreground-covers-monitor",
  "fullscreen-on-other-monitor",
  "not-fullscreen",
  "own-window",
  "no-foreground",
  "desktop-foreground",
]);

export function isFullscreenSnapshot(value: unknown): value is FullscreenSnapshot {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== 4
    || typeof record.isFullscreen !== "boolean"
    || !(record.foregroundHwnd === null || typeof record.foregroundHwnd === "number")
    || typeof record.reason !== "string"
    || !reasons.has(record.reason as FullscreenSnapshot["reason"])) return false;
  if (record.monitorRect === null) return true;
  if (!record.monitorRect || typeof record.monitorRect !== "object" || Array.isArray(record.monitorRect)) return false;
  const rect = record.monitorRect as Record<string, unknown>;
  return Object.keys(rect).length === 4
    && [rect.left, rect.top, rect.right, rect.bottom].every((coordinate) => typeof coordinate === "number" && Number.isFinite(coordinate));
}

export interface FullscreenProbeLoopPorts {
  setInterval(callback: () => void, delayMs: number): number;
  clearInterval(id: number): void;
  probe(): Promise<FullscreenSnapshot>;
  update(snapshot: FullscreenSnapshot): Promise<unknown>;
  reconcile(): Promise<unknown>;
  diagnose?(error: unknown): void;
}

export interface FullscreenProbeLoopWiring {
  destroy(): void;
}

export function wireFullscreenProbeLoop(
  ports: FullscreenProbeLoopPorts,
  delayMs = 750,
): FullscreenProbeLoopWiring {
  let destroyed = false;
  let probeInFlight = false;
  let probePending = false;
  let inFlight: FullscreenSnapshot | null = null;
  let pending: FullscreenSnapshot | null = null;

  const diagnose = (error: unknown): void => {
    try { ports.diagnose?.(error); } catch { /* diagnostics are observational */ }
  };
  const send = (snapshot: FullscreenSnapshot): void => {
    if (destroyed) return;
    if (inFlight) {
      pending = snapshot.isFullscreen === inFlight.isFullscreen ? null : snapshot;
      return;
    }
    inFlight = snapshot;
    void ports.update(snapshot)
      .then(() => ports.reconcile())
      .catch(diagnose)
      .finally(() => {
        const completed = inFlight;
        inFlight = null;
        if (destroyed) {
          pending = null;
          return;
        }
        const next = pending;
        pending = null;
        if (next && completed && next.isFullscreen !== completed.isFullscreen) send(next);
      });
  };
  const tick = (): void => {
    if (destroyed) return;
    if (probeInFlight) {
      probePending = true;
      return;
    }
    probeInFlight = true;
    void ports.probe()
      .then(send)
      .catch(diagnose)
      .finally(() => {
        probeInFlight = false;
        if (destroyed) {
          probePending = false;
          return;
        }
        if (probePending) {
          probePending = false;
          tick();
        }
      });
  };
  const timer = ports.setInterval(tick, delayMs);

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      pending = null;
      probePending = false;
      ports.clearInterval(timer);
    },
  };
}
