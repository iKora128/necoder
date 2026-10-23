//! necoder 紹介ムービーのサウンドトラックを合成する。録音素材は使わない（猫の声も
//! `scripts/gen-chime.py` が合成した通知音）。
//!
//! 120 BPM・4/4・41 小節（82 秒）。1 拍 = 0.5 秒・1 小節 = 2 秒なので、60fps の動画では
//! 1 拍 = 30 フレームにそろう。場面の割り当ては `promo/src/timeline.ts` と同じ小節で切る:
//!
//! | 小節 | 場面 | 音 |
//! |---|---|---|
//! | 0–3 | 迷子（ウィンドウ 20 枚） | 持続音と秒針 |
//! | 4–7 | だから作った | ビルドアップ・ライザー・スネアロール |
//! | 8–23 | ロゴ → 色 → AI | ドロップ 1（IV–V–iii–vi の王道進行） |
//! | 24–27 | かゆいところ（プレビュー・Chat モード） | ブレイク |
//! | 28–35 | スマホ連携・SSH・Rust × GPUI | ドロップ 2（リードが主旋律） |
//! | 36–40 | 締め | Fmaj7 → G6 → Cmaj9 で明るく終わる |
//!
//! 使い方: `cargo run --release -- ja`（または `en`）。打鍵音などの効果音の時刻は
//! `promo/src/cues.json` から読むので、動画側の文字数が変わっても追従する。

mod dsp;
mod instruments;

use std::error::Error;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use dsp::{db_to_gain, limit, seconds_to_samples, Rng, StereoBuffer, Svf, SAMPLE_RATE};
use instruments::PadSettings;

const BAR: f32 = 2.0;
const STEP: f32 = 0.125;
const TOTAL_BARS: usize = 41;
const TOTAL_SECONDS: f32 = TOTAL_BARS as f32 * BAR;

#[derive(Deserialize)]
struct Cues {
    typing: Vec<TypingCue>,
    #[serde(default)]
    whoosh: Vec<f32>,
    #[serde(default)]
    blip: Vec<BlipCue>,
    #[serde(default)]
    error: Vec<f32>,
    nya: Option<f32>,
}

#[derive(Deserialize)]
struct TypingCue {
    start: f32,
    interval: f32,
    ja: String,
    en: String,
}

#[derive(Deserialize)]
struct BlipCue {
    time: f32,
    note: f32,
}

struct Chord {
    bass: f32,
    pad: &'static [f32],
    arp: &'static [f32],
}

const A_MINOR_ADD9: Chord = Chord { bass: 45.0, pad: &[57.0, 60.0, 64.0, 71.0], arp: &[69.0, 71.0, 72.0, 76.0, 81.0] };
const F_MAJOR7: Chord = Chord { bass: 41.0, pad: &[53.0, 57.0, 60.0, 64.0], arp: &[65.0, 69.0, 72.0, 76.0, 77.0] };
const G_SIX: Chord = Chord { bass: 43.0, pad: &[55.0, 59.0, 62.0, 64.0], arp: &[67.0, 71.0, 74.0, 76.0, 79.0] };
const E_MINOR7: Chord = Chord { bass: 40.0, pad: &[52.0, 55.0, 59.0, 62.0], arp: &[64.0, 67.0, 71.0, 74.0, 76.0] };
const E_SEVEN: Chord = Chord { bass: 40.0, pad: &[52.0, 56.0, 59.0, 62.0], arp: &[64.0, 68.0, 71.0, 74.0, 76.0] };
const A_MINOR7: Chord = Chord { bass: 45.0, pad: &[57.0, 60.0, 64.0, 67.0], arp: &[69.0, 72.0, 76.0, 79.0, 81.0] };
const D_MINOR7: Chord = Chord { bass: 38.0, pad: &[50.0, 53.0, 57.0, 60.0], arp: &[62.0, 65.0, 69.0, 72.0, 74.0] };
const G_SUS4: Chord = Chord { bass: 43.0, pad: &[55.0, 60.0, 62.0, 65.0], arp: &[67.0, 72.0, 74.0, 77.0, 79.0] };
const C_MAJOR9: Chord = Chord { bass: 36.0, pad: &[48.0, 55.0, 59.0, 62.0, 64.0], arp: &[60.0, 64.0, 67.0, 71.0, 74.0] };

/// ドロップの 4 小節ループ。奇数周目だけ iii を III7 に替えて、日本の歌ものっぽい泣きを足す。
fn drop_chord(bar_in_drop: usize) -> &'static Chord {
    let loop_index = bar_in_drop / 4;
    match bar_in_drop % 4 {
        0 => &F_MAJOR7,
        1 => &G_SIX,
        2 if loop_index % 2 == 1 => &E_SEVEN,
        2 => &E_MINOR7,
        _ => &A_MINOR7,
    }
}

