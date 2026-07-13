#!/usr/bin/env python3
"""frames/ の 8フレームから アニメGIF / WebP(透過) / コンタクトシート を生成する。"""
import os, glob
from PIL import Image

FR = sorted(glob.glob("frames/frame_*.png"),
            key=lambda p: int(''.join(filter(str.isdigit, os.path.basename(p)))))
frames = [Image.open(f).convert("RGBA") for f in FR]
SCALE = 4
BG = (22, 24, 29, 255)          # #16181d エディタ背景


def on_bg(im):
    b = Image.new("RGBA", im.size, BG)
    b.alpha_composite(im)
    return b.convert("RGB")


up = [on_bg(f).resize((f.width * SCALE, f.height * SCALE), Image.NEAREST) for f in frames]
upa = [f.resize((f.width * SCALE, f.height * SCALE), Image.NEAREST) for f in frames]

# アニメGIF（暗背景に合成）
up[0].save("neko-typing.gif", save_all=True, append_images=up[1:],
           duration=110, loop=0, disposal=2)
# アニメWebP（透過保持）
upa[0].save("neko-typing.webp", save_all=True, append_images=upa[1:],
            duration=110, loop=0)

# コンタクトシート（4x2, 暗背景, 目視用）
cols, rows, pad = 4, 2, 12
cw, ch = up[0].width, up[0].height
sheet = Image.new("RGB", (cols * cw + (cols + 1) * pad, rows * ch + (rows + 1) * pad), (18, 18, 24))
for i, im in enumerate(up):
    r, c = divmod(i, cols)
    sheet.paste(im, (pad + c * (cw + pad), pad + r * (ch + pad)))
sheet.save("contact.png")
print("frames:", len(frames), "cell:", frames[0].size, "scaled:", up[0].size)
print("wrote neko-typing.gif / neko-typing.webp / contact.png")
