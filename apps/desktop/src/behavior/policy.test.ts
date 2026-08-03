import { describe, expect, it } from "vitest";
import { cooldownRemaining } from "./policy";

describe("cooldownRemaining", () => {
  it("returns zero when no cooldown is recorded", () => {
    expect(cooldownRemaining({ cooldowns: {} }, "react-happy", 1_000)).toBe(0);
  });

  it("returns remaining milliseconds until the allowed timestamp", () => {
    expect(cooldownRemaining({ cooldowns: { "react-happy": 5_000 } }, "react-happy", 4_000)).toBe(1_000);
  });

  it("returns zero when the cooldown has expired", () => {
    expect(cooldownRemaining({ cooldowns: { "react-happy": 5_000 } }, "react-happy", 6_000)).toBe(0);
  });
});
