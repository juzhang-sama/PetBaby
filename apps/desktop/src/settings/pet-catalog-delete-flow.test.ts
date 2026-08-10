import { describe, expect, it, vi } from "vitest";
import {
  catalogSwitchStatus,
  deleteCurrentCatalogPet,
  mergeCatalogWarnings,
} from "./pet-catalog-delete-flow";

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

  it("propagates a successful builtin switch warning through deletion", async () => {
    const switchToBuiltin = vi.fn(async () => ({ ok: true as const, warning: "finish 未确认" }));
    const remove = vi.fn(async () => ({ warning: "隔离目录待清理" }));
    const refresh = vi.fn(async () => undefined);

    await expect(deleteCurrentCatalogPet({ switchToBuiltin, remove, refresh })).resolves.toEqual({
      kind: "deleted",
      outcome: { warning: "隔离目录待清理" },
      switchWarning: "finish 未确认",
    });
    expect(mergeCatalogWarnings("finish 未确认", "隔离目录待清理")).toBe(
      "finish 未确认；隔离目录待清理",
    );
  });

  it("maps ordinary and warning switch successes to distinct catalog tones", () => {
    expect(catalogSwitchStatus()).toEqual({ message: "已设为当前桌面宠物。", tone: "info" });
    expect(catalogSwitchStatus("finish 未确认")).toEqual({ message: "finish 未确认", tone: "warning" });
  });
});
