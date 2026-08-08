#!/usr/bin/env python3
"""v4: フレーム間の「軸」を固定してブレ（ただ揺れて見える問題）を消す。
座位の土台＝下部バンドの水平重心でアンカーし、下辺そろえ。→ 胴体が固定され手/頭/表情だけ動く。
検証: onion.png（全フレーム平均）で土台が鮮明なら軸が固定できている。
使い方: slice4.py SHEET OUTDIR [cols rows] [--frames ...] [--dur 120]
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
    cw = (hi - lo) / n; cuts = [lo]; win = cw * 0.42
    for i in range(1, n):
        c = lo + i * cw; x0, x1 = int(max(lo + 1, c - win)), int(min(hi - 1, c + win))
        cuts.append(x0 + int(np.argmin(proj[x0:x1])) if x1 > x0 else int(c))
    cuts.append(hi + 1); return cuts


def isolate_h(cell, gap_frac=0.055):
    h, w = cell.shape; col = cell.sum(0); thr = max(2, int(0.02 * h)); on = col > thr
    runs, s = [], None
    for i, v in enumerate(on):
        if v and s is None: s = i
        if not v and s is not None: runs.append((s, i)); s = None
    if s is not None: runs.append((s, w))
    if not runs: return 0, w
    dense = int(np.argmax(col)); main = next((r for r in runs if r[0] <= dense < r[1]), runs[0])
    lo, hi = main; gap = int(gap_frac * w); changed = True
    while changed:
        changed = False
        for r in runs:
            if r[1] <= lo and lo - r[1] <= gap: lo = r[0]; changed = True
            elif r[0] >= hi and r[0] - hi <= gap: hi = r[1]; changed = True
    return lo, hi


# --- hi-res スライス → isolate_h → スケール正規化 → snap ---
im = Image.open(SHEET).convert("RGB"); arr = np.array(im); bg = arr[0, 0].astype(int)
content = ~border_bg_mask(arr, bg)
rgba = np.dstack([arr, np.where(content, 255, 0).astype(np.uint8)])
xs = np.where(content.any(0))[0]; ys = np.where(content.any(1))[0]
xcuts = snap_cuts(content.sum(0).astype(float), xs.min(), xs.max(), cols)
ycuts = snap_cuts(content.sum(1).astype(float), ys.min(), ys.max(), rows)
boxes = []
for r in range(rows):
    for c in range(cols):
        ry0, ry1, cx0, cx1 = ycuts[r], ycuts[r + 1], xcuts[c], xcuts[c + 1]
        cell = content[ry0:ry1, cx0:cx1]; lo, hi = isolate_h(cell)
        sub = cell[:, lo:hi]; yy, xx = np.where(sub)
        boxes.append(None if len(xx) == 0 else
                     (cx0 + lo + xx.min(), ry0 + yy.min(), cx0 + lo + xx.max() + 1, ry0 + yy.min() + (yy.max() - yy.min()) + 1))
order = [int(x) for x in sel.split(",")] if sel else list(range(len(boxes)))
crops = [Image.fromarray(rgba[boxes[i][1]:boxes[i][3], boxes[i][0]:boxes[i][2]]) for i in order if boxes[i]]
th = int(statistics.median([c.height for c in crops]))
norm = [c.resize((max(1, round(c.width * th / c.height)), th), Image.LANCZOS) for c in crops]
PAD = 10
FW = max(n.width for n in norm) + PAD * 2; FH = th + PAD * 2
strip = Image.new("RGB", (FW * len(norm), FH), BGC)
for i, n in enumerate(norm):
    tmp = Image.new("RGBA", (FW, FH), BGC + (255,)); tmp.paste(n, ((FW - n.width) // 2, FH - n.height - PAD), n)
    strip.paste(tmp.convert("RGB"), (i * FW, 0))
hi = os.path.join(OUT, "_strip_hi.png"); strip.save(hi)
snp = os.path.join(OUT, "_strip_snap.png")
subprocess.run([SNAP, hi, snp, "32"], capture_output=True, text=True)

# --- 透過 → 土台アンカー配置（軸固定） ---
sarr = np.array(Image.open(snp).convert("RGB")); sbg = sarr[0, 0].astype(int)
sct = ~border_bg_mask(sarr, sbg)
srgba = np.dstack([sarr, np.where(sct, 255, 0).astype(np.uint8)])
W = sarr.shape[1]; n = len(norm); fw = W // n
tights, anchors = [], []
for i in range(n):
    cell = sct[:, i * fw:(i + 1) * fw]; yy, xx = np.where(cell)
    if len(xx) == 0:
        tights.append(None); anchors.append(0.0); continue
    x0, y0, x1, y1 = xx.min(), yy.min(), xx.max() + 1, yy.max() + 1
    tights.append(Image.fromarray(srgba[y0:y1, i * fw + x0:i * fw + x1]))
    m = cell[y0:y1, x0:x1].astype(float)                 # tight mask
    h = m.shape[0]; band = m[max(0, h - max(2, int(0.30 * h))):, :]  # 下部30%＝座位の土台
    col = band.sum(0)
    anchors.append((np.arange(len(col)) * col).sum() / max(col.sum(), 1.0))  # 土台の水平重心
maxleft = max(anchors[i] for i in range(n) if tights[i])
maxright = max(tights[i].width - anchors[i] for i in range(n) if tights[i])
FW2 = int(np.ceil(maxleft + maxright)) + 4; FH2 = max(t.height for t in tights if t) + 4
cxc = maxleft + 2
frames = []
for t, an in zip(tights, anchors):
    f = Image.new("RGBA", (FW2, FH2), (0, 0, 0, 0))
    if t:
        f.paste(t, (int(round(cxc - an)), FH2 - t.height - 2), t)  # 土台重心→中心 / 下辺そろえ
    frames.append(f)
for i, f in enumerate(frames): f.save(os.path.join(OUT, f"frame_{i}.png"))

# --- 出力: strip / contact / gif / onion（検証） ---
fw2, fh2 = frames[0].size; SC = 4; pad = 10
strip2 = Image.new("RGBA", (fw2 * n * SC, fh2 * SC), (0, 0, 0, 0))
sheet = Image.new("RGB", (n * (fw2 * SC + pad) + pad, fh2 * SC + 2 * pad), (18, 18, 24))
gif = []; acc = np.zeros((fh2 * SC, fw2 * SC, 3), float)
for i, f in enumerate(frames):
    up = f.resize((fw2 * SC, fh2 * SC), Image.NEAREST)
    strip2.paste(up, (i * fw2 * SC, 0))
    b = Image.new("RGBA", (fw2 * SC, fh2 * SC), (22, 24, 29, 255)); b.alpha_composite(up)
    sheet.paste(b.convert("RGB"), (pad + i * (fw2 * SC + pad), pad)); gif.append(b.convert("RGB"))
    acc += np.array(b.convert("RGB"), float)
strip2.save(os.path.join(OUT, "filmstrip.png"))
sheet.save(os.path.join(OUT, "contact.png"))
gif[0].save(os.path.join(OUT, "anim.gif"), save_all=True, append_images=gif[1:], duration=dur, loop=0, disposal=2)
Image.fromarray((acc / n).astype(np.uint8)).save(os.path.join(OUT, "onion.png"))  # 平均＝土台鮮明なら軸OK
print(f"{OUT}: frames {n} cell {frames[0].size}")