fn chord_for_bar(bar: usize) -> &'static Chord {
    match bar {
        0..=3 => &A_MINOR_ADD9,
        4 => &F_MAJOR7,
        5 => &G_SIX,
        6 => &E_MINOR7,
        7 => &A_MINOR7,
        8..=23 => drop_chord(bar - 8),
        24 => &D_MINOR7,
        25 => &E_MINOR7,
        26 => &F_MAJOR7,
        27 => &G_SUS4,
        28..=35 => drop_chord(bar - 28),
        36 => &F_MAJOR7,
        37 => &G_SIX,
        _ => &C_MAJOR9,
    }
}

struct MelodyNote {
    step: usize,
    length: usize,
    note: f32,
}

const fn melody(step: usize, length: usize, note: f32) -> MelodyNote {
    MelodyNote { step, length, note }
}

/// 主旋律。3+3+2 のシンコペーションで和音を駆け上がる。A は iii、B は III7 の周に乗る。
const HOOK_A: [&[MelodyNote]; 4] = [
    &[melody(0, 3, 69.0), melody(3, 3, 72.0), melody(6, 4, 76.0), melody(10, 2, 74.0), melody(12, 4, 72.0)],
    &[melody(0, 3, 71.0), melody(3, 3, 74.0), melody(6, 4, 79.0), melody(10, 2, 76.0), melody(12, 4, 74.0)],
    &[melody(0, 3, 67.0), melody(3, 3, 71.0), melody(6, 4, 76.0), melody(10, 2, 74.0), melody(12, 4, 76.0)],
    &[melody(0, 3, 72.0), melody(3, 3, 76.0), melody(6, 8, 81.0), melody(14, 2, 79.0)],
];
const HOOK_B: [&[MelodyNote]; 4] = [
    &[melody(0, 3, 69.0), melody(3, 3, 72.0), melody(6, 4, 77.0), melody(10, 2, 76.0), melody(12, 4, 72.0)],
    &[melody(0, 3, 71.0), melody(3, 3, 74.0), melody(6, 4, 79.0), melody(10, 2, 81.0), melody(12, 4, 79.0)],
    &[melody(0, 3, 68.0), melody(3, 3, 71.0), melody(6, 4, 76.0), melody(10, 2, 74.0), melody(12, 4, 71.0)],
    &[melody(0, 3, 69.0), melody(3, 3, 72.0), melody(6, 4, 76.0), melody(10, 6, 81.0)],
];

fn hook_for_bar(bar_in_drop: usize) -> &'static [MelodyNote] {
    let hook = if (bar_in_drop / 4) % 2 == 1 { &HOOK_B } else { &HOOK_A };
    hook[bar_in_drop % 4]
}

/// 16 分のアルペジオ。和音の 5 音（根・3・5・7/9・上）を行き来する。
const ARP_PATTERN: [usize; 16] = [0, 2, 4, 1, 3, 4, 2, 4, 0, 2, 4, 1, 3, 2, 4, 3];

fn bar_time(bar: usize) -> f32 {
    bar as f32 * BAR
}

fn step_time(bar: usize, step: f32) -> f32 {
    bar_time(bar) + step * STEP
}

/// 指数的に動かす（フィルタの開閉は対数で聞こえるので）
fn exponential_ramp(time: f32, start_time: f32, end_time: f32, from: f32, to: f32) -> f32 {
    let progress = ((time - start_time) / (end_time - start_time)).clamp(0.0, 1.0);
    from * (to / from).powf(progress)
}

fn pad_cutoff(time: f32) -> f32 {
    let wobble = 1.0 + 0.12 * (std::f32::consts::TAU * 0.125 * time).sin();
    let base = match time {
        t if t < bar_time(4) => exponential_ramp(t, 0.0, bar_time(4), 420.0, 1_400.0),
        t if t < bar_time(8) => exponential_ramp(t, bar_time(4), bar_time(8), 1_400.0, 2_800.0),
        t if t < bar_time(24) => 2_900.0,
        t if t < bar_time(28) => exponential_ramp(t, bar_time(24), bar_time(28), 700.0, 4_200.0),
        t if t < bar_time(36) => 3_300.0,
        t => exponential_ramp(t, bar_time(36), bar_time(40), 3_000.0, 700.0),
    };
    base * wobble
}

struct Tracks {
    kick: StereoBuffer,
    clap: StereoBuffer,
    hats: StereoBuffer,
    cymbals: StereoBuffer,
    bass: StereoBuffer,
    pad: StereoBuffer,
    arp: StereoBuffer,
    bell: StereoBuffer,
    lead: StereoBuffer,
    effects: StereoBuffer,
    interface: StereoBuffer,
    drone: StereoBuffer,
}

