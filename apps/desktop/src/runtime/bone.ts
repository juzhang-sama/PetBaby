export interface Vec2 {
  x: number;
  y: number;
}

/**
 * Skeleton bone definition (foundation for a Live2D/Spine-style runtime).
 * `pivot` is the normalized 0..1 joint position inside the attached part's
 * texture; `offset` is the local offset in pixels from the parent joint to
 * this joint. The affine math only needs `offset` + pose; `pivot` is joint
 * metadata and must match the attached part's `pivot` in the manifest.
 */
export interface Bone {
  id: string;
  parent?: string;
  pivot: Vec2;
  offset: Vec2;
}

/** Per-frame animation state of a bone (defaults: rotation 0, scale 1). */
export interface BonePose {
  rotation?: number;
  scaleX?: number;
  scaleY?: number;
}

/** 2D affine transform matching Pixi's Matrix convention: x' = a*x + c*y + tx. */
export interface AffineTransform {
  a: number;
  b: number;
  c: number;
  d: number;
  tx: number;
  ty: number;
}

export function identityTransform(): AffineTransform {
  return { a: 1, b: 0, c: 0, d: 1, tx: 0, ty: 0 };
}

export function translateTransform(tx: number, ty: number): AffineTransform {
  return { a: 1, b: 0, c: 0, d: 1, tx, ty };
}

export function rotateTransform(radians: number): AffineTransform {
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  return { a: cos, b: sin, c: -sin, d: cos, tx: 0, ty: 0 };
}

export function scaleTransform(sx: number, sy: number): AffineTransform {
  return { a: sx, b: 0, c: 0, d: sy, tx: 0, ty: 0 };
}

/** `parent ∘ local`: apply `local` first, then `parent`. */
export function composeTransforms(
  parent: AffineTransform,
  local: AffineTransform,
): AffineTransform {
  return {
    a: parent.a * local.a + parent.c * local.b,
    b: parent.b * local.a + parent.d * local.b,
    c: parent.a * local.c + parent.c * local.d,
    d: parent.b * local.c + parent.d * local.d,
    tx: parent.a * local.tx + parent.c * local.ty + parent.tx,
    ty: parent.b * local.tx + parent.d * local.ty + parent.ty,
  };
}

/**
 * World transform of a bone: parent chain followed by
 * `translate(offset) ∘ rotate(rotation) ∘ scale(scaleX, scaleY)`.
 * The transform maps points expressed relative to this bone's joint
 * (pivot) into world coordinates.
 */
export function applyBoneTransform(
  bone: Bone,
  pose: BonePose,
  parentWorld: AffineTransform | null,
): AffineTransform {
  const local = composeTransforms(
    translateTransform(bone.offset.x, bone.offset.y),
    composeTransforms(
      rotateTransform(pose.rotation ?? 0),
      scaleTransform(pose.scaleX ?? 1, pose.scaleY ?? 1),
    ),
  );
  return parentWorld ? composeTransforms(parentWorld, local) : local;
}

export function transformPoint(transform: AffineTransform, point: Vec2): Vec2 {
  return {
    x: transform.a * point.x + transform.c * point.y + transform.tx,
    y: transform.b * point.x + transform.d * point.y + transform.ty,
  };
}
