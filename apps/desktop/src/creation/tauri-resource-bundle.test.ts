import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("Tauri creation-content resource bundle", () => {
  it("walks the content directory so adoption and composer hierarchy is preserved", () => {
    const config = JSON.parse(readFileSync(
      new URL("../../src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    )) as { bundle: { resources: Record<string, string> } };

    expect(config.bundle.resources).toEqual({
      "../public/creation-content": "creation-content",
    });
  });

  it("keeps every hashed runtime JSON byte-stable on Windows checkouts", () => {
    const attributes = readFileSync(
      new URL("../../../../.gitattributes", import.meta.url),
      "utf8",
    ).split(/\r?\n/);

    expect(attributes).toContain("apps/desktop/public/creation-content/**/*.json text eol=lf");
    expect(attributes).toContain("apps/desktop/public/builtin-pets/pet-live2d-v1/*.model3.json text eol=lf");
  });
});