impl Tracks {
    fn new(length: usize) -> Self {
        Self {
            kick: StereoBuffer::new(length),
            clap: StereoBuffer::new(length),
            hats: StereoBuffer::new(length),
            cymbals: StereoBuffer::new(length),
            bass: StereoBuffer::new(length),
            pad: StereoBuffer::new(length),
            arp: StereoBuffer::new(length),
            bell: StereoBuffer::new(length),
            lead: StereoBuffer::new(length),
            effects: StereoBuffer::new(length),
            interface: StereoBuffer::new(length),
            drone: StereoBuffer::new(length),
        }
    }
}

/// 楽曲本体（小節で決まるもの）を書き込み、サイドチェインに使うキックの時刻を返す。
fn compose(tracks: &mut Tracks, rng: &mut Rng) -> Vec<f32> {
    let mut kick_times = Vec::new();
    let mut play_kick = |tracks: &mut Tracks, time: f32, velocity: f32, rng: &mut Rng, kick_times: &mut Vec<f32>| {
        instruments::kick(&mut tracks.kick, time, velocity, rng);
        kick_times.push(time);
    };

    // ── 0–3: 迷子。低い持続音・秒針・曇った Am(add9) ──────────────────────
    instruments::drone(&mut tracks.drone, 0.0, bar_time(7), 33.0, 0.75, rng);
    let intro_pad = PadSettings { attack: 3.5, release: 1.2, velocity: 0.55, cutoff: &pad_cutoff };
    instruments::pad_chord(&mut tracks.pad, 0.0, bar_time(4), A_MINOR_ADD9.pad, &intro_pad, rng);
    for bar in 0..4 {
        for eighth in 0..8 {
            let accent = if eighth % 2 == 0 { 1.0 } else { 0.6 };
            let pan = if eighth % 2 == 0 { -0.35 } else { 0.35 };
            instruments::tick(&mut tracks.hats, step_time(bar, eighth as f32 * 2.0), 0.22 * accent, pan, rng);
        }
        // 小節頭の鈍い鼓動（サイドチェインには数えない）
        instruments::kick(&mut tracks.kick, bar_time(bar), 0.38, rng);
    }
    instruments::reverse_crash(&mut tracks.cymbals, bar_time(4), BAR, 0.5, rng);

    // ── 4–7: ビルドアップ ──────────────────────────────────────────────
    for bar in 4..8 {
        let chord = chord_for_bar(bar);
        let progress = (bar - 4) as f32 / 4.0;
        let is_last = bar == 7;
        let build_pad = PadSettings {
            attack: 0.12,
            release: if is_last { 0.08 } else { 0.6 },
            velocity: 0.62,
            cutoff: &pad_cutoff,
        };
        // 最後の小節は裏拍の 8 分を残して止め、ドロップ直前に無音をつくる
        let duration = if is_last { BAR - STEP * 2.0 } else { BAR };
        instruments::pad_chord(&mut tracks.pad, bar_time(bar), duration, chord.pad, &build_pad, rng);
        for step in [2.0, 6.0, 10.0, 14.0] {
            if is_last && step > 12.0 {
                continue;
            }
            instruments::bass_note(&mut tracks.bass, step_time(bar, step), 0.2, chord.bass, 0.75, 0.2 + progress * 0.5);
        }
        let kick_steps: &[f32] = if bar < 6 { &[0.0, 8.0] } else { &[0.0, 4.0, 8.0, 12.0] };
        for &step in kick_steps {
            play_kick(tracks, step_time(bar, step), 0.95, rng, &mut kick_times);
        }
        if bar < 6 {
            for step in [2.0, 6.0, 10.0, 14.0] {
                instruments::closed_hat(&mut tracks.hats, step_time(bar, step), 0.35, 0.2, rng);
            }
        } else {
            for step in 0..16 {
                if is_last && step >= 14 {
                    continue;
                }
                let ramp = 0.2 + 0.45 * ((bar - 6) as f32 * 16.0 + step as f32) / 32.0;
                let pan = if step % 2 == 0 { -0.25 } else { 0.25 };
                instruments::closed_hat(&mut tracks.hats, step_time(bar, step as f32), ramp, pan, rng);
            }
            // 暗いプラックで次の旋律を予告する
            for step in 0..16 {
                if is_last && step >= 14 {
                    continue;
                }
                let note = chord.arp[ARP_PATTERN[step]];
                let brightness = 0.22 + 0.3 * ((bar - 6) as f32 * 16.0 + step as f32) / 32.0;
                let pan = if step % 2 == 0 { -0.3 } else { 0.3 };
                instruments::pluck(&mut tracks.arp, step_time(bar, step as f32), note, 0.5, pan, brightness);
            }
        }
        if bar == 6 {
            instruments::clap(&mut tracks.clap, step_time(bar, 4.0), 0.8, rng);
            instruments::clap(&mut tracks.clap, step_time(bar, 12.0), 0.8, rng);
        }
    }
    snare_roll(tracks, 7, rng);
    instruments::noise_riser(&mut tracks.effects, bar_time(6), BAR * 2.0 - STEP * 2.0, 0.9, rng);

    // ── 8–23: ドロップ 1（ロゴ → 色 → AI）──────────────────────────────
    for bar in 8..24 {
        let bar_in_drop = bar - 8;
        drop_groove(tracks, bar, bar_in_drop, false, rng, &mut kick_times, &mut play_kick);
        // 後半 8 小節（AI の場面）はガラスのベルで主旋律を先に聞かせる
        if bar >= 16 {
            for note in hook_for_bar(bar_in_drop) {
                let pan = if note.step % 2 == 0 { -0.15 } else { 0.15 };
                instruments::bell(
                    &mut tracks.bell,
                    step_time(bar, note.step as f32),
                    note.length as f32 * STEP,
                    note.note + 12.0,
                    0.42,
                    pan,
                );
            }
        }
    }
    impact_hit(tracks, bar_time(8), 1.0, rng);
    instruments::crash(&mut tracks.cymbals, bar_time(16), 0.8, rng);
    snare_fill(tracks, 15, 12, rng);
    snare_fill(tracks, 23, 8, rng);
    instruments::reverse_crash(&mut tracks.cymbals, bar_time(24), BAR, 0.45, rng);

    // ── 24–27: ブレイク（かゆいところ）─────────────────────────────────────
    instruments::downlifter(&mut tracks.effects, bar_time(24), 2.4, 0.55, rng);
    instruments::impact(&mut tracks.effects, bar_time(24), 0.45, rng);
    for bar in 24..28 {
        let chord = chord_for_bar(bar);
        let is_last = bar == 27;
        let break_pad = PadSettings {
            attack: 0.35,
            release: if is_last { 0.08 } else { 0.9 },
            velocity: 0.72,
            cutoff: &pad_cutoff,
        };
        let duration = if is_last { BAR - STEP * 2.0 } else { BAR };
        instruments::pad_chord(&mut tracks.pad, bar_time(bar), duration, chord.pad, &break_pad, rng);
        instruments::sub_note(&mut tracks.bass, bar_time(bar), duration - 0.05, chord.bass, 0.55);
        let progress = (bar - 24) as f32 / 4.0;
        let steps: Vec<usize> = if bar < 26 { (0..16).step_by(2).collect() } else { (0..16).collect() };
        for step in steps {
            if is_last && step >= 14 {
                continue;
            }
            let note = chord.arp[ARP_PATTERN[step]];
            let pan = if step % 2 == 0 { -0.3 } else { 0.3 };
            instruments::pluck(&mut tracks.arp, step_time(bar, step as f32), note, 0.5, pan, 0.25 + progress * 0.6);
        }
        if bar >= 26 {
            for step in 0..16 {
                if is_last && step >= 14 {
                    continue;
                }
                let every = if bar == 26 { 2 } else { 1 };
                if step % every != 0 {
                    continue;
                }
                let ramp = 0.18 + 0.4 * ((bar - 26) as f32 * 16.0 + step as f32) / 32.0;
                instruments::closed_hat(&mut tracks.hats, step_time(bar, step as f32), ramp, 0.2, rng);
            }
        }
    }
    snare_roll(tracks, 27, rng);
    instruments::noise_riser(&mut tracks.effects, bar_time(26), BAR * 2.0 - STEP * 2.0, 1.0, rng);

    // ── 28–35: ドロップ 2（スマホ・SSH・Rust × GPUI）─────────────────────────────────
    for bar in 28..36 {
        let bar_in_drop = bar - 28;
        drop_groove(tracks, bar, bar_in_drop, true, rng, &mut kick_times, &mut play_kick);
        for note in hook_for_bar(bar_in_drop) {
            instruments::lead_note(
                &mut tracks.lead,
                step_time(bar, note.step as f32),
                note.length as f32 * STEP * 0.92,
                note.note,
                0.62,
            );
            if bar >= 32 {
                instruments::bell(
                    &mut tracks.bell,
                    step_time(bar, note.step as f32),
                    note.length as f32 * STEP,
                    note.note + 12.0,
                    0.2,
                    0.0,
                );
            }
        }
    }
    impact_hit(tracks, bar_time(28), 1.0, rng);
    instruments::crash(&mut tracks.cymbals, bar_time(32), 0.75, rng);
    snare_fill(tracks, 35, 12, rng);
    instruments::reverse_crash(&mut tracks.cymbals, bar_time(36), BAR, 0.5, rng);

    // ── 36–40: 締め ───────────────────────────────────────────────────
    impact_hit(tracks, bar_time(36), 0.8, rng);
    play_kick(tracks, bar_time(36), 1.0, rng, &mut kick_times);
    let outro_pad = |release: f32| PadSettings { attack: 0.05, release, velocity: 0.7, cutoff: &pad_cutoff };
    instruments::pad_chord(&mut tracks.pad, bar_time(36), BAR, F_MAJOR7.pad, &outro_pad(0.8), rng);
    instruments::pad_chord(&mut tracks.pad, bar_time(37), BAR, G_SIX.pad, &outro_pad(0.8), rng);
    instruments::pad_chord(&mut tracks.pad, bar_time(38), BAR * 2.0, C_MAJOR9.pad, &outro_pad(3.0), rng);
    instruments::sub_note(&mut tracks.bass, bar_time(36), BAR - 0.05, F_MAJOR7.bass, 0.55);
    instruments::sub_note(&mut tracks.bass, bar_time(37), BAR - 0.05, G_SIX.bass, 0.55);
    instruments::sub_note(&mut tracks.bass, bar_time(38), BAR * 1.6, C_MAJOR9.bass, 0.55);
    for (bar, chord) in [(36, &F_MAJOR7), (37, &G_SIX)] {
        for eighth in 0..8 {
            let fade = 1.0 - ((bar - 36) as f32 * 8.0 + eighth as f32) / 20.0;
            let note = chord.arp[ARP_PATTERN[eighth * 2]];
            let pan = if eighth % 2 == 0 { -0.3 } else { 0.3 };
            instruments::pluck(&mut tracks.arp, step_time(bar, eighth as f32 * 2.0), note, 0.5 * fade, pan, 0.55);
        }
    }
    // 最後は Cmaj9 を駆け上がって鐘で止める
    for (index, &note) in [60.0, 64.0, 67.0, 71.0, 74.0, 76.0].iter().enumerate() {
        instruments::pluck(&mut tracks.arp, step_time(38, index as f32), note + 12.0, 0.42, if index % 2 == 0 { -0.3 } else { 0.3 }, 0.6);
    }
    instruments::bell(&mut tracks.bell, step_time(38, 6.0), 2.5, 88.0, 0.35, -0.1);
    instruments::bell(&mut tracks.bell, step_time(38, 6.0), 2.5, 83.0, 0.25, 0.1);

    kick_times
}

