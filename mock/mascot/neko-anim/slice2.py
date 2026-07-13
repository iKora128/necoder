#!/usr/bin/env python3
"""v2: 高解像で切り出し→スケール正規化→下辺中央そろえ→spritefusion snap→均一フレーム。
「フレーム間で大きさが変わる」ドリフトを、被写体高さを中央値にそろえて解消する。

使い方: slice2.py SHEET OUTDIR [cols rows] [--frames 0,2,4,6] [--dur 120]
"""
import sys, os, subprocess, statistics
from collections import deque
from PIL import Image
import numpy as np

SNAP = "/Users/daichi/Work/experience/spritefusion-pixel-snapper/target/release/spritefusion-pixel-snapper"
BGC = (27, 27, 34)  # #1b1b22

a = sys.argv[1:]
SHEET, OUT = a[0], a[1]
cols, rows = 4, 2
if len(a) > 3 and not a[2].startswith("--"):
    cols, rows = int(a[2]), int(a[3])
def opt(name, d):
    return a[a.index(name) + 1] if name in a else d
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


def bands(has, n):
    segs, s = [], None
    for i, v in enumerate(has):
        if v and s is None: s = i
        if not v and s is not None: segs.append((s, i)); s = None
    if s is not None: segs.append((s, len(has)))
    segs.sort(key=lambda t: -(t[1] - t[0]))
    return sorted(segs[:n])


def key_and_frames(strip_img, n):
    """snap 済みフィルムストリップの背景を透過し、n等分→各コマ bbox→下辺中央そろえ均一化。"""
    arr = np.array(strip_img.convert("RGB"))
    bg = arr[0, 0].astype(int)
    content = ~border_bg_mask(arr, bg)
    rgba = np.dstack([arr, np.where(content, 255, 0).astype(np.uint8)])
    W = arr.shape[1]; fw = W // n
    boxes, imgs = [], []
    for i in range(n):
        cell = content[:, i * fw:(i + 1) * fw]
        ys, xs = np.where(cell)
        if len(xs) == 0:
            boxes.append(None); imgs.append(None); continue
        x0, y0, x1, y1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
        boxes.append((x0, y0, x1, y1))
        imgs.append(Image.fromarray(rgba[y0:y1, i * fw + x0:i * fw + x1]))
    fw2 = max(b[2] - b[0] for b in boxes if b) + 2
    fh2 = max(b[3] - b[1] for b in boxes if b) + 2
    out = []
    for im in imgs:
        f = Image.new("RGBA", (fw2, fh2), (0, 0, 0, 0))
        if im: f.paste(im, ((fw2 - im.width) // 2, fh2 - im.height - 1), im)
        out.append(f)
    return out


# --- 1. 高解像スライス ---
im = Image.open(SHEET).convert("RGB"); arr = np.array(im); bg = arr[0, 0].astype(int)
content = ~border_bg_mask(arr, bg)
rgba = np.dstack([arr, np.where(content, 255, 0).astype(np.uint8)])
cb = bands(content.any(0), cols); rb = bands(content.any(1), rows)
cells = []
for (ry0, ry1) in rb:
    for (cx0, cx1) in cb:
        sub = content[ry0:ry1, cx0:cx1]; ys, xs = np.where(sub)
        cells.append(None if len(xs) == 0 else
                     (cx0 + xs.min(), ry0 + ys.min(), cx0 + xs.max() + 1, ry0 + ys.max() + 1))
order = [int(x) for x in sel.split(",")] if sel else list(range(len(cells)))
crops = [Image.fromarray(rgba[c[1]:c[3], c[0]:c[2]]) for c in (cells[i] for i in order) if c]

# --- 2. スケール正規化（被写体高さ→中央値） ---
th = int(statistics.median([c.height for c in crops]))
norm = [c.resize((max(1, round(c.width * th / c.height)), th), Image.LANCZOS) for c in crops]
PAD = 10
FW = max(n.width for n in norm) + PAD * 2
FH = th + PAD * 2
# --- 3. #1b1b22 キャンバスに下辺中央そろえ → 高解像フィルムストリップ ---
strip = Image.new("RGB", (FW * len(norm), FH), BGC)
for i, n in enumerate(norm):
    tmp = Image.new("RGBA", (FW, FH), BGC + (255,))
    tmp.paste(n, ((FW - n.width) // 2, FH - n.height - PAD), n)
    strip.paste(tmp.convert("RGB"), (i * FW, 0))
hi = os.path.join(OUT, "_strip_hi.png"); strip.save(hi)

# --- 4. spritefusion snap（均一スケールなので pixel-size 一貫） ---
snp = os.path.join(OUT, "_strip_snap.png")
r = subprocess.run([SNAP, hi, snp, "32"], capture_output=True, text=True)
print(r.stdout.strip().split("\n")[-2] if r.stdout else r.stderr[-200:])

# --- 5. 透過＋均一フレーム化 ---
frames = key_and_frames(Image.open(snp), len(norm))
for i, f in enumerate(frames):
    f.save(os.path.join(OUT, f"frame_{i}.png"))
print("frames", len(frames), "cell", frames[0].size)

# filmstrip（等幅）＋ 番号付きコンタクト＋ GIF
fw, fh = frames[0].size
SC = 4
strip2 = Image.new("RGBA", (fw * len(frames) * SC, fh * SC), (0, 0, 0, 0))
for i, f in enumerate(frames):
    strip2.paste(f.resize((fw * SC, fh * SC), Image.NEAREST), (i * fw * SC, 0))
strip2.save(os.path.join(OUT, "filmstrip.png"))
# contact（暗背景・番号なし・左右順）
pad = 10
sheet = Image.new("RGB", (len(frames) * (fw * SC + pad) + pad, fh * SC + 2 * pad), (18, 18, 24))
for i, f in enumerate(frames):
    b = Image.new("RGBA", (fw * SC, fh * SC), (22, 24, 29, 255)); b.alpha_composite(f.resize((fw * SC, fh * SC), Image.NEAREST))
    sheet.paste(b.convert("RGB"), (pad + i * (fw * SC + pad), pad))
sheet.save(os.path.join(OUT, "contact.png"))
# gif（暗背景合成）
gif = [Image.new("RGBA", (fw * SC, fh * SC), (22, 24, 29, 255)) for _ in frames]
for g, f in zip(gif, frames): g.alpha_composite(f.resize((fw * SC, fh * SC), Image.NEAREST))
gif = [g.convert("RGB") for g in gif]
gif[0].save(os.path.join(OUT, "anim.gif"), save_all=True, append_images=gif[1:], duration=dur, loop=0, disposal=2)
print("wrote filmstrip.png contact.png anim.gif")
