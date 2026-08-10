declare module "@cubism-runtime" {
  import type { CubismAdapter } from "./cubism-adapter";

  export function createCubismAdapter(): CubismAdapter;
}

declare module "@cubism-framework/rendering/cubismshader_webgl" {
  export class CubismShaderManager_WebGL {
    static getInstance(): CubismShaderManager_WebGL;
    static deleteInstance(): void;
    getShader(gl: WebGLRenderingContext): { release(): void } | undefined;
  }
}