/// ドロップの 1 小節ぶんのリズム隊・ベース・パッド・アルペジオ
fn drop_groove(
    tracks: &mut Tracks,
    bar: usize,
    bar_in_drop: usize,
    second_drop: bool,
    rng: &mut Rng,
    kick_times: &mut Vec<f32>,
    play_kick: &mut impl FnMut(&mut Tracks, f32, f32, &mut Rng, &mut Vec<f32>),
) {
    let chord = drop_chord(bar_in_drop);
    let is_fill_bar = bar == 15 || bar == 23 || bar == 35;
    let fill_from = if bar == 23 { 8.0 } else { 12.0 };

    for step in [0.0, 4.0, 8.0, 12.0] {
        if is_fill_bar && step >= fill_from {
            continue;
        }
        play_kick(tracks, step_time(bar, step), 1.0, rng, kick_times);
    }
    for step in [4.0, 12.0] {
        if is_fill_bar && step >= fill_from {
            continue;
        }
        instruments::clap(&mut tracks.clap, step_time(bar, step), 0.85, rng);
    }
    for step in 0..16 {
        let time = step_time(bar, step as f32);
        if step % 4 == 2 {
            instruments::open_hat(&mut tracks.hats, time, 0.42, 0.15, rng);
        } else if step % 2 == 1 {
            let accent = if step % 4 == 3 { 0.3 } else { 0.22 };
            instruments::closed_hat(&mut tracks.hats, time, accent, -0.2, rng);
        }
        if second_drop {
            let velocity = if step % 2 == 0 { 0.2 } else { 0.12 };
            instruments::shaker(&mut tracks.hats, time, velocity, 0.45, rng);
        }
    }

    if second_drop {
        // 転がる 16 分のベース（キックの位置は空ける）
        for step in 0..16 {
            if step % 4 == 0 {
                continue;
            }
            let (note, velocity) = match step % 4 {
                2 => (chord.bass, 0.9),
                3 => (chord.bass + 12.0, 0.55),
                _ => (chord.bass, 0.6),
            };
            instruments::bass_note(&mut tracks.bass, step_time(bar, step as f32), 0.105, note, velocity, 0.72);
        }
    } else {
        for step in [2.0, 6.0, 10.0, 14.0] {
            instruments::bass_note(&mut tracks.bass, step_time(bar, step), 0.22, chord.bass, 0.88, 0.62);
        }
        for step in [3.0, 11.0] {
            instruments::bass_note(&mut tracks.bass, step_time(bar, step), 0.09, chord.bass + 12.0, 0.45, 0.62);
        }
    }

    let pad = PadSettings { attack: 0.02, release: 0.35, velocity: 0.7, cutoff: &pad_cutoff };
    instruments::pad_chord(&mut tracks.pad, bar_time(bar), BAR, chord.pad, &pad, rng);

    let brightness = if second_drop { 0.82 } else if bar >= 16 { 0.72 } else { 0.6 };
    for step in 0..16 {
        let note = chord.arp[ARP_PATTERN[step]];
        let pan = if step % 2 == 0 { -0.32 } else { 0.32 };
        let velocity = if step % 4 == 0 { 0.58 } else { 0.46 };
        instruments::pluck(&mut tracks.arp, step_time(bar, step as f32), note, velocity, pan, brightness);
    }
}

