import type { CreationMethod } from "../creation/contracts";

export type CreationPageOperation = "open" | "submit" | "poll" | "preview" | "finalize" | "retry" | "abandon";

export interface CreationPageOperationToken {
  id: number;
  visit: number;
  kind: CreationPageOperation;
  sessionId: string;
}

export class CreationPageRun {
  private visit = 0;
  private active = false;
  private readonly busy = new Map<string, number>();
  private readonly mutations = new Map<string, number>();
  private nextTokenId = 0;

  enter(_method: CreationMethod): number {
    this.visit += 1;
    this.active = true;
    return this.visit;
  }

  leave(): void {
    this.visit += 1;
    this.active = false;
    this.busy.clear();
    this.mutations.clear();
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
    const id = ++this.nextTokenId;
    if (!this.isCurrent(visit) || this.busy.has(key)) return null;
    if (isMutation(kind) && this.mutations.has(sessionId)) return null;
    this.busy.set(key, id);
    if (isMutation(kind)) this.mutations.set(sessionId, id);
    return { id, visit, kind, sessionId };
  }

  settle(token: CreationPageOperationToken): void {
    const key = operationKey(token.kind, token.sessionId);
    if (this.busy.get(key) === token.id) this.busy.delete(key);
    if (this.mutations.get(token.sessionId) === token.id) {
      this.mutations.delete(token.sessionId);
    }
  }

  isMutating(sessionId: string | null): boolean {
    return sessionId !== null && this.mutations.has(sessionId);
  }

  shouldApply(token: CreationPageOperationToken, currentSessionId: string | null): boolean {
    return this.isCurrent(token.visit) && token.sessionId === currentSessionId;
  }
}

function isMutation(kind: CreationPageOperation): boolean {
  return kind === "submit" || kind === "finalize" || kind === "retry" || kind === "abandon";
}

function operationKey(kind: CreationPageOperation, sessionId: string): string {
  return `${kind}:${sessionId}`;
}
