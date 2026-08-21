import type {
  WindowMode,
  WindowModeClient,
  WindowModeSnapshot,
} from "../runtime/window-mode-client";

export interface WindowModeControlElements {
  root: HTMLElement;
  choices: readonly HTMLInputElement[];
  status: HTMLElement;
  error: HTMLElement;
  compatibility: HTMLElement;
  retry: HTMLButtonElement;
}

export interface WindowModeControlOptions {
  client: WindowModeClient;
  elements: WindowModeControlElements;
  diagnose?: (error: unknown) => void;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  const message = String(error).trim();
  return message || "桌面宠物没有确认这次模式切换";
}

function isMode(value: string): value is WindowMode {
  return value === "companion" || value === "desktop";
}

function isDegraded(snapshot: WindowModeSnapshot): boolean {
  return snapshot.actualMode === null || snapshot.suppressions.includes("transition");
}

export class WindowModeControl {
  private readonly client: WindowModeClient;
  private readonly elements: WindowModeControlElements;
  private readonly diagnose: (error: unknown) => void;
  private canonical: WindowModeSnapshot | null = null;
  private loaded = false;
  private busy = false;
  private mounted = false;
  private destroyed = false;
  private revision = 0;
  private subscriptionWarning = "";
  private unlistenSnapshot: (() => void) | null = null;
  private page: Pick<Window, "addEventListener" | "removeEventListener"> | null = null;

  constructor(options: WindowModeControlOptions) {
    this.client = options.client;
    this.elements = options.elements;
    this.diagnose = options.diagnose ?? (() => {});
    const modes = new Set(options.elements.choices.map((choice) => choice.value));
    if (options.elements.choices.length !== 2
      || !modes.has("companion")
      || !modes.has("desktop")) {
      throw new TypeError("window mode control requires companion and desktop choices");
    }
  }

  async mount(): Promise<void> {
    if (this.destroyed || this.mounted) return;
    this.mounted = true;
    for (const choice of this.elements.choices) {
      choice.addEventListener("change", this.onChoice);
    }
    this.elements.retry.addEventListener("click", this.onRetry);
    let unlistenSnapshot: (() => void) | null = null;
    try {
      unlistenSnapshot = await this.client.subscribe(
        this.onCanonicalSnapshot,
        this.onInvalidSnapshot,
      );
    } catch (error) {
      this.diagnose(error);
      this.subscriptionWarning = `实时同步不可用，重新进入页面会刷新：${errorMessage(error)}`;
    }
    if (this.destroyed) {
      unlistenSnapshot?.();
      return;
    }
    this.unlistenSnapshot = unlistenSnapshot;
    await this.reload();
  }

  attachPageLifecycle(page: Pick<Window, "addEventListener" | "removeEventListener">): void {
    if (this.destroyed || this.page === page) return;
    this.page?.removeEventListener("pageshow", this.onPageShow);
    this.page?.removeEventListener("focus", this.onPageShow);
    this.page = page;
    page.addEventListener("pageshow", this.onPageShow);
    page.addEventListener("focus", this.onPageShow);
  }

  refresh(): void {
    if (!this.destroyed && !this.busy) void this.reload();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.revision += 1;
    this.unlistenSnapshot?.();
    this.unlistenSnapshot = null;
    this.page?.removeEventListener("pageshow", this.onPageShow);
    this.page?.removeEventListener("focus", this.onPageShow);
    this.page = null;
    if (!this.mounted) return;
    for (const choice of this.elements.choices) {
      choice.removeEventListener("change", this.onChoice);
    }
    this.elements.retry.removeEventListener("click", this.onRetry);
  }

  async choose(mode: WindowMode): Promise<void> {
    if (this.destroyed || this.busy || !this.loaded) return;
    if (this.canonical?.actualMode === mode && !isDegraded(this.canonical)) return;
    const revision = ++this.revision;
    this.busy = true;
    this.elements.status.textContent = mode === "desktop" ? "正在移到桌面图标层…" : "正在恢复陪伴模式…";
    this.elements.error.textContent = "";
    this.elements.compatibility.hidden = true;
    this.elements.compatibility.textContent = "";
    this.renderAvailability();
    try {
      const snapshot = await this.client.set(mode);
      if (!this.isCurrent(revision)) return;
      if (!this.acceptCanonical(snapshot)) return;
      this.renderCanonical();
      if (!isDegraded(snapshot)) {
        this.elements.status.textContent = snapshot.actualMode === "desktop"
          ? "桌面模式已启用"
          : "陪伴模式已启用";
      }
    } catch (error) {
      if (!this.isCurrent(revision)) return;
      this.renderCanonical();
      this.elements.status.textContent = "";
      this.elements.error.textContent = `切换失败：${errorMessage(error)}。可以重试。`;
    } finally {
      if (this.isCurrent(revision)) {
        this.busy = false;
        this.renderAvailability();
      }
    }
  }

