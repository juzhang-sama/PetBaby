# Appearance Generation Experiment (M3)

独立实验工具：单张宠物照片 → 招牌画风卡通候选。不进入桌宠主流程。

## 安全

- API Key 只从环境变量读取：复制 `.env.example` 为 `.env` 并填写 `LK888_API_KEY`。
- `.env` 与 `output/`（含实验图片）已被 `.gitignore` 排除，绝不提交。

## 使用

```powershell
pip install -r requirements.txt
python -m src.lk888 --smoke      # 冒烟：生成一张测试图到 output/smoke/
python -m src.run_experiment --photo samples/xxx.jpg --traits traits.json
```

## 自动拆层（Live2D 式分层资产实验）

把一张抠好的宠物透明 PNG 拆成可绑骨骼的部件层：

- GPT-4o 视觉标注返回每个部件的多边形 + 枢轴 + 层级
- MobileSAM（ONNX，CPU 可跑）用框提示把多边形精修为精确掩码
- 输出 `body/head/leftEar/rightEar/leftEye/rightEye/tail` 七张透明 PNG
- `parts.json` 与桌面端 manifest `parts` 契约对齐（anchor/pivot/zIndex/boneId）
- 自动质量门：几何检查（左右对称、耳在头上、头在身之上）+ 不合格自动重试一次

首次使用先下载 SAM 模型（约 45MB，Apache-2.0，目录已被 gitignore）：

```powershell
New-Item -ItemType Directory -Force models\sam-onnx | Out-Null
curl.exe -sL -o models\sam-onnx\mobile_sam_image_encoder.onnx https://huggingface.co/Heliosoph/sam-onnx/resolve/main/mobile_sam_image_encoder.onnx
curl.exe -sL -o models\sam-onnx\sam_mask_decoder_single.onnx https://huggingface.co/Heliosoph/sam-onnx/resolve/main/sam_mask_decoder_single.onnx
```

运行拆层：

```powershell
python -m src.layering --image src/output/style-compare/sample/signature-cartoon-v1-cutout.png --species cat --out output/layering/demo --sam
```

产物：`layers/*.png`（七层）、`parts.json`（manifest parts）、`preview.png`（原图/合成/逐层对比）、`segmentation.png`（分区着色图）、`annotation.json`（模型原始标注）。

## 目录

- `src/`：适配层、提示词、后处理、过滤、评估
- `src/layering.py`：拆层纯逻辑（多边形→掩码→深度分配→图层→还原校验）
- `src/sam_segment.py`：MobileSAM ONNX 掩码精修
- `samples/`：真实测试照片（用户同意后放入，不进 git）
- `output/`：运行产物（不进 git）
