import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

type BoundaryContract = {
  schemaVersion: number;
  allowedRustFiles: string[];
  directWindowsPatterns: string[];
};

const rustRoot = fileURLToPath(new URL("../src-tauri/src/", import.meta.url));
const contract = JSON.parse(readFileSync(
  new URL("../src-tauri/windows-native-boundary.json", import.meta.url),
  "utf8",
)) as BoundaryContract;
const expectedPatterns = [
  "#\\s*\\[\\s*link\\s*\\(",
  "\\bAsRawHandle\\b",
  "\\bextern\\s*\"system\"",
  "\\bFromRawHandle\\b",
  "\\bOwnedHandle\\b",
  "\\bRawHandle\\b",
  "\\b(?:Win32WindowHandle|WindowsDisplayHandle|RawWindowHandle\\s*::\\s*Win32|RawDisplayHandle\\s*::\\s*Windows)\\b",
  "\\bstd\\s*::\\s*os\\s*::\\s*(?:windows\\b|\\{[^{}]*\\bwindows\\s*::)",
  "\\bwinapi\\b(?=\\s*(?:::|as\\b))",
  "\\bWin32_",
  "\\bwindows\\s*::\\s*Win32\\b",
  "\\bwindows_sys\\b(?=\\s*(?:::|as\\b))",
].sort();
const directWindowsPatterns = contract.directWindowsPatterns.map((pattern) => new RegExp(pattern, "m"));

function rustFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) return rustFiles(absolute);
    return entry.isFile() && entry.name.endsWith(".rs") ? [absolute] : [];
  });
}

function rustCharacterLiteralEnd(source: string, start: number): number | undefined {
  const quote = source[start] === "b" && source[start + 1] === "'"
    ? start + 1
    : source[start] === "'" ? start : undefined;
  if (quote === undefined) return undefined;

  let cursor = quote + 1;
  if (source[cursor] === "\\") {
    const escape = cursor + 1;
    if (source[escape] === "x" && /^[0-9a-fA-F]{2}$/.test(source.slice(escape + 1, escape + 3))) {
      cursor = escape + 3;
    } else if (source.startsWith("u{", escape)) {
      const closingBrace = source.indexOf("}", escape + 2);
      if (closingBrace === -1) return undefined;
      cursor = closingBrace + 1;
    } else {
      cursor = escape + 1;
    }
  } else {
    const codePoint = source.codePointAt(cursor);
    if (codePoint === undefined || source[cursor] === "\r" || source[cursor] === "\n") return undefined;
    cursor += codePoint > 0xffff ? 2 : 1;
  }

  return source[cursor] === "'" ? cursor + 1 : undefined;
}

function rustCodeOnly(source: string): string {
  let result = "";
  let index = 0;

  const mask = (text: string) => text.replace(/[^\r\n]/g, " ");
  while (index < source.length) {
    const characterEnd = rustCharacterLiteralEnd(source, index);
    if (characterEnd !== undefined) {
      result += mask(source.slice(index, characterEnd));
      index = characterEnd;
      continue;
    }

    if (source.startsWith("//", index)) {
      const end = source.indexOf("\n", index);
      const next = end === -1 ? source.length : end;
      result += mask(source.slice(index, next));
      index = next;
      continue;
    }

    if (source.startsWith("/*", index)) {
      let depth = 1;
      let end = index + 2;
      while (end < source.length && depth > 0) {
        if (source.startsWith("/*", end)) {
          depth += 1;
          end += 2;
        } else if (source.startsWith("*/", end)) {
          depth -= 1;
          end += 2;
        } else {
          end += 1;
        }
      }
      result += mask(source.slice(index, end));
      index = end;
      continue;
    }

    if (source[index] === "r") {
      let quote = index + 1;
      while (source[quote] === "#") quote += 1;
      if (source[quote] === "\"") {
        const hashes = source.slice(index + 1, quote);
        const terminator = `\"${hashes}`;
        const close = source.indexOf(terminator, quote + 1);
        const end = close === -1 ? source.length : close + terminator.length;
        result += mask(source.slice(index, end));
        index = end;
        continue;
      }
    }

    if (source[index] === "\"") {
      let end = index + 1;
      while (end < source.length) {
        if (source[end] === "\\") {
          end += 2;
          continue;
        }
        end += 1;
        if (source[end - 1] === "\"") break;
      }
      const literal = source.slice(index, end);
      const isSystemAbi = literal === "\"system\"" && /\bextern\s*$/.test(result);
      result += isSystemAbi ? literal : mask(literal);
      index = end;
      continue;
    }

    result += source[index];
    index += 1;
  }

  return result;
}

