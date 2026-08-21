import type {
  PetGender,
  PetProfile,
  PetProfileClient,
} from "../pets/pet-profile-contract";

export interface PetProfileEditorElements {
  root: HTMLElement;
  form: HTMLFormElement;
  name: HTMLInputElement;
  gender: HTMLSelectElement;
  birthDate: HTMLInputElement;
  cancel: HTMLButtonElement;
  save: HTMLButtonElement;
  loading: HTMLElement;
  error: HTMLElement;
}

export interface PetProfileEditorOptions {
  elements: PetProfileEditorElements;
  refreshCatalog(): Promise<boolean>;
  setStatus(message: string, tone: "info" | "error"): void;
}

export class PetProfileEditor {
  private activePetId: string | null = null;
  private anchor: HTMLElement | null = null;
  private opener: HTMLButtonElement | null = null;
  private revision = 0;
  private loaded = false;
  private saving = false;
  private pendingOpen: Promise<void> | null = null;

  constructor(
    private readonly client: PetProfileClient,
    private readonly options: PetProfileEditorOptions,
  ) {
    options.elements.error.setAttribute("role", "alert");
    options.elements.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.save();
    });
    options.elements.cancel.addEventListener("click", () => this.cancel());
    options.elements.root.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      this.cancel();
    });
  }

  get editingPetId(): string | null {
    return this.activePetId;
  }

  open(petId: string, anchor: HTMLElement, opener: HTMLButtonElement): Promise<void> {
    const elements = this.options.elements;
    if (this.activePetId === petId && !elements.root.hidden) {
      this.anchor = anchor;
      this.opener = opener;
      anchor.append(elements.root);
      if (this.loaded) {
        elements.name.focus();
        return Promise.resolve();
      }
      if (this.pendingOpen) {
        elements.cancel.focus();
        return this.pendingOpen;
      }
      return this.beginLoad(petId);
    }

    this.activePetId = petId;
    this.anchor = anchor;
    this.opener = opener;
    elements.root.hidden = false;
    anchor.append(elements.root);
    return this.beginLoad(petId);
  }

  private beginLoad(petId: string): Promise<void> {
    const revision = ++this.revision;
    this.loaded = false;
    this.saving = false;
    this.clearProfile();
    this.setError("");
    this.setBusy(true, "正在读取宠物资料…");
    this.options.elements.cancel.focus();
    const operation = this.load(revision, petId);
    this.pendingOpen = operation;
    return operation;
  }

  async save(): Promise<void> {
    if (!this.activePetId || !this.loaded || this.saving) return;
    const gender = this.options.elements.gender.value;
    if (!isGender(gender)) {
      this.setError("性别选项无效，请重新选择。");
      return;
    }

    const petId = this.activePetId;
    const revision = this.revision;
    const elements = this.options.elements;
    this.saving = true;
    this.setError("");
    this.setBusy(true, "正在保存资料…");
    try {
      const canonical = await this.client.update({
        petId,
        value: {
          displayName: elements.name.value,
          gender,
          birthDate: elements.birthDate.value || null,
        },
      });
      if (!this.isCurrent(revision, petId)) return;
      if (canonical.petId !== petId || !canonical.editable) {
        throw new Error("后端返回了不匹配的宠物资料");
      }
      this.fill(canonical);
      let refreshed = false;
      try {
        refreshed = await this.options.refreshCatalog();
      } catch {
        // The canonical profile is already saved; a refresh failure is a distinct partial success.
      }
      if (!this.isCurrent(revision, petId)) return;
      if (!refreshed) {
        this.setError("资料已保存，但列表刷新失败，请稍后重试。");
        this.setBusy(false, "");
        return;
      }
      this.close(false);
      this.options.setStatus("资料已保存", "info");
    } catch (error) {
      if (!this.isCurrent(revision, petId)) return;
      this.setError(`保存失败。请稍后重试：${errorMessage(error)}`);
      this.setBusy(false, "");
    } finally {
      if (this.isCurrent(revision, petId)) this.saving = false;
    }
  }

  cancel(): void {
    this.close(true);
  }

  reconcileAnchor(anchor: HTMLElement | null, opener: HTMLButtonElement | null): void {
    if (!this.activePetId) return;
    if (!anchor || !opener) {
      this.close(false);
      return;
    }
    this.anchor = anchor;
    this.opener = opener;
    anchor.append(this.options.elements.root);
  }

  private async load(revision: number, petId: string): Promise<void> {
    try {
      const loaded = await this.client.get(petId);
      if (!this.isCurrent(revision, petId)) return;
      if (loaded.petId !== petId || !loaded.editable) {
        this.options.setStatus("这只宠物的资料不可编辑。", "error");
        this.close(true);
        return;
      }
      this.fill(loaded);
      this.loaded = true;
      this.setBusy(false, "");
      this.options.elements.name.focus();
    } catch (error) {
      if (!this.isCurrent(revision, petId)) return;
      this.setError(`读取资料失败。请稍后重试：${errorMessage(error)}`);
      this.setBusy(false, "", true);
    } finally {
      if (this.isCurrent(revision, petId)) this.pendingOpen = null;
    }
  }

  private fill(profile: PetProfile): void {
    const elements = this.options.elements;
    elements.name.value = profile.displayName;
    elements.gender.value = profile.gender;
    elements.birthDate.value = profile.birthDate ?? "";
  }

  private clearProfile(): void {
    const elements = this.options.elements;
    elements.name.value = "";
    elements.gender.value = "";
    elements.birthDate.value = "";
  }

  private setBusy(busy: boolean, message: string, controlsDisabled = busy): void {
    const elements = this.options.elements;
    elements.name.disabled = controlsDisabled;
    elements.gender.disabled = controlsDisabled;
    elements.birthDate.disabled = controlsDisabled;
    elements.save.disabled = controlsDisabled;
    elements.cancel.disabled = false;
    elements.root.setAttribute("aria-busy", String(busy));
    elements.loading.textContent = message;
  }

  private setError(message: string): void {
    this.options.elements.error.textContent = message;
  }

  private isCurrent(revision: number, petId: string): boolean {
    return this.revision === revision && this.activePetId === petId;
  }

  private close(restoreFocus: boolean): void {
    const opener = this.opener;
    ++this.revision;
    this.activePetId = null;
    this.anchor = null;
    this.opener = null;
    this.pendingOpen = null;
    this.loaded = false;
    this.saving = false;
    this.setBusy(false, "");
    this.setError("");
    this.options.elements.root.hidden = true;
    if (restoreFocus) opener?.focus();
  }
}

function isGender(value: string): value is PetGender {
  return value === "unknown" || value === "male" || value === "female";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
