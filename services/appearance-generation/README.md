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

## 目录

- `src/`：适配层、提示词、后处理、过滤、评估
- `samples/`：真实测试照片（用户同意后放入，不进 git）
- `output/`：运行产物（不进 git）
