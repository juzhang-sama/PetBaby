export type ParticleKind = "heart" | "spark" | "zzz";

export interface Particle {
  kind: ParticleKind;
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  maxLife: number;
  size: number;
  seed: number;
}

const PARTICLE_LIFE_MS: Record<ParticleKind, number> = {
  heart: 1_200,
  spark: 650,
  zzz: 1_700,
};

const PARTICLE_VY: Record<ParticleKind, number> = {
  heart: -38,
  spark: -70,
  zzz: -16,
};

/**
 * Lightweight particle state used for interaction feedback (hearts on happy,
 * sparks on curious, Zzz while sleeping). Kept free of Pixi so it is trivially
 * unit-testable; the stage draws the particles each frame.
 */
export class ParticleSystem {
  private items: Particle[] = [];

  get count(): number {
    return this.items.length;
  }

  get active(): readonly Particle[] {
    return this.items;
  }

  spawn(
    kind: ParticleKind,
    x: number,
    y: number,
    options: { count?: number; spread?: number } = {},
  ): void {
    const count = options.count ?? 1;
    const spread = options.spread ?? 24;
    for (let i = 0; i < count; i += 1) {
      const maxLife = PARTICLE_LIFE_MS[kind];
      this.items.push({
        kind,
        x: x + (Math.random() - 0.5) * spread,
        y: y + (Math.random() - 0.5) * 8,
        vx: (Math.random() - 0.5) * 18,
        vy: PARTICLE_VY[kind],
        life: maxLife,
        maxLife,
        size: kind === "zzz"
          ? 9 + Math.random() * 5
          : 5 + Math.random() * 4,
        seed: Math.random() * Math.PI * 2,
      });
    }
  }

  update(dtMs: number): void {
    const dt = dtMs / 1000;
    for (const particle of this.items) {
      particle.life -= dtMs;
      particle.x += particle.vx * dt;
      particle.y += particle.vy * dt;
      if (particle.kind === "heart") {
        // gentle horizontal sway while floating up
        const age = particle.maxLife - particle.life;
        particle.x += Math.sin(age / 220 + particle.seed) * 0.5;
      } else if (particle.kind === "zzz") {
        particle.x += Math.sin(particle.seed) * 0.4;
      }
    }
    this.items = this.items.filter((particle) => particle.life > 0);
  }

  clear(): void {
    this.items = [];
  }
}
