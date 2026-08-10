import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readdir, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

import { build } from "esbuild";

import { describe, expect, it } from "vitest";

import type { ComposerRecipe } from "./contracts";
import { parseComposerPack, validateRecipe, type ComposerPackManifest } from "./composer-pack";
import { motionProfileForRecipe } from "./composer-renderer";

const packRoot = fileURLToPath(new URL("../../public/creation-content/composer/cat-cute-v1/", import.meta.url));
const galleryPath = fileURLToPath(new URL("../../../../docs/验证记录/证据/三条宠物创建链路/组合素材总览.png", import.meta.url));

const expected = {
  bodies: ["body-round", "body-slim", "body-fluffy"],
  ears: ["ears-round", "ears-pointed", "ears-folded", "ears-tufted"],
  eyes: ["eyes-amber", "eyes-blue", "eyes-green", "eyes-gold", "eyes-violet"],
  muzzles: ["muzzle-gentle", "muzzle-smile", "muzzle-curious", "muzzle-sleepy"],
  tails: ["tail-curl", "tail-straight", "tail-plume", "tail-short"],
  colors: ["color-cream", "color-orange", "color-gray", "color-black", "color-white", "color-brown"],
  patterns: ["pattern-none", "pattern-tabby", "pattern-tuxedo", "pattern-calico", "pattern-spots"],
} as const;

type Raster = { width: number; height: number; pixels: Uint8ClampedArray };

function readU32(data: Uint8Array, offset: number): number {
  return ((data[offset]! << 24) | (data[offset + 1]! << 16) | (data[offset + 2]! << 8) | data[offset + 3]!) >>> 0;
}

function decodeRgbaPng(data: Uint8Array): Raster {
  expect([...data.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  let offset = 8;
  let width = 0;
  let height = 0;
  const idat: Uint8Array[] = [];
  while (offset + 12 <= data.length) {
    const length = readU32(data, offset);
    const type = new TextDecoder().decode(data.subarray(offset + 4, offset + 8));
    const chunk = data.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = readU32(chunk, 0);
      height = readU32(chunk, 4);
      expect([...chunk.subarray(8, 13)]).toEqual([8, 6, 0, 0, 0]);
    } else if (type === "IDAT") {
      idat.push(chunk);
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }
  const compressed = Buffer.concat(idat.map((chunk) => Buffer.from(chunk)));
  const raw = inflateSync(compressed);
  const stride = width * 4;
  expect(raw.length).toBe((stride + 1) * height);
  const pixels = new Uint8ClampedArray(stride * height);
  let source = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[source++]!;
    expect(filter).toBeLessThanOrEqual(4);
    for (let x = 0; x < stride; x += 1) {
      const encoded = raw[source++]!;
      const left = x >= 4 ? pixels[y * stride + x - 4]! : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x]! : 0;
      const upperLeft = y > 0 && x >= 4 ? pixels[(y - 1) * stride + x - 4]! : 0;
      let predictor = 0;
      if (filter === 1) predictor = left;
      else if (filter === 2) predictor = up;
      else if (filter === 3) predictor = Math.floor((left + up) / 2);
      else if (filter === 4) {
        const candidate = left + up - upperLeft;
        const pa = Math.abs(candidate - left);
        const pb = Math.abs(candidate - up);
        const pc = Math.abs(candidate - upperLeft);
        predictor = pa <= pb && pa <= pc ? left : pb <= pc ? up : upperLeft;
      }
      pixels[y * stride + x] = (encoded + predictor) & 0xff;
    }
  }
  return { width, height, pixels };
}

async function readProductionPack(): Promise<ComposerPackManifest> {
  return parseComposerPack(JSON.parse(await readFile(path.join(packRoot, "manifest.json"), "utf8")));
}

function declaredPaths(pack: ComposerPackManifest): string[] {
  const values: string[] = [];
  for (const item of [...pack.bodies, ...pack.ears, ...pack.muzzles, ...pack.tails]) {
    values.push(item.image);
    if (item.colorMask) values.push(item.colorMask);
    if (item.patternMask) values.push(item.patternMask);
  }
  for (const eye of pack.eyes) values.push(eye.openImage, eye.closedImage);
  for (const pattern of pack.patterns) if (pattern.image) values.push(pattern.image);
  return [...new Set(values)].sort();
}

async function diskPngs(directory: string, root = directory): Promise<string[]> {
  const values: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) values.push(...await diskPngs(absolute, root));
    else if (entry.isFile() && entry.name.endsWith(".png")) values.push(path.relative(root, absolute).replaceAll("\\", "/"));
  }
  return values.sort();
}

