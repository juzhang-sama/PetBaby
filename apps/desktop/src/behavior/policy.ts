export interface PolicySnapshot {
  cooldowns: Record<string, number>;
}

/**
 * cooldowns stores the epoch-ms timestamp at which the interaction is
 * allowed again. Zero or missing means immediately allowed.
 */
export function cooldownRemaining(policy: PolicySnapshot, key: string, now: number): number {
  const allowedAt = policy.cooldowns[key];
  if (allowedAt === undefined || allowedAt <= now) return 0;
  return allowedAt - now;
}

export function markCooldown(policy: PolicySnapshot, key: string, now: number, durationMs: number): void {
  policy.cooldowns[key] = now + durationMs;
}
