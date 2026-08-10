export type BuiltinSwitchResult =
  | { ok: true; warning?: string }
  | { ok: false; message: string };

export interface CurrentCatalogPetDeletionPorts<T> {
  switchToBuiltin(): Promise<BuiltinSwitchResult>;
  remove(): Promise<T>;
  refresh(): Promise<void>;
}

export type CurrentCatalogPetDeletionResult<T> =
  | { kind: "switchFailed"; message: string }
  | { kind: "deleted"; outcome: T; switchWarning?: string }
  | { kind: "deleteFailed"; error: unknown };

export async function deleteCurrentCatalogPet<T>(
  ports: CurrentCatalogPetDeletionPorts<T>,
): Promise<CurrentCatalogPetDeletionResult<T>> {
  const switched = await ports.switchToBuiltin();
  if (!switched.ok) return { kind: "switchFailed", message: switched.message };

  await ports.refresh();
  try {
    const outcome = await ports.remove();
    return {
      kind: "deleted",
      outcome,
      ...(switched.warning ? { switchWarning: switched.warning } : {}),
    };
  } catch (error) {
    await ports.refresh();
    return { kind: "deleteFailed", error };
  }
}

export function catalogSwitchStatus(warning?: string): {
  message: string;
  tone: "info" | "warning";
} {
  return warning
    ? { message: warning, tone: "warning" }
    : { message: "已设为当前桌面宠物。", tone: "info" };
}

export function mergeCatalogWarnings(...warnings: Array<string | null | undefined>): string | undefined {
  const present = warnings.filter((warning): warning is string => Boolean(warning));
  return present.length > 0 ? present.join("；") : undefined;
}
