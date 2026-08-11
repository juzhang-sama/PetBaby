import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

import { describe, expect, it } from "vitest";

import { parseMotionProfile, type MotionProfileV1 } from "../runtime/animated-image-manifest";

const adoptionRoot = fileURLToPath(new URL("../../public/creation-content/adoption/", import.meta.url));

const expectedTemplates = [
  ["cat-misty", "雾雾", "安静沉稳，喜欢用温柔的陪伴带来安心。"],
  ["cat-tangerine", "橘子", "活泼又好奇，总对身边的新鲜事物充满兴趣。"],
  ["cat-dumpling", "团子", "黏人又温柔，亲近的神情让陪伴格外柔软。"],
  ["cat-ink", "墨墨", "冷静而警觉，沉稳的气质让人感到可靠。"],
  ["cat-cloud", "云朵", "慢热而治愈，安静熟悉后会流露柔软的一面。"],
  ["cat-chestnut", "栗子", "勇敢又可靠，坚定的眼神带着让人安心的力量。"],
  ["cat-sesame", "芝麻", "机灵又贪玩，灵动的模样总透着一点小聪明。"],
  ["cat-starlight", "星星", "梦幻而敏感，细腻的神情像在感受周围每一点变化。"],
] as const;

const templateKeys = [
  "templateId",
  "templateVersion",
  "runtimeSchemaVersion",
  "defaultName",
  "personality",
  "thumbnailPath",
  "bodyPath",
  "motionProfilePath",
  "thumbnailSha256",
  "bodySha256",
  "motionProfileSha256",
] as const;

type Template = {
  [Key in typeof templateKeys[number]]: Key extends "templateVersion" | "runtimeSchemaVersion" ? number : string;
};

type Raster = { width: number; height: number; pixels: Uint8Array };

function object(value: unknown, label: string): Record<string, unknown> {
  expect(value, label).not.toBeNull();
  expect(typeof value, label).toBe("object");
  expect(Array.isArray(value), label).toBe(false);
  return value as Record<string, unknown>;
}

function readU32(data: Uint8Array, offset: number): number {
  return ((data[offset]! << 24) | (data[offset + 1]! << 16) | (data[offset + 2]! << 8) | data[offset + 3]!) >>> 0;
}

function decodeRgbaPng(data: Uint8Array, label: string): Raster {
  expect([...data.subarray(0, 8)], `${label} PNG signature`).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  let offset = 8;
  let width = 0;
  let height = 0;
  let sawHeader = false;
  let sawEnd = false;
  const idat: Uint8Array[] = [];
  while (offset < data.length) {
    expect(offset + 12, `${label} truncated PNG chunk`).toBeLessThanOrEqual(data.length);
    const length = readU32(data, offset);
    expect(offset + 12 + length, `${label} truncated PNG payload`).toBeLessThanOrEqual(data.length);
    const type = new TextDecoder().decode(data.subarray(offset + 4, offset + 8));
    const chunk = data.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      expect(sawHeader, `${label} duplicate IHDR`).toBe(false);
      expect(length, `${label} IHDR length`).toBe(13);
      width = readU32(chunk, 0);
      height = readU32(chunk, 4);
      expect([...chunk.subarray(8, 13)], `${label} must be 8-bit RGBA PNG`).toEqual([8, 6, 0, 0, 0]);
      sawHeader = true;
    } else if (type === "IDAT") {
      idat.push(chunk);
    } else if (type === "IEND") {
      expect(length, `${label} IEND length`).toBe(0);
      sawEnd = true;
      offset += 12;
      break;
    }
    offset += 12 + length;
  }
  expect(sawHeader, `${label} has IHDR`).toBe(true);
  expect(idat.length, `${label} has IDAT`).toBeGreaterThan(0);
  expect(sawEnd, `${label} has IEND`).toBe(true);
  expect(offset, `${label} has no trailing bytes`).toBe(data.length);

  const raw = inflateSync(Buffer.concat(idat.map((chunk) => Buffer.from(chunk))));
  const stride = width * 4;
  expect(raw.length, `${label} fully decoded byte count`).toBe((stride + 1) * height);
  const pixels = new Uint8Array(stride * height);
  let source = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[source++]!;
    expect(filter, `${label} row ${y} filter`).toBeLessThanOrEqual(4);
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
        const leftDistance = Math.abs(candidate - left);
        const upDistance = Math.abs(candidate - up);
        const upperLeftDistance = Math.abs(candidate - upperLeft);
        predictor = leftDistance <= upDistance && leftDistance <= upperLeftDistance
          ? left
          : upDistance <= upperLeftDistance ? up : upperLeft;
      }
      pixels[y * stride + x] = (encoded + predictor) & 0xff;
    }
  }
  return { width, height, pixels };
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function alphaBounds(raster: Raster): { left: number; top: number; right: number; bottom: number; visible: number } {
  let left = raster.width;
  let top = raster.height;
  let right = -1;
  let bottom = -1;
  let visible = 0;
  for (let y = 0; y < raster.height; y += 1) {
    for (let x = 0; x < raster.width; x += 1) {
      if (raster.pixels[(y * raster.width + x) * 4 + 3]! < 8) continue;
      visible += 1;
      left = Math.min(left, x);
      top = Math.min(top, y);
      right = Math.max(right, x);
      bottom = Math.max(bottom, y);
    }
  }
  return { left, top, right, bottom, visible };
}

