#!/usr/bin/env python3
"""Shirushi のアプリアイコン（.icns）を生成する。

原画 PNG（正方・full-bleed）に macOS 風の角丸（squircle 近似の rounded-rect）を掛け、
iconset の全サイズを作って `iconutil` で `.icns` にまとめる。マスコット＝猫耳コーダー娘。

使い方:
    python3 scripts/make-icon.py [source.png] [out_dir]
既定: source=mock/mascot/01-neko-coder.png / out_dir=crates/shirushi/assets/icon
"""
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw

# macOS 12+ のアイコンは角丸タイル。1024 で半径 ~184px（約 0.18）が近い。
RADIUS_RATIO = 0.18


def rounded(image: Image.Image) -> Image.Image:
    """正方画像に角丸アルファマスクを掛ける。"""
    size = image.size[0]
    image = image.convert("RGBA")
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle([0, 0, size - 1, size - 1], radius=int(size * RADIUS_RATIO), fill=255)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(image, (0, 0), mask)
    return out


def square_1024(source: str) -> Image.Image:
    """原画を正方 1024 に整える（正方でなければ中央クロップ）。"""
    base = Image.open(source).convert("RGBA")
    width, height = base.size
    if width != height:
        side = min(width, height)
        left, top = (width - side) // 2, (height - side) // 2
        base = base.crop((left, top, left + side, top + side))
    if base.size[0] != 1024:
        base = base.resize((1024, 1024), Image.LANCZOS)
    return base


def main() -> None:
    source = sys.argv[1] if len(sys.argv) > 1 else "mock/mascot/01-neko-coder.png"
    out_dir = sys.argv[2] if len(sys.argv) > 2 else "crates/shirushi/assets/icon"
    os.makedirs(out_dir, exist_ok=True)

    master = rounded(square_1024(source))
    master_path = os.path.join(out_dir, "shirushi.png")
    master.save(master_path)

    # iconset は中間物なので temp に吐く（コミットするのは master と .icns だけ）。
    specs = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]
    with tempfile.TemporaryDirectory() as work:
        iconset = os.path.join(work, "Shirushi.iconset")
        os.makedirs(iconset)
        for size, scale in specs:
            pixels = size * scale
            suffix = "@2x" if scale == 2 else ""
            master.resize((pixels, pixels), Image.LANCZOS).save(
                os.path.join(iconset, f"icon_{size}x{size}{suffix}.png")
            )
        icns = os.path.join(out_dir, "Shirushi.icns")
        subprocess.run(["iconutil", "-c", "icns", "-o", icns, iconset], check=True)
        print(f"生成: {master_path}\n生成: {icns}")


if __name__ == "__main__":
    main()
