import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("settings upload creation assembly", () => {
  it("offers three parallel semantic creation routes without fake sequence numbers", () => {
    const html = readFileSync(new URL("../settings.html", import.meta.url), "utf8");

    expect(html.match(/data-creation-route=/g)).toHaveLength(3);
    expect(html).toContain('data-creation-route="upload"');
    expect(html).toContain("上传图片");
    expect(html).toContain("我有一张宠物照片");
    expect(html).toContain('data-creation-route="composer"');
    expect(html).toContain("引导组合");
    expect(html).toContain("我想亲手组合一只猫");
    expect(html).toContain('data-creation-route="adoption"');
    expect(html).toContain("直接认领");
    expect(html).toContain("我想马上拥有一只猫");
    expect(html).not.toMatch(/>\s*0[123]\s*</);
  });

  it("assembles all three views through one durable page router", () => {
    const source = readFileSync(new URL("./settings.ts", import.meta.url), "utf8");

    expect(source).toContain("new CreationPageRun(");
    expect(source).toContain("new UploadCreationView(");
    expect(source).toContain("new ComposerCreationView(");
    expect(source).toContain("new AdoptionCreationView(");
    expect(source).toContain("creationApi.adoptionCatalog");
    expect(source).toContain("creationApi.adoptionStart");
  });

  it("keeps adoption keyboard reachable, announced and responsive without decorative motion", () => {
    const html = readFileSync(new URL("../settings.html", import.meta.url), "utf8");
    const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

    expect(html).toContain('id="adoption-catalog"');
    expect(html).toContain('role="listbox"');
    expect(html).toContain('aria-label="可认领猫咪目录"');
    expect(html).toContain('id="adoption-status"');
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain('for="adoption-pet-name"');
    expect(html).toContain('id="draft-choice-dialog"');
    expect(css).toContain(".adoption-layout");
    expect(css).toContain("@media (max-width: 520px)");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
    expect(css).not.toContain("adoption-float");
    const source = readFileSync(new URL("./settings/adoption-creation-view.ts", import.meta.url), "utf8");
    expect(source).toContain('button.setAttribute("role", "option")');
    expect(source).toContain('button.setAttribute("aria-selected"');
    expect(source).toContain('button.setAttribute("aria-disabled"');
  });

  it("ships the settings page as a production HTML entry", () => {
    const config = readFileSync(new URL("../vite.config.ts", import.meta.url), "utf8");

    expect(config).toContain('index: join(process.cwd(), "index.html")');
    expect(config).toContain('settings: join(process.cwd(), "settings.html")');
  });

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
    expect(html).toContain('aria-describedby="pet-name-help pet-name-error"');
    expect(html).toContain("满意，出现在桌面");
    expect(html).toContain("上传到第三方生成平台");
    expect(html).not.toContain('maxlength="40"');
    expect(html).toContain('accept="image/png,image/jpeg"');
    expect(html).toContain("1–20 个可见字符");
    expect(html).toContain("按完整字素计");
    expect(html).toContain("原图会仅在本机临时保存用于失败恢复");
    expect(html).toContain("完成或放弃后删除");
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
