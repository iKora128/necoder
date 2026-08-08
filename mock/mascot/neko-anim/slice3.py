#!/usr/bin/env python3
"""v3: 切り取り精度を上げる。
- セル境界を「列/行プロファイルの谷」にスナップ（隣コマ混入を元から減らす）
- セル内で列ランを解析し、本体ラン＋近接ランのみ採用、左右に離れた断片（隣の尻尾/見切れ）を除去
- 以降は v2 同様: スケール正規化→下辺中央そろえ→snap→透過
使い方: slice3.py SHEET OUTDIR [cols rows] [--frames ...] [--dur 120]
"""
import sys, os, subprocess, statistics
from collections import deque
from PIL import Image
import numpy as np

# pixel-snap ツール（別リポジトリのバイナリ）。PATH か SPRITEFUSION_SNAP で指す。
SNAP = os.environ.get("SPRITEFUSION_SNAP", "spritefusion-pixel-snapper")
BGC = (27, 27, 34)
a = sys.argv[1:]
SHEET, OUT = a[0], a[1]
cols, rows = 4, 2
if len(a) > 3 and not a[2].startswith("--"):
    cols, rows = int(a[2]), int(a[3])
def opt(n, d): return a[a.index(n) + 1] if n in a else d
sel = opt("--frames", None)
dur = int(opt("--dur", 120))
os.makedirs(OUT, exist_ok=True)


def border_bg_mask(arr, bg, thresh=60):
    h, w = arr.shape[:2]
    close = np.abs(arr.astype(int) - bg).sum(2) < thresh
    mask = np.zeros((h, w), bool); dq = deque()
    for x in range(w):
        for y in (0, h - 1):
            if close[y, x] and not mask[y, x]: mask[y, x] = True; dq.append((y, x))
    for y in range(h):
        for x in (0, w - 1):
            if close[y, x] and not mask[y, x]: mask[y, x] = True; dq.append((y, x))
    while dq:
        y, x = dq.popleft()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h and 0 <= nx < w and close[ny, nx] and not mask[ny, nx]:
                mask[ny, nx] = True; dq.append((ny, nx))
    return mask


def snap_cuts(proj, lo, hi, n):
    """[lo,hi] を n 分割する内部境界を、各公称位置の周辺で proj 最小の列にスナップ。"""
    cw = (hi - lo) / n
    cuts = [lo]
    win = cw * 0.42
    for i in range(1, n):
        c = lo + i * cw
        x0, x1 = int(max(lo + 1, c - win)), int(min(hi - 1, c + win))
        cuts.append(x0 + int(np.argmin(proj[x0:x1])) if x1 > x0 else int(c))
    cuts.append(hi + 1)
    return cuts


def isolate_h(cell, gap_frac=0.055):
    """列ランで本体＋近接だけ残す水平範囲 [lo,hi) を返す（左右の離れ断片を除去）。"""
    h, w = cell.shape
    col = cell.sum(0)
    thr = max(2, int(0.02 * h))
    on = col > thr
    runs, s = [], None
    for i, v in enumerate(on):
        if v and s is None: s = i
        if not v and s is not None: runs.append((s, i)); s = None
    if s is not None: runs.append((s, w))
    if not runs: return 0, w
    dense = int(np.argmax(col))
    main = next((r for r in runs if r[0] <= dense < r[1]), runs[0])
    lo, hi = main
    gap = int(gap_frac * w)
    changed = True
    while changed:
        changed = False
        for r in runs:
            if r[1] <= lo and lo - r[1] <= gap: lo = r[0]; changed = True
            elif r[0] >= hi and r[0] - hi <= gap: hi = r[1]; changed = True
    return lo, hi


im = Image.open(SHEET).convert("RGB"); arr = np.array(im); bg = arr[0, 0].astype(int)
content = ~border_bg_mask(arr, bg)
rgba = np.dstack([arr, np.where(content, 255, 0).astype(np.uint8)])
xs = np.where(content.any(0))[0]; ys = np.where(content.any(1))[0]
xcuts = snap_cuts(content.sum(0).astype(float), xs.min(), xs.max(), cols)
ycuts = snap_cuts(content.sum(1).astype(float), ys.min(), ys.max(), rows)

