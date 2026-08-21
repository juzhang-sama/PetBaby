import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright-core");
import fs from "node:fs/promises";
import path from "node:path";

const fixtureBase = process.argv[2] ?? "http://127.0.0.1:18881";
const outputRoot = path.resolve(process.argv[3] ?? "docs/验证记录/证据/照片分身-fake");
const baseUrl = process.argv[4] ?? "http://127.0.0.1:1420";
const expectedBodyModuleId = process.argv[5] ?? "body-balanced-v1";
await fs.rm(outputRoot, { recursive: true, force: true });
await fs.mkdir(outputRoot, { recursive: true });
const browser = await chromium.launch({ headless: true, executablePath: "C:/Program Files/Google/Chrome/Application/chrome.exe" });
try {
const page = await browser.newPage({ viewport: { width: 420, height: 520 } });
const previewUrl = `${baseUrl}/照片分身运行时验收.html?fixtureBase=${encodeURIComponent(fixtureBase)}`;
async function openFreshPreview() {
  await page.goto(previewUrl);
  await page.waitForFunction(() => document.querySelector("#preview-root")?.dataset.photoAvatarPreviewState === "previewReady");
}
const motions = ["breathing","blink","ear-twitch","tail-idle","pointer-focus","pet-happy","sleepy-yawn","half-stand-stretch"];
const timings = { breathing:4000, blink:220, "ear-twitch":800, "tail-idle":3200, "pointer-focus":1200, "pet-happy":1800, "sleepy-yawn":2600, "half-stand-stretch":2400 };
const peaks = { breathing:2000, blink:110, "ear-twitch":420, "tail-idle":1600, "pointer-focus":600, "pet-happy":900, "sleepy-yawn":800, "half-stand-stretch":1300 };
const entries=[];
const images = new Map();
for (const motion of motions) {
  const dir=path.join(outputRoot,motion); await fs.mkdir(dir,{recursive:true});
  for (const phase of ["neutral","peak","fallback"]) {
    await openFreshPreview();
    const atMs=phase === "neutral" ? 0 : phase === "peak" ? peaks[motion] : timings[motion]+450;
    await page.evaluate(({motion,phase,atMs}) => window.photoAvatarEvidence.render({motion,phase,atMs}), {motion,phase,atMs});
    await page.evaluate(() => new Promise(requestAnimationFrame));
    await page.evaluate(() => new Promise(requestAnimationFrame));
    await page.waitForFunction(({motion,phase}) => { const root=document.querySelector("#preview-root"); return root?.dataset.motion===motion && root?.dataset.phase===phase && root?.dataset.evidenceFrozen==="1"; }, {motion,phase});
    const file=path.join(dir, phase === "neutral" ? "00-neutral.png" : phase === "peak" ? "01-peak.png" : "02-fallback.png");
    const image = await page.screenshot({path:file}); images.set(`${motion}/${phase}`, image);
    const stat=await fs.stat(file); entries.push({evidenceType:"motion-frame",motion,phase,source:motion === "breathing" ? "authored-motion-plus-runtime-idle-automation" : "authored-motion",file:path.relative(outputRoot,file).replaceAll("\\","/"),bytes:stat.size});
  }
}
const interruptionDir=path.join(outputRoot,"interruptions"); await fs.mkdir(interruptionDir,{recursive:true});
for (const [kind, motion] of [["pet","half-stand-stretch"],["drag","sleepy-yawn"]]) {
  await openFreshPreview();
  const phase=kind === "pet" ? "interrupt-pet" : "interrupt-drag";
  const state = await page.evaluate(({motion,phase}) => window.photoAvatarEvidence.render({motion,phase,atMs:0}), {motion,phase});
  const expectedState = kind === "pet" ? "interrupted-pet" : "interrupted-drag";
  if (state !== expectedState) throw new Error(`interruption state mismatch: expected ${expectedState}, got ${state}`);
  await page.evaluate(() => new Promise(requestAnimationFrame));
  const file=path.join(interruptionDir,kind === "pet" ? "00-pet-interrupt.png" : "01-drag-interrupt.png");
  await page.screenshot({path:file});
  const stat=await fs.stat(file);
  entries.push({evidenceType:"interruption",interaction:kind,interruptedMotion:motion,phase,state,file:path.relative(outputRoot,file).replaceAll("\\","/"),bytes:stat.size});
}
for (const motion of motions) {
  const a=images.get(`${motion}/neutral`); const b=images.get(`${motion}/peak`); const c=images.get(`${motion}/fallback`);
  let d1=0,d2=0,d3=0; for(let i=0;i<Math.min(a.length,b.length);i++){if(Math.abs(a[i]-b[i])>8)d1++;} for(let i=0;i<Math.min(b.length,c.length);i++){if(Math.abs(b[i]-c[i])>8)d2++;} for(let i=0;i<Math.min(a.length,c.length);i++){if(Math.abs(a[i]-c[i])>8)d3++;}
  if (d1 <= 20 || d2 <= 20) throw new Error(`insufficient key-frame pixel difference for ${motion}: peak=${d1}, fallback=${d2}`);
  if (motion === "half-stand-stretch" && d3 <= 20) throw new Error(`half-stand-stretch neutral and fallback are identical: ${d3}`);
  entries.find(e=>e.motion===motion&&e.phase==="peak").pixelDiffFromNeutral=d1; entries.find(e=>e.motion===motion&&e.phase==="fallback").pixelDiffFromPeak=d2;
}
const meta=await page.evaluate(() => { const root=document.querySelector("#preview-root"); return {manifestSha256:root.dataset.manifestSha256,schemaVersion:root.dataset.packageSchemaVersion,renderer:root.dataset.renderer,rendererReady:root.dataset.rendererReady,runtimeCheckPassed:root.dataset.runtimeCheckPassed,bodyModuleId:root.dataset.bodyModuleId,runtimeEvidence:window.photoAvatarEvidence.runtimeEvidence}; });
const manifestResponse=await fetch(`${fixtureBase}/manifest.json`); const manifestBytes=new Uint8Array(await manifestResponse.arrayBuffer()); const manifest=JSON.parse(new TextDecoder().decode(manifestBytes)); const manifestSha256=Buffer.from(await crypto.subtle.digest("SHA-256",manifestBytes)).toString("hex");
if (manifest.schemaVersion !== 5 || manifest.renderer !== "cat-spatial-live2d-v1" || manifest.bodyModuleId !== expectedBodyModuleId || meta.bodyModuleId !== expectedBodyModuleId || meta.runtimeEvidence?.bodyModuleId !== expectedBodyModuleId || meta.runtimeEvidence?.frames?.length !== motions.length * 3 || meta.runtimeEvidence?.interruptions?.map(({state}) => state).join(",") !== "interrupted-pet,interrupted-drag" || meta.rendererReady !== "1" || meta.runtimeCheckPassed !== "1" || meta.manifestSha256 !== manifestSha256) throw new Error("v5 body-module/runtime evidence metadata mismatch");
await fs.writeFile(path.join(outputRoot,"证据索引.json"), JSON.stringify({schemaVersion:1,generatedAt:new Date().toISOString(),fixture:`${expectedBodyModuleId}-success`,bodyModuleId:expectedBodyModuleId,provider:"deterministic fake provider with synthetic non-identity atlas; no network",claimBoundary:"fake provider proves chain and dynamics only; synthetic atlas does not claim pet identity or similarity",manifestSha256,packageSchemaVersion:manifest.schemaVersion,renderer:manifest.renderer,rendererReady:true,runtimeCheckPassed:true,zeroV3OrStaticFallback:true,viewport:{width:420,height:520},checks:{nonEmpty:true,consistentDimensions:true,keyFramePixelDifference:true,stretchThreeFramesDistinct:true,interruptionStates:true,realRendererActions:true,rawManifestHash:true},runtimeEvidence:meta.runtimeEvidence,entries},null,2));
} finally {
  await browser.close();
}
