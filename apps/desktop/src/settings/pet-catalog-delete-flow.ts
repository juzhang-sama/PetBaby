export type BuiltinSwitchResult =
  | { ok: true }
  | { ok: false; message: string };

export interface CurrentCatalogPetDeletionPorts<T> {
  switchToBuiltin(): Promise<BuiltinSwitchResult>;
  remove(): Promise<T>;
  refresh(): Promise<void>;
}

export type CurrentCatalogPetDeletionResult<T> =
  | { kind: "switchFailed"; message: string }
  | { kind: "deleted"; outcome: T }
  | { kind: "deleteFailed"; error: unknown };

export async function deleteCurrentCatalogPet<T>(
  ports: CurrentCatalogPetDeletionPorts<T>,
): Promise<CurrentCatalogPetDeletionResult<T>> {
  const switched = await ports.switchToBuiltin();
  if (!switched.ok) return { kind: "switchFailed", message: switched.message };

  await ports.refresh();
  try {
    return { kind: "deleted", outcome: await ports.remove() };
  } catch (error) {
    await ports.refresh();
    return { kind: "deleteFailed", error };
  }
}
