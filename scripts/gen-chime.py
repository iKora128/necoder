#!/usr/bin/env python3
"""通知音（`assets/sounds/`）を生成する。

necoder の音は**録音ではなく合成**で作る。理由は2つ:

- 権利が完全に自前になる（AGPL 配布で説明が要らない・`THIRD_PARTY_NOTICES.md` が増えない）
- 後から調律できる。「高い」「長い」と思ったらこのファイルの数値を直して再生成すればいい

中身は**声道モデル付きの加算合成**。猫に聞こえるかどうかを決めるのは、初版が動かしていた
音程の軌跡ではなく次の4つだった（初版は全部外していて「犬のくぅーん」に聞こえた）:

1. **基本周波数が猫の帯にあること**（900-1300Hz 前後）。600Hz を切ると犬の鳴きに寄る
2. **「にゃ」の入りは F2 が高いところから落ちる**（[ɲ] = 口蓋音）。上げると「まおー」になる
3. **平らな持続を作らない**。滑らかに伸びる音は犬のくぅーんそのもの
4. **ざらつき**（不規則な振幅の揺れ）。サイン波の足し算だけではツルツルで生き物に聞こえない

    python3 scripts/gen-chime.py                # 同梱する音を書き出す
    python3 scripts/gen-chime.py --candidates   # 耳で選ぶための候補を /tmp へ書き出す

出力: `assets/sounds/<声>-<場面>.wav`
  声 = nya（「にゃっ」既定）/ nyaan（尾を引く「にゃー」）/ mew（子猫のチャープ「みゃっ」）
  場面 = done（ターン完了・一声）/ waiting（入力待ち・二声の呼びかけ）
依存: numpy のみ（標準の wave モジュールで書き出す）。ビルドには不要・音を作り直すときだけ使う。
"""

from pathlib import Path
import sys
import wave

import numpy as np

RATE = 44100
OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "sounds"

# 猫の声道は人のおよそ半分の長さなので、フォルマントは人の母音の倍あたりに来る。
# 各キーフレームは (位置 0..1, 基本周波数 Hz, (F1, F2, F3), 口の開き 0..1)。
# 「口の開き」は音量と**高い倍音の生き死に**の両方を動かす＝閉じると鼻にかかったこもった音になる。

# 「にゃっ」の素。閉じた鼻音 [ɲ]（F2 が高い）→ 開いた [あ] → 閉じ、と一往復する。
# 実際に鳴らすのはこれを 1.2 倍した NYA（倍率は耳で選んだ）。
MEOW = [
    (0.00, 780, (360, 2900, 3600), 0.15),
    (0.12, 1000, (1050, 2050, 3200), 1.00),
    (0.42, 1080, (980, 1850, 3100), 0.90),
    (0.72, 920, (700, 1450, 3000), 0.50),
    (1.00, 800, (480, 1150, 2900), 0.08),
]

# 子猫のチャープ「みゃっ」。口をあまり閉じない（鼻音で終わらない）ので語尾が軽い。
CHIRP = [
    (0.00, 980, (420, 3000, 3800), 0.35),
    (0.20, 1240, (1100, 2400, 3600), 1.00),
    (0.60, 1300, (1000, 2200, 3500), 0.90),
    (1.00, 1020, (700, 1700, 3300), 0.40),
]


def shift(frames, f0=1.0, formant=1.0, open_scale=1.0):
    """声のキーフレームを丸ごと移調する。上げるときは**フォルマントも一緒に上げる** —
    体が小さい（＝声道が短い）という辻褄を合わせないと「高い大人の猫」になって不自然になる。"""
    return [
        (pos, base * f0, tuple(f * formant for f in formants), min(1.0, opening * open_scale))
        for pos, base, formants, opening in frames
    ]


# 同梱する声。倍率は候補を8本ずつ2周聴いて選んだもの（`--candidates`・JOURNAL 2026-09-14）。
NYA = shift(MEOW, f0=1.20, formant=1.06)   # F0 940-1300Hz
MEW = shift(CHIRP, f0=1.20, formant=1.06)  # F0 1180-1560Hz

