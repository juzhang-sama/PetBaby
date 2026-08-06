export interface CubismAdapter {
  initialize(canvas: HTMLCanvasElement): Promise<void>;
  loadModel(modelUrl: string): Promise<void>;
  resize(width: number, height: number, dpr: number): void;
  update(deltaMs: number): void;
  draw(): void;
  destroy(): void;
}

type CubismParts = any;

/** Thin adapter around the official Cubism Framework copied by prepare:cubism. */
export class CubismAdapterImpl implements CubismAdapter {
  private canvas: HTMLCanvasElement | null = null;
  private gl: WebGLRenderingContext | WebGL2RenderingContext | null = null;
  private model: any = null;
  private framework: CubismParts["CubismFramework"] | null = null;

  async initialize(canvas: HTMLCanvasElement): Promise<void> {
    this.canvas = canvas;
    this.gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false }) ?? canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false });
    if (!this.gl) throw new Error("WebGL unavailable");
    const framework: any = await import("../../.vendor/live2d-cubism-sdk/Framework/src/live2dcubismframework");
    this.framework = framework.CubismFramework;
    if (!this.framework.isStarted()) this.framework.startUp();
    if (!this.framework.isInitialized()) this.framework.initialize();
  }

  async loadModel(modelUrl: string): Promise<void> {
    if (!this.canvas || !this.gl || !this.framework) throw new Error("Cubism adapter is not initialized");
    const [settingsModule, modelModule]: any = await Promise.all([
      import("../../.vendor/live2d-cubism-sdk/Framework/src/cubismmodelsettingjson"),
      import("../../.vendor/live2d-cubism-sdk/Framework/src/model/cubismusermodel"),
    ]);
    const response = await fetch(modelUrl);
    if (!response.ok) throw new Error(`Failed to load model settings (${response.status})`);
    const json = await response.arrayBuffer();
    const setting = new settingsModule.CubismModelSettingJson(json, json.byteLength);
    const userModel = new modelModule.CubismUserModel();
    const base = modelUrl.slice(0, modelUrl.lastIndexOf("/") + 1);
    const mocResponse = await fetch(`${base}${setting.getModelFileName()}`);
    if (!mocResponse.ok) throw new Error(`Failed to load moc3 (${mocResponse.status})`);
    userModel.loadModel(await mocResponse.arrayBuffer());
    userModel.createRenderer(this.canvas.width, this.canvas.height);
    const renderer = userModel.getRenderer();
    renderer.startUp(this.gl);
    renderer.setIsPremultipliedAlpha(true);
    renderer.loadShaders();
    for (let i = 0; i < setting.getTextureCount(); i++) {
      const textureUrl = `${base}${setting.getTextureFileName(i)}`;
      const image = await this.loadImage(textureUrl);
      const texture = this.gl.createTexture();
      if (!texture) throw new Error("Failed to create texture");
      this.gl.bindTexture(this.gl.TEXTURE_2D, texture);
      this.gl.texParameteri(this.gl.TEXTURE_2D, this.gl.TEXTURE_MIN_FILTER, this.gl.LINEAR);
      this.gl.texParameteri(this.gl.TEXTURE_2D, this.gl.TEXTURE_MAG_FILTER, this.gl.LINEAR);
      this.gl.texImage2D(this.gl.TEXTURE_2D, 0, this.gl.RGBA, this.gl.RGBA, this.gl.UNSIGNED_BYTE, image);
      renderer.bindTexture(i, texture);
    }
    this.model = userModel;
  }

  resize(width: number, height: number, dpr: number): void {
    if (!this.canvas) return;
    this.canvas.width = Math.max(1, Math.round(width * dpr));
    this.canvas.height = Math.max(1, Math.round(height * dpr));
    this.model?.setRenderTargetSize(this.canvas.width, this.canvas.height);
  }

  update(_deltaMs: number): void {
    this.model?.getModel()?.update();
  }

  draw(): void {
    if (!this.model || !this.gl) return;
    this.gl.viewport(0, 0, this.canvas!.width, this.canvas!.height);
    this.gl.clearColor(0, 0, 0, 0);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);
    const matrixModule = this.model.getModelMatrix().constructor;
    const matrix = new matrixModule();
    this.model.draw(matrix);
  }

  destroy(): void {
    this.model?.release();
    this.model = null;
    if (this.framework?.isInitialized()) this.framework.dispose();
    this.framework?.cleanUp();
    this.framework = null;
    this.gl = null;
    this.canvas = null;
  }

  private async loadImage(url: string): Promise<HTMLImageElement | ImageBitmap> {
    if (typeof Image === "undefined") throw new Error("Image API unavailable");
    const image = new Image();
    image.src = url;
    await image.decode();
    return image;
  }
}

/** Repository-side seam; the official Cubism SDK is intentionally supplied out-of-band. */
export class UnavailableCubismAdapter implements CubismAdapter {
  async initialize(_canvas: HTMLCanvasElement): Promise<void> {
    throw new Error("Cubism SDK 未提供：请先运行 scripts/准备CubismSDK.ps1");
  }
  async loadModel(_modelUrl: string): Promise<void> {
    throw new Error("Cubism SDK 未提供，无法加载模型");
  }
  resize(_width: number, _height: number, _dpr: number): void {}
  update(_deltaMs: number): void {}
  draw(): void {}
  destroy(): void {}
}
