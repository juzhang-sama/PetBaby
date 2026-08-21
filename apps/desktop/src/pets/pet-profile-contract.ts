import { invoke } from "@tauri-apps/api/core";

export type PetGender = "unknown" | "male" | "female";

export interface PetProfile {
  schemaVersion: 1;
  petId: string;
  displayName: string;
  gender: PetGender;
  birthDate: string | null;
  editable: boolean;
  updatedAt: string;
}

export interface PetProfileUpdate {
  displayName: string;
  gender: PetGender;
  birthDate: string | null;
}

export interface PetProfileUpdateRequest {
  petId: string;
  value: PetProfileUpdate;
}

export interface PetProfileClient {
  get(petId: string): Promise<PetProfile>;
  update(request: PetProfileUpdateRequest): Promise<PetProfile>;
}

export type PetProfileInvoke = (
  command: string,
  args: Record<string, unknown>,
) => Promise<unknown>;

const SAFE_ID = /^[A-Za-z0-9_-]+$/;
const BIRTH_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;
const GENDERS = new Set<PetGender>(["unknown", "male", "female"]);

export function parsePetProfile(value: unknown): PetProfile {
  if (!isRecord(value)) throw invalidProfile("must be an object");
  if (value.schemaVersion !== 1) throw invalidProfile("schemaVersion must be 1");
  const petId = requireSafeId(value.petId, "pet profile pet id");
  if (typeof value.displayName !== "string") throw invalidProfile("displayName must be a string");
  if (!GENDERS.has(value.gender as PetGender)) throw invalidProfile("gender is invalid");
  if (value.birthDate !== null && (
    typeof value.birthDate !== "string" || !isGregorianDate(value.birthDate)
  )) throw invalidProfile("birthDate must be null or YYYY-MM-DD");
  if (typeof value.editable !== "boolean") throw invalidProfile("editable must be a boolean");
  if (typeof value.updatedAt !== "string") throw invalidProfile("updatedAt must be a string");
  return {
    schemaVersion: 1,
    petId,
    displayName: value.displayName,
    gender: value.gender as PetGender,
    birthDate: value.birthDate,
    editable: value.editable,
    updatedAt: value.updatedAt,
  };
}

export function createPetProfileClient(
  call: PetProfileInvoke = (command, args) => invoke<unknown>(command, args),
  createRequestId: () => string = () => crypto.randomUUID(),
): PetProfileClient {
  const load = async (petId: string): Promise<PetProfile> => {
    const safePetId = requireSafeId(petId, "pet id");
    const result = parsePetProfile(await call("pet_profile_get", { petId: safePetId }));
    requireMatchingPetId(result, safePetId);
    return result;
  };
  const update = async ({ petId, value }: PetProfileUpdateRequest): Promise<PetProfile> => {
    const safePetId = requireSafeId(petId, "pet id");
    const requestId = requireSafeId(createRequestId(), "request id");
    const result = parsePetProfile(await call("pet_profile_update", {
      requestId,
      petId: safePetId,
      value,
    }));
    requireMatchingPetId(result, safePetId);
    return result;
  };
  return { get: load, update };
}

function requireMatchingPetId(profile: PetProfile, expected: string): void {
  if (profile.petId !== expected) {
    throw invalidProfile(`pet id mismatch: expected ${expected}`);
  }
}

function requireSafeId(value: unknown, label: string): string {
  if (typeof value !== "string" || !SAFE_ID.test(value)) {
    throw new TypeError(`${label} is invalid`);
  }
  return value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isGregorianDate(value: string): boolean {
  const match = BIRTH_DATE.exec(value);
  if (!match) return false;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (year === 0 || month < 1 || month > 12 || day < 1) return false;
  const daysInMonth = month === 2
    ? (year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28)
    : ([4, 6, 9, 11].includes(month) ? 30 : 31);
  return day <= daysInMonth;
}

function invalidProfile(reason: string): TypeError {
  return new TypeError(`invalid pet profile: ${reason}`);
}
