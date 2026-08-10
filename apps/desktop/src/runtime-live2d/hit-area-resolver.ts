import type { PetHitArea } from "../runtime/pet-renderer";

export interface HitAreaTestPort {
  hitTest(name: string, point: { x: number; y: number }): boolean;
}

export class HitAreaResolver {
  constructor(
    private readonly mappings: Partial<Record<PetHitArea, string>>,
    private readonly port: HitAreaTestPort,
  ) {}

  resolve(point: { x: number; y: number }): PetHitArea | null {
    for (const area of ["head", "body"] as const) {
      const cubismName = this.mappings[area];
      if (cubismName && this.port.hitTest(cubismName, point)) return area;
    }
    return null;
  }
}
