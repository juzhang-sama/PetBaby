/**
 * 照片分身预览的**共用契约**。
 *
 * 两条产线的预览是两个完全不同的渲染栈（像素风 = 单图 + 动作档案；
 * 写实风 = schema 7 帧序列），但**对外形状是同一个**：
 * 视图只需要「挂上去 / 撤下来」，以及一份「我确实看到了在动」的证据。
 *
 * `evidence` 刻意只固定 `renderer` 与 `manifestSha256` 两个字段 ——
 * 它们是两条产线都有的（渲染器名 + 被确认的那份 manifest 的哈希），
 * 其余细节由各自扩展。宽一点的类型让新增产线不用去改视图。
 */

/** 所有渲染器共有的那部分运行证据。 */
export interface PhotoAvatarPreviewEvidence {
  readonly renderer: string;
  /** 与磁盘上那份 `manifest.json` **原始字节**的 sha256 —— 会被回传给后端做绑定校验。 */
  readonly manifestSha256: string;
}

/** 像素风那条（`animated-image-v1`：单图 + 动作档案）。 */
export type PixelRuntimeEvidence = PhotoAvatarPreviewEvidence & {
  readonly renderer: "animated-image-v1";
  readonly neutralPixels: number;
  readonly changedPixels: number;
};

/** 写实风那条（`frame-sequence-v1`：整包逐帧）。 */
export type FrameSequenceRuntimeEvidence = PhotoAvatarPreviewEvidence & {
  readonly renderer: "frame-sequence-v1";
  /** 实际加载并播放的帧数。**必须等于包里的帧数**（全播，不做抽样）。 */
  readonly frameCount: number;
  readonly changedPixels: number;
};

export interface PhotoAvatarPreviewHandle {
  readonly evidence: PhotoAvatarPreviewEvidence | null;
  destroy(): void;
}