function expectTransparentProductionRaster(raster: Raster, size: number, label: string): void {
  expect([raster.width, raster.height], label).toEqual([size, size]);
  const bounds = alphaBounds(raster);
  expect(bounds.visible, `${label} visible subject area`).toBeGreaterThan(size * size * 0.08);
  expect(bounds.visible, `${label} retains transparent background`).toBeLessThan(size * size * 0.85);
  expect(bounds.left, `${label} is not cropped on the left`).toBeGreaterThan(8);
  expect(bounds.top, `${label} is not cropped on the top`).toBeGreaterThan(8);
  expect(bounds.right, `${label} is not cropped on the right`).toBeLessThan(size - 9);
  expect(bounds.bottom, `${label} is not cropped on the bottom`).toBeLessThan(size - 9);
  for (const [x, y] of [[0, 0], [size - 1, 0], [0, size - 1], [size - 1, size - 1]] as const) {
    expect(raster.pixels[(y * size + x) * 4 + 3], `${label} transparent corner ${x},${y}`).toBe(0);
  }
  let chromaPixels = 0;
  for (let index = 0; index < raster.pixels.length; index += 4) {
    const red = raster.pixels[index]!;
    const green = raster.pixels[index + 1]!;
    const blue = raster.pixels[index + 2]!;
    const alpha = raster.pixels[index + 3]!;
    if (alpha > 8 && ((green > 240 && red < 20 && blue < 20) || (red > 240 && blue > 240 && green < 20))) {
      chromaPixels += 1;
    }
  }
  expect(chromaPixels, `${label} visible green/magenta chroma pixels`).toBe(0);
}

function expectNoChromaFringe(raster: Raster, label: string): void {
  let fringePixels = 0;
  for (let y = 3; y < raster.height - 3; y += 1) {
    for (let x = 3; x < raster.width - 3; x += 1) {
      const index = (y * raster.width + x) * 4;
      const red = raster.pixels[index]!;
      const green = raster.pixels[index + 1]!;
      const blue = raster.pixels[index + 2]!;
      const alpha = raster.pixels[index + 3]!;
      if (alpha <= 4 || green <= Math.max(red, blue)) continue;
      let touchesTransparent = false;
      for (let yOffset = -3; yOffset <= 3 && !touchesTransparent; yOffset += 1) {
        for (let xOffset = -3; xOffset <= 3; xOffset += 1) {
          const neighbor = ((y + yOffset) * raster.width + x + xOffset) * 4;
          if (raster.pixels[neighbor + 3]! <= 4) {
            touchesTransparent = true;
            break;
          }
        }
      }
      if (touchesTransparent) fringePixels += 1;
    }
  }
  expect(fringePixels, `${label} key-green-dominant 3px edge pixels`).toBe(0);
}

function expectSameSilhouette(body: Raster, thumbnail: Raster, label: string): void {
  const bodyBounds = alphaBounds(body);
  const thumbnailBounds = alphaBounds(thumbnail);
  const normalizedBody = [bodyBounds.left, bodyBounds.top, bodyBounds.right + 1, bodyBounds.bottom + 1].map((value) => value / body.width);
  const normalizedThumbnail = [thumbnailBounds.left, thumbnailBounds.top, thumbnailBounds.right + 1, thumbnailBounds.bottom + 1].map((value) => value / thumbnail.width);
  normalizedBody.forEach((value, index) => {
    expect(Math.abs(value - normalizedThumbnail[index]!), `${label} thumbnail silhouette matches body`).toBeLessThanOrEqual(0.006);
  });
}

function expectFaceSafeLifeV1(profile: MotionProfileV1, label: string): void {
  const faceRegion = {
    left: profile.alphaBounds.left,
    top: profile.alphaBounds.top,
    right: profile.alphaBounds.right,
    bottom: profile.alphaBounds.top + (profile.alphaBounds.bottom - profile.alphaBounds.top) * 0.4,
  };
  const positiveOverlap = Math.max(faceRegion.left, profile.breathZone.left) < Math.min(faceRegion.right, profile.breathZone.right)
    && Math.max(faceRegion.top, profile.breathZone.top) < Math.min(faceRegion.bottom, profile.breathZone.bottom);
  expect(positiveOverlap, `${label} face and breath regions do not overlap`).toBe(false);
}

