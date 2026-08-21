import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../../../../scripts/live2d/cubism-runtime.ts", import.meta.url), "utf8");

describe("Cubism motion API contract", () => {
  it("uses the installed SDK loop method instead of the nonexistent legacy spelling", () => {
    expect(source).toContain("motion.setLoop(options.loop)");
    expect(source).not.toContain("motion.setIsLoop");
  });
});
