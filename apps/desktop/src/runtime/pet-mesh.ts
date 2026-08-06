import { Mesh, MeshGeometry, Texture } from "pixi.js";
import {
  buildGrid,
  heuristicFeatures,
  type FeatureRects,
  type GridData,
  type MeshParams,
} from "./mesh-rig";
import type { Rect } from "./mesh-rig";
import {
  buildSkinBones,
  computeBoneWeights,
  deformSkinnedGrid,
  meshParamsToPoses,
  type SkinRig,
} from "./skinning";

/**
 * Deformable single-image pet body. The texture is drawn on a regular grid
 * whose vertices are moved locally for blink / ear / tail animation, so a
 * single generated PNG can show real local motion without being cut into
 * layers.
 */
export class PetMesh {
  readonly mesh: Mesh;
  private readonly grid: GridData;
  private readonly basePositions: Float32Array;
  private readonly features: FeatureRects;
  private readonly rig: SkinRig;
  private params: MeshParams = { blink: 0, earWobble: 0, tailSway: 0 };

  constructor(texture: Texture, subject: Rect, features?: FeatureRects) {
    this.grid = buildGrid(texture.width, texture.height);
    this.basePositions = new Float32Array(this.grid.positions);
    this.features = features ?? heuristicFeatures(subject, texture.width, texture.height);
    const bones = buildSkinBones(this.features, subject);
    this.rig = {
      bones,
      weights: computeBoneWeights(this.grid.positions, bones),
      boneCount: bones.length,
      vertexCount: this.grid.positions.length / 2,
    };
    this.mesh = new Mesh({
      geometry: new MeshGeometry({
        positions: this.grid.positions,
        uvs: this.grid.uvs,
        indices: this.grid.indices,
      }),
      texture,
    });
    // mirror the sprite anchor convention (0.5, 0): pivot at top-center
    this.mesh.pivot.set(texture.width / 2, 0);
  }

  setParams(params: MeshParams): void {
    this.params = params;
  }

  update(): void {
    this.mesh.geometry.positions = deformSkinnedGrid(
      this.grid,
      this.rig,
      meshParamsToPoses(this.params, this.features),
      this.basePositions,
    );
  }
}