async function readCatalog(): Promise<Template[]> {
  const document = object(JSON.parse(await readFile(path.join(adoptionRoot, "catalog.json"), "utf8")), "catalog");
  expect(Object.keys(document).sort()).toEqual(["schemaVersion", "templates"]);
  expect(document.schemaVersion).toBe(1);
  expect(Array.isArray(document.templates)).toBe(true);
  return (document.templates as unknown[]).map((value, index) => {
    const template = object(value, `templates[${index}]`);
    expect(Object.keys(template).sort(), `templates[${index}] keys`).toEqual([...templateKeys].sort());
    expect(template.templateVersion).toBe(1);
    expect(template.runtimeSchemaVersion).toBe(3);
    for (const key of templateKeys.filter((key) => key !== "templateVersion" && key !== "runtimeSchemaVersion")) {
      expect(typeof template[key], `templates[${index}].${key}`).toBe("string");
      expect((template[key] as string).length, `templates[${index}].${key}`).toBeGreaterThan(0);
    }
    for (const key of ["thumbnailSha256", "bodySha256", "motionProfileSha256"] as const) {
      expect(template[key], `templates[${index}].${key}`).toMatch(/^[0-9a-f]{64}$/);
    }
    return template as Template;
  });
}

describe("production adoption content", () => {
  it("ships eight distinct dynamic adoption cats", async () => {
    const catalog = await readCatalog();
    expect(catalog.map(({ templateId, defaultName, personality }) => [templateId, defaultName, personality])).toEqual(expectedTemplates);
    expect(new Set(catalog.map(({ templateId }) => templateId)).size).toBe(8);
    expect(new Set(catalog.map(({ personality }) => personality)).size).toBe(8);
    expect(catalog.every(({ templateId }) => templateId !== "pet-live2d-v1")).toBe(true);

    const rootEntries = await readdir(adoptionRoot, { withFileTypes: true });
    expect(rootEntries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort()).toEqual(["catalog.json"]);
    expect(rootEntries.filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort())
      .toEqual(expectedTemplates.map(([templateId]) => templateId).sort());

    const bodyHashes = new Set<string>();
    for (const template of catalog) {
      expect([template.thumbnailPath, template.bodyPath, template.motionProfilePath])
        .toEqual(["thumbnail.png", "body.png", "motion-profile.json"]);
      const directory = path.join(adoptionRoot, template.templateId);
      const entries = await readdir(directory, { withFileTypes: true });
      expect(entries.every((entry) => entry.isFile()), `${template.templateId} only contains files`).toBe(true);
      expect(entries.map((entry) => entry.name).sort(), `${template.templateId} exact files`)
        .toEqual(["body.png", "motion-profile.json", "thumbnail.png"]);

      const thumbnailBytes = await readFile(path.join(directory, template.thumbnailPath));
      const bodyBytes = await readFile(path.join(directory, template.bodyPath));
      const profileBytes = await readFile(path.join(directory, template.motionProfilePath));
      expect((await stat(path.join(directory, template.thumbnailPath))).isFile()).toBe(true);
      expect((await stat(path.join(directory, template.bodyPath))).isFile()).toBe(true);
      expect((await stat(path.join(directory, template.motionProfilePath))).isFile()).toBe(true);
      expect(sha256(thumbnailBytes), `${template.templateId} thumbnail hash`).toBe(template.thumbnailSha256);
      expect(sha256(bodyBytes), `${template.templateId} body hash`).toBe(template.bodySha256);
      expect(sha256(profileBytes), `${template.templateId} motion hash`).toBe(template.motionProfileSha256);
      bodyHashes.add(sha256(bodyBytes));

      const thumbnail = decodeRgbaPng(thumbnailBytes, `${template.templateId}/thumbnail.png`);
      const body = decodeRgbaPng(bodyBytes, `${template.templateId}/body.png`);
      expectTransparentProductionRaster(thumbnail, 512, `${template.templateId}/thumbnail.png`);
      expectTransparentProductionRaster(body, 1024, `${template.templateId}/body.png`);
      expectNoChromaFringe(thumbnail, `${template.templateId}/thumbnail.png`);
      expectNoChromaFringe(body, `${template.templateId}/body.png`);
      expectSameSilhouette(body, thumbnail, template.templateId);

      const profileDocument = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(profileBytes));
      expect(Object.keys(object(profileDocument, `${template.templateId} motion profile`)).sort())
        .toEqual(["alphaBounds", "breathZone", "engineProfile", "profileVersion", "swayPivot"]);
      const profile = parseMotionProfile(profileDocument);
      const bounds = alphaBounds(body);
      expect(profile.alphaBounds, `${template.templateId} profile matches final body alpha`).toEqual({
        left: bounds.left / body.width,
        top: bounds.top / body.height,
        right: (bounds.right + 1) / body.width,
        bottom: (bounds.bottom + 1) / body.height,
      });
      expectFaceSafeLifeV1(profile, template.templateId);
    }
    expect(bodyHashes.size).toBe(8);
  });
});
