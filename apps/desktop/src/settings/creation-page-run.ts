import type { CreationMethod } from "../creation/contracts";

export type CreationRoute = "upload" | "composer" | "adoption";
export type DraftChoice = "continue" | "abandon" | "cancel";

export interface CreationActivityOwner {
  route: CreationRoute;
  kind: string;
  sessionId: string | null;
}

export interface CreationPageActivityPort {
  run<T>(owner: CreationActivityOwner, operation: () => Promise<T>): Promise<T>;
}

export class CreationPageActivity {
  private tail: Promise<void> = Promise.resolve();
  private pending = 0;

  constructor(private readonly onBusy: (busy: boolean) => void) {}

  run<T>(_owner: CreationActivityOwner, operation: () => Promise<T>): Promise<T> {
    this.pending += 1;
    if (this.pending === 1) this.onBusy(true);
    const result = this.tail.then(operation);
    this.tail = result.then(() => undefined, () => undefined);
    return result.finally(() => {
      this.pending -= 1;
      if (this.pending === 0) this.onBusy(false);
    });
  }

  isBusy(): boolean {
    return this.pending > 0;
  }

  async waitForIdle(): Promise<void> {
    const tail = this.tail;
    await tail;
    if (tail !== this.tail) await this.waitForIdle();
  }
}

export class CreationPageFocusManager {
  private readonly triggers = new Map<CreationRoute, HTMLElement>();
  private revision = 0;

  constructor(
    private readonly schedule: (callback: () => void) => void = (callback) => {
      window.requestAnimationFrame(() => callback());
    },
  ) {}

  remember(route: CreationRoute, trigger: HTMLElement): void {
    this.triggers.set(route, trigger);
  }

  enter(_route: CreationRoute, workspace: HTMLElement): void {
    const revision = ++this.revision;
    this.schedule(() => {
      if (revision !== this.revision) return;
      workspace.querySelector<HTMLElement>(
        "[data-creation-entry-focus], button:not([disabled]), [tabindex]:not([tabindex='-1'])",
      )?.focus();
    });
  }

  returnToTrigger(route: CreationRoute): void {
    const revision = ++this.revision;
    const trigger = this.triggers.get(route);
    this.schedule(() => {
      if (revision === this.revision) trigger?.focus();
    });
  }

  cancel(): void {
    this.revision += 1;
  }
}

export interface CreationRouteViewPort {
  open(sessionId: string | null): Promise<void>;
  leave(): void;
}

export interface CreationPageRouterPorts {
  creation: {
    draft(): Promise<import("../creation/contracts").CreationSnapshot | null>;
    abandon(sessionId: string): Promise<void>;
  };
  views: Record<CreationRoute, CreationRouteViewPort>;
  dialog: { showDraftChoice(method: "upload" | "composer"): Promise<DraftChoice> };
  onRoute(route: CreationRoute): void;
  onBusy(busy: boolean): void;
  activity?: CreationPageActivityPort & { waitForIdle(): Promise<void> };
}

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
  private activeRoute: CreationRoute | null = null;
  private routerBusyCount = 0;
  private readonly abandoningDrafts = new Map<string, Promise<void>>();

  constructor(private readonly router?: CreationPageRouterPorts) {}

  routes(): CreationRoute[] {
    return ["upload", "composer", "adoption"];
  }

  async open(requestedRoute: CreationRoute): Promise<CreationRoute | null> {
    const router = this.requireRouter();
    await router.activity?.waitForIdle();
    const visit = this.enter(requestedRoute);
    this.leaveActiveRoute();
    this.setRouterBusy(true);
    try {
      if (requestedRoute === "adoption") {
        return await this.openRoute("adoption", null, visit);
      }

      const draft = await router.creation.draft();
      if (!this.isCurrent(visit)) return null;
      if (!draft) return await this.openRoute(requestedRoute, null, visit);
      if (draft.method === requestedRoute) {
        return await this.openRoute(requestedRoute, draft.sessionId, visit);
      }
      if (draft.method === "adoption") {
        throw new Error("短期认领会话不能占用长期创建草稿");
      }

      const choice = await router.dialog.showDraftChoice(draft.method);
      if (!this.isCurrent(visit)) return null;
      if (choice === "cancel") return null;
      if (choice === "continue") {
        return await this.openRoute(draft.method, draft.sessionId, visit);
      }
      await this.abandonDraft(draft.sessionId, requestedRoute);
      if (!this.isCurrent(visit)) return null;
      return await this.openRoute(requestedRoute, null, visit);
    } finally {
      this.setRouterBusy(false);
    }
  }

  close(): void {
    this.leaveActiveRoute();
    this.leave();
  }

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

  private async openRoute(
    route: CreationRoute,
    sessionId: string | null,
    visit: number,
  ): Promise<CreationRoute | null> {
    const router = this.requireRouter();
    await router.views[route].open(sessionId);
    if (!this.isCurrent(visit)) {
      if (this.activeRoute !== route) router.views[route].leave();
      return null;
    }
    this.activeRoute = route;
    router.onRoute(route);
    return route;
  }

  private leaveActiveRoute(): void {
    if (!this.router || !this.activeRoute) return;
    this.router.views[this.activeRoute].leave();
    this.activeRoute = null;
  }

  private abandonDraft(sessionId: string, route: CreationRoute): Promise<void> {
    const existing = this.abandoningDrafts.get(sessionId);
    if (existing) return existing;
    const router = this.requireRouter();
    const execute = () => router.creation.abandon(sessionId);
    const operation = router.activity
      ? router.activity.run({ route, kind: "abandon-conflicting-draft", sessionId }, execute)
      : execute();
    const tracked = operation.finally(() => {
      if (this.abandoningDrafts.get(sessionId) === tracked) this.abandoningDrafts.delete(sessionId);
    });
    this.abandoningDrafts.set(sessionId, tracked);
    return tracked;
  }

  private setRouterBusy(busy: boolean): void {
    if (!this.router) return;
    this.routerBusyCount += busy ? 1 : -1;
    if (this.routerBusyCount < 0) this.routerBusyCount = 0;
    if (busy && this.routerBusyCount === 1) this.router.onBusy(true);
    if (!busy && this.routerBusyCount === 0) this.router.onBusy(false);
  }

  private requireRouter(): CreationPageRouterPorts {
    if (!this.router) throw new Error("creation page router is not configured");
    return this.router;
  }
}

function isMutation(kind: CreationPageOperation): kind is CreationPageMutation {
  return kind === "submit" || kind === "finalize" || kind === "retry" || kind === "abandon";
}

function operationKey(kind: CreationPageOperation, sessionId: string): string {
  return `${kind}:${sessionId}`;
}
