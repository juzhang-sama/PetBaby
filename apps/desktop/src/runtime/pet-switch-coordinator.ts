import type { PreparedRuntimeSwap, PetRuntimeSlot, MountedPetRuntime } from "./pet-runtime-slot";
import type {
  CommitCompensation,
  CommitReconciliation,
  PetSwitchErrorCode,
  PetSwitchRequest,
  PetSwitchResult,
  RuntimePetDescriptor,
} from "./pet-switch-protocol";

export interface PetSwitchCoordinatorPorts {
  prepare(requestId: string, petId: string): Promise<RuntimePetDescriptor>;
  load(descriptor: RuntimePetDescriptor, stagingRoot: HTMLElement): Promise<MountedPetRuntime>;
  probe(surface: HTMLCanvasElement): void;
  commit(request: PetSwitchRequest): Promise<void>;
  rollbackCommit(previousPetId: string, request: PetSwitchRequest): Promise<CommitCompensation>;
  reconcileCommit(previousPetId: string, request: PetSwitchRequest): Promise<CommitReconciliation>;
  cancel(requestId: string): Promise<void>;
  finish(requestId: string): Promise<void>;
  refreshHitRegion(): Promise<void>;
}

export class PetSwitchCoordinator {
  private busy = false;

  constructor(
    private readonly slot: PetRuntimeSlot,
    private readonly ports: PetSwitchCoordinatorPorts,
  ) {}

  async switch(request: PetSwitchRequest): Promise<PetSwitchResult> {
    if (this.busy) {
      this.log(request, "request-stale");
      return failure(request, "request-stale", "已有宠物切换正在进行");
    }

    this.busy = true;
    let swap: PreparedRuntimeSwap | undefined;
    let committed = false;
    let cleanup: "open" | "cancelled" | "released" | "releaseUnknown" = "open";
    const cancel = async (): Promise<void> => {
      if (cleanup !== "open") return;
      cleanup = "cancelled";
      try {
        await this.ports.cancel(request.requestId);
      } catch (error) {
        this.log(request, "cancel-failed", error);
      }
    };
    const finish = async (): Promise<string> => {
      if (cleanup !== "open") return "";
      try {
        await this.ports.finish(request.requestId);
        cleanup = "released";
        return "";
      } catch (error) {
        cleanup = "releaseUnknown";
        this.log(request, "finish-failed", error);
        return `；变更门释放未确认：${messageOf(error)}`;
      }
    };

    try {
      this.log(request, "prepare");
      const descriptor = await this.ports.prepare(request.requestId, request.petId);
      this.log(request, "load");
      const runtime = await this.ports.load(descriptor, document.createElement("div"));
      this.log(request, "probe");
      swap = this.slot.prepare(runtime);
      this.ports.probe(runtime.getSurface());
      this.log(request, "activate");
      swap.activate();
      await this.ports.refreshHitRegion();

      if (runtime.isPreviewFallback?.()) {
        const rollbackConverged = this.rollbackSafely(request, swap);
        await this.ports.refreshHitRegion().catch(() => undefined);
        await cancel();
        return failure(
          request,
          "load-failed",
          withRollbackState("候选运行时已降级为预览帧", rollbackConverged),
        );
      }

      try {
        this.log(request, "commit");
        await this.ports.commit(request);
        committed = true;
      } catch (error) {
        this.log(request, "persist-failed", error);
        const rollbackConverged = this.rollbackSafely(request, swap);
        await this.ports.refreshHitRegion().catch(() => undefined);
        let reconciliation: CommitReconciliation;
        try {
          reconciliation = await this.ports.reconcileCommit(swap.previous.petId, request);
        } catch (reconciliationError) {
          this.log(request, "commit-reconciliation-failed", reconciliationError);
          reconciliation = { status: "unknown", warning: messageOf(reconciliationError) };
        }
        if (reconciliation.status === "notCommitted") await cancel();
        const finalization = reconciliation.status === "compensated" ? await finish() : "";
        const warning = reconciliation.warning ? `；对账提示：${reconciliation.warning}` : "";
        return failure(
          request,
          "persist-failed",
          withRollbackState(`${messageOf(error)}${warning}${finalization}`, rollbackConverged),
        );
      }

      if (runtime.isPreviewFallback?.()) {
        const compensation = await this.compensateCommit(request, swap.previous.petId);
        const rollbackConverged = this.rollbackSafely(request, swap);
        await this.ports.refreshHitRegion().catch(() => undefined);
        const finalization = compensation.converged ? await finish() : "";
        return failure(
          request,
          "load-failed",
          withRollbackState(`候选运行时已降级为预览帧${compensation.message}${finalization}`, rollbackConverged),
        );
      }

      try {
        swap.commit();
      } catch (error) {
        this.log(request, "cleanup", error);
      }
      const finalization = await finish();
      this.log(request, "complete");
      return finalization
        ? { ok: true, requestId: request.requestId, petId: request.petId, warning: finalization.slice(1) }
        : { ok: true, requestId: request.requestId, petId: request.petId };
    } catch (error) {
      this.log(request, "failed", error);
      const rollbackConverged = swap ? this.rollbackSafely(request, swap) : true;
      await this.ports.refreshHitRegion().catch(() => undefined);
      let compensationMessage = "";
      if (committed && swap) {
        const compensation = await this.compensateCommit(request, swap.previous.petId);
        compensationMessage = compensation.message;
        if (compensation.converged) compensationMessage += await finish();
      } else {
        await cancel();
      }
      return failure(
        request,
        classify(error),
        withRollbackState(`${messageOf(error)}${compensationMessage}`, rollbackConverged),
      );
    } finally {
      this.busy = false;
    }
  }