function hash(data: Uint8Array): string { return createHash("sha256").update(data).digest("hex"); }

function recipe(values: Omit<ComposerRecipe, "recipeVersion" | "packId" | "packVersion" | "layerContractVersion">): ComposerRecipe {
  return { recipeVersion: 1, packId: "cat-cute-v1", packVersion: 1, layerContractVersion: 1, ...values };
}

const representativeRecipes = [
  recipe({ bodyId: "body-round", earsId: "ears-round", eyesId: "eyes-amber", muzzleId: "muzzle-gentle", tailId: "tail-curl", colorId: "color-cream", patternId: "pattern-none" }),
  recipe({ bodyId: "body-slim", earsId: "ears-pointed", eyesId: "eyes-green", muzzleId: "muzzle-curious", tailId: "tail-straight", colorId: "color-gray", patternId: "pattern-tabby" }),
  recipe({ bodyId: "body-fluffy", earsId: "ears-tufted", eyesId: "eyes-gold", muzzleId: "muzzle-smile", tailId: "tail-plume", colorId: "color-orange", patternId: "pattern-calico" }),
  recipe({ bodyId: "body-round", earsId: "ears-tufted", eyesId: "eyes-gold", muzzleId: "muzzle-sleepy", tailId: "tail-short", colorId: "color-black", patternId: "pattern-calico" }),
  recipe({ bodyId: "body-slim", earsId: "ears-round", eyesId: "eyes-violet", muzzleId: "muzzle-gentle", tailId: "tail-curl", colorId: "color-white", patternId: "pattern-spots" }),
  recipe({ bodyId: "body-fluffy", earsId: "ears-pointed", eyesId: "eyes-amber", muzzleId: "muzzle-smile", tailId: "tail-straight", colorId: "color-brown", patternId: "pattern-none" }),
  recipe({ bodyId: "body-round", earsId: "ears-folded", eyesId: "eyes-blue", muzzleId: "muzzle-curious", tailId: "tail-plume", colorId: "color-cream", patternId: "pattern-tabby" }),
  recipe({ bodyId: "body-slim", earsId: "ears-tufted", eyesId: "eyes-green", muzzleId: "muzzle-sleepy", tailId: "tail-short", colorId: "color-orange", patternId: "pattern-tuxedo" }),
  recipe({ bodyId: "body-fluffy", earsId: "ears-round", eyesId: "eyes-gold", muzzleId: "muzzle-gentle", tailId: "tail-curl", colorId: "color-gray", patternId: "pattern-calico" }),
  recipe({ bodyId: "body-round", earsId: "ears-pointed", eyesId: "eyes-violet", muzzleId: "muzzle-smile", tailId: "tail-straight", colorId: "color-black", patternId: "pattern-spots" }),
  recipe({ bodyId: "body-slim", earsId: "ears-folded", eyesId: "eyes-amber", muzzleId: "muzzle-curious", tailId: "tail-plume", colorId: "color-white", patternId: "pattern-none" }),
  recipe({ bodyId: "body-fluffy", earsId: "ears-tufted", eyesId: "eyes-blue", muzzleId: "muzzle-sleepy", tailId: "tail-short", colorId: "color-brown", patternId: "pattern-tabby" }),
] as const;

function alphaStats(raster: Raster): { transparent: number; visible: number; opaque: number } {
  let transparent = 0; let visible = 0; let opaque = 0;
  for (let index = 3; index < raster.pixels.length; index += 4) {
    const alpha = raster.pixels[index]!;
    if (alpha === 0) transparent += 1;
    else { visible += 1; if (alpha === 255) opaque += 1; }
  }
  return { transparent, visible, opaque };
}