/// 4 拍目に向けて密度が上がるスネアロール。最後の 8 分は空ける。
fn snare_roll(tracks: &mut Tracks, bar: usize, rng: &mut Rng) {
    let mut positions = Vec::new();
    positions.extend([0.0, 2.0, 4.0, 6.0]);
    positions.extend([8.0, 9.0, 10.0, 11.0]);
    positions.extend([12.0, 12.5, 13.0, 13.5]);
    let count = positions.len() as f32;
    for (index, &step) in positions.iter().enumerate() {
        let progress = index as f32 / (count - 1.0);
        let velocity = 0.3 + 0.65 * progress;
        let pitch = 185.0 + 110.0 * progress;
        instruments::snare(&mut tracks.clap, step_time(bar, step), velocity, pitch, rng);
    }
}

/// 小節末のスネアのフィル（`from_step` から 16 分で打つ）
fn snare_fill(tracks: &mut Tracks, bar: usize, from_step: usize, rng: &mut Rng) {
    for step in from_step..16 {
        let progress = (step - from_step) as f32 / (16 - from_step) as f32;
        instruments::snare(&mut tracks.clap, step_time(bar, step as f32), 0.45 + 0.5 * progress, 190.0 + 60.0 * progress, rng);
    }
}

fn impact_hit(tracks: &mut Tracks, time: f32, velocity: f32, rng: &mut Rng) {
    instruments::impact(&mut tracks.effects, time, velocity, rng);
    instruments::crash(&mut tracks.cymbals, time, 0.95 * velocity, rng);
}