  private log(request: PetSwitchRequest, stage: string, error?: unknown): void {
    const details = { requestId: request.requestId, petId: request.petId, stage };
    if (error === undefined) console.info("Pet switch", details);
    else console.error("Pet switch", { ...details, message: messageOf(error) });
  }

  private async compensateCommit(
    request: PetSwitchRequest,
    previousPetId: string,
  ): Promise<{ converged: boolean; message: string }> {
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      try {
        const compensation = await this.ports.rollbackCommit(previousPetId, request);
        if (compensation.status === "compensated") {
          const warning = compensation.warning ? `；补偿提示：${compensation.warning}` : "";
          return { converged: true, message: warning };
        }
        this.log(request, `persist-compensation-${attempt}`, compensation.warning ?? "database state unknown");
        if (attempt === 2) {
          const warning = compensation.warning ? `：${compensation.warning}` : "";
          return { converged: false, message: `；持久化补偿状态未知${warning}` };
        }
      } catch (error) {
        this.log(request, `persist-compensation-${attempt}`, error);
        if (attempt === 2) {
          return { converged: false, message: `；持久化补偿失败：${messageOf(error)}` };
        }
      }
    }
    return { converged: false, message: "；持久化补偿状态未知" };
  }

  private rollbackSafely(request: PetSwitchRequest, swap: PreparedRuntimeSwap): boolean {
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      try {
        swap.rollback();
        return true;
      } catch (error) {
        this.log(request, `rollback-${attempt}`, error);
      }
    }
    return false;
  }
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function classify(error: unknown): PetSwitchErrorCode {
  const message = messageOf(error).toLowerCase();
  if (message.includes("blank-frame")) return "blank-frame";
  if (message.includes("corrupt") || message.includes("hash")) return "asset-corrupt";
  if (message.includes("not found") || message.includes("missing")) return "target-not-found";
  return "load-failed";
}

function failure(
  request: PetSwitchRequest,
  code: PetSwitchErrorCode,
  message: string,
): PetSwitchResult {
  return { ok: false, requestId: request.requestId, petId: request.petId, code, message };
}

function withRollbackState(message: string, rollbackConverged: boolean): string {
  return rollbackConverged ? message : `${message}；rollback 未收敛，visual state unknown`;
}
