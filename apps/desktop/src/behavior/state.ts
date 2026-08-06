export interface PetStateSnapshot {
  schemaVersion: 1;
  petId: string;
  energy: number;
  mood: number;
  bond: number;
  lastSeenAt: string;
  lastInteractionAt: string;
}

const ENERGY_DRAIN_PER_HOUR = 0.05;
const MOOD_GAIN_PER_INTERACTION = 0.08;
const BOND_GAIN_PER_INTERACTION = 0.01;

export type PetAction = "pet" | "feed" | "play";

export function evolveState(
  snapshot: PetStateSnapshot,
  now: Date,
  elapsedMs: number,
  action?: PetAction,
): PetStateSnapshot {
  const hours = elapsedMs / 3_600_000;
  let energy = snapshot.energy - ENERGY_DRAIN_PER_HOUR * hours;
  if (energy < 0) energy = 0;

  let mood = snapshot.mood;
  let bond = snapshot.bond;
  if (action === "feed") {
    energy = Math.min(1, energy + 0.25);
  } else if (action === "pet") {
    mood = Math.min(1, mood + MOOD_GAIN_PER_INTERACTION);
    bond = Math.min(1, bond + BOND_GAIN_PER_INTERACTION);
  } else if (action === "play") {
    mood = Math.min(1, mood + MOOD_GAIN_PER_INTERACTION + 0.04);
    bond = Math.min(1, bond + BOND_GAIN_PER_INTERACTION * 2);
  }

  return {
    schemaVersion: 1,
    petId: snapshot.petId,
    energy,
    mood,
    bond,
    lastSeenAt: now.toISOString(),
    lastInteractionAt: action ? now.toISOString() : snapshot.lastInteractionAt,
  };
}
