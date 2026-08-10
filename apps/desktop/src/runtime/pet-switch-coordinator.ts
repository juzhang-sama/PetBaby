import type { PreparedRuntimeSwap, PetRuntimeSlot, MountedPetRuntime } from "./pet-runtime-slot";
import type {
  PetSwitchErrorCode,
  PetSwitchRequest,
  PetSwitchResult,
  RuntimePetDescriptor,
} from "./pet-switch-protocol";

export interface PetSwitchCoordinatorPorts {
  prepare(petId: string): Promise<RuntimePetDescriptor>;
  load(descriptor: RuntimePetDescriptor, stagingRoot: HTMLElement): Promise<MountedPetRuntime>;
  probe(surface: HTMLCanvasElement): void;
  commit(petId: string, acceptedVariantId?: string): Promise<void>;
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
    try {
      this.log(request, "prepare");
      const descriptor = await this.ports.prepare(request.petId);
      this.log(request, "load");
      const runtime = await this.ports.load(descriptor, document.createElement("div"));
      this.log(request, "probe");
      swap = this.slot.prepare(runtime);
      this.ports.probe(runtime.getSurface());
      this.log(request, "activate");
      swap.activate();
      await this.ports.refreshHitRegion();
      try {
        this.log(request, "commit");
        await this.ports.commit(request.petId, request.acceptedVariantId);
      } catch (error) {
        this.log(request, "persist-failed", error);
        this.rollbackSafely(request, swap);
        await this.ports.refreshHitRegion().catch(() => undefined);
        return failure(request, "persist-failed", messageOf(error));
      }
      try {
        swap.commit();
      } catch (error) {
        this.log(request, "cleanup", error);
      }
      this.log(request, "complete");
      return { ok: true, requestId: request.requestId, petId: request.petId };
    } catch (error) {
      this.log(request, "failed", error);
      if (swap) this.rollbackSafely(request, swap);
      await this.ports.refreshHitRegion().catch(() => undefined);
      return failure(request, classify(error), messageOf(error));
    } finally {
      this.busy = false;
    }
  }

  private log(request: PetSwitchRequest, stage: string, error?: unknown): void {
    const details = { requestId: request.requestId, petId: request.petId, stage };
    if (error === undefined) console.info("Pet switch", details);
    else console.error("Pet switch", { ...details, message: messageOf(error) });
  }

  private rollbackSafely(request: PetSwitchRequest, swap: PreparedRuntimeSwap): void {
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      try {
        swap.rollback();
        return;
      } catch (error) {
        this.log(request, `rollback-${attempt}`, error);
      }
    }
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
