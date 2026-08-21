export interface CatBlinkOverlayPort {
  setEyesOpen(left: number, right: number): void;
  setVisible(visible: boolean): void;
  destroy(): void;
}

export interface CatBlinkOverlayOptions {
  createImage?: () => HTMLImageElement;
}

export class CatBlinkOverlay implements CatBlinkOverlayPort {
  private readonly left: HTMLImageElement;
  private readonly right: HTMLImageElement;
  private visible = false;
  private destroyed = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    imageUrl: string,
    options: CatBlinkOverlayOptions = {},
  ) {
    const createImage = options.createImage ?? (() => document.createElement("img"));
    this.left = createImage();
    this.right = createImage();
    this.configure(this.left, imageUrl, "left");
    this.configure(this.right, imageUrl, "right");
  }

  setEyesOpen(left: number, right: number): void {
    if (this.destroyed) return;
    this.mount();
    this.setEyeOpen(this.left, left);
    this.setEyeOpen(this.right, right);
  }

  setVisible(visible: boolean): void {
    if (this.destroyed) return;
    this.visible = visible;
    this.mount();
    const visibility = visible ? "visible" : "hidden";
    this.left.style.visibility = visibility;
    this.right.style.visibility = visibility;
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.left.remove();
    this.right.remove();
  }

  private configure(image: HTMLImageElement, imageUrl: string, eye: "left" | "right"): void {
    image.className = "pet-blink-overlay";
    image.dataset.eye = eye;
    image.src = imageUrl;
    image.alt = "";
    image.draggable = false;
    image.style.opacity = "0";
    image.style.transform = "scaleY(0)";
    image.style.visibility = this.visible ? "visible" : "hidden";
  }

  private setEyeOpen(image: HTMLImageElement, eyeOpen: number): void {
    const closure = 1 - clamp01(eyeOpen);
    image.style.opacity = closure === 0 ? "0" : "1";
    image.style.transform = `scaleY(${closure})`;
  }

  private mount(): void {
    const parent = this.canvas.parentElement;
    if (!parent) return;
    if (this.left.parentElement !== parent) parent.append(this.left);
    if (this.right.parentElement !== parent) parent.append(this.right);
  }
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, Number.isFinite(value) ? value : 1));
}
