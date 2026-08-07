import { describe, expect, it, vi } from "vitest";
import { HitAreaResolver } from "./hit-area-resolver";

describe("HitAreaResolver", () => {
  it("maps declared product hit areas to Cubism names", () => {
    const hitTest = vi.fn((name: string) => name === "Head");
    const resolver = new HitAreaResolver({ head: "Head", body: "Body" }, { hitTest });

    expect(resolver.resolve({ x: 10, y: 20 })).toBe("head");
    expect(hitTest).toHaveBeenCalledWith("Head", { x: 10, y: 20 });
  });

  it("does not fall back to an undeclared body or the whole canvas", () => {
    const hitTest = vi.fn(() => true);
    const resolver = new HitAreaResolver({}, { hitTest });

    expect(resolver.resolve({ x: 10, y: 20 })).toBeNull();
    expect(hitTest).not.toHaveBeenCalled();
  });
});
