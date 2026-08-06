import { describe, expect, it } from "vitest";
import { ClipPlayer, sampleClip, sampleTrack, type AnimClip } from "./anim-clip";

describe("sampleTrack", () => {
  const keyframes = [
    { timeMs: 0, value: 0 },
    { timeMs: 100, value: 10 },
  ];

  it("clamps before the first keyframe", () => {
    expect(sampleTrack(keyframes, -50, false)).toBe(0);
  });

  it("clamps after the last keyframe when not looping", () => {
    expect(sampleTrack(keyframes, 500, false)).toBe(10);
  });

  it("interpolates linearly between keyframes", () => {
    expect(sampleTrack(keyframes, 50, false)).toBe(5);
  });

  it("wraps time by the clip duration when looping", () => {
    expect(sampleTrack(keyframes, 150, true, 100)).toBe(5);
  });

  it("returns the final value at the last keyframe", () => {
    expect(sampleTrack(keyframes, 100, false)).toBe(10);
  });
});

describe("sampleClip", () => {
  it("samples only the channels declared by the clip", () => {
    const clip: AnimClip = {
      name: "bounce",
      durationMs: 100,
      tracks: {
        squash: [
          { timeMs: 0, value: 1 },
          { timeMs: 100, value: 1.1 },
        ],
      },
    };
    const out = sampleClip(clip, 50);
    expect(out.squash).toBeCloseTo(1.05, 3);
    expect(out.tilt).toBeUndefined();
  });
});

describe("ClipPlayer", () => {
  it("reports finished after a non-loop clip ends", () => {
    const player = new ClipPlayer({ name: "once", durationMs: 100, tracks: {} });
    player.start(0);
    expect(player.finished).toBe(false);
    player.sample(100);
    expect(player.finished).toBe(true);
  });

  it("never finishes a looping clip", () => {
    const player = new ClipPlayer({ name: "loop", durationMs: 100, loop: true, tracks: {} });
    player.start(0);
    player.sample(10_000);
    expect(player.finished).toBe(false);
  });
});