# 尺に対する立ち上がり・終いの比。短い音に長い終いを付けると余韻だけが残って
# 「にゃっ」の切れが消えるので、尺を変えるときはここも連動させる。
ATTACK_RATIO = 0.048
RELEASE_RATIO = 0.28
ROUGHNESS = 0.22


def track(frames, index, time, duration):
    """キーフレームの index 番目の値を時間軸へ線形補間する。"""
    positions = [pos for pos, *_ in frames]
    values = [
        frame[1] if index == "f0" else (frame[3] if index == "open" else frame[2][index])
        for frame in frames
    ]
    return np.interp(time / duration, positions, values)


def wobble(length, rng, hz, depth):
    """滑らかな乱れ（ゆらぎ）。生き物の声は周期的なビブラートだけだと機械に聞こえる。"""
    span = max(1, int(RATE / hz))
    noise = rng.standard_normal(length + span * 2)
    kernel = np.hanning(span * 2 + 1)
    smooth = np.convolve(noise, kernel / kernel.sum(), mode="same")[:length]
    peak = np.max(np.abs(smooth)) or 1.0
    return smooth / peak * depth


def voice(frames, duration, harmonics=16, widths=(280, 450, 700), rolloff=1.0,
          vibrato_hz=6.5, vibrato_depth=0.012, jitter=0.010, damping=0.5,
          attack=0.030, release=0.16, breath=0.012, roughness=0.0, subharmonic=0.0,
          seed=7):
    """キーフレームを鳴らす。声道（フォルマント）と口の開閉をここで波形にする。

    harmonics: 倍音の本数。rolloff: 声帯音源の傾き（1/h**rolloff・大きいほど柔らかい）。
    damping: 口を閉じたときに高い倍音を殺す強さ（鼻音化）。
    attack/release: 秒。roughness: 不規則な振幅の揺れ（声のざらつき）。
    subharmonic: 基本周波数の半分を足す量（猫の鳴きに出る subharmonic・太さとがらつきが出る）。
    """
    rng = np.random.default_rng(seed)
    length = int(duration * RATE)
    time = np.arange(length) / RATE

    base = track(frames, "f0", time, duration)
    base = base * (1 + vibrato_depth * np.sin(2 * np.pi * vibrato_hz * time))
    base = base * (1 + wobble(length, rng, 14.0, jitter))
    opening = np.clip(track(frames, "open", time, duration), 0.0, 1.0)
    formants = [track(frames, index, time, duration) for index in (0, 1, 2)]

    # 周波数が動くので位相は積分して作る（sin(2πft) では不連続になる）
    phase = 2 * np.pi * np.cumsum(base) / RATE
    tone = np.zeros(length)
    orders = list(range(1, harmonics + 1))
    if subharmonic:
        orders.insert(0, 0.5)
    for harmonic in orders:
        partial = base * harmonic
        weight = np.zeros(length)
        for center, width in zip(formants, widths):
            weight += np.exp(-(((partial - center) / width) ** 2))
        weight *= 1.0 / harmonic**rolloff
        # 口を閉じるほど上の倍音が消える（鼻に抜けた音）。1 倍音は残す。
        weight *= np.exp(-damping * (1.0 - opening) * max(harmonic - 1, 0))
        weight[partial > RATE * 0.45] = 0.0
        if harmonic < 1:
            weight *= subharmonic
        tone += weight * np.sin(harmonic * phase)

    # ざらつき。生き物の声はここが無いと「滑らかな持続音」＝犬の鳴きに寄る。
    if roughness:
        tone *= 1.0 + roughness * wobble(length, rng, 95.0, 1.0)

    envelope = 0.30 + 0.70 * opening
    envelope *= np.clip(time / attack, 0, 1)
    envelope *= np.clip((duration - time) / release, 0, 1) ** 1.5
    tone *= envelope
    # 息の成分。声と同じ包絡で混ぜると合成臭さが減る（多いと「シャー」になるので薄く）。
    if breath:
        air = wobble(length, rng, 2600.0, 1.0) * opening * envelope
        tone += air * breath * (np.max(np.abs(tone)) or 1.0)
    return tone


