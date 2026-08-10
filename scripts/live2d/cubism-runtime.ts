import { CubismModelSettingJson } from "@cubism-framework/cubismmodelsettingjson";
import { CubismFramework, LogLevel } from "@cubism-framework/live2dcubismframework";
import type { CubismIdHandle } from "@cubism-framework/id/cubismid";
import { CubismMatrix44 } from "@cubism-framework/math/cubismmatrix44";
import { CubismUserModel } from "@cubism-framework/model/cubismusermodel";
import type { ACubismMotion } from "@cubism-framework/motion/acubismmotion";
import type { CubismMotion } from "@cubism-framework/motion/cubismmotion";
import { CubismShaderManager_WebGL } from "@cubism-framework/rendering/cubismshader_webgl";
import { resolveCubismResourceUrl } from "../../apps/desktop/src/runtime-live2d/cubism-adapter";
import {
  WebViewCubismFrameworkLifetime,
  type CubismFrameworkLease,
} from "../../apps/desktop/src/runtime-live2d/cubism-framework-lifetime";
import {
  WebViewCubismShaderContextLifetime,
  type CubismShaderContextLease,
  type CubismShaderContextManager,
} from "../../apps/desktop/src/runtime-live2d/cubism-shader-context-lifetime";
import type {
  CubismControlAdapter,
  CubismMotionOptions,
} from "../../apps/desktop/src/runtime-live2d/cubism-model-loader";

const frameworkLifetime = new WebViewCubismFrameworkLifetime(CubismFramework, {
  logFunction: (message: string) => console.info(`[Cubism] ${message}`),
  loggingLevel: LogLevel.LogLevel_Verbose,
});

type CubismGlContext = WebGLRenderingContext | WebGL2RenderingContext;

function getPinnedShaderManager(): CubismShaderContextManager<CubismGlContext> {
  const manager = CubismShaderManager_WebGL.getInstance();
  // The pinned Cubism SDK has no public per-context delete API. Keep this
  // version-specific cast at the integration boundary and expose a narrow port.
  const shaderMap = (manager as unknown as {
    _shaderMap: Map<CubismGlContext, { release(): void }>;
  })._shaderMap;
  return {
    getShader: (gl) => manager.getShader(gl),
    deleteShader: (gl) => shaderMap.delete(gl),
  };
}

const shaderContextLifetime = new WebViewCubismShaderContextLifetime(
  getPinnedShaderManager,
  (error) => console.error("Cubism shader context 释放失败", error),
);

class ProbeModel extends CubismUserModel {
  private readonly textures: WebGLTexture[] = [];
  private readonly eyeBlinkIds: CubismIdHandle[] = [];
  private readonly lipSyncIds: CubismIdHandle[] = [];
  private readonly motions = new Map<string, CubismMotion>();
  private readonly expressions = new Map<string, ACubismMotion>();
  private readonly hitAreas = new Map<string, CubismIdHandle>();
  private viewProjection = new CubismMatrix44();

