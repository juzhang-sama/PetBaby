/** Guards asynchronous wizard work against later visits and pet mutations. */
export type WizardOperation = "compile" | "activate";

export interface WizardOperationToken {
  visit: number;
  petId: string;
  kind: WizardOperation;
  revision: number;
}

export interface WizardRefreshToken {
  visit: number;
  petId: string;
  revision: number;
}

export function refreshFailureDisposition(input: {
  currentVisitSamePet: boolean;
  revisionMatches: boolean;
}): { syncControls: boolean; message: string | null } {
  if (!input.currentVisitSamePet) return { syncControls: false, message: null };
  return {
    syncControls: true,
    message: input.revisionMatches ? "恢复最新状态失败，可重试" : null,
  };
}

export function resumeDisposition(status: string, canApply: boolean): "ignore" | "corrupt" | "restore" {
  if (!canApply) return "ignore";
  return status === "corrupt" ? "corrupt" : "restore";
}

export class CreationWizardRun {
  private visit = 0;
  private active = false;
  private submittingVisit: number | null = null;
  private readonly petRevisions = new Map<string, number>();
  private readonly compilingPets = new Set<string>();
  private readonly activatingPets = new Set<string>();

  enter(): number {
    this.visit += 1;
    this.active = true;
    this.submittingVisit = null;
    return this.visit;
  }

  leave(): void {
    this.visit += 1;
    this.active = false;
    this.submittingVisit = null;
  }

  isCurrent(visit: number): boolean {
    return this.active && this.visit === visit;
  }

  beginSubmission(visit: number): boolean {
    if (!this.isCurrent(visit) || this.submittingVisit !== null) return false;
    this.submittingVisit = visit;
    return true;
  }

  endSubmission(visit: number): void {
    if (this.submittingVisit === visit) this.submittingVisit = null;
  }

  shouldCompensateCreatedPet(visit: number): boolean {
    return !this.isCurrent(visit);
  }

  shouldPersistPet(visit: number): boolean {
    return this.isCurrent(visit);
  }

  beginGeneration(visit: number, petId: string): number | null {
    if (!this.isCurrent(visit)) return null;
    const revision = (this.petRevisions.get(petId) ?? 0) + 1;
    this.petRevisions.set(petId, revision);
    return revision;
  }

  beginOperation(visit: number, kind: WizardOperation, petId: string): WizardOperationToken | null {
    if (!this.isCurrent(visit) || this.busyPets(kind).has(petId)) return null;
    this.busyPets(kind).add(petId);
    return { visit, petId, kind, revision: this.petRevisions.get(petId) ?? 0 };
  }

  settleOperation(token: WizardOperationToken): void {
    this.busyPets(token.kind).delete(token.petId);
  }

  isPetBusy(petId: string | null): boolean {
    return !!petId && (this.compilingPets.has(petId) || this.activatingPets.has(petId));
  }

  shouldRefreshStaleOperation(token: WizardOperationToken, currentPetId: string | null): boolean {
    return this.active
      && !this.isCurrent(token.visit)
      && token.petId === currentPetId
      && token.revision === (this.petRevisions.get(token.petId) ?? 0);
  }

  beginRefresh(visit: number, petId: string, revision: number): WizardRefreshToken | null {
    if (!this.isCurrent(visit) || revision !== (this.petRevisions.get(petId) ?? 0)) return null;
    return { visit, petId, revision };
  }

  shouldApplyRefresh(token: WizardRefreshToken, currentPetId: string | null): boolean {
    return this.isCurrent(token.visit)
      && token.petId === currentPetId
      && token.revision === (this.petRevisions.get(token.petId) ?? 0);
  }

  private busyPets(kind: WizardOperation): Set<string> {
    return kind === "compile" ? this.compilingPets : this.activatingPets;
  }
}
