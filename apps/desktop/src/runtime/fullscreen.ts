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
  reason: "foreground-covers-monitor" | "not-fullscreen" | "own-window" | "no-foreground";
}