  async load(modelUrl: string, gl: WebGLRenderingContext | WebGL2RenderingContext, canvas: HTMLCanvasElement): Promise<void> {
    const modelSettings = await fetchBuffer(modelUrl, "model3.json");
    const setting = new CubismModelSettingJson(modelSettings, modelSettings.byteLength);
    const resolveResource = (resourceUrl: string) => resolveCubismResourceUrl(modelUrl, resourceUrl);
    const moc = await fetchBuffer(resolveResource(setting.getModelFileName()), "moc3");
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
      const image = await loadImage(resolveResource(setting.getTextureFileName(index)));
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

    for (let index = 0; index < setting.getEyeBlinkParameterCount(); index += 1) {
      this.eyeBlinkIds.push(setting.getEyeBlinkParameterId(index));
    }
    for (let index = 0; index < setting.getLipSyncParameterCount(); index += 1) {
      this.lipSyncIds.push(setting.getLipSyncParameterId(index));
    }
    for (let groupIndex = 0; groupIndex < setting.getMotionGroupCount(); groupIndex += 1) {
      const group = setting.getMotionGroupName(groupIndex);
      for (let index = 0; index < setting.getMotionCount(group); index += 1) {
        const motionBuffer = await fetchBuffer(
          resolveResource(setting.getMotionFileName(group, index)),
          `motion ${group}[${index}]`,
        );
        const motion = this.loadMotion(
          motionBuffer,
          motionBuffer.byteLength,
          `${group}_${index}`,
          undefined,
          undefined,
          setting,
          group,
          index,
        );
        motion.setEffectIds(this.eyeBlinkIds, this.lipSyncIds);
        this.motions.set(`${group}:${index}`, motion);
      }
    }
    for (let index = 0; index < setting.getExpressionCount(); index += 1) {
      const name = setting.getExpressionName(index);
      const buffer = await fetchBuffer(resolveResource(setting.getExpressionFileName(index)), `expression ${name}`);
      this.expressions.set(name, this.loadExpression(buffer, buffer.byteLength, name));
    }
    for (let index = 0; index < setting.getHitAreasCount(); index += 1) {
      this.hitAreas.set(setting.getHitAreaName(index), setting.getHitAreaId(index));
    }
    const physicsFile = setting.getPhysicsFileName();
    if (physicsFile) {
      const buffer = await fetchBuffer(resolveResource(physicsFile), "physics");
      this.loadPhysics(buffer, buffer.byteLength);
    }
    const poseFile = setting.getPoseFileName();
    if (poseFile) {
      const buffer = await fetchBuffer(resolveResource(poseFile), "pose");
      this.loadPose(buffer, buffer.byteLength);
    }
    this.getModel().saveParameters();
  }

  tick(deltaSeconds: number, parameterValues: ReadonlyMap<string, number>): void {
    const model = this.getModel();
    model.loadParameters();
    this._motionManager.updateMotion(model, deltaSeconds);
    model.saveParameters();
    this._expressionManager.updateMotion(model, deltaSeconds);
    for (const [parameterId, value] of parameterValues) {
      this.applyParameter(parameterId, value);
    }
    this._physics?.evaluate(model, deltaSeconds);
    this._pose?.updateParameters(model, deltaSeconds);
    model.update();
  }

  playMotion(
    group: string,
    index: number,
    options: CubismMotionOptions,
    onFinished: () => void,
  ): { cancel(): void } {
    const motion = this.motions.get(`${group}:${index}`);
    if (!motion) return { cancel() {} };
    motion.setIsLoop(options.loop);
    motion.setFinishedMotionHandler(() => onFinished());
    this._motionManager.startMotionPriority(motion, false, options.priority);
    let cancelled = false;
    return {
      cancel: () => {
        if (cancelled) return;
        cancelled = true;
        this._motionManager.stopAllMotions();
      },
    };
  }

  stopAllMotions(): void {
    this._motionManager.stopAllMotions();
  }

  setExpression(name: string, weight: number): void {
    const expression = this.expressions.get(name);
    if (!expression) return;
    expression.setWeight(weight);
    this._expressionManager.startMotion(expression, false);
  }

  private applyParameter(parameterId: string, value: number): void {
    const model = this.getModel();
    const id = CubismFramework.getIdManager().getId(parameterId);
    const index = model.getParameterIndex(id);
    if (index < 0 || index >= model.getParameterCount()) return;
    model.setParameterValueByIndex(index, value);
  }

  getParameterRange(parameterId: string): { min: number; max: number } | null {
    const model = this.getModel();
    const id = CubismFramework.getIdManager().getId(parameterId);
    const index = model.getParameterIndex(id);
    if (index < 0 || index >= model.getParameterCount()) return null;
    return {
      min: model.getParameterMinimumValue(index),
      max: model.getParameterMaximumValue(index),
    };
  }

  hitTest(name: string, point: { x: number; y: number }, width: number, height: number): boolean {
    const drawableId = this.hitAreas.get(name);
    if (!drawableId || width <= 0 || height <= 0) return false;
    const screenX = point.x / width * 2 - 1;
    const screenY = 1 - point.y / height * 2;
    return this.isHit(
      drawableId,
      this.viewProjection.invertTransformX(screenX),
      this.viewProjection.invertTransformY(screenY),
    );
  }

