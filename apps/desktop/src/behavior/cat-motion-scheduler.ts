import type { CatMotionEvent } from "./events";
import type { CatMotionCommand, CatMotionTransition } from "./intents";
import type { CatMotionNameV1 } from "../runtime-live2d/cat-motion-contract";

export type CatMotionState =
  | "idle"
  | "pointerFocus"
  | "autonomous"
  | "petHappy"
  | "dragging"
  | "edgeHidden";

export interface CatMotionSchedulerState {
  mode: CatMotionState;
  activeToken: number | null;
  activePriority: number;
  autonomousElapsedMs: number;
  nextToken: number;
}

export interface CatMotionSchedulerContext {
  localHour: number;
  random: () => number;
  paused: boolean;
}

export interface CatMotionScheduleResult {
  state: CatMotionSchedulerState;
  commands: CatMotionCommand[];
}

const TRANSITION: CatMotionTransition = { fadeInMs: 180, fadeOutMs: 140 };
const AUTONOMOUS_INTERVAL_MS = 30_000;

export function initialCatMotionSchedulerState(): CatMotionSchedulerState {
  return {
    mode: "idle",
    activeToken: null,
    activePriority: 0,
    autonomousElapsedMs: 0,
    nextToken: 1,
  };
}

export function scheduleCatMotion(
  state: CatMotionSchedulerState,
  event: CatMotionEvent,
  context: CatMotionSchedulerContext,
): CatMotionScheduleResult {
  switch (event.type) {
    case "start":
      if (state.activePriority > 10) return unchanged(state);
      return play(state, "idle", "breathing", 10, true);
    case "tick": {
      if (context.paused || state.mode !== "idle") return unchanged(state);
      const autonomousElapsedMs = state.autonomousElapsedMs + Math.max(0, event.elapsedMs);
      if (autonomousElapsedMs < AUTONOMOUS_INTERVAL_MS) {
        return unchanged({ ...state, autonomousElapsedMs });
      }
      return startAutonomous({ ...state, autonomousElapsedMs: 0 }, context);
    }
    case "autonomous-due":
      if (state.activePriority > 30 || state.mode === "edgeHidden" || state.mode === "dragging") {
        return unchanged(state);
      }
      return startAutonomous(state, context);
    case "pointer-enter":
      if (state.activePriority > 60 || state.mode === "edgeHidden" || state.mode === "dragging") {
        return unchanged(state);
      }
      return play(state, "pointerFocus", "pointer-focus", 60, true);
    case "pointer-leave":
      return state.mode === "pointerFocus" ? play(state, "idle", "breathing", 10, true, true) : unchanged(state);
    case "pet":
      if (state.mode === "edgeHidden" || state.mode === "dragging" || state.activePriority > 90) {
        return unchanged(state);
      }
      return play(state, "petHappy", "pet-happy", 90, false);
    case "drag-start":
      return hold(state, "dragging");
    case "drag-end":
      return state.mode === "dragging" ? play(state, "idle", "breathing", 10, true, true) : unchanged(state);
    case "edge-hidden":
      return hold(state, "edgeHidden");
    case "edge-recall":
      if (state.mode !== "edgeHidden") return unchanged(state);
      return play(state, "idle", "breathing", 100, false, true);
    case "motion-complete":
      if (event.token !== state.activeToken) return unchanged(state);
      return play(state, "idle", "breathing", 10, true, true, 10, false);
  }
}

function startAutonomous(
  state: CatMotionSchedulerState,
  context: CatMotionSchedulerContext,
): CatMotionScheduleResult {
  return play(state, "autonomous", chooseAutonomousMotion(context), 30, false);
}

function chooseAutonomousMotion(context: CatMotionSchedulerContext): CatMotionNameV1 {
  const random = Math.min(0.999_999, Math.max(0, context.random()));
  const night = context.localHour >= 22 || context.localHour < 7;
  if (night) {
    if (random < 0.2) return "ear-twitch";
    if (random < 0.7) return "sleepy-yawn";
    return "half-stand-stretch";
  }
  if (random < 0.6) return "ear-twitch";
  if (random < 0.8) return "sleepy-yawn";
  return "half-stand-stretch";
}

function play(
  state: CatMotionSchedulerState,
  mode: CatMotionState,
  motion: CatMotionNameV1,
  commandPriority: number,
  loop: boolean,
  force = false,
  statePriority = commandPriority,
  cancelCurrent = true,
): CatMotionScheduleResult {
  if (!force && commandPriority < state.activePriority) return unchanged(state);
  const token = state.nextToken;
  const commands: CatMotionCommand[] = [];
  if (cancelCurrent && state.activeToken !== null) {
    commands.push({ type: "cancel", token: state.activeToken, ...TRANSITION });
  }
  commands.push({ type: "play", token, motion, priority: commandPriority, loop, ...TRANSITION });
  return {
    state: {
      ...state,
      mode,
      activeToken: token,
      activePriority: statePriority,
      autonomousElapsedMs: mode === "idle" ? state.autonomousElapsedMs : 0,
      nextToken: token + 1,
    },
    commands,
  };
}

function hold(
  state: CatMotionSchedulerState,
  mode: "dragging" | "edgeHidden",
): CatMotionScheduleResult {
  const commands: CatMotionCommand[] = [];
  if (state.activeToken !== null) commands.push({ type: "cancel", token: state.activeToken, ...TRANSITION });
  commands.push({ type: "hold", priority: 100, ...TRANSITION });
  return {
    state: {
      ...state,
      mode,
      activeToken: null,
      activePriority: 100,
      autonomousElapsedMs: 0,
    },
    commands,
  };
}

function unchanged(state: CatMotionSchedulerState): CatMotionScheduleResult {
  return { state, commands: [] };
}