async function renderRecipesInChrome(
  pack: ComposerPackManifest,
  recipes: readonly ComposerRecipe[],
): Promise<{ outputs: Uint8Array[]; gallery: Uint8Array }> {
  const entry = `
    import { parseComposerPack } from "./composer-pack.ts";
    import { exportComposerPng } from "./composer-renderer.ts";
    const pack = parseComposerPack(${JSON.stringify(pack)});
    const recipes = ${JSON.stringify(recipes)};
    const ports = {
      createSurface(width, height) { const canvas = document.createElement("canvas"); canvas.width = width; canvas.height = height; return canvas; },
      context(surface) { const context = surface.getContext("2d"); if (!context) throw new Error("native 2D canvas unavailable"); return context; },
      loadImage(url) { return new Promise((resolve, reject) => { const image = new Image(); image.onload = () => resolve(image); image.onerror = () => reject(new Error("image load failed: " + url)); image.src = url; }); },
      assetUrl(relativePath) { return "/asset/" + relativePath; },
      toPng(surface) { return new Promise((resolve, reject) => surface.toBlob((blob) => blob ? resolve(blob) : reject(new Error("canvas PNG export failed")), "image/png")); },
    };
    async function send(url, body) { const response = await fetch(url, { method: "POST", body }); if (!response.ok) throw new Error(url + " upload failed"); }
    (async () => { try {
      const blobs = [];
      for (let index = 0; index < recipes.length; index += 1) {
        const blob = await exportComposerPng(pack, recipes[index], ports);
        blobs.push(blob);
        await send("/result/" + index, blob);
      }
      const gallery = document.createElement("canvas"); gallery.width = 1024; gallery.height = 768;
      const context = gallery.getContext("2d");
      for (let y = 0; y < 768; y += 16) for (let x = 0; x < 1024; x += 16) { context.fillStyle = ((x / 16 + y / 16) & 1) ? "#cccccc" : "#e8e8e8"; context.fillRect(x, y, 16, 16); }
      for (let index = 0; index < blobs.length; index += 1) { const bitmap = await createImageBitmap(blobs[index]); context.drawImage(bitmap, index % 4 * 256, Math.floor(index / 4) * 256, 256, 256); bitmap.close(); }
      const galleryBlob = await ports.toPng(gallery); await send("/gallery", galleryBlob);
    } catch (error) { await send("/error", String(error && error.stack || error)); }
    await send("/finished", "done"); })();
  `;
  const bundle = await build({
    stdin: { contents: entry, loader: "ts", resolveDir: path.dirname(fileURLToPath(import.meta.url)), sourcefile: "production-composer-harness.ts" },
    bundle: true,
    format: "iife",
    platform: "browser",
    target: "chrome120",
    write: false,
  });
  const script = bundle.outputFiles[0]!.contents;
  const allowedAssets = new Set(declaredPaths(pack));
  const outputs = new Map<number, Uint8Array>();
  let gallery: Uint8Array | undefined;
  let browserError: string | undefined;
  let resolveFinished!: () => void;
  const finished = new Promise<void>((resolve) => { resolveFinished = resolve; });
  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const body = async () => {
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
      return new Uint8Array(Buffer.concat(chunks));
    };
    try {
      if (url.pathname === "/") { response.setHeader("content-type", "text/html"); response.end('<!doctype html><meta charset="utf-8"><script src="/bundle.js"></script>'); return; }
      if (url.pathname === "/bundle.js") { response.setHeader("content-type", "text/javascript"); response.end(script); return; }
      if (url.pathname.startsWith("/asset/")) {
        const relative = decodeURIComponent(url.pathname.slice(7));
        if (!allowedAssets.has(relative)) { response.statusCode = 404; response.end(); return; }
        response.setHeader("content-type", "image/png"); response.end(await readFile(path.join(packRoot, relative))); return;
      }
      if (url.pathname.startsWith("/result/")) { outputs.set(Number(url.pathname.slice(8)), await body()); response.end("ok"); return; }
      if (url.pathname === "/gallery") { gallery = await body(); response.end("ok"); return; }
      if (url.pathname === "/error") { browserError = new TextDecoder().decode(await body()); response.end("ok"); return; }
      if (url.pathname === "/finished") { await body(); response.end("ok"); resolveFinished(); return; }
      response.statusCode = 404; response.end();
    } catch (error) { response.statusCode = 500; response.end(String(error)); }
  });
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("test server did not bind a TCP port");
  const browser = [
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  ].find(existsSync);
  if (!browser) throw new Error("Chrome/Edge is required for production Canvas content verification");
  const profile = await mkdtemp(path.join(tmpdir(), "pet-baby-composer-browser-"));
  const process = spawn(browser, ["--headless=new", "--disable-gpu", "--no-sandbox", "--remote-debugging-port=0", `--user-data-dir=${profile}`, `http://127.0.0.1:${address.port}/`], { stdio: "ignore" });
  try {
    await Promise.race([finished, new Promise<never>((_, reject) => setTimeout(() => reject(new Error("browser composer verification timed out")), 90_000))]);
  } finally {
    const exited = process.exitCode === null ? new Promise<void>((resolve) => process.once("exit", () => resolve())) : Promise.resolve();
    process.kill();
    await Promise.race([exited, new Promise<void>((resolve) => setTimeout(resolve, 2_000))]);
    await new Promise<void>((resolve) => server.close(() => resolve()));
    try { await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); } catch { /* Browser cleanup is best effort on Windows. */ }
  }
  if (browserError) throw new Error(browserError);
  if (outputs.size !== recipes.length || !gallery) throw new Error(`browser returned ${outputs.size}/${recipes.length} outputs and gallery=${Boolean(gallery)}`);
  return { outputs: recipes.map((_, index) => outputs.get(index)!), gallery };
}

