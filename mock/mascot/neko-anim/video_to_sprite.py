#!/usr/bin/env python3
"""image→video で得た「キャラ固定」動画をドット絵スプライトに変換する。
動画はキャラがロックされてるので **共通の1枚窓で切り出し**（per-frame 再センタリングしない＝登録保持）
→ 縮小 → **全フレーム共通パレットに量子化**（色フリッカー防止・レトロ感）→ 背景透過。
出力: frames / filmstrip / anim.gif / onion（全身鮮明ならロック成功）/ contact
使い方: video_to_sprite.py FRAMES_DIR OUT [--step 5] [--h 72] [--k 32] [--dur 90] [--pingpong]
"""
import sys, os, glob
from collections import deque
from PIL import Image
import numpy as np

a = sys.argv[1:]
SRC, OUT = a[0], a[1]
def opt(n, d): return a[a.index(n) + 1] if n in a else d
step = int(opt("--step", 5)); H = int(opt("--h", 72)); K = int(opt("--k", 32))
dur = int(opt("--dur", 90)); pingpong = "--pingpong" in a
os.makedirs(OUT, exist_ok=True)

fs = sorted(glob.glob(os.path.join(SRC, "f_*.png")))[::step]
ims = [Image.open(f).convert("RGB") for f in fs]
bg = np.array(ims[0])[0, 0].astype(int)


def mask(im):
    arr = np.array(im).astype(int)
    return np.abs(arr - bg).sum(2) > 70


# 共通コンテンツ bbox（キャラ固定なので全フレーム同じ窓で切る＝登録保持）
U = np.zeros(mask(ims[0]).shape, bool)
for im in ims:
    U |= mask(im)
ys, xs = np.where(U)
p = 12
x0, y0 = max(0, xs.min() - p), max(0, ys.min() - p)
x1, y1 = min(ims[0].width, xs.max() + p), min(ims[0].height, ys.max() + p)
crops = [im.crop((x0, y0, x1, y1)) for im in ims]
w = round(crops[0].width * H / crops[0].height)
small = [c.resize((w, H), Image.LANCZOS) for c in crops]

# 全フレーム共通パレット（連結して量子化 → 各フレームへ適用）
montage = Image.new("RGB", (w * len(small), H))
for i, s in enumerate(small):
    montage.paste(s, (i * w, 0))
pal = montage.quantize(colors=K, method=Image.MEDIANCUT)
quant = [s.quantize(palette=pal, dither=Image.NONE).convert("RGB") for s in small]


def key_transparent(im):
    arr = np.array(im); h, wd = arr.shape[:2]
    b = arr[0, 0].astype(int)
    close = np.abs(arr.astype(int) - b).sum(2) < 30
    m = np.zeros((h, wd), bool); dq = deque()
    for x in range(wd):
        for yy in (0, h - 1):
            if close[yy, x] and not m[yy, x]: m[yy, x] = True; dq.append((yy, x))
    for yy in range(h):
        for x in (0, wd - 1):
            if close[yy, x] and not m[yy, x]: m[yy, x] = True; dq.append((yy, x))
    while dq:
        yy, x = dq.popleft()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = yy + dy, x + dx
            if 0 <= ny < h and 0 <= nx < wd and close[ny, nx] and not m[ny, nx]:
                m[ny, nx] = True; dq.append((ny, nx))
    out = np.dstack([arr, np.where(m, 0, 255).astype(np.uint8)])
    return Image.fromarray(out, "RGBA")


frames = [key_transparent(q) for q in quant]
order = list(range(len(frames)))
if pingpong:
    order = order + list(range(len(frames) - 2, 0, -1))
frames = [frames[i] for i in order]
for i, f in enumerate(frames):
    f.save(os.path.join(OUT, f"frame_{i}.png"))

SC = 4
strip = Image.new("RGBA", (w * len(frames) * SC, H * SC), (0, 0, 0, 0))
sheet = Image.new("RGB", (min(len(frames), 8) * (w * SC + 8) + 8, ((len(frames) + 7) // 8) * (H * SC + 8) + 8), (18, 18, 24))
gif = []; acc = np.zeros((H * SC, w * SC, 3), float)
for i, f in enumerate(frames):
    up = f.resize((w * SC, H * SC), Image.NEAREST)
    strip.paste(up, (i * w * SC, 0))
    b = Image.new("RGBA", (w * SC, H * SC), (22, 24, 29, 255)); b.alpha_composite(up)
    r, cc = divmod(i, 8); sheet.paste(b.convert("RGB"), (8 + cc * (w * SC + 8), 8 + r * (H * SC + 8)))
    gif.append(b.convert("RGB")); acc += np.array(b.convert("RGB"), float)
strip.save(os.path.join(OUT, "filmstrip.png"))
sheet.save(os.path.join(OUT, "contact.png"))
gif[0].save(os.path.join(OUT, "anim.gif"), save_all=True, append_images=gif[1:], duration=dur, loop=0, disposal=2)
Image.fromarray((acc / len(frames)).astype(np.uint8)).save(os.path.join(OUT, "onion.png"))
print(f"{OUT}: {len(frames)} frames, cell {frames[0].size}, filmstrip {strip.size}")
