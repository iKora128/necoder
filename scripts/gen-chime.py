#!/usr/bin/env python3
"""通知音（`assets/sounds/`）を生成する。

necoder の音は**録音ではなく合成**で作る。理由は2つ:

- 権利が完全に自前になる（AGPL 配布で説明が要らない・`THIRD_PARTY_NOTICES.md` が増えない）
- 後から調律できる。「高い」「長い」と思ったらこのファイルの数値を直して再生成すればいい

中身は加算合成（倍音を並べて指数減衰させるだけ）。猫の鳴き声を**似せている**のではなく、
猫の**節回し**（周波数が上がって下がる）だけを音程のある音に載せている。生録音は部屋鳴りと
ノイズ床があって合図にならない — 通知音の条件は「0.5秒以内・音程がある・立ち上がりが鋭い」。

    python3 scripts/gen-chime.py

出力: assets/sounds/done.wav（完了・「にゃー」）/ waiting.wav（入力待ち・「にゃにゃっ」）
依存: numpy のみ（標準の wave モジュールで書き出す）。ビルドには不要・音を作り直すときだけ使う。
"""

from pathlib import Path
import wave

import numpy as np

RATE = 44100
OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "sounds"


def glide(points, duration, formants, harmonics=11, vibrato_hz=6.0,
          vibrato_depth=0.02, attack=0.12, close=0.35):
    """周波数を時間で動かした倍音列。鳴き声の「節」はここで作る。

    points: [(位置 0..1, Hz)] を線形補間した基本周波数の軌跡。上げてから下げると「にゃー」になる。
    formants: [(中心 Hz, 幅 Hz)] — 倍音をこの山で重み付けする。声道の共鳴の代わり。
    close: 終端の減衰にかける割合。大きいほど「ー」が長く尾を引く。
    """
    length = int(duration * RATE)
    time = np.arange(length) / RATE
    curve = np.interp(time / duration, [p for p, _ in points], [f for _, f in points])
    curve *= 1 + vibrato_depth * np.sin(2 * np.pi * vibrato_hz * time)
    tone = np.zeros(length)
    for harmonic in range(1, harmonics + 1):
        weight = np.zeros(length)
        for center, width in formants:
            weight += np.exp(-(((curve * harmonic) - center) / width) ** 2)
        # 周波数が動くので位相は積分して作る（sin(2πft) では不連続になる）
        phase = 2 * np.pi * np.cumsum(curve * harmonic) / RATE
        tone += (1 / harmonic) * weight * np.sin(phase)
    norm = time / duration
    return tone * np.clip(norm / attack, 0, 1) * np.clip((1 - norm) / close, 0, 1)


def overlay(parts, total):
    """(開始秒, 波形) を重ねる。"""
    track = np.zeros(int(total * RATE))
    for start, wave_data in parts:
        offset = int(start * RATE)
        end = min(offset + len(wave_data), len(track))
        track[offset:end] += wave_data[: end - offset]
    return track


def write(name, samples):
    """ピーク -3dBFS に揃えて 16bit モノラルで書き出す（通知音なので控えめに）。"""
    peak = np.max(np.abs(samples)) or 1.0
    data = (samples / peak * 0.707 * 32767).astype(np.int16)
    path = OUT_DIR / name
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(data.tobytes())
    print(f"{path}  {len(data) / RATE:.2f}s  {path.stat().st_size}B")


def main():
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    # 完了（ターンが終わった）: 一声の「にゃー」。フォルマント2本（780/1900Hz）で声に寄せる。
    # 上がって下がりきる＝話が終わった形。0.45s。
    write("done.wav", glide(
        [(0, 540), (0.25, 850), (0.55, 810), (1, 540)],
        0.45,
        formants=((780, 420), (1900, 900)),
    ))

    # 入力待ち（承認・質問で止まっている）: 短い「にゃにゃっ」。
    # 二回鳴らすと**呼びかけ**になる（完了の一声と耳で区別できる）。0.45s だが密度が違う。
    chirp = dict(formants=((1150, 950),), vibrato_depth=0.01)
    write("waiting.wav", overlay([
        (0.00, glide([(0, 640), (0.35, 1000), (1, 760)], 0.17, **chirp)),
        (0.20, glide([(0, 700), (0.35, 1080), (1, 700)], 0.20, **chirp) * 0.85),
    ], 0.45))


if __name__ == "__main__":
    main()
