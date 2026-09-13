"""帧序列模块共用的路径与哈希小工具。

`rel_to` / `sha256_of` 原先在 `matting.py`、`packing.py` 里各有一份 —— 三处重复必然漂移
（尤其是 `sha256_of` 的分块大小，改一处不改另一处就会让同一文件算出两个哈希）。
抽到这里共用。两个都是纯函数，没有副作用。
"""
from __future__ import annotations

import hashlib
from pathlib import Path

__all__ = ["rel_to", "sha256_of"]


def rel_to(path: Path, base: Path | None) -> str:
    """`base` 给了就输出相对它的路径，否则输出绝对路径。

    服务层不该知道仓库布局，所以基准目录由调用方传（CLI 传仓库根，保持旧输出不变）。
    """
    resolved = Path(path).resolve()
    if base is None:
        return str(resolved)
    try:
        return str(resolved.relative_to(Path(base).resolve()))
    except ValueError:
        return str(resolved)


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()
