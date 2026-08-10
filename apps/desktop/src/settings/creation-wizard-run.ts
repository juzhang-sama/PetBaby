/** Guards asynchronous wizard work against a later tab change or restore. */
export type WizardOperation = "compile" | "activate";
export type WizardOperationOutcome = "success" | "failure";

export class CreationWizardRun {
  private visit = 0;
  private active = false;
  private submittingVisit: number | null = null;
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

  beginActivation(visit: number, petId: string): boolean {
    if (!this.isCurrent(visit) || this.activatingPets.has(petId)) return false;
    this.activatingPets.add(petId);
    return true;
  }

  endActivation(petId: string): void {
    this.activatingPets.delete(petId);
  }

  shouldRefreshStaleOperation(visit: number, operationPetId: string, currentPetId: string | null): boolean {
    return this.active && !this.isCurrent(visit) && operationPetId === currentPetId;
  }

  settledStaleOperation(
    visit: number,
    _operation: WizardOperation,
    _outcome: WizardOperationOutcome,
    operationPetId: string,
    currentPetId: string | null,
  ): { refreshCurrentPet: boolean; resetCurrentControls: boolean } {
    const refreshCurrentPet = this.shouldRefreshStaleOperation(visit, operationPetId, currentPetId);
    return { refreshCurrentPet, resetCurrentControls: refreshCurrentPet };
  }
}
