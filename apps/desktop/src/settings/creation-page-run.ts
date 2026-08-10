import type { CreationMethod } from "../creation/contracts";

export type CreationPageOperation = "open" | "submit" | "poll" | "preview" | "finalize" | "retry" | "abandon";
export type CreationPageMutation = "submit" | "finalize" | "retry" | "abandon";

export interface ActiveCreationMutation {
  kind: CreationPageMutation;
  sessionId: string;
}

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
  private readonly mutations = new Map<string, {
    id: number;
    kind: CreationPageMutation;
    promise: Promise<void>;
    resolve: () => void;
  }>();
  private nextTokenId = 0;

  enter(_method: CreationMethod): number {
    this.visit += 1;
    this.active = true;
    return this.visit;
  }

  leave(): void {
    this.visit += 1;
    this.active = false;
    const mutationOwners = new Set(Array.from(this.mutations.values(), (owner) => owner.id));
    for (const [key, owner] of this.busy) {
      if (!mutationOwners.has(owner)) this.busy.delete(key);
    }
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
    if (isMutation(kind)) {
      let resolve!: () => void;
      const promise = new Promise<void>((settled) => { resolve = settled; });
      this.mutations.set(sessionId, { id, kind, promise, resolve });
    }
    return { id, visit, kind, sessionId };
  }

  settle(token: CreationPageOperationToken): void {
    const key = operationKey(token.kind, token.sessionId);
    if (this.busy.get(key) === token.id) this.busy.delete(key);
    const mutation = this.mutations.get(token.sessionId);
    if (mutation?.id === token.id) {
      this.mutations.delete(token.sessionId);
      mutation.resolve();
    }
  }

  waitForMutation(sessionId: string): Promise<void> {
    return this.mutations.get(sessionId)?.promise ?? Promise.resolve();
  }

  activeMutations(): ActiveCreationMutation[] {
    return Array.from(this.mutations, ([sessionId, owner]) => ({
      kind: owner.kind,
      sessionId,
    }));
  }

  isMutating(sessionId: string | null): boolean {
    return sessionId !== null && this.mutations.has(sessionId);
  }

  isRunning(kind: CreationPageOperation, sessionId: string | null): boolean {
    return sessionId !== null && this.busy.has(operationKey(kind, sessionId));
  }

  shouldApply(token: CreationPageOperationToken, currentSessionId: string | null): boolean {
    return this.isCurrent(token.visit) && token.sessionId === currentSessionId;
  }
}

function isMutation(kind: CreationPageOperation): kind is CreationPageMutation {
  return kind === "submit" || kind === "finalize" || kind === "retry" || kind === "abandon";
}

function operationKey(kind: CreationPageOperation, sessionId: string): string {
  return `${kind}:${sessionId}`;
}
