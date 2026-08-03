import { invoke } from "@tauri-apps/api/core";
import { Application, Assets, Container, Graphics, Sprite } from "pixi.js";

interface CalibrationParams {
  schemaVersion: 1;
  breathAmplitudePercent: number;
  blinkIntervalScale: number;
  feedbackStrength: number;
}

const DEFAULT_PARAMS: CalibrationParams = {
  schemaVersion: 1,
  breathAmplitudePercent: 2,
  blinkIntervalScale: 1,
  feedbackStrength: 0.6,
};

const breathEl = document.querySelector<HTMLInputElement>("#breath");
const blinkEl = document.querySelector<HTMLInputElement>("#blink");
const feedbackEl = document.querySelector<HTMLInputElement>("#feedback");
const saveBtnEl = document.querySelector<HTMLButtonElement>("#save");
const statusEl = document.querySelector<HTMLDivElement>("#status");
const previewEl = document.querySelector<HTMLDivElement>("#preview");
if (!breathEl || !blinkEl || !feedbackEl || !saveBtnEl || !statusEl || !previewEl) {
  throw new Error("calibration page is missing required elements");
}
const breath = breathEl;
const blink = blinkEl;
const feedback = feedbackEl;
const saveBtn = saveBtnEl;
const status = statusEl;
const preview = previewEl;

const loadCalibration = (): Promise<CalibrationParams | null> =>
  invoke("pet_calibration_load").then((value) => (value ? value as CalibrationParams : null));
const saveCalibration = (value: CalibrationParams): Promise<void> =>
  invoke("pet_calibration_save", { value });

let params = { ...DEFAULT_PARAMS };
let spriteScale = 1;

function updateLabels(): void {
  document.querySelector<HTMLSpanElement>("#breath-v")!.textContent = `${params.breathAmplitudePercent}%`;
  document.querySelector<HTMLSpanElement>("#blink-v")!.textContent = `${params.blinkIntervalScale}x`;
  document.querySelector<HTMLSpanElement>("#feedback-v")!.textContent = params.feedbackStrength.toFixed(2);
}

function updatePreview(pet: Container, body: Sprite, squish: Graphics): void {
  const amplitude = params.breathAmplitudePercent / 100;
  const phase = performance.now() / 4000;
  const breathe = 1 + Math.sin(phase * Math.PI * 2) * amplitude;
  const bounce = Math.abs(Math.sin(phase * Math.PI)) * params.feedbackStrength * 0.08;
  body.scale.set(1 / breathe, breathe);
  squish.alpha = params.feedbackStrength * 0.6;
  pet.position.y = -bounce * 60;
}

async function main(): Promise<void> {
  const loaded = await loadCalibration();
  if (loaded) params = loaded;
  breath.value = String(params.breathAmplitudePercent);
  blink.value = String(params.blinkIntervalScale);
  feedback.value = String(params.feedbackStrength);
  updateLabels();

  const app = new Application();
  await app.init({ resizeTo: preview, backgroundAlpha: 0, antialias: true, preference: "webgl" });
  preview.replaceChildren(app.canvas);

  const bodyTexture = await Assets.load("/test-assets/layered/body.png");
  const body = new Sprite(bodyTexture);
  body.anchor.set(0.5, 1);
  body.scale.set(0.35);
  const squish = new Graphics()
    .ellipse(0, 0, 60, 18)
    .fill({ color: 0xff8a8a, alpha: 0 });
  squish.position.set(0, 60);
  const pet = new Container();
  pet.addChild(squish, body);
  app.stage.addChild(pet);

  app.ticker.add(() => updatePreview(pet, body, squish));

  breath.addEventListener("input", () => {
    params.breathAmplitudePercent = Number(breath.value);
    updateLabels();
  });
  blink.addEventListener("input", () => {
    params.blinkIntervalScale = Number(blink.value);
    updateLabels();
  });
  feedback.addEventListener("input", () => {
    params.feedbackStrength = Number(feedback.value);
    updateLabels();
  });

  saveBtn.addEventListener("click", async () => {
    try {
      await saveCalibration(params);
      status.textContent = "已保存";
    } catch (error) {
      status.textContent = `保存失败: ${String(error)}`;
    }
  });
}

void main();
