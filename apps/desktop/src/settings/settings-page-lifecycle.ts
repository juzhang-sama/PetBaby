export interface SettingsPageEventTarget {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

export interface SettingsPageLifecyclePorts {
  suspend(): void;
  resume(): void;
  destroy(): void;
}

export function wireSettingsPageLifecycle(
  events: SettingsPageEventTarget,
  ports: SettingsPageLifecyclePorts,
): { destroy(): void } {
  let destroyed = false;
  const removeListeners = (): void => {
    events.removeEventListener("pagehide", onPageHide);
    events.removeEventListener("pageshow", onPageShow);
    events.removeEventListener("beforeunload", onBeforeUnload);
  };
  const destroy = (): void => {
    if (destroyed) return;
    destroyed = true;
    removeListeners();
    ports.destroy();
  };
  const onPageHide = (): void => { if (!destroyed) ports.suspend(); };
  const onPageShow = (): void => { if (!destroyed) ports.resume(); };
  const onBeforeUnload = (): void => destroy();
  events.addEventListener("pagehide", onPageHide);
  events.addEventListener("pageshow", onPageShow);
  events.addEventListener("beforeunload", onBeforeUnload);
  return { destroy };
}
