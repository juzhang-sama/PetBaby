declare module "@cubism-runtime" {
  import type { CubismAdapter } from "./cubism-adapter";

  export function createCubismAdapter(): CubismAdapter;
}