crops = []
order_cells = []
for r in range(rows):
    for c in range(cols):
        ry0, ry1, cx0, cx1 = ycuts[r], ycuts[r + 1], xcuts[c], xcuts[c + 1]
        cell = content[ry0:ry1, cx0:cx1]
        lo, hi = isolate_h(cell)             # 左右の余計物を除去
        sub = cell[:, lo:hi]
        yy, xx = np.where(sub)
        if len(xx) == 0:
            order_cells.append(None); continue
        gx0, gy0 = cx0 + lo + xx.min(), ry0 + yy.min()
        gx1, gy1 = cx0 + lo + xx.max() + 1, ry0 + yy.max() + 1
        order_cells.append((gx0, gy0, gx1, gy1))
order = [int(x) for x in sel.split(",")] if sel else list(range(len(order_cells)))
crops = [Image.fromarray(rgba[order_cells[i][1]:order_cells[i][3], order_cells[i][0]:order_cells[i][2]])
         for i in order if order_cells[i]]

# --- スケール正規化＋下辺そろえ ---
th = int(statistics.median([c.height for c in crops]))
norm = [c.resize((max(1, round(c.width * th / c.height)), th), Image.LANCZOS) for c in crops]
PAD = 10
FW = max(n.width for n in norm) + PAD * 2
FH = th + PAD * 2
strip = Image.new("RGB", (FW * len(norm), FH), BGC)
for i, n in enumerate(norm):
    tmp = Image.new("RGBA", (FW, FH), BGC + (255,))
    tmp.paste(n, ((FW - n.width) // 2, FH - n.height - PAD), n)
    strip.paste(tmp.convert("RGB"), (i * FW, 0))
hi = os.path.join(OUT, "_strip_hi.png"); strip.save(hi)
snp = os.path.join(OUT, "_strip_snap.png")
subprocess.run([SNAP, hi, snp, "32"], capture_output=True, text=True)


def key_frames(strip_img, n):
    arr = np.array(strip_img.convert("RGB")); bg = arr[0, 0].astype(int)
    ct = ~border_bg_mask(arr, bg)
    rgba = np.dstack([arr, np.where(ct, 255, 0).astype(np.uint8)])
    W = arr.shape[1]; fw = W // n; boxes, imgs = [], []
    for i in range(n):
        cell = ct[:, i * fw:(i + 1) * fw]; yy, xx = np.where(cell)
        if len(xx) == 0: boxes.append(None); imgs.append(None); continue
        x0, y0, x1, y1 = xx.min(), yy.min(), xx.max() + 1, yy.max() + 1
        boxes.append((x0, y0, x1, y1)); imgs.append(Image.fromarray(rgba[y0:y1, i * fw + x0:i * fw + x1]))
    fw2 = max(b[2] - b[0] for b in boxes if b) + 2
    fh2 = max(b[3] - b[1] for b in boxes if b) + 2
    out = []
    for im in imgs:
        f = Image.new("RGBA", (fw2, fh2), (0, 0, 0, 0))
        if im: f.paste(im, ((fw2 - im.width) // 2, fh2 - im.height - 1), im)
        out.append(f)
    return out


frames = key_frames(Image.open(snp), len(norm))
for i, f in enumerate(frames): f.save(os.path.join(OUT, f"frame_{i}.png"))
fw, fh = frames[0].size; SC = 4
pad = 10
sheet = Image.new("RGB", (len(frames) * (fw * SC + pad) + pad, fh * SC + 2 * pad), (18, 18, 24))
gif = []
for i, f in enumerate(frames):
    up = f.resize((fw * SC, fh * SC), Image.NEAREST)
    b = Image.new("RGBA", (fw * SC, fh * SC), (22, 24, 29, 255)); b.alpha_composite(up)
    sheet.paste(b.convert("RGB"), (pad + i * (fw * SC + pad), pad)); gif.append(b.convert("RGB"))
sheet.save(os.path.join(OUT, "contact.png"))
gif[0].save(os.path.join(OUT, "anim.gif"), save_all=True, append_images=gif[1:], duration=dur, loop=0, disposal=2)
print(f"{OUT}: frames {len(frames)} cell {frames[0].size}")
