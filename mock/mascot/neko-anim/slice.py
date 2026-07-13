#!/usr/bin/env python3
"""neko 8コマシートを 8フレームに分割する。
- 背景は縁からのフラッドフィルで透過キーイング（内部の同色は残す）
- 列/行の投影でセル境界を自動検出（等分割に頼らない）
- 各フレームは共通サイズ・下辺中央そろえ（座り姿勢の土台を固定し頭/手だけ動かす）
出力: <OUT>/frame_0..N.png（透過・ドット原寸）
"""
import sys, os
from collections import deque
from PIL import Image
import numpy as np

SRC = sys.argv[1] if len(sys.argv) > 1 else "neko-sheet8-snap.png"
OUT = sys.argv[2] if len(sys.argv) > 2 else "frames"
COLS = int(sys.argv[3]) if len(sys.argv) > 3 else 4
ROWS = int(sys.argv[4]) if len(sys.argv) > 4 else 2
os.makedirs(OUT, exist_ok=True)

im = Image.open(SRC).convert("RGB")
arr = np.array(im)
H, W = arr.shape[:2]
bg = arr[0, 0].astype(int)


def border_bg_mask(a, bg, thresh=48):
    h, w = a.shape[:2]
    close = (np.abs(a.astype(int) - bg).sum(2) < thresh)
    mask = np.zeros((h, w), bool)
    dq = deque()
    for x in range(w):
        for y in (0, h - 1):
            if close[y, x] and not mask[y, x]:
                mask[y, x] = True
                dq.append((y, x))
    for y in range(h):
        for x in (0, w - 1):
            if close[y, x] and not mask[y, x]:
                mask[y, x] = True
                dq.append((y, x))
    while dq:
        y, x = dq.popleft()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h and 0 <= nx < w and close[ny, nx] and not mask[ny, nx]:
                mask[ny, nx] = True
                dq.append((ny, nx))
    return mask


content = ~border_bg_mask(arr, bg)


def bands(has, n):
    segs, s = [], None
    for i, v in enumerate(has):
        if v and s is None:
            s = i
        if not v and s is not None:
            segs.append((s, i)); s = None
    if s is not None:
        segs.append((s, len(has)))
    segs.sort(key=lambda t: -(t[1] - t[0]))
    return sorted(segs[:n])


col_bands = bands(content.any(axis=0), COLS)
row_bands = bands(content.any(axis=1), ROWS)
print("col_bands", col_bands)
print("row_bands", row_bands)

cells = []
for (ry0, ry1) in row_bands:
    for (cx0, cx1) in col_bands:
        sub = content[ry0:ry1, cx0:cx1]
        ys, xs = np.where(sub)
        if len(xs) == 0:
            cells.append(None); continue
        cells.append((cx0 + xs.min(), ry0 + ys.min(), cx0 + xs.max() + 1, ry0 + ys.max() + 1))

ws = [c[2] - c[0] for c in cells if c]
hs = [c[3] - c[1] for c in cells if c]
FW, FH = max(ws) + 4, max(hs) + 4
print("frame size", FW, "x", FH, "count", len([c for c in cells if c]))

rgba = np.dstack([arr, np.where(content, 255, 0).astype(np.uint8)])
for i, c in enumerate(cells):
    frame = Image.new("RGBA", (FW, FH), (0, 0, 0, 0))
    if c:
        x0, y0, x1, y1 = c
        crop = Image.fromarray(rgba[y0:y1, x0:x1], "RGBA")
        cw, ch = x1 - x0, y1 - y0
        frame.paste(crop, ((FW - cw) // 2, FH - ch - 2), crop)  # 下辺中央そろえ
    frame.save(f"{OUT}/frame_{i}.png")
print("saved", len(cells), "frames to", OUT)
