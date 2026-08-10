import { describe, expect, it } from "vitest";
import type { PetCatalogEntry, PetLifecycle } from "../pets/pet-catalog-contract";
import { buildPetListRows } from "./pet-catalog-view-model";

function userEntry(overrides: Partial<PetCatalogEntry> = {}): PetCatalogEntry {
  return {
    petId: "user-pet-1",
    source: "user",
    species: "dog",
    identityMode: "realPet",
    createdAt: "2026-08-10T08:30:00Z",
    isCurrent: false,
    deletable: true,
    status: "ready",
    issue: null,
    ...overrides,
  };
}

function builtinEntry(overrides: Partial<PetCatalogEntry> = {}): PetCatalogEntry {
  return {
    petId: "pet-live2d-v1",
    source: "builtin",
    species: "cat",
    identityMode: "builtin",
    createdAt: null,
    isCurrent: false,
    deletable: false,
    status: "ready",
    issue: null,
    ...overrides,
  };
}

describe("buildPetListRows", () => {
  it("pins the built-in pet first and never offers delete", () => {
    const rows = buildPetListRows([userEntry(), builtinEntry({ isCurrent: true })]);

    expect(rows[0]).toMatchObject({
      petId: "pet-live2d-v1",
      title: "默认猫 · Live2D",
      badge: "当前使用",
      actions: [],
    });
  });

  it.each([
    ["ready", ["switch", "delete"]],
    ["generating", ["continue", "delete"]],
    ["generationFailed", ["continue", "delete"]],
    ["awaitingConfirm", ["continue", "delete"]],
    ["compileRetryable", ["continue", "delete"]],
    ["awaitingActivation", ["continue", "delete"]],
    ["corrupt", ["delete"]],
  ] as const)("maps %s to allowed actions", (status: PetLifecycle, actions) => {
    expect(buildPetListRows([userEntry({ status })])[0]!.actions.map((item) => item.kind)).toEqual(actions);
  });

  it("uses the entry issue as a lifecycle detail when the backend provides one", () => {
    const [row] = buildPetListRows([userEntry({ status: "generationFailed", issue: "API Key 已失效" })]);

    expect(row).toMatchObject({ detail: "API Key 已失效" });
  });
});
