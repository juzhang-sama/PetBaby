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
    expect(source).toContain("new PhotoAvatarCreationView(");
    expect(source).toContain("new ComposerCreationView(");
    expect(source).toContain("new AdoptionCreationView(");
    expect(source).toContain("creationApi.adoptionCatalog");
    expect(source).toContain("creationApi.adoptionStart");
  });

  it("wires one shared mutation owner and focus manager across all creation routes", () => {
    const source = readFileSync(new URL("./settings.ts", import.meta.url), "utf8");

    expect(source).toContain("new CreationPageActivity(");
    expect(source.match(/activity: creationActivity/g)).toHaveLength(3);
    expect(source).toContain("new CreationPageFocusManager(");
    expect(source).toContain("creationFocus.enter(");
    expect(source).toContain("creationFocus.returnToTrigger(");
    expect(source).toContain("tabList, tabCreate");
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
    expect(html).toContain("重试认领会沿用首次确认的名称");
    expect(html.match(/data-creation-entry-focus/g)).toHaveLength(3);
    expect(css).toContain(".adoption-layout");
    expect(css).toContain("@media (max-width: 520px)");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
    expect(css).toContain("[data-creation-entry-focus]:focus-visible");
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

  it("replaces the standalone calibration demo with accessible current-pet controls", () => {
    const html = readFileSync(new URL("../settings.html", import.meta.url), "utf8");
    const source = readFileSync(new URL("./settings.ts", import.meta.url), "utf8");
    const rust = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
    const config = readFileSync(new URL("../vite.config.ts", import.meta.url), "utf8");
    const capabilities = JSON.parse(readFileSync(
      new URL("../src-tauri/capabilities/settings-close.json", import.meta.url),
      "utf8",
    )) as { windows: string[]; permissions: string[] };

    expect(html).toContain('id="calibration-section"');
    expect(html).toContain('for="calibration-breath"');
    expect(html).toContain('min="0" max="5" step="0.1" value="2"');
    expect(html).not.toContain('for="calibration-blink"');
    expect(html).not.toContain('id="calibration-blink"');
    expect(html).toContain('for="calibration-feedback"');
    expect(html).toContain('min="0" max="1" step="0.05" value="0.6"');
    expect(html).toContain('role="status" aria-live="polite"');
    expect(html).toContain('role="alert"');
    expect(source).toContain("new PetCalibrationControl(");
    expect(source).toContain("entry.isCurrent");
    expect(source).toContain('"pet_calibration_save"');
    expect(source).toContain('"settings:navigate"');
    expect(source).toContain("initializeSettingsNavigation({");
    expect(source).toContain('listen<unknown>("settings:navigate"');
    expect(source).toContain('invoke<string | null>("settings_take_pending_navigation")');
    expect(source).toContain("wireSettingsPageLifecycle(window");
    expect(source).toContain("new SettingsCloseCoordinator({");
    expect(source).toContain("settingsWindow.onCloseRequested(handler)");
    expect(source).toContain("destroy: () => settingsWindow.destroy()");
    expect(source).not.toContain("close: () => settingsWindow.close()");
    expect(capabilities.windows).toEqual(["settings"]);
    expect(capabilities.permissions).toEqual(["core:window:allow-destroy"]);
    expect(source).toContain("calibrationControl.settleForClose()");
    expect(source).toContain("calibrationControl.restoreBeforeClose()");
    expect(source).toContain("[settings] calibration close coordination");
    expect(rust).toContain('show_settings_window(app, Some("calibration"))');
    expect(rust).toContain('app.state::<SettingsNavigationState>().publish(section)?');
    expect(rust).toMatch(/\.emit\(\s*"settings:navigate"/);
    expect(rust).not.toContain("calibration.html");
    expect(config).not.toMatch(/calibration\s*:/);
  });

  it("keeps one accessible photo-avatar path with final Live2D actions", () => {
    const html = readFileSync(new URL("../settings.html", import.meta.url), "utf8");
    const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
    const workspaceStart = html.indexOf('<section id="photo-avatar-workspace"');
    const workspaceEnd = html.indexOf("</section>", workspaceStart);
    const previewStart = html.indexOf('<div id="photo-avatar-preview"', workspaceStart);
    const previewEnd = html.indexOf('<div id="photo-avatar-complete"', previewStart);
    const regenerate = html.indexOf('id="photo-avatar-regenerate"', workspaceStart);

    expect(html).toContain('id="photo-avatar-workspace"');
    expect(html).toContain('id="photo-avatar-files"');
    expect(html).toContain('accept="image/png,image/jpeg" multiple');
    expect(html).toContain('id="photo-avatar-live2d"');
    expect(html).toContain("照片分身像素动态预览");
    expect(html).toContain('id="photo-avatar-accept"');
    expect(html).toContain('id="photo-avatar-regenerate"');
    expect(html).toContain('id="photo-avatar-revise"');
    expect(html).toContain('id="photo-avatar-consent-dialog"');
    expect(html).toContain("lk888.ai");
    expect(html).toContain("gpt-4o 仅用于分析与补全");
    expect(html).toContain("gpt-image-2 仅用于生成纹理");
    expect(html).toContain("没有公开删除 API");
    expect(html).toContain("保存期限尚未核验");
    expect(html).toContain("隐私政策版本尚未核验");
    expect(html).not.toContain("lk888 已删除");
    expect(css).toMatch(/\.settings-shell \.photo-avatar-live2d\s*{[^}]*position:\s*relative;/s);
    expect(previewStart).toBeLessThan(previewEnd);
    expect(previewEnd).toBeLessThan(regenerate);
    expect(regenerate).toBeLessThan(workspaceEnd);
  });

});