function hasDirectWindowsUse(source: string): boolean {
  const code = rustCodeOnly(source);
  return directWindowsPatterns.some((pattern) => pattern.test(code));
}

describe("Windows-native Rust boundary", () => {
  it("keeps the versioned allowlist canonical and the pattern set complete", () => {
    expect(contract.schemaVersion).toBe(2);
    expect(contract.allowedRustFiles).toEqual([...new Set(contract.allowedRustFiles)].sort());
    expect(contract.directWindowsPatterns).toEqual([...new Set(contract.directWindowsPatterns)].sort());
    expect(contract.directWindowsPatterns).toEqual(expectedPatterns);
  });

  it("ignores obvious comments and string literals when detecting direct use", () => {
    const decoys = [
      "// windows::Win32",
      "/* winapi:: nested /* windows_sys:: */ comment */",
      "const NORMAL: &str = \"raw_window_handle::Win32\";",
      "const RAW: &str = r#\"extern \\\"system\\\" #[link(\"#;",
    ].join("\n");
    expect(hasDirectWindowsUse(decoys)).toBe(false);
    expect(hasDirectWindowsUse("use windows_sys::Win32_Foundation::HANDLE;")).toBe(true);
  });

  it("detects real Windows code after a Rust character literal containing a quote", () => {
    const source = `const QUOTE: char = '\"';\nuse windows_sys::Win32_Foundation::HANDLE;`;
    expect(hasDirectWindowsUse(source)).toBe(true);
  });

  it("detects real Windows code after a Rust byte-character literal containing a quote", () => {
    const source = `const QUOTE: u8 = b'\"';\nuse windows_sys::Win32_Foundation::HANDLE;`;
    expect(hasDirectWindowsUse(source)).toBe(true);
  });

  it("detects crate aliases and raw Windows handle signatures", () => {
    expect(hasDirectWindowsUse("use windows_sys as ws;")).toBe(true);
    expect(hasDirectWindowsUse("use raw_window_handle::Win32WindowHandle;")).toBe(true);
    expect(hasDirectWindowsUse("use raw_window_handle::Win32WindowHandle as NativeWindowHandle;")).toBe(true);
    expect(hasDirectWindowsUse("use raw_window_handle::{RawWindowHandle, Win32WindowHandle};")).toBe(true);
    expect(hasDirectWindowsUse("let handle = raw_window_handle::RawWindowHandle::Win32(value);")).toBe(true);
    expect(hasDirectWindowsUse("let display = raw_window_handle::RawDisplayHandle::Windows(value);")).toBe(true);
    expect(hasDirectWindowsUse("use raw_window_handle::RawWindowHandle;\nlet handle = RawWindowHandle::Win32(value);")).toBe(true);
    expect(hasDirectWindowsUse("use raw_window_handle as rwh;\nlet handle = rwh::RawWindowHandle::Win32(value);")).toBe(true);
  });

  it("detects a grouped std::os Windows import", () => {
    const source = "use std::os::{windows::ffi::OsStrExt, unix::ffi::OsStrExt as UnixOsStrExt};";
    expect(hasDirectWindowsUse(source)).toBe(true);
  });

  it("detects a comment-separated system ABI", () => {
    expect(hasDirectWindowsUse('extern /* platform ABI */ "system" fn callback() {}')).toBe(true);
  });

  it("does not allow direct Windows APIs to spread to another Rust file", () => {
    const sources = rustFiles(rustRoot).map((file) => ({
      relative: path.relative(rustRoot, file).replaceAll("\\", "/"),
      source: readFileSync(file, "utf8"),
    }));
    const directUsers = sources
      .filter(({ source }) => hasDirectWindowsUse(source))
      .map(({ relative }) => relative)
      .sort();

    expect(directUsers).toEqual(contract.allowedRustFiles);
  });
});
