import { describe, expect, it } from "vitest";
import type { PetCatalogEntry, PetLifecycle } from "../pets/pet-catalog-contract";
import { buildPetListRows } from "./pet-catalog-view-model";

function userEntry(overrides: Partial<PetCatalogEntry> = {}): PetCatalogEntry {
  return {
    petId: "user-pet-1",
    displayName: "奶糖",
    creationMethod: "upload",
    sourceTemplateId: null,
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
    displayName: "默认猫 · Live2D",
    creationMethod: "upload",
    sourceTemplateId: null,
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

  it("offers edit only for user pets and never for the built-in pet", () => {
    const rows = buildPetListRows([builtinEntry(), userEntry()]);

    expect(rows.find((row) => row.petId === "user-pet-1")?.actions).toContainEqual({
      kind: "edit",
      label: "编辑",
    });
    expect(rows.find((row) => row.petId === "pet-live2d-v1")?.actions).not.toContainEqual(
      expect.objectContaining({ kind: "edit" }),
    );
  });

  it.each([
    ["ready", ["edit", "switch", "delete"]],
    ["generating", ["edit", "delete"]],
    ["generationFailed", ["edit", "delete"]],
    ["awaitingConfirm", ["edit", "delete"]],
    ["compileRetryable", ["edit", "delete"]],
    ["awaitingActivation", ["edit", "delete"]],
    ["corrupt", ["edit", "delete"]],
  ] as const)("maps %s to allowed actions", (status: PetLifecycle, actions) => {
    expect(buildPetListRows([userEntry({ status })])[0]!.actions.map((item) => item.kind)).toEqual(actions);
  });

  it.each([
    ["upload", "上传创建"],
    ["composer", "引导组合"],
    ["adoption", "直接认领"],
  ] as const)("uses displayName and the %s creation source", (creationMethod, detail) => {
    const [row] = buildPetListRows([userEntry({ creationMethod })]);

    expect(row).toMatchObject({ title: "奶糖", detail });
  });

  it("keeps the creation source visible alongside a backend issue", () => {
    const [row] = buildPetListRows([userEntry({ status: "generationFailed", issue: "API Key 已失效" })]);

    expect(row).toMatchObject({ detail: "上传创建 · API Key 已失效" });
  });
});