/// 動画の出来事に合わせる効果音（打鍵・風切り・UI 音・猫）
fn place_cues(tracks: &mut Tracks, cues: &Cues, language: &str, nya: &[f32], rng: &mut Rng) {
    for typing in &cues.typing {
        let text = if language == "en" { &typing.en } else { &typing.ja };
        for (index, character) in text.chars().enumerate() {
            let velocity = if character == ' ' { 0.55 } else { rng.range(0.7, 1.0) };
            let jitter = rng.range(-0.006, 0.006);
            instruments::key_click(&mut tracks.interface, typing.start + index as f32 * typing.interval + jitter, velocity, rng);
        }
    }
    for (index, &time) in cues.whoosh.iter().enumerate() {
        let direction = if index % 2 == 0 { 1.0 } else { -1.0 };
        instruments::whoosh(&mut tracks.interface, time - 0.35, 0.55, 0.55, direction, rng);
    }
    for (index, cue) in cues.blip.iter().enumerate() {
        let pan = if index % 2 == 0 { -0.2 } else { 0.2 };
        instruments::blip(&mut tracks.interface, cue.time, cue.note, 0.45, pan);
    }
    for &time in &cues.error {
        instruments::error_buzz(&mut tracks.interface, time, 0.5);
    }
    if let Some(time) = cues.nya {
        instruments::place_sample(&mut tracks.interface, time, nya, 0.9, 0.0);
    }
}

/// キックの頭で沈み、0.22 秒かけて戻る曲線（1.0 = 最も沈んだ瞬間）
fn sidechain_curve(length: usize, kick_times: &[f32]) -> Vec<f32> {
    let mut curve = vec![0.0f32; length];
    let release = seconds_to_samples(0.22);
    let attack = seconds_to_samples(0.003).max(1);
    for &time in kick_times {
        let start = seconds_to_samples(time);
        for offset in 0..release {
            let index = start + offset;
            if index >= length {
                break;
            }
            let shape = if offset < attack {
                offset as f32 / attack as f32
            } else {
                let progress = offset as f32 / release as f32;
                (1.0 - progress) * (1.0 - progress)
            };
            curve[index] = curve[index].max(shape);
        }
    }
    curve
}

fn ducking(curve: &[f32], depth: f32) -> Vec<f32> {
    curve.iter().map(|amount| 1.0 - depth * amount).collect()
}

struct Stem<'a> {
    buffer: &'a StereoBuffer,
    gain: f32,
    /// 低域の濁りを避けるためのローカット（Hz・0 で素通し）
    highpass: f32,
    reverb_send: f32,
    delay_send: f32,
    duck_depth: f32,
}

fn highpassed(buffer: &StereoBuffer, cutoff: f32) -> StereoBuffer {
    let mut output = buffer.clone();
    let mut left = Svf::default();
    let mut right = Svf::default();
    for index in 0..output.len() {
        output.left[index] = left.process(output.left[index], cutoff, 0.7).high;
        output.right[index] = right.process(output.right[index], cutoff, 0.7).high;
    }
    output
}