  private readonly onChoice = (event: Event): void => {
    const choice = event.currentTarget as HTMLInputElement | null;
    if (choice?.checked && isMode(choice.value)) {
      this.renderCanonical();
      void this.choose(choice.value);
    }
  };

  private readonly onRetry = (): void => { void this.reload(); };
  private readonly onPageShow = (): void => { this.refresh(); };

  private readonly onCanonicalSnapshot = (snapshot: WindowModeSnapshot): void => {
    if (this.destroyed) return;
    if (!this.acceptCanonical(snapshot)) return;
    this.revision += 1;
    this.loaded = true;
    this.busy = false;
    this.renderCanonical();
    this.renderAvailability();
  };

  private readonly onInvalidSnapshot = (error: TypeError): void => {
    if (this.destroyed) return;
    this.elements.error.textContent = error.message;
  };

  private async reload(): Promise<void> {
    if (this.destroyed || this.busy) return;
    const revision = ++this.revision;
    this.busy = true;
    this.elements.root.setAttribute("aria-busy", "true");
    this.elements.status.textContent = "正在读取当前模式…";
    this.elements.error.textContent = "";
    this.elements.retry.hidden = true;
    this.renderAvailability();
    try {
      const snapshot = await this.client.get();
      if (!this.isCurrent(revision)) return;
      this.acceptCanonical(snapshot);
      this.loaded = true;
      this.renderCanonical();
      if (!isDegraded(snapshot)) this.elements.status.textContent = "";
    } catch (error) {
      if (!this.isCurrent(revision)) return;
      this.canonical = null;
      this.loaded = false;
      this.clearSelection();
      this.elements.status.textContent = "";
      this.elements.error.textContent = `无法读取当前模式：${errorMessage(error)}。`;
      this.elements.retry.hidden = false;
    } finally {
      if (this.isCurrent(revision)) {
        this.busy = false;
        this.elements.root.removeAttribute("aria-busy");
        this.renderAvailability();
      }
    }
  }

  private isCurrent(revision: number): boolean {
    return !this.destroyed && revision === this.revision;
  }

  private acceptCanonical(snapshot: WindowModeSnapshot): boolean {
    if (this.canonical && snapshot.revision < this.canonical.revision) return false;
    if (this.canonical
      && snapshot.revision === this.canonical.revision
      && !sameSnapshot(this.canonical, snapshot)) {
      const error = new TypeError("相同版本的窗口模式状态发生冲突，正在重新读取");
      this.diagnose(error);
      this.revision += 1;
      this.busy = false;
      void this.reload();
      return false;
    }
    this.canonical = snapshot;
    return true;
  }

  private clearSelection(): void {
    for (const choice of this.elements.choices) choice.checked = false;
  }

  private renderCanonical(): void {
    const snapshot = this.canonical;
    this.elements.compatibility.hidden = true;
    this.elements.compatibility.textContent = "";
    if (!snapshot || isDegraded(snapshot)) {
      this.clearSelection();
      this.elements.status.textContent = "";
      this.elements.error.textContent = "当前窗口模式无法确认。请选择一种模式重试。";
      return;
    }
    for (const choice of this.elements.choices) {
      choice.checked = choice.value === snapshot.actualMode;
    }
    this.elements.error.textContent = this.subscriptionWarning;
    if (snapshot.actualMode === "desktop" && snapshot.desktopStrategy === "bottomFallback") {
      this.elements.compatibility.textContent = "已使用兼容桌面层";
      this.elements.compatibility.hidden = false;
    }
  }

  private renderAvailability(): void {
    const disabled = this.busy || !this.loaded;
    for (const choice of this.elements.choices) choice.disabled = disabled;
    this.elements.root.setAttribute("aria-busy", String(this.busy));
  }
}

function sameSnapshot(left: WindowModeSnapshot, right: WindowModeSnapshot): boolean {
  return left.revision === right.revision
    && left.desiredMode === right.desiredMode
    && left.actualMode === right.actualMode
    && left.desktopStrategy === right.desktopStrategy
    && left.userVisible === right.userVisible
    && left.suppressions.length === right.suppressions.length
    && left.suppressions.every((value, index) => value === right.suppressions[index]);
}
