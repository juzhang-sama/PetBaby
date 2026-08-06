import { describe, expect, it } from "vitest";
import { createPetClips } from "./pet-clips";

describe("pet clips", () => {
  const clips = createPetClips();

  it("defines a looping idle clip with a 4 second breath track", () => {
    expect(clips.idle.loop).toBe(true);
    expect(clips.idle.durationMs).toBe(4_000);
    expect(clips.idle.tracks.breath?.length).toBeGreaterThanOrEqual(2);
  });

  it("defines one-shot clips that return to idle", () => {
    for (const id of ["look-left", "look-right", "react-happy", "react-curious", "landed"] as const) {
      expect(clips[id].loop ?? false).toBe(false);
    }
  });

  it("carried clip loops with a tilt that swings both ways", () => {
    const carried = clips.carried;
    expect(carried.loop).toBe(true);
    const values = (carried.tracks.tilt ?? []).map((keyframe) => keyframe.value);
    expect(Math.max(...values)).toBeGreaterThan(0);
    expect(Math.min(...values)).toBeLessThan(0);
  });
});
