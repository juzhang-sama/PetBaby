import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { BODY_MODULE_IDS_V1 } from "../runtime-assets/cat-motion-spatial-profile";
import { CAT_MOTION_SET_V1 } from "./cat-motion-contract";
import {
  advanceCatMotionEvidenceTime,
  assertCompleteCatMotionEvidence,
  CAT_MOTION_EVIDENCE_TIMING,
  isCatMotionEvidenceMode,
  parseCatMotionEvidencePhase,
  parseCatMotionEvidenceRequest,
  renderCatMotionEvidencePhase,
} from "./cat-motion-evidence";

function completeEvidence(bodyModuleId: (typeof BODY_MODULE_IDS_V1)[number]) {
  return {
    schemaVersion: 1 as const,
    bodyModuleId,
    renderer: "cat-spatial-live2d-v1" as const,
    frames: CAT_MOTION_SET_V1.flatMap((motion, motionIndex) => (
      ["neutral", "peak", "fallback"] as const
    ).map((phase, phaseIndex) => ({
      motion,
      phase,
      framebufferNonEmpty: true as const,
      changedPixelCount: phase === "neutral" ? 0 : 21,
      sha256: (motionIndex * 3 + phaseIndex + 1).toString(16).padStart(64, "0"),
      renderer: "cat-spatial-live2d-v1" as const,
    }))),
    interruptions: [
      {
        motion: "half-stand-stretch" as const,
        phase: "interrupt-pet" as const,
        state: "interrupted-pet" as const,
        framebufferNonEmpty: true as const,
        sha256: "a".repeat(64),
        renderer: "cat-spatial-live2d-v1" as const,
      },
      {
        motion: "sleepy-yawn" as const,
        phase: "interrupt-drag" as const,
        state: "interrupted-drag" as const,
        framebufferNonEmpty: true as const,
        sha256: "b".repeat(64),
        renderer: "cat-spatial-live2d-v1" as const,
      },
    ],
  };
}

