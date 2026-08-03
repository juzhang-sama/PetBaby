import { Application, Assets, Sprite } from "pixi.js";
import { applyHitRegion } from "./bridge";
import { computeContainRect } from "./geometry";
import { alphaToRegionSpans } from "./hit-mask";

export class PetStage {
  private readonly app = new Application();

  async mount(root: HTMLElement): Promise<void> {
    await this.app.init({
      resizeTo: root,
      backgroundAlpha: 0,
      antialias: true,
      autoStart: false,
      preference: "webgl",
    });
    root.replaceChildren(this.app.canvas);

    const texture = await Assets.load("/test-assets/pet-probe.png");
    const sprite = new Sprite(texture);
    const layout = computeContainRect(
      { width: texture.width, height: texture.height },
      { width: root.clientWidth, height: root.clientHeight },
    );
    sprite.position.set(layout.x, layout.y);
    sprite.scale.set(layout.scale);
    this.app.stage.addChild(sprite);
    this.app.render();

    const source = texture.source.resource as HTMLImageElement;
    const canvas = document.createElement("canvas");
    canvas.width = root.clientWidth;
    canvas.height = root.clientHeight;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("2D canvas is unavailable for hit-mask extraction");
    context.drawImage(source, layout.x, layout.y, layout.width, layout.height);
    const image = context.getImageData(0, 0, canvas.width, canvas.height);
    const spans = alphaToRegionSpans(image.data, image.width, image.height, {
      alphaThreshold: 32,
      rowStep: 2,
    });
    await applyHitRegion({
      canvasWidth: canvas.width,
      canvasHeight: canvas.height,
      scaleFactor: window.devicePixelRatio,
      spans,
    });
  }
}