def call(frames, duration, seed=7, **overrides):
    """一声。立ち上がり・終いを尺に比例させる（ATTACK_RATIO / RELEASE_RATIO）。"""
    options = dict(
        roughness=ROUGHNESS,
        attack=duration * ATTACK_RATIO,
        release=duration * RELEASE_RATIO,
        seed=seed,
    )
    options.update(overrides)
    return voice(frames, duration, **options)


def overlay(parts, total):
    """(開始秒, 波形) を重ねる。"""
    track_out = np.zeros(int(total * RATE))
    for start, wave_data in parts:
        offset = int(start * RATE)
        end = min(offset + len(wave_data), len(track_out))
        track_out[offset:end] += wave_data[: end - offset]
    return track_out


def calling(frames, duration, gap=0.04, **overrides):
    """二声の呼びかけ（入力待ち）。一声の完了音と耳で区別するための形。

    二声目は種を変えて少しだけ小さくする — 同じ波形を2回貼ると機械の繰り返しに聞こえる。
    """
    second = duration + gap
    return overlay([
        (0.0, call(frames, duration, **overrides)),
        (second, call(frames, duration, seed=11, **overrides) * 0.9),
    ], second + duration)


def write(name, samples, directory=None):
    """ピーク -3dBFS に揃えて 16bit モノラルで書き出す（通知音なので控えめに）。"""
    peak = np.max(np.abs(samples)) or 1.0
    data = (samples / peak * 0.707 * 32767).astype(np.int16)
    path = (directory or OUT_DIR) / name
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(data.tobytes())
    print(f"{path.name}  {len(data) / RATE:.2f}s  {path.stat().st_size}B")
    return path


# ── 候補（`--candidates`）────────────────────────────────────────────────
# 耳で選ぶための試作。/tmp に書き出して聴き、選ばれた数値を上の定数へ昇格させる。
# 次に調律するときもここを書き換えて回すこと（机上で当てにいって2回外している）。
CANDIDATES = [
    (f"{index}-{round(duration * 1000):03d}ms", f"にゃっ・{round(duration * 1000)}ms",
     NYA, duration, {})
    for index, duration in enumerate([0.12, 0.15, 0.18, 0.21, 0.25, 0.30, 0.36, 0.45], start=1)
]


def write_candidates():
    """候補を /tmp へ書き出す（リポジトリには入れない）。"""
    out = Path("/tmp/necoder-cat-candidates")
    out.mkdir(parents=True, exist_ok=True)
    for name, note, frames, duration, options in CANDIDATES:
        write(f"{name}.wav", call(frames, duration, **options), directory=out)
        print(f"    {note}")
    print(f"\n聴く: for f in {out}/*.wav; do echo $f; afplay $f; sleep 0.5; done")


def main():
    if "--candidates" in sys.argv:
        write_candidates()
        return

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    # ── nya: 「にゃっ」（既定）────────────────────────────────────
    # 完了は 250ms の一声、入力待ちは 180ms を二声。どちらも耳で選んだ長さ。
    write("nya-done.wav", call(NYA, 0.25))
    write("nya-waiting.wav", calling(NYA, 0.18))

    # ── nyaan: 尾を引く「にゃー」──────────────────────────────────
    # 同じ声のまま尺だけ伸ばした版。呼ばれている感じが強い代わりに通知としては重い。
    write("nyaan-done.wav", call(NYA, 0.45))
    write("nyaan-waiting.wav", calling(NYA, 0.25))

    # ── mew: 子猫のチャープ「みゃっ」───────────────────────────────
    # 口を閉じずに終わるので語尾が軽い。倍音を減らして細い声にする。
    kitten = dict(harmonics=10)
    write("mew-done.wav", call(MEW, 0.20, **kitten))
    write("mew-waiting.wav", calling(MEW, 0.15, **kitten))


if __name__ == "__main__":
    main()