describe("standard cat motion evidence mode", () => {
  it.each(BODY_MODULE_IDS_V1)("requires complete real-renderer motion evidence for %s", (bodyModuleId) => {
    expect(assertCompleteCatMotionEvidence(completeEvidence(bodyModuleId))).toEqual(
      completeEvidence(bodyModuleId),
    );
  });

  it("rejects identical stretch phases or a mismatched interruption state", () => {
    const identicalStretch = completeEvidence("body-balanced-v1");
    const stretchNeutral = identicalStretch.frames.find((entry) => (
      entry.motion === "half-stand-stretch" && entry.phase === "neutral"
    ))!;
    const stretchFallback = identicalStretch.frames.find((entry) => (
      entry.motion === "half-stand-stretch" && entry.phase === "fallback"
    ))!;
    stretchFallback.sha256 = stretchNeutral.sha256;
    expect(() => assertCompleteCatMotionEvidence(identicalStretch)).toThrow(/half-stand-stretch.*distinct/i);

    const wrongInterruption = completeEvidence("body-balanced-v1");
    wrongInterruption.interruptions[0]!.state = "interrupted-drag";
    expect(() => assertCompleteCatMotionEvidence(wrongInterruption)).toThrow(/interrupted-pet/i);
  });

  it("only accepts the explicit evidence flag with a supported action", () => {
    expect(isCatMotionEvidenceMode("?catMotionEvidence=1&motion=breathing")).toBe(true);
    expect(isCatMotionEvidenceMode("?catMotionEvidence=0&motion=breathing")).toBe(false);
    expect(isCatMotionEvidenceMode("?catMotionEvidence=1&motion=unknown")).toBe(false);
  });

  it("parses one of the fixed eight motions and rejects unknown input", () => {
    expect(parseCatMotionEvidenceRequest("?catMotionEvidence=1&motion=pet-happy")).toEqual({
      motion: "pet-happy",
      timing: CAT_MOTION_EVIDENCE_TIMING["pet-happy"],
    });
    expect(() => parseCatMotionEvidenceRequest("?catMotionEvidence=1&motion=dance"))
      .toThrow(/unsupported evidence motion/i);
  });

  it("keeps every peak inside the real motion duration", () => {
    for (const timing of Object.values(CAT_MOTION_EVIDENCE_TIMING)) {
      expect(timing.peakMs).toBeGreaterThan(0);
      expect(timing.peakMs).toBeLessThan(timing.durationMs);
      expect(timing.fallbackMs).toBeGreaterThan(timing.durationMs);
    }
  });

  it("uses the real automation duration for natural blink evidence", () => {
    expect(CAT_MOTION_EVIDENCE_TIMING.blink).toEqual({
      durationMs: 220,
      peakMs: 110,
      fallbackMs: 620,
    });
  });

  it("samples the authored ear twitch at its first visible crest", () => {
    expect(CAT_MOTION_EVIDENCE_TIMING["ear-twitch"].peakMs).toBe(420);
  });

  it("clamps sequence frame offsets to the selected motion", () => {
    expect(parseCatMotionEvidencePhase("?phase=frame&atMs=600", 1_200)).toEqual({
      phase: "frame",
      atMs: 600,
    });
    expect(parseCatMotionEvidencePhase("?phase=frame&atMs=9999", 1_200)).toEqual({
      phase: "frame",
      atMs: 1_199,
    });
    expect(parseCatMotionEvidencePhase("?phase=neutral", 1_200)).toEqual({ phase: "neutral" });
  });

  it("advances evidence playback synchronously to an exact frozen frame", () => {
    const deltas: number[] = [];

    advanceCatMotionEvidenceTime((deltaMs) => deltas.push(deltaMs), 50, 16);

    expect(deltas).toEqual([16, 16, 16, 2]);
    expect(deltas.reduce((sum, deltaMs) => sum + deltaMs, 0)).toBe(50);
  });

  it("does not advance a zero-offset evidence frame beyond its requested time", () => {
    const deltas: number[] = [];

    advanceCatMotionEvidenceTime((deltaMs) => deltas.push(deltaMs), 0, 16);

    expect(deltas).toEqual([0]);
  });

  it("renders a requested authored-motion frame without leaving a live clock running", () => {
    const played: string[] = [];
    let elapsedMs = 0;
    const state = renderCatMotionEvidencePhase({
      playCatMotion: (motion) => {
        played.push(motion);
      },
      setCatAutomationMode: () => undefined,
      setLookTarget: () => undefined,
      update: (deltaMs) => {
        elapsedMs += deltaMs;
      },
    }, {
      motion: "ear-twitch",
      timing: CAT_MOTION_EVIDENCE_TIMING["ear-twitch"],
    }, {
      phase: "frame",
      atMs: 80,
    });

    expect(played).toEqual(["ear-twitch"]);
    expect(elapsedMs).toBe(80);
    expect(state).toBe("frame");
  });

  it("uses the existing idle automation for visible breathing evidence", () => {
    const modes: string[] = [];

    renderCatMotionEvidencePhase({
      playCatMotion: () => undefined,
      setCatAutomationMode: (mode) => {
        modes.push(mode);
      },
      setLookTarget: () => undefined,
      update: () => undefined,
    }, {
      motion: "breathing",
      timing: CAT_MOTION_EVIDENCE_TIMING.breathing,
    }, {
      phase: "peak",
    });

    expect(modes).toEqual(["idle"]);
  });

  it("renders natural blink through the automation path used at runtime", () => {
    const modes: string[] = [];
    const played: string[] = [];
    let elapsedMs = 0;
    const state = renderCatMotionEvidencePhase({
      playCatMotion: (motion) => {
        played.push(motion);
      },
      setCatAutomationMode: (mode) => {
        modes.push(mode);
      },
      setLookTarget: () => undefined,
      update: (deltaMs) => {
        elapsedMs += deltaMs;
      },
    }, {
      motion: "blink",
      timing: { durationMs: 220, peakMs: 110, fallbackMs: 620 },
    }, {
      phase: "peak",
    });

    expect(played).toEqual([]);
    expect(modes).toEqual(["idle"]);
    expect(elapsedMs).toBeCloseTo(2_910, 8);
    expect(state).toBe("peak");
  });

  it("renders authored-motion fallback only after the selected motion has finished", () => {
    const played: string[] = [];
    let elapsedMs = 0;
    const state = renderCatMotionEvidencePhase({
      playCatMotion: (motion) => {
        played.push(motion);
      },
      setCatAutomationMode: () => undefined,
      setLookTarget: () => undefined,
      update: (deltaMs) => {
        elapsedMs += deltaMs;
      },
    }, {
      motion: "ear-twitch",
      timing: CAT_MOTION_EVIDENCE_TIMING["ear-twitch"],
    }, {
      phase: "fallback",
    });

    expect(played).toEqual(["ear-twitch", "breathing"]);
    expect(elapsedMs).toBeCloseTo(
      CAT_MOTION_EVIDENCE_TIMING["ear-twitch"].fallbackMs + 240,
      8,
    );
    expect(state).toBe("fallback");
  });

  it("renders pointer-focus with the same look target used by real pointer input", () => {
    const targets: Array<{ x: number; y: number } | null> = [];
    const state = renderCatMotionEvidencePhase({
      playCatMotion: () => undefined,
      setCatAutomationMode: () => undefined,
      setLookTarget: (target) => targets.push(target),
      update: () => undefined,
    }, {
      motion: "pointer-focus",
      timing: CAT_MOTION_EVIDENCE_TIMING["pointer-focus"],
    }, {
      phase: "peak",
    });

    expect(targets).toEqual([{ x: 0.65, y: 0.35 }]);
    expect(state).toBe("peak");
  });

  it("keeps the recording script aligned with frozen runtime evidence", () => {
    const script = readFileSync(
      resolve(process.cwd(), "..", "..", "scripts", "录制标准猫动作证据.ps1"),
      "utf8",
    );

    expect(script).toMatch(/"blink"\s*=\s*220/);
    expect(script).toContain("frozen:Number(el.dataset.evidenceFrozen) === 1");
    expect(script).toContain("Evidence runtime state mismatch");
  });
});
