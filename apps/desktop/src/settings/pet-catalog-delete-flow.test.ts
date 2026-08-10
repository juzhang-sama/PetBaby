import { describe, expect, it, vi } from "vitest";
import { deleteCurrentCatalogPet } from "./pet-catalog-delete-flow";

describe("deleteCurrentCatalogPet", () => {
  it("refreshes the catalog before and after a delete failure that follows a successful builtin switch", async () => {
    const switchToBuiltin = vi.fn(async () => ({ ok: true as const }));
    const remove = vi.fn(async () => { throw new Error("database unavailable"); });
    const refresh = vi.fn(async () => undefined);

    await expect(deleteCurrentCatalogPet({ switchToBuiltin, remove, refresh })).resolves.toMatchObject({
      kind: "deleteFailed",
      error: expect.any(Error),
    });
    expect(refresh).toHaveBeenCalledTimes(2);
    expect(refresh.mock.invocationCallOrder[0]).toBeLessThan(remove.mock.invocationCallOrder[0]!);
  });

  it("does not try to delete when switching to the builtin pet fails", async () => {
    const switchToBuiltin = vi.fn(async () => ({ ok: false as const, message: "pet window unavailable" }));
    const remove = vi.fn(async () => undefined);
    const refresh = vi.fn(async () => undefined);

    await expect(deleteCurrentCatalogPet({ switchToBuiltin, remove, refresh })).resolves.toEqual({
      kind: "switchFailed",
      message: "pet window unavailable",
    });
    expect(remove).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });
});
