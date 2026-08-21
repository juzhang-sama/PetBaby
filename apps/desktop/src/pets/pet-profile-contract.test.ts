import { describe, expect, it, vi } from "vitest";
import {
  createPetProfileClient,
  parsePetProfile,
  type PetProfile,
  type PetProfileInvoke,
} from "./pet-profile-contract";

function profile(overrides: Partial<PetProfile> = {}): PetProfile {
  return {
    schemaVersion: 1,
    petId: "pet-a",
    displayName: "小白",
    gender: "unknown",
    birthDate: null,
    editable: true,
    updatedAt: "2026-08-12T08:00:00Z",
    ...overrides,
  };
}

describe("pet profile contract", () => {
  it("parses the complete backend profile schema", () => {
    expect(parsePetProfile(profile({ gender: "female", birthDate: "2024-02-29" }))).toEqual(
      profile({ gender: "female", birthDate: "2024-02-29" }),
    );
  });

  it.each([
    ["object", null],
    ["schema", profile({ schemaVersion: 2 as 1 })],
    ["pet id type", { ...profile(), petId: 7 }],
    ["unsafe pet id", profile({ petId: "../pet-a" })],
    ["display name", { ...profile(), displayName: null }],
    ["gender", profile({ gender: "other" as "unknown" })],
    ["birth date type", { ...profile(), birthDate: 20240229 }],
    ["birth date shape", profile({ birthDate: "2024-2-29" })],
    ["birth date value", profile({ birthDate: "2025-02-29" })],
    ["editable", { ...profile(), editable: "yes" }],
    ["updated at", { ...profile(), updatedAt: 123 }],
  ])("rejects an invalid %s at the runtime boundary", (_label, value) => {
    expect(() => parsePetProfile(value)).toThrow(/pet profile/i);
  });

  it("loads through unknown and validates the returned pet identity", async () => {
    const invoke = vi.fn<PetProfileInvoke>(async () => profile({ petId: "pet-b" }));
    const client = createPetProfileClient(invoke, () => "request-unused");

    await expect(client.get("pet-a")).rejects.toThrow(/pet id/i);
    expect(invoke).toHaveBeenCalledWith("pet_profile_get", { petId: "pet-a" });
  });

  it("uses a fresh secure request id for every update and returns only canonical backend data", async () => {
    const canonical = profile({ displayName: "米米", gender: "female", birthDate: "2024-02-29" });
    const invoke = vi.fn<PetProfileInvoke>(async () => canonical);
    const createRequestId = vi.fn()
      .mockReturnValueOnce("8e5ba940-4324-4f7c-8e78-3c1b5ce7bb3a")
      .mockReturnValueOnce("6759d223-340e-4ff3-9b9d-6d01f7180109");
    const client = createPetProfileClient(invoke, createRequestId);
    const value = { displayName: " 米米 ", gender: "female" as const, birthDate: "2024-02-29" };

    await expect(client.update({ petId: "pet-a", value })).resolves.toEqual(canonical);
    await client.update({ petId: "pet-a", value });

    expect(createRequestId).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenNthCalledWith(1, "pet_profile_update", {
      requestId: "8e5ba940-4324-4f7c-8e78-3c1b5ce7bb3a",
      petId: "pet-a",
      value,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "pet_profile_update", {
      requestId: "6759d223-340e-4ff3-9b9d-6d01f7180109",
      petId: "pet-a",
      value,
    });
  });

  it("rejects unsafe caller and request identifiers before invoking", async () => {
    const invoke = vi.fn<PetProfileInvoke>(async () => profile());
    const invalidPet = createPetProfileClient(invoke, () => "request-1");
    const invalidRequest = createPetProfileClient(invoke, () => "not safe!");

    await expect(invalidPet.get("../pet-a")).rejects.toThrow(/pet id/i);
    await expect(invalidRequest.update({
      petId: "pet-a",
      value: { displayName: "米米", gender: "unknown", birthDate: null },
    })).rejects.toThrow(/request id/i);
    expect(invoke).not.toHaveBeenCalled();
  });
});
