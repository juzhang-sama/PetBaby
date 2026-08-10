/** Guards asynchronous wizard work against a later tab change or restore. */
export class CreationWizardRun {
  private visit = 0;
  private active = false;
  private submittingVisit: number | null = null;

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
}