fn mix(tracks: &Tracks, kick_times: &[f32], master_gain_db: f32) -> StereoBuffer {
    let length = tracks.kick.len();
    let curve = sidechain_curve(length, kick_times);
    let stems = [
        Stem { buffer: &tracks.kick, gain: 0.72, highpass: 0.0, reverb_send: 0.0, delay_send: 0.0, duck_depth: 0.0 },
        Stem { buffer: &tracks.clap, gain: 0.55, highpass: 160.0, reverb_send: 0.22, delay_send: 0.0, duck_depth: 0.0 },
        Stem { buffer: &tracks.hats, gain: 0.38, highpass: 0.0, reverb_send: 0.05, delay_send: 0.0, duck_depth: 0.1 },
        Stem { buffer: &tracks.cymbals, gain: 0.26, highpass: 0.0, reverb_send: 0.18, delay_send: 0.0, duck_depth: 0.0 },
        Stem { buffer: &tracks.bass, gain: 0.34, highpass: 0.0, reverb_send: 0.0, delay_send: 0.0, duck_depth: 0.8 },
        Stem { buffer: &tracks.pad, gain: 0.42, highpass: 230.0, reverb_send: 0.35, delay_send: 0.0, duck_depth: 0.55 },
        Stem { buffer: &tracks.arp, gain: 0.3, highpass: 260.0, reverb_send: 0.22, delay_send: 0.28, duck_depth: 0.3 },
        Stem { buffer: &tracks.bell, gain: 0.3, highpass: 300.0, reverb_send: 0.35, delay_send: 0.3, duck_depth: 0.15 },
        Stem { buffer: &tracks.lead, gain: 0.34, highpass: 160.0, reverb_send: 0.3, delay_send: 0.22, duck_depth: 0.25 },
        Stem { buffer: &tracks.effects, gain: 0.45, highpass: 28.0, reverb_send: 0.45, delay_send: 0.0, duck_depth: 0.0 },
        Stem { buffer: &tracks.interface, gain: 0.45, highpass: 120.0, reverb_send: 0.12, delay_send: 0.04, duck_depth: 0.0 },
        Stem { buffer: &tracks.drone, gain: 0.2, highpass: 40.0, reverb_send: 0.3, delay_send: 0.0, duck_depth: 0.0 },
    ];

    let mut master = StereoBuffer::new(length);
    let mut reverb_bus = StereoBuffer::new(length);
    let mut delay_bus = StereoBuffer::new(length);
    for stem in &stems {
        let mut processed = if stem.highpass > 0.0 {
            highpassed(stem.buffer, stem.highpass)
        } else {
            stem.buffer.clone()
        };
        if stem.duck_depth > 0.0 {
            processed.apply_gain_curve(&ducking(&curve, stem.duck_depth));
        }
        master.mix_from(&processed, stem.gain);
        if stem.reverb_send > 0.0 {
            reverb_bus.mix_from(&processed, stem.gain * stem.reverb_send);
        }
        if stem.delay_send > 0.0 {
            delay_bus.mix_from(&processed, stem.gain * stem.delay_send);
        }
    }

    // 付点 8 分のピンポンディレイ（120 BPM で 0.375 秒）。返りの一部は残響へも送る
    let mut delay = dsp::PingPongDelay::new(0.375, 0.38);
    let mut delay_return = StereoBuffer::new(length);
    for index in 0..length {
        let (left, right) = delay.process(delay_bus.left[index], delay_bus.right[index]);
        delay_return.left[index] = left;
        delay_return.right[index] = right;
    }
    delay_return.apply_gain_curve(&ducking(&curve, 0.2));
    reverb_bus.mix_from(&delay_return, 0.25);
    master.mix_from(&delay_return, 0.45);

    let mut reverb = dsp::Reverb::new(0.86, 0.45, 0.025);
    let mut reverb_return = StereoBuffer::new(length);
    for index in 0..length {
        let (left, right) = reverb.process(reverb_bus.left[index], reverb_bus.right[index]);
        reverb_return.left[index] = left;
        reverb_return.right[index] = right;
    }
    reverb_return.apply_gain_curve(&ducking(&curve, 0.25));
    master.mix_from(&reverb_return, 0.6);

    // 超低域の揺れ（DC とサブソニック）を落とし、3kHz より上を少し持ち上げて輪郭を出してから、
    // 音圧を上げて天井で止める
    let mut highpass_left = Svf::default();
    let mut highpass_right = Svf::default();
    let mut presence_left = dsp::OnePole::default();
    let mut presence_right = dsp::OnePole::default();
    let presence = db_to_gain(3.0) - 1.0;
    let gain = db_to_gain(master_gain_db);
    for index in 0..length {
        let left = highpass_left.process(master.left[index], 24.0, 0.7).high;
        let right = highpass_right.process(master.right[index], 24.0, 0.7).high;
        master.left[index] = (left + presence_left.highpass(left, 3_000.0) * presence) * gain;
        master.right[index] = (right + presence_right.highpass(right, 3_000.0) * presence) * gain;
    }
    limit(&mut master, db_to_gain(-1.2), 0.004, 0.09);

    // 頭 10ms のフェードインと、最後 2 秒のフェードアウト
    let fade_in = seconds_to_samples(0.01);
    let fade_out_start = seconds_to_samples(TOTAL_SECONDS - 2.0);
    for index in 0..length {
        let mut envelope = 1.0;
        if index < fade_in {
            envelope *= index as f32 / fade_in as f32;
        }
        if index >= fade_out_start {
            let progress = (index - fade_out_start) as f32 / (length - fade_out_start) as f32;
            envelope *= (1.0 - progress).max(0.0).powf(1.6);
        }
        master.left[index] *= envelope;
        master.right[index] *= envelope;
    }
    master
}

