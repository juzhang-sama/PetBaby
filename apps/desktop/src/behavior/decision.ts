import type { PetEvent } from "./events";
import type { BehaviorIntent } from "./intents";

export interface PetStateSnapshot {
  schemaVersion: 1;
  petId: string;
  energy: number;
  mood: number;
  bond: number;
  lastSeenAt: string;
  lastInteractionAt: string;
}

export interface PolicySnapshot {
  cooldowns: Record<string, number>;
}

export interface DecisionInput {
  event: PetEvent;
  state: PetStateSnapshot;
  policy: PolicySnapshot;
}

const IDLE_LOOK_AFTER_MS = 15_000;
const LOOK_TARGETS = ["front", "left", "right"] as const;

const INTENT_KEYS: Record<string, string> = {
  "react-happy": "react-happy",
  "react-curious": "react-curious",
  carried: "carried",
};

export function decide(input: DecisionInput): BehaviorIntent[] {
  const { event, policy } = input;
  const intents: BehaviorIntent[] = [];

  switch (event.type) {
    case "head-clicked":
      intents.push({ type: "look", target: "front" });
      break;
    case "body-clicked":
      intents.push({ type: "react-curious" });
      break;
    case "double-clicked":
      if (!withinCooldown(policy, "react-happy", Date.now())) {
        intents.push({ type: "react-happy" });
      }
      break;
    case "drag-start":
      if (!withinCooldown(policy, "carried", Date.now())) {
        intents.push({ type: "carried" });
      }
      break;
    case "drag-end":
      intents.push({ type: "landed" });
      break;
    case "pet-shown":
      intents.push({ type: "awake" });
      break;
    case "pet-hidden":
      intents.push({ type: "sleep" });
      break;
    case "idle-tick":
      if (event.elapsedMs >= IDLE_LOOK_AFTER_MS) {
        const target = LOOK_TARGETS[Math.floor(Math.random() * LOOK_TARGETS.length)] ?? "front";
        intents.push({ type: "look", target });
      }
      break;
  }

  return intents;
}

function withinCooldown(policy: PolicySnapshot, key: string, now: number): boolean {
  const remaining = policy.cooldowns[key];
  if (remaining === undefined) return false;
  return remaining > now;
}

export { INTENT_KEYS };
