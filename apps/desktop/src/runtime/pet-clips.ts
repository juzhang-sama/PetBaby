import type { AnimClip } from "./anim-clip";

export interface PetClips {
  idle: AnimClip;
  sleep: AnimClip;
  "look-left": AnimClip;
  "look-right": AnimClip;
  "react-happy": AnimClip;
  "react-curious": AnimClip;
  carried: AnimClip;
  landed: AnimClip;
  falling: AnimClip;
  stroll: AnimClip;
}

export function createPetClips(): PetClips {
  return {
    idle: {
      name: "idle",
      durationMs: 4_000,
      loop: true,
      tracks: {
        breath: [
          { timeMs: 0, value: 0 },
          { timeMs: 1_000, value: 0.25 },
          { timeMs: 2_000, value: 0.5 },
          { timeMs: 3_000, value: 0.75 },
          { timeMs: 4_000, value: 1 },
        ],
      },
    },
    sleep: {
      name: "sleep",
      durationMs: 6_000,
      loop: true,
      tracks: {
        breath: [
          { timeMs: 0, value: 0 },
          { timeMs: 3_000, value: 0.5 },
          { timeMs: 6_000, value: 1 },
        ],
      },
    },
    "look-left": {
      name: "look-left",
      durationMs: 900,
      tracks: {
        lookX: [
          { timeMs: 0, value: 0, ease: "easeOutCubic" },
          { timeMs: 140, value: 1, ease: "easeOutCubic" },
          { timeMs: 420, value: 1, ease: "linear" },
          { timeMs: 900, value: 0, ease: "easeOutCubic" },
        ],
      },
    },
    "look-right": {
      name: "look-right",
      durationMs: 900,
      tracks: {
        lookX: [
          { timeMs: 0, value: 0, ease: "easeOutCubic" },
          { timeMs: 140, value: -1, ease: "easeOutCubic" },
          { timeMs: 420, value: -1, ease: "linear" },
          { timeMs: 900, value: 0, ease: "easeOutCubic" },
        ],
      },
    },
    "react-happy": {
      name: "react-happy",
      durationMs: 500,
      tracks: {
        squash: [
          { timeMs: 0, value: 1, ease: "easeOutCubic" },
          { timeMs: 100, value: 1.06, ease: "easeOutCubic" },
          { timeMs: 250, value: 1, ease: "easeOutCubic" },
          { timeMs: 400, value: 1.05, ease: "easeOutCubic" },
          { timeMs: 500, value: 1 },
        ],
        shiftY: [
          { timeMs: 0, value: 0, ease: "easeOutCubic" },
          { timeMs: 100, value: -10, ease: "easeOutCubic" },
          { timeMs: 300, value: -10, ease: "easeOutCubic" },
          { timeMs: 500, value: 0 },
        ],
        accent: [
          { timeMs: 0, value: 0 },
          { timeMs: 60, value: 1, ease: "easeOutCubic" },
          { timeMs: 260, value: 1, ease: "easeOutCubic" },
          { timeMs: 500, value: 0 },
        ],
      },
    },
    "react-curious": {
      name: "react-curious",
      durationMs: 400,
      tracks: {
        shiftY: [
          { timeMs: 0, value: 0, ease: "easeOutCubic" },
          { timeMs: 100, value: -6, ease: "easeOutCubic" },
          { timeMs: 250, value: -6, ease: "easeOutCubic" },
          { timeMs: 400, value: 0 },
        ],
        accent: [
          { timeMs: 0, value: 0 },
          { timeMs: 50, value: 1, ease: "easeOutCubic" },
          { timeMs: 200, value: 1, ease: "easeOutCubic" },
          { timeMs: 400, value: 0 },
        ],
      },
    },
    carried: {
      name: "carried",
      durationMs: 240,
      loop: true,
      tracks: {
        tilt: [
          { timeMs: 0, value: 0 },
          { timeMs: 60, value: 5 },
          { timeMs: 120, value: 0 },
          { timeMs: 180, value: -5 },
          { timeMs: 240, value: 0 },
        ],
      },
    },
    landed: {
      name: "landed",
      durationMs: 400,
      tracks: {
        squash: [
          { timeMs: 0, value: 1, ease: "easeOutCubic" },
          { timeMs: 90, value: 0.9, ease: "easeOutCubic" },
          { timeMs: 200, value: 1.05, ease: "easeOutCubic" },
          { timeMs: 340, value: 1, ease: "easeOutCubic" },
          { timeMs: 400, value: 1 },
        ],
        shiftY: [
          { timeMs: 0, value: 0 },
          { timeMs: 90, value: 2 },
          { timeMs: 400, value: 0 },
        ],
      },
    },
    falling: {
      name: "falling",
      durationMs: 300,
      loop: true,
      tracks: {
        tilt: [
          { timeMs: 0, value: 0 },
          { timeMs: 75, value: 6 },
          { timeMs: 150, value: 0 },
          { timeMs: 225, value: -6 },
          { timeMs: 300, value: 0 },
        ],
      },
    },
    stroll: {
      name: "stroll",
      durationMs: 500,
      loop: true,
      tracks: {
        shiftY: [
          { timeMs: 0, value: 0 },
          { timeMs: 125, value: -3 },
          { timeMs: 250, value: 0 },
          { timeMs: 375, value: -3 },
          { timeMs: 500, value: 0 },
        ],
        lookX: [
          { timeMs: 0, value: 0 },
          { timeMs: 250, value: 0.3 },
          { timeMs: 500, value: 0 },
        ],
      },
    },
  };
}
