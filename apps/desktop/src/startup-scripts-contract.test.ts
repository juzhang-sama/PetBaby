import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
const cmdPath = path.join(repoRoot, "一键启动桌宠开发环境.cmd");
const powershellPath = path.join(repoRoot, "scripts", "start-desktop-pet-dev.ps1");
const legacyPowershellPath = path.join(repoRoot, "scripts", "启动桌宠开发环境.ps1");
const cmdBytes = readFileSync(cmdPath);
const cmd = cmdBytes.toString("utf8");
const powershellBytes = readFileSync(powershellPath);
const powershell = powershellBytes.toString("utf8");
const temporaryRoots: string[] = [];

type LauncherFixtureOptions = {
  cargoMissing?: boolean;
  cubismComplete?: boolean;
  npmMissing?: boolean;
  npmLsExitCode?: number;
  npmInstallExitCode?: number;
  tauriExitCode?: number;
  validateOnly?: boolean;
};

function createLauncherFixture(options: LauncherFixtureOptions = {}) {
  const root = mkdtempSync(path.join(tmpdir(), "petbaby-startup-"));
  temporaryRoots.push(root);
  const desktopRoot = path.join(root, "desktop");
  const commandRoot = path.join(root, "commands");
  mkdirSync(commandRoot, { recursive: true });
  mkdirSync(path.join(desktopRoot, "node_modules", ".bin"), { recursive: true });
  writeFileSync(path.join(desktopRoot, "node_modules", ".bin", "tauri.cmd"), "@exit /b 0\r\n");

  if (options.cubismComplete) {
    for (const relative of [
      ".vendor/live2d-cubism-sdk/Core/live2dcubismcore.min.js",
      ".vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework.ts",
      "public/live2d/Core/live2dcubismcore.min.js",
    ]) {
      const target = path.join(desktopRoot, ...relative.split("/"));
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, "fixture\n");
    }
  }

  const npmCommand = path.join(commandRoot, "npm.cmd");
  if (!options.npmMissing) {
    writeFileSync(
      npmCommand,
      [
        "@echo off",
        `if "%~1"=="ls" exit /b ${options.npmLsExitCode ?? 0}`,
        `if "%~1"=="install" exit /b ${options.npmInstallExitCode ?? 0}`,
        `if "%~1"=="run" if "%~2"=="tauri" exit /b ${options.tauriExitCode ?? 0}`,
        "exit /b 0",
        "",
      ].join("\r\n"),
    );
  }

  const cargoCommand = options.cargoMissing
    ? path.join(commandRoot, "missing-cargo.exe")
    : (process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe");

  return { desktopRoot, npmCommand, cargoCommand };
}

function runLauncher(options: LauncherFixtureOptions = {}) {
  const fixture = createLauncherFixture(options);
  const args = [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    powershellPath,
    "-DesktopRootOverride",
    fixture.desktopRoot,
    "-NpmCommandOverride",
    fixture.npmCommand,
    "-CargoCommandOverride",
    fixture.cargoCommand,
  ];
  if (options.validateOnly) args.push("-ValidateOnly");

  return spawnSync(
    "powershell.exe",
    args,
    {
      encoding: "utf8",
      env: { ...process.env, CUBISM_SDK_ROOT: "" },
      timeout: 15_000,
    },
  );
}

function expectLauncherStatus(options: LauncherFixtureOptions, expected: number) {
  const result = runLauncher(options);
  expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(expected);
}