/// 16bit PCM の WAV（TPDF ディザ付き）
fn write_wav(path: &Path, buffer: &StereoBuffer, rng: &mut Rng) -> std::io::Result<()> {
    let length = buffer.len();
    let data_size = (length * 2 * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE as u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for index in 0..length {
        for sample in [buffer.left[index], buffer.right[index]] {
            let dither = rng.unit() - rng.unit();
            let value = (sample * 32_767.0 + dither).round().clamp(-32_768.0, 32_767.0) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(path, bytes)
}

/// 16bit モノラルの WAV を読み、48kHz へ線形補間で直す（猫の声用・短いので十分）
fn read_mono_wav(path: &Path) -> Result<Vec<f32>, Box<dyn Error>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("WAV ではない: {}", path.display()).into());
    }
    let mut position = 12;
    let mut source_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut samples = Vec::new();
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = u32::from_le_bytes([bytes[position + 4], bytes[position + 5], bytes[position + 6], bytes[position + 7]]) as usize;
        let body_start = position + 8;
        let body_end = (body_start + size).min(bytes.len());
        if id == b"fmt " && body_end - body_start >= 16 {
            channels = u16::from_le_bytes([bytes[body_start + 2], bytes[body_start + 3]]);
            source_rate = u32::from_le_bytes([bytes[body_start + 4], bytes[body_start + 5], bytes[body_start + 6], bytes[body_start + 7]]);
            bits = u16::from_le_bytes([bytes[body_start + 14], bytes[body_start + 15]]);
        } else if id == b"data" {
            if bits != 16 || channels != 1 {
                return Err(format!("16bit モノラルのみ対応: {} ({}ch {}bit)", path.display(), channels, bits).into());
            }
            samples = bytes[body_start..body_end]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0)
                .collect();
        }
        position = body_start + size + (size % 2);
    }
    if samples.is_empty() || source_rate == 0 {
        return Err(format!("音声データが無い: {}", path.display()).into());
    }
    let ratio = source_rate as f32 / SAMPLE_RATE;
    let output_length = (samples.len() as f32 / ratio) as usize;
    Ok((0..output_length)
        .map(|index| {
            let source = index as f32 * ratio;
            let lower = source.floor() as usize;
            let fraction = source - lower as f32;
            let first = samples.get(lower).copied().unwrap_or(0.0);
            let second = samples.get(lower + 1).copied().unwrap_or(first);
            first + (second - first) * fraction
        })
        .collect())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let language = arguments.next().unwrap_or_else(|| "ja".to_string());
    let master_gain_db: f32 = match arguments.next() {
        Some(value) => value.parse()?,
        None => -4.2,
    };

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let promo = manifest
        .parent()
        .ok_or("promo ディレクトリが見つからない")?
        .to_path_buf();
    let repository = promo.parent().ok_or("リポジトリのルートが見つからない")?.to_path_buf();
    let cues: Cues = serde_json::from_str(&std::fs::read_to_string(promo.join("src/cues.json"))?)?;
    let nya = read_mono_wav(&repository.join("assets/sounds/nya-done.wav"))?;

    let length = seconds_to_samples(TOTAL_SECONDS);
    let mut tracks = Tracks::new(length);
    let mut rng = Rng::new(0x6e65_636f_6465_72);
    let kick_times = compose(&mut tracks, &mut rng);
    place_cues(&mut tracks, &cues, &language, &nya, &mut rng);
    let master = mix(&tracks, &kick_times, master_gain_db);

    let output = promo.join(format!("public/soundtrack-{language}.wav"));
    write_wav(&output, &master, &mut rng)?;
    println!(
        "{} ({:.1} 秒・ピーク {:.2} dBFS)",
        output.display(),
        TOTAL_SECONDS,
        20.0 * master.peak().max(1e-9).log10()
    );
    Ok(())
}