describe("production cat composer content", () => {
  it("ships the complete fixed-ID cat-cute-v1 pack with legal defaults and motion semantics", async () => {
    const pack = await readProductionPack();
    expect(pack).toMatchObject({ schemaVersion: 1, packId: "cat-cute-v1", packVersion: 1, species: "cat", canvas: { width: 1024, height: 1024 }, layerContractVersion: 1 });
    for (const [category, ids] of Object.entries(expected)) expect(pack[category as keyof typeof expected].map(({ id }) => id)).toEqual(ids);
    for (const body of pack.bodies) {
      const defaults = recipe({ bodyId: body.id, ...body.defaults });
      expect(validateRecipe(pack, defaults)).toEqual([]);
      expect(() => motionProfileForRecipe(pack, defaults)).not.toThrow();
      expect(body.faceSafeZone.bottom).toBeLessThanOrEqual(body.breathZone.top);
      expect(body.faceSafeZone.left).toBeGreaterThanOrEqual(body.alphaBounds.left);
      expect(body.faceSafeZone.right).toBeLessThanOrEqual(body.alphaBounds.right);
    }
  });

  it("declares exactly every safe production PNG", async () => {
    const pack = await readProductionPack();
    const root = await realpath(packRoot);
    expect(declaredPaths(pack)).toEqual(await diskPngs(packRoot));
    for (const relative of declaredPaths(pack)) {
      expect(relative).toMatch(/^[a-z0-9][a-z0-9/-]*\.png$/);
      const absolute = await realpath(path.join(packRoot, relative));
      expect(absolute.startsWith(root + path.sep)).toBe(true);
      expect((await stat(absolute)).isFile()).toBe(true);
    }
  });

  it("fully decodes every image as transparent non-empty 1024 RGBA", async () => {
    const pack = await readProductionPack();
    for (const relative of declaredPaths(pack)) {
      const raster = decodeRgbaPng(await readFile(path.join(packRoot, relative)));
      expect([raster.width, raster.height], relative).toEqual([1024, 1024]);
      const stats = alphaStats(raster);
      expect(stats.visible, relative).toBeGreaterThan(100);
      expect(stats.transparent, relative).toBeGreaterThan(1024);
      let chromaResidue = 0;
      for (let index = 0; index < raster.pixels.length; index += 4) {
        const [red, green, blue, alpha] = raster.pixels.subarray(index, index + 4);
        if (alpha! > 8 && ((red! > 240 && blue! > 240 && green! < 20) || (green! > 240 && red! < 20 && blue! < 20))) {
          chromaResidue += 1;
        }
      }
      expect(chromaResidue, `${relative} retains visible chroma-key pixels`).toBe(0);
      for (const [x, y] of [[0, 0], [1023, 0], [0, 1023], [1023, 1023]] as const) {
        expect(raster.pixels[(y * 1024 + x) * 4 + 3], relative).toBe(0);
      }
    }
  });

  it("keeps every tail away from canvas crops and free of long flat alpha cutoffs", async () => {
    const pack = await readProductionPack();
    for (const tail of pack.tails) {
      const raster = decodeRgbaPng(await readFile(path.join(packRoot, tail.image)));
      const visible = (x: number, y: number) => raster.pixels[(y * raster.width + x) * 4 + 3]! > 8;
      let left = raster.width;
      let top = raster.height;
      let right = -1;
      let bottom = -1;
      for (let y = 0; y < raster.height; y += 1) {
        for (let x = 0; x < raster.width; x += 1) {
          if (!visible(x, y)) continue;
          left = Math.min(left, x);
          top = Math.min(top, y);
          right = Math.max(right, x);
          bottom = Math.max(bottom, y);
        }
      }
      expect(left, tail.image).toBeGreaterThan(16);
      expect(top, tail.image).toBeGreaterThan(16);
      expect(right, tail.image).toBeLessThan(1007);
      expect(bottom, tail.image).toBeLessThan(1007);
      const longestRun = (values: boolean[]) => {
        let longest = 0;
        let current = 0;
        for (const value of values) {
          current = value ? current + 1 : 0;
          longest = Math.max(longest, current);
        }
        return longest;
      };
      const boundaryRuns = [
        longestRun(Array.from({ length: bottom - top + 1 }, (_, offset) => visible(left, top + offset))),
        longestRun(Array.from({ length: bottom - top + 1 }, (_, offset) => visible(right, top + offset))),
        longestRun(Array.from({ length: right - left + 1 }, (_, offset) => visible(left + offset, top))),
        longestRun(Array.from({ length: right - left + 1 }, (_, offset) => visible(left + offset, bottom))),
      ];
      expect(Math.max(...boundaryRuns), `${tail.image} has a long flat alpha edge: ${boundaryRuns.join(",")}`).toBeLessThan(80);
    }
  });

  it("has distinct authored variants and honest eye/pattern state files", async () => {
    const pack = await readProductionPack();
    const hashes = async (paths: readonly string[]) => Promise.all(paths.map(async (value) => hash(await readFile(path.join(packRoot, value)))));
    for (const paths of [pack.bodies.map((x) => x.image), pack.ears.map((x) => x.image), pack.muzzles.map((x) => x.image), pack.tails.map((x) => x.image)]) {
      const values = await hashes(paths); expect(new Set(values).size).toBe(values.length);
    }
    for (const eye of pack.eyes) expect(hash(await readFile(path.join(packRoot, eye.openImage)))).not.toBe(hash(await readFile(path.join(packRoot, eye.closedImage))));
    expect(pack.patterns[0]).toEqual({ id: "pattern-none", image: null });
    const patternHashes = await hashes(pack.patterns.slice(1).map((x) => x.image!));
    expect(new Set(patternHashes).size).toBe(4);
  });

  it("keeps every non-empty mask strictly inside its authored part alpha", async () => {
    const pack = await readProductionPack();
    for (const item of [...pack.bodies, ...pack.ears, ...pack.tails]) {
      const base = decodeRgbaPng(await readFile(path.join(packRoot, item.image)));
      for (const maskPath of [item.colorMask, item.patternMask]) {
        expect(maskPath).toBeDefined();
        const mask = decodeRgbaPng(await readFile(path.join(packRoot, maskPath!)));
        let visible = 0; let outside = 0;
        for (let index = 3; index < mask.pixels.length; index += 4) if (mask.pixels[index]! > 8) {
          visible += 1;
          if (base.pixels[index]! <= 2) outside += 1;
        }
        expect(visible, maskPath).toBeGreaterThan(100);
        expect(visible, maskPath).toBeLessThan(1024 * 1024 * 0.75);
        expect(outside, maskPath).toBe(0);
      }
    }
  });

  it("exports 12 representative recipes through the production renderer and writes the checked gallery", async () => {
    const pack = await readProductionPack();
    const hashes = new Set<string>();
    expect(new Set(representativeRecipes.map((selected) => selected.bodyId))).toEqual(new Set(expected.bodies));
    expect(new Set(representativeRecipes.map((selected) => selected.earsId))).toEqual(new Set(expected.ears));
    expect(new Set(representativeRecipes.map((selected) => selected.eyesId))).toEqual(new Set(expected.eyes));
    expect(new Set(representativeRecipes.map((selected) => selected.muzzleId))).toEqual(new Set(expected.muzzles));
    expect(new Set(representativeRecipes.map((selected) => selected.tailId))).toEqual(new Set(expected.tails));
    expect(new Set(representativeRecipes.map((selected) => selected.colorId))).toEqual(new Set(expected.colors));
    expect(new Set(representativeRecipes.map((selected) => selected.patternId))).toEqual(new Set(expected.patterns));
    for (const selected of representativeRecipes) {
      expect(validateRecipe(pack, selected)).toEqual([]);
      expect(() => motionProfileForRecipe(pack, selected)).not.toThrow();
    }
    const browser = await renderRecipesInChrome(pack, representativeRecipes);
    for (const bytes of browser.outputs) {
      const raster = decodeRgbaPng(bytes);
      expect([raster.width, raster.height]).toEqual([1024, 1024]);
      const stats = alphaStats(raster);
      expect(stats.visible).toBeGreaterThan(100_000);
      expect(stats.transparent).toBeGreaterThan(100_000);
      hashes.add(hash(bytes));
    }
    expect(hashes.size).toBe(representativeRecipes.length);
    await writeFile(galleryPath, browser.gallery);
    expect(decodeRgbaPng(await readFile(galleryPath))).toMatchObject({ width: 1024, height: 768 });
  }, 120_000);
});
