export interface SettingsNavigationPorts {
  listen(handler: (payload: unknown) => void): Promise<() => void>;
  takePending(): Promise<string | null>;
  focusCalibration(): void;
}

export interface SettingsNavigationLifecycle {
  destroy(): void;
}

export async function initializeSettingsNavigation(
  ports: SettingsNavigationPorts,
): Promise<SettingsNavigationLifecycle> {
  let destroyed = false;
  let takeTail: Promise<void> = Promise.resolve();
  const consumePending = (): void => {
    takeTail = takeTail.then(async () => {
      const section = await ports.takePending();
      if (!destroyed && section === "calibration") ports.focusCalibration();
    }).catch(() => undefined);
  };
  const unlisten = await ports.listen((payload) => {
    if (!destroyed && isCalibrationNavigation(payload)) consumePending();
  });
  consumePending();
  await takeTail;
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      unlisten();
    },
  };
}

function isCalibrationNavigation(payload: unknown): boolean {
  return typeof payload === "object"
    && payload !== null
    && !Array.isArray(payload)
    && (payload as Record<string, unknown>).section === "calibration";
}
