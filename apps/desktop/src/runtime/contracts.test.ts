import { describe, expect, it } from "vitest";
import { PROBE_VERSION } from "./contracts";

describe("probe contract", () => {
  it("pins the M0 contract version", () => {
    expect(PROBE_VERSION).toBe("m0");
  });
});
