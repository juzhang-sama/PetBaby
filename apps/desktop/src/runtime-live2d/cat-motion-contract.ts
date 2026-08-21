export const CAT_MOTION_SET_V1 = [
  "breathing",
  "blink",
  "ear-twitch",
  "tail-idle",
  "pointer-focus",
  "pet-happy",
  "sleepy-yawn",
  "half-stand-stretch",
] as const;

export const CAT_PARAMETER_SET_V1 = [
  "eyeOpenLeft",
  "eyeOpenRight",
  "eyeBallX",
  "eyeBallY",
  "earLeft",
  "earRight",
  "tailAngle",
  "tailCurl",
  "tailTip",
  "bodyBreath",
  "bodyStretch",
  "mouthOpen",
] as const;

export const CAT_HIT_AREAS_V1 = ["body", "edgeTail"] as const;
export const CAT_EDGE_TAIL_STATES_V1 = ["left", "right", "top", "bottom"] as const;

export type CatMotionNameV1 = (typeof CAT_MOTION_SET_V1)[number];
export type CatParameterNameV1 = (typeof CAT_PARAMETER_SET_V1)[number];
export type CatHitAreaNameV1 = (typeof CAT_HIT_AREAS_V1)[number];
export type CatEdgeTailStateV1 = (typeof CAT_EDGE_TAIL_STATES_V1)[number];

export interface CatAutomationFrame {
  breath: number;
  eyeLeftOpen: number;
  eyeRightOpen: number;
  earLeft: number;
  earRight: number;
  tailAngle: number;
  tailCurl: number;
  tailTip: number;
}
