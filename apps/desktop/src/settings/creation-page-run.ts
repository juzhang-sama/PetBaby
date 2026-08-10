import type { CreationMethod } from "../creation/contracts";

export type CreationPageOperation = "open" | "submit" | "poll" | "preview" | "finalize" | "retry" | "abandon";

export interface CreationPageOperationToken {
  visit: number;
  kind: CreationPageOperation;
  sessionId: string;
}

export class CreationPageRun {
  private visit = 0;
  private active = false;
  private readonly busy = new Set<string>();

  enter(_method: CreationMethod): number {
    this.visit += 1;
    this.active = true;
    return this.visit;
  }

  leave(): void {
    this.visit += 1;
    this.active = false;
  }

  isCurrent(visit: number): boolean {
    return this.active && visit === this.visit;
  }

  begin(
    visit: number,
    kind: CreationPageOperation,
    sessionId: string,
  ): CreationPageOperationToken | null {
    const key = operationKey(kind, sessionId);
    if (!this.isCurrent(visit) || this.busy.has(key)) return null;
    this.busy.add(key);
    return { visit, kind, sessionId };
  }

  settle(token: CreationPageOperationToken): void {
    this.busy.delete(operationKey(token.kind, token.sessionId));
  }

  shouldApply(token: CreationPageOperationToken, currentSessionId: string | null): boolean {
    return this.isCurrent(token.visit) && token.sessionId === currentSessionId;
  }
}

function operationKey(kind: CreationPageOperation, sessionId: string): string {
  return `${kind}:${sessionId}`;
}