  render(gl: WebGLRenderingContext | WebGL2RenderingContext, width: number, height: number): void {
    const projection = new CubismMatrix44();
    if (this.getModel().getCanvasWidth() > 1 && width < height) {
      this.getModelMatrix().setWidth(2);
      projection.scale(1, width / height);
    } else {
      projection.scale(height / width, 1);
    }
    this.viewProjection = projection.clone();
    projection.multiplyByMatrix(this.getModelMatrix());

    const renderer = this.getRenderer();
    renderer.setMvpMatrix(projection);
    renderer.setRenderState(gl.getParameter(gl.FRAMEBUFFER_BINDING), [0, 0, width, height]);
    renderer.drawModel();
  }

  releaseWithTextures(gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    for (const texture of this.textures) gl.deleteTexture(texture);
    this.textures.length = 0;
    this.motions.clear();
    this.expressions.clear();
    this.hitAreas.clear();
    this.release();
  }
}

class OfficialCubismAdapter implements CubismControlAdapter {
  private canvas: HTMLCanvasElement | null = null;
  private gl: WebGLRenderingContext | WebGL2RenderingContext | null = null;
  private model: ProbeModel | null = null;
  private width = 1;
  private height = 1;
  private frameworkLease: CubismFrameworkLease | null = null;
  private shaderContextLease: CubismShaderContextLease | null = null;
  private pendingParameters = new Map<string, number>();

  async initialize(canvas: HTMLCanvasElement): Promise<void> {
    this.canvas = canvas;
    this.gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false })
      ?? canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false });
    if (!this.gl) throw new Error("WebGL 不可用");
    if (!this.frameworkLease && !this.shaderContextLease) {
      const frameworkLease = frameworkLifetime.acquire();
      try {
        this.shaderContextLease = shaderContextLifetime.acquire(this.gl);
        this.frameworkLease = frameworkLease;
      } catch (error) {
        frameworkLease.release();
        throw error;
      }
    }
  }

  async loadModel(modelUrl: string): Promise<void> {
    if (!this.canvas || !this.gl) throw new Error("Cubism 适配器尚未初始化");
    const model = new ProbeModel();
    this.model = model;
    await model.load(modelUrl, this.gl, this.canvas);
  }

  resize(width: number, height: number, dpr: number): void {
    if (!this.canvas) return;
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    this.canvas.width = Math.max(1, Math.round(width * dpr));
    this.canvas.height = Math.max(1, Math.round(height * dpr));
    this.model?.setRenderTargetSize(this.canvas.width, this.canvas.height);
  }

  update(deltaMs: number): void {
    const parameters = this.pendingParameters;
    this.pendingParameters = new Map();
    this.model?.tick(deltaMs / 1000, parameters);
  }

  draw(): void {
    if (!this.model || !this.gl || !this.canvas) return;
    this.gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    this.gl.clearColor(0, 0, 0, 0);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);
    this.model.render(this.gl, this.canvas.width, this.canvas.height);
  }

  playMotion(
    group: string,
    index: number,
    options: CubismMotionOptions,
    onFinished: () => void,
  ): { cancel(): void } {
    return this.model?.playMotion(group, index, options, onFinished) ?? { cancel() {} };
  }

  stopAllMotions(): void {
    this.model?.stopAllMotions();
  }

  setExpression(name: string, weight: number): void {
    this.model?.setExpression(name, weight);
  }

  setParameter(parameterId: string, value: number): void {
    this.pendingParameters.set(parameterId, value);
  }

  getParameterRange(parameterId: string): { min: number; max: number } | null {
    return this.model?.getParameterRange(parameterId) ?? null;
  }

  hitTest(name: string, point: { x: number; y: number }): boolean {
    return this.model?.hitTest(name, point, this.width, this.height) ?? false;
  }

  destroy(): void {
    try {
      if (this.model && this.gl) this.model.releaseWithTextures(this.gl);
    } catch (error) {
      console.error("Cubism 模型释放失败", error);
    } finally {
      this.model = null;
      this.pendingParameters.clear();
      try {
        this.shaderContextLease?.release();
      } catch (error) {
        console.error("Cubism shader context lease 释放失败", error);
      } finally {
        this.shaderContextLease = null;
        try {
          this.frameworkLease?.release();
        } catch (error) {
          console.error("Cubism Framework 释放失败", error);
        } finally {
          this.frameworkLease = null;
          this.gl = null;
          this.canvas = null;
        }
      }
    }
  }
}

export function createCubismAdapter(): CubismControlAdapter {
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
