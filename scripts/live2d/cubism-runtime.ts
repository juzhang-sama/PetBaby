import { CubismModelSettingJson } from "@cubism-framework/cubismmodelsettingjson";
import { CubismFramework, LogLevel } from "@cubism-framework/live2dcubismframework";
import type { CubismIdHandle } from "@cubism-framework/id/cubismid";
import { CubismMatrix44 } from "@cubism-framework/math/cubismmatrix44";
import { CubismUserModel } from "@cubism-framework/model/cubismusermodel";
import type { CubismMotion } from "@cubism-framework/motion/cubismmotion";
import { CubismShaderManager_WebGL } from "@cubism-framework/rendering/cubismshader_webgl";
import type { CubismAdapter } from "../../apps/desktop/src/runtime-live2d/cubism-adapter";

class ProbeModel extends CubismUserModel {
  private readonly textures: WebGLTexture[] = [];
  private readonly eyeBlinkIds: CubismIdHandle[] = [];
  private readonly lipSyncIds: CubismIdHandle[] = [];
  private idleMotion: CubismMotion | null = null;

  async load(modelUrl: string, gl: WebGLRenderingContext | WebGL2RenderingContext, canvas: HTMLCanvasElement): Promise<void> {
    const modelSettings = await fetchBuffer(modelUrl, "model3.json");
    const setting = new CubismModelSettingJson(modelSettings, modelSettings.byteLength);
    const baseUrl = modelUrl.slice(0, modelUrl.lastIndexOf("/") + 1);

    const moc = await fetchBuffer(`${baseUrl}${setting.getModelFileName()}`, "moc3");
    this.loadModel(moc);
    const layout = new Map<string, number>();
    setting.getLayoutMap(layout);
    this.getModelMatrix().setupFromLayout(layout);

    this.createRenderer(canvas.width, canvas.height);
    const renderer = this.getRenderer();
    renderer.startUp(gl);
    renderer.setIsPremultipliedAlpha(true);
    renderer.loadShaders("/live2d/Framework/Shaders/WebGL/");
    await waitForShaders(gl);

    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, 1);
    for (let index = 0; index < setting.getTextureCount(); index += 1) {
      const image = await loadImage(`${baseUrl}${setting.getTextureFileName(index)}`);
      const texture = gl.createTexture();
      if (!texture) throw new Error("Cubism 纹理创建失败");
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
      renderer.bindTexture(index, texture);
      this.textures.push(texture);
    }

    if (setting.getMotionCount("Idle") > 0) {
      const motionFile = setting.getMotionFileName("Idle", 0);
      const motionBuffer = await fetchBuffer(`${baseUrl}${motionFile}`, "idle motion");
      this.idleMotion = this.loadMotion(
        motionBuffer,
        motionBuffer.byteLength,
        "Idle_0",
        undefined,
        undefined,
        setting,
        "Idle",
        0,
      );
      for (let index = 0; index < setting.getEyeBlinkParameterCount(); index += 1) {
        this.eyeBlinkIds.push(setting.getEyeBlinkParameterId(index));
      }
      for (let index = 0; index < setting.getLipSyncParameterCount(); index += 1) {
        this.lipSyncIds.push(setting.getLipSyncParameterId(index));
      }
      this.idleMotion?.setEffectIds(this.eyeBlinkIds, this.lipSyncIds);
    }
    this.getModel().saveParameters();
  }

  tick(deltaSeconds: number): void {
    const model = this.getModel();
    model.loadParameters();
    if (this.idleMotion && this._motionManager.isFinished()) {
      this._motionManager.startMotionPriority(this.idleMotion, false, 1);
    }
    this._motionManager.updateMotion(model, deltaSeconds);
    model.saveParameters();
    model.update();
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, width: number, height: number): void {
    const projection = new CubismMatrix44();
    if (this.getModel().getCanvasWidth() > 1 && width < height) {
      this.getModelMatrix().setWidth(2);
      projection.scale(1, width / height);
    } else {
      projection.scale(height / width, 1);
    }
    projection.multiplyByMatrix(this.getModelMatrix());

    const renderer = this.getRenderer();
    renderer.setMvpMatrix(projection);
    renderer.setRenderState(gl.getParameter(gl.FRAMEBUFFER_BINDING), [0, 0, width, height]);
    renderer.drawModel();
  }

  releaseWithTextures(gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    for (const texture of this.textures) gl.deleteTexture(texture);
    this.textures.length = 0;
    this.idleMotion = null;
    this.release();
  }
}

class OfficialCubismAdapter implements CubismAdapter {
  private canvas: HTMLCanvasElement | null = null;
  private gl: WebGLRenderingContext | WebGL2RenderingContext | null = null;
  private model: ProbeModel | null = null;

  async initialize(canvas: HTMLCanvasElement): Promise<void> {
    this.canvas = canvas;
    this.gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false })
      ?? canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false });
    if (!this.gl) throw new Error("WebGL 不可用");
    if (!CubismFramework.isStarted() && !CubismFramework.startUp({
      logFunction: (message: string) => console.info(`[Cubism] ${message}`),
      loggingLevel: LogLevel.LogLevel_Verbose,
    })) {
      throw new Error("Cubism Framework 启动失败");
    }
    if (!CubismFramework.isInitialized()) CubismFramework.initialize();
  }

  async loadModel(modelUrl: string): Promise<void> {
    if (!this.canvas || !this.gl) throw new Error("Cubism 适配器尚未初始化");
    const model = new ProbeModel();
    this.model = model;
    await model.load(modelUrl, this.gl, this.canvas);
  }

  resize(width: number, height: number, dpr: number): void {
    if (!this.canvas) return;
    this.canvas.width = Math.max(1, Math.round(width * dpr));
    this.canvas.height = Math.max(1, Math.round(height * dpr));
    this.model?.setRenderTargetSize(this.canvas.width, this.canvas.height);
  }

  update(deltaMs: number): void {
    this.model?.tick(deltaMs / 1000);
  }

  draw(): void {
    if (!this.model || !this.gl || !this.canvas) return;
    this.gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    this.gl.clearColor(0, 0, 0, 0);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);
    this.model.render(this.gl, this.canvas.width, this.canvas.height);
  }

  destroy(): void {
    try {
      if (this.model && this.gl) this.model.releaseWithTextures(this.gl);
    } catch (error) {
      console.error("Cubism 模型释放失败", error);
    }
    this.model = null;
    try {
      if (CubismFramework.isInitialized()) CubismFramework.dispose();
      if (CubismFramework.isStarted()) CubismFramework.cleanUp();
    } catch (error) {
      console.error("Cubism Framework 释放失败", error);
    }
    this.gl = null;
    this.canvas = null;
  }
}

export function createCubismAdapter(): CubismAdapter {
  return new OfficialCubismAdapter();
}

async function fetchBuffer(url: string, label: string): Promise<ArrayBuffer> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${label} 加载失败 (${response.status}): ${url}`);
  return response.arrayBuffer();
}

async function loadImage(url: string): Promise<HTMLImageElement> {
  const image = new Image();
  image.src = url;
  await image.decode();
  return image;
}

async function waitForShaders(gl: WebGLRenderingContext | WebGL2RenderingContext): Promise<void> {
  const startedAt = performance.now();
  while (!CubismShaderManager_WebGL.getInstance().getShader(gl)._isShaderLoaded) {
    if (performance.now() - startedAt > 5000) throw new Error("Cubism shader 初始化超时");
    await new Promise<void>((resolve) => setTimeout(resolve, 16));
  }
}
