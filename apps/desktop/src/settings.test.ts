import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("settings upload creation assembly", () => {
  it("assembles the durable upload view without legacy creation truth sources", () => {
    const source = readFileSync(new URL("./settings.ts", import.meta.url), "utf8");

    expect(source).toContain("new UploadCreationView(");
    expect(source).toContain("creationApi");
    expect(source).toContain("finalizeCreation");
    expect(source).toContain("if (selectedView === view) return;");
    for (const forbidden of [
      ["pet", "create"].join("_"),
      ["pet", "creation", "resume"].join("_"),
      ["session", "Storage"].join(""),
      "CreationWizardRun",
      "CreationResume",
    ]) {
      expect(source).not.toContain(forbidden);
    }
  });

  it("keeps one accessible cat upload path with explicit naming and final action", () => {
    const html = readFileSync(new URL("../settings.html", import.meta.url), "utf8");

    expect(html).not.toContain('value="dog"');
    expect(html).toContain('for="pet-name"');
    expect(html).toContain('aria-describedby="pet-name-error"');
    expect(html).toContain("满意，出现在桌面");
    expect(html).toContain("照片将上传到第三方生成平台");
    expect(html).not.toContain('maxlength="40"');
  });

  it("loads source and candidate contracts from strongly typed backend commands", () => {
    const source = readFileSync(new URL("./settings.ts", import.meta.url), "utf8");
    const api = readFileSync(new URL("./creation/api.ts", import.meta.url), "utf8");
    expect(source).toContain('"creation_upload_candidate_assets"');
    expect(api).toContain('"creation_upload_source"');
    expect(source).not.toContain("schemaVersion: 3");
    expect(source).not.toContain('"gen_cutout_b64"');
    expect(source).not.toContain('"gen_motion_profile"');
  });
});