afterEach(() => {
  for (const root of temporaryRoots.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

describe("Windows one-click development startup", () => {
  it("keeps the CMD wrapper ASCII-only and points at the ASCII PowerShell path", () => {
    expect([...cmdBytes].every((byte) => byte <= 0x7f)).toBe(true);
    expect(cmd).toContain("%~dp0scripts\\start-desktop-pet-dev.ps1");
    expect(existsSync(powershellPath)).toBe(true);
    expect(existsSync(legacyPowershellPath)).toBe(false);
    expect([...powershellBytes.subarray(0, 3)]).toEqual([0xef, 0xbb, 0xbf]);
    expect(cmd).toContain("-NoProfile -ExecutionPolicy Bypass");
    expect(cmd).toContain("%ERRORLEVEL%");
    expect(cmd).toMatch(/if not .*==.*0[\s\S]*pause/i);
    expect(cmd).toMatch(/exit \/b/i);
  });

  it("checks complete npm and Cubism runtime state before the only Tauri dev start", () => {
    expect(powershell).toContain("$PSScriptRoot");
    expect(powershell).toContain("Get-Command npm.cmd");
    expect(powershell).toContain("Get-Command cargo.exe");
    expect(powershell).toContain("& $npmCommand ls --depth=0 --silent");
    expect(powershell).toContain("& $npmCommand install");
    expect(powershell).toContain("node_modules/.bin/tauri.cmd");
    expect(powershell).toContain(".vendor/live2d-cubism-sdk/Core/live2dcubismcore.min.js");
    expect(powershell).toContain(".vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework.ts");
    expect(powershell).toContain("public/live2d/Core/live2dcubismcore.min.js");
    expect(powershell).toContain("$env:CUBISM_SDK_ROOT");
    expect(powershell).toContain("& $npmCommand run prepare:cubism");
    expect(powershell).toContain("exit 12");
    expect(powershell).toContain("exit 14");

    const starts = powershell.match(/& \$npmCommand run tauri -- dev/g) ?? [];
    expect(starts).toHaveLength(1);
    expect(powershell).not.toMatch(/npm run dev\b/i);
    expect(powershell).not.toMatch(/\b(?:vite|Start-Process)\b/i);
  });

  it("keeps user-actionable PowerShell diagnostics in Chinese", () => {
    expect(powershell).toMatch(/未找到 npm/);
    expect(powershell).toMatch(/未找到 cargo/);
    expect(powershell).toMatch(/正在安装 npm 依赖/);
    expect(powershell).toMatch(/正在准备 Cubism SDK/);
    expect(powershell).toMatch(/Cubism SDK.*缺失/);
    expect(powershell).toMatch(/正在启动 PetBaby/);
  });

  it("returns reserved code 10 when npm is missing", () => {
    expectLauncherStatus({ npmMissing: true }, 10);
  });

  it("returns reserved code 11 when cargo is missing", () => {
    expectLauncherStatus({ cargoMissing: true }, 11);
  });

  it("propagates dependency installation failures", () => {
    expectLauncherStatus({ npmLsExitCode: 1, npmInstallExitCode: 23 }, 23);
  });

  it("returns reserved code 12 when Cubism is incomplete without an SDK root", () => {
    expectLauncherStatus({}, 12);
  });

  it("validates a complete temporary environment without starting Tauri", () => {
    expectLauncherStatus({ cubismComplete: true, validateOnly: true }, 0);
  });

  it("propagates the Tauri command exit code", () => {
    expectLauncherStatus({ cubismComplete: true, tauriExitCode: 37 }, 37);
  });

  it("does not embed destructive, network, provider, secret, or production-content operations", () => {
    const combined = `${cmd}\n${powershell}`;
    expect(combined).not.toMatch(/public[\\/]creation-content/i);
    expect(combined).not.toMatch(/\bRemove-Item\b/i);
    expect(combined).not.toMatch(/^\s*(?:del|erase|rd|rmdir|rm)\b/im);

    // 网络规则：启动脚本不得访问外网（下载/回传），仅允许对本机回环地址
    // (127.0.0.1 / localhost) 的健康探测——一键启动需要探测照片分身受控后端是否就绪。
    // 1) 所有字面 http(s) URL 必须是回环地址。
    const urls = combined.match(/https?:\/\/[^\s"'`]+/gi) ?? [];
    for (const url of urls) {
      expect(url, `启动脚本不得包含外网地址: ${url}`).toMatch(
        /^https?:\/\/(?:127\.0\.0\.1|localhost)\b/i,
      );
    }
    // 2) 下载/探测类命令与原始网络 API 一律禁止（即使指向回环）。
    expect(combined).not.toMatch(
      /\b(?:Start-BitsTransfer|curl|wget|WebClient|System\.Net|Test-NetConnection|Invoke-RestMethod)\b/i,
    );
    // 3) Invoke-WebRequest 仅允许用于回环健康探测；其目标已由规则 1 强制为回环地址。

    expect(combined).not.toMatch(/\bprovider\b|API[_ -]?key|LK888_API_KEY/i);
    expect(combined).not.toMatch(/gen_start|creation_abandon|(?:pet|draft)[_ -]?delete|delete[_ -]?(?:pet|draft)/i);
  });
});
