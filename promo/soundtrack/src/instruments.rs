//! 楽器と効果音。どれも「曲のどの時刻に、どれだけの強さで」を受け取ってトラックへ足し込む。
//!
//! 動画はノート PC やスマホのスピーカーで見られることが多いので、低音は倍音で聞こえるように作る
//! （キックは歪ませ、ベースはサインにノコギリ波を重ねる）。

use std::f32::consts::{PI, TAU};

use crate::dsp::{
    cents_to_ratio, midi_to_frequency, pan_gains, seconds_to_samples, OnePole, Oscillator, Rng,
    StereoBuffer, Svf, SAMPLE_RATE,
};

fn seconds(offset: usize) -> f32 {
    offset as f32 / SAMPLE_RATE
}

/// 0→1 を滑らかにつなぐ（アタックの角でプチッと鳴らさない）
fn smoothstep(value: f32) -> f32 {
    let clamped = value.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

/// バスドラム: 急降下するサイン + アタックのクリック + 歪みで倍音を足す
pub fn kick(track: &mut StereoBuffer, time: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.55);
    let mut phase = 0.0f32;
    let mut click_filter = OnePole::default();
    for offset in 0..length {
        let t = seconds(offset);
        let frequency = 50.0 + 165.0 * (-t / 0.026).exp() + 30.0 * (-t / 0.10).exp();
        phase += frequency / SAMPLE_RATE;
        let body = (TAU * phase).sin();
        let amplitude = (t / 0.0012).min(1.0) * (-t / 0.22).exp();
        let click = click_filter.highpass(rng.bipolar(), 2_000.0) * (-t / 0.0022).exp() * 0.30;
        let fade = if t > 0.48 { (1.0 - (t - 0.48) / 0.07).max(0.0) } else { 1.0 };
        let sample = ((body * amplitude * 2.2).tanh() * 0.9 + click) * velocity * fade;
        track.add(start + offset, sample, sample);
    }
}

/// スネアの胴鳴り + 響き線。ロールでは音程を上げながら使う。
pub fn snare(track: &mut StereoBuffer, time: f32, velocity: f32, pitch: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.28);
    let mut body = Oscillator::new(0.0);
    let mut noise_left = Svf::default();
    let mut noise_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let tone = body.sine(pitch * (1.0 + 0.35 * (-t / 0.012).exp())) * (-t / 0.065).exp() * 0.55;
        let attack = (t / 0.0008).min(1.0);
        let envelope = (-t / 0.11).exp() * attack;
        let left = noise_left.process(rng.bipolar(), 2_600.0, 0.7).high * envelope * 0.5;
        let right = noise_right.process(rng.bipolar(), 2_600.0, 0.7).high * envelope * 0.5;
        track.add(
            start + offset,
            (tone * attack + left) * velocity,
            (tone * attack + right) * velocity,
        );
    }
}

/// クラップ: 数 ms ずらしたノイズの束 + 短い残り + スネアの胴を薄く
pub fn clap(track: &mut StereoBuffer, time: f32, velocity: f32, rng: &mut Rng) {
    const BURSTS: [f32; 4] = [0.0, 0.010, 0.021, 0.031];
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.42);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let mut envelope = 0.0;
        for (burst_index, &burst_time) in BURSTS.iter().enumerate() {
            if t >= burst_time {
                let local = t - burst_time;
                envelope += if burst_index == BURSTS.len() - 1 {
                    (-local / 0.12).exp()
                } else {
                    (-local / 0.006).exp() * 0.85
                };
            }
        }
        let shared = rng.bipolar();
        let left = filter_left.process(shared * 0.5 + rng.bipolar() * 0.5, 1_200.0, 1.1).band;
        let right = filter_right.process(shared * 0.5 + rng.bipolar() * 0.5, 1_380.0, 1.1).band;
        track.add(
            start + offset,
            left * envelope * velocity * 0.95,
            right * envelope * velocity * 0.95,
        );
    }
    snare(track, time, velocity * 0.45, 195.0, rng);
}

/// ハイハット: 808 と同じ「6 本の矩形波 + ノイズ」を高域だけ残す
pub fn hat(track: &mut StereoBuffer, time: f32, velocity: f32, decay: f32, pan: f32, rng: &mut Rng) {
    const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(decay * 7.0 + 0.01);
    let mut oscillators: Vec<Oscillator> = METAL.iter().map(|_| Oscillator::new(rng.unit())).collect();
    let mut highpass = Svf::default();
    let mut sparkle = OnePole::default();
    for offset in 0..length {
        let t = seconds(offset);
        let mut metal = 0.0;
        for (oscillator, &frequency) in oscillators.iter_mut().zip(METAL.iter()) {
            metal += oscillator.square(frequency * 1.9);
        }
        let raw = metal / METAL.len() as f32 * 0.55 + rng.bipolar() * 0.6;
        let filtered = highpass.process(raw, 7_800.0, 0.8).high;
        let smoothed = sparkle.lowpass(filtered, 15_000.0);
        let envelope = (t / 0.0006).min(1.0) * (-t / decay).exp();
        track.add_panned(start + offset, smoothed * envelope * velocity, pan);
    }
}

pub fn closed_hat(track: &mut StereoBuffer, time: f32, velocity: f32, pan: f32, rng: &mut Rng) {
    hat(track, time, velocity, 0.032, pan, rng);
}

pub fn open_hat(track: &mut StereoBuffer, time: f32, velocity: f32, pan: f32, rng: &mut Rng) {
    hat(track, time, velocity * 0.8, 0.16, pan, rng);
}

/// シェイカー: 帯域ノイズ・立ち上がりを少し遅らせて粒っぽく
pub fn shaker(track: &mut StereoBuffer, time: f32, velocity: f32, pan: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.1);
    let mut filter = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let envelope = smoothstep(t / 0.008) * (-t / 0.028).exp();
        let sample = filter.process(rng.bipolar(), 7_200.0, 1.3).band * envelope * velocity;
        track.add_panned(start + offset, sample, pan);
    }
}

fn crash_samples(duration: f32, velocity: f32, rng: &mut Rng) -> (Vec<f32>, Vec<f32>) {
    const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
    let length = seconds_to_samples(duration);
    let mut left = vec![0.0; length];
    let mut right = vec![0.0; length];
    let mut oscillators: Vec<Oscillator> = METAL.iter().map(|_| Oscillator::new(rng.unit())).collect();
    let mut highpass_left = Svf::default();
    let mut highpass_right = Svf::default();
    let mut lowpass_left = OnePole::default();
    let mut lowpass_right = OnePole::default();
    for offset in 0..length {
        let t = seconds(offset);
        let mut metal = 0.0;
        for (oscillator, &frequency) in oscillators.iter_mut().zip(METAL.iter()) {
            metal += oscillator.square(frequency * 3.1);
        }
        let metal = metal / METAL.len() as f32 * 0.35;
        let envelope = (t / 0.002).min(1.0) * (-t / 0.85).exp() * velocity;
        let raw_left = highpass_left.process(metal + rng.bipolar() * 0.7, 4_800.0, 0.7).high;
        let raw_right = highpass_right.process(metal + rng.bipolar() * 0.7, 4_800.0, 0.7).high;
        left[offset] = lowpass_left.lowpass(raw_left, 11_000.0) * envelope;
        right[offset] = lowpass_right.lowpass(raw_right, 11_000.0) * envelope;
    }
    (left, right)
}

pub fn crash(track: &mut StereoBuffer, time: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let (left, right) = crash_samples(3.0, velocity, rng);
    for offset in 0..left.len() {
        track.add(start + offset, left[offset], right[offset]);
    }
}

/// 逆再生のシンバル。`end_time` でちょうど頂点に達する。
pub fn reverse_crash(track: &mut StereoBuffer, end_time: f32, duration: f32, velocity: f32, rng: &mut Rng) {
    let (left, right) = crash_samples(duration, velocity, rng);
    let end = seconds_to_samples(end_time);
    let length = left.len();
    let start = end.saturating_sub(length);
    for offset in 0..length {
        // 頂点の直前 6ms で切り落とす（頭のアタックをそのまま出すとクリックになる）
        let reverse_index = length - 1 - offset;
        let fade = (reverse_index as f32 / seconds_to_samples(0.006) as f32).min(1.0);
        track.add(start + offset, left[reverse_index] * fade, right[reverse_index] * fade);
    }
}

/// ベース: サイン（重さ）+ フィルタ付きノコギリ（スピーカーで聞こえる輪郭）
pub fn bass_note(track: &mut StereoBuffer, time: f32, duration: f32, note: f32, velocity: f32, brightness: f32) {
    let start = seconds_to_samples(time);
    let release = 0.035;
    let length = seconds_to_samples(duration + release);
    let frequency = midi_to_frequency(note);
    let mut saw = Oscillator::new(0.0);
    let mut sub = Oscillator::new(0.0);
    let mut filter = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let cutoff = 170.0 + brightness * (300.0 + 2_000.0 * (-t / 0.085).exp());
        let growl = filter.process(saw.saw(frequency), cutoff, 1.25).low;
        let body = sub.sine(frequency);
        let attack = smoothstep(t / 0.004);
        let gate = if t < duration { 1.0 } else { (1.0 - (t - duration) / release).max(0.0) };
        let sample = ((body * 0.55 + growl * 0.75) * 1.25).tanh() * attack * gate * velocity;
        track.add(start + offset, sample, sample);
    }
}

/// 長く伸ばす純サインの低音（ブレイクとアウトロ用）
pub fn sub_note(track: &mut StereoBuffer, time: f32, duration: f32, note: f32, velocity: f32) {
    let start = seconds_to_samples(time);
    let release = 0.6;
    let length = seconds_to_samples(duration + release);
    let frequency = midi_to_frequency(note);
    let mut fundamental = Oscillator::new(0.0);
    let mut octave = Oscillator::new(0.0);
    for offset in 0..length {
        let t = seconds(offset);
        let attack = smoothstep(t / 0.08);
        let gate = if t < duration { 1.0 } else { (-(t - duration) / (release / 4.0)).exp() };
        let sample = (fundamental.sine(frequency) + octave.sine(frequency * 2.0) * 0.22) * attack * gate * velocity;
        track.add(start + offset, sample, sample);
    }
}

/// パッド: 1 音あたり 5 本のノコギリを散らしたスーパーソウ。カットオフは曲の時刻で外から与える。
pub struct PadSettings<'a> {
    pub attack: f32,
    pub release: f32,
    pub velocity: f32,
    pub cutoff: &'a dyn Fn(f32) -> f32,
}

pub fn pad_chord(
    track: &mut StereoBuffer,
    time: f32,
    duration: f32,
    notes: &[f32],
    settings: &PadSettings,
    rng: &mut Rng,
) {
    const DETUNE_CENTS: [f32; 5] = [-13.0, -6.0, 0.0, 6.5, 12.5];
    const PANS: [f32; 5] = [-0.85, -0.4, 0.0, 0.45, 0.85];
    struct Voice {
        oscillator: Oscillator,
        frequency: f32,
        left_gain: f32,
        right_gain: f32,
    }
    let mut voices = Vec::new();
    for &note in notes {
        for (&detune, &pan) in DETUNE_CENTS.iter().zip(PANS.iter()) {
            let (left_gain, right_gain) = pan_gains(pan);
            voices.push(Voice {
                oscillator: Oscillator::new(rng.unit()),
                frequency: midi_to_frequency(note) * cents_to_ratio(detune + rng.range(-1.5, 1.5)),
                left_gain,
                right_gain,
            });
        }
    }
    let normalization = 1.0 / (voices.len() as f32).sqrt();
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(duration + settings.release * 1.5);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let absolute_time = time + t;
        let vibrato = 1.0 + 0.0011 * (TAU * 0.31 * absolute_time).sin();
        let mut left = 0.0;
        let mut right = 0.0;
        for voice in &mut voices {
            let sample = voice.oscillator.saw(voice.frequency * vibrato);
            left += sample * voice.left_gain;
            right += sample * voice.right_gain;
        }
        let cutoff = (settings.cutoff)(absolute_time);
        let left = filter_left.process(left * normalization, cutoff, 0.72).low;
        let right = filter_right.process(right * normalization, cutoff, 0.72).low;
        let attack = smoothstep(t / settings.attack.max(0.001));
        let gate = if t < duration {
            1.0
        } else {
            (-(t - duration) / (settings.release / 4.0).max(0.001)).exp()
        };
        let envelope = attack * gate * settings.velocity;
        track.add(start + offset, left * envelope, right * envelope);
    }
}

/// プラック: 2 本のノコギリ + 矩形を、一瞬開いて閉じるフィルタに通す
pub fn pluck(track: &mut StereoBuffer, time: f32, note: f32, velocity: f32, pan: f32, brightness: f32) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.5);
    let frequency = midi_to_frequency(note);
    let mut first = Oscillator::new(0.0);
    let mut second = Oscillator::new(0.33);
    let mut third = Oscillator::new(0.66);
    let mut filter = Svf::default();
    let (left_gain, right_gain) = pan_gains(pan);
    for offset in 0..length {
        let t = seconds(offset);
        let raw = first.saw(frequency * cents_to_ratio(-5.0)) * 0.5
            + second.saw(frequency * cents_to_ratio(5.0)) * 0.5
            + third.square(frequency) * 0.22;
        let cutoff = 260.0 + brightness * (500.0 + 5_600.0 * (-t / 0.055).exp());
        let filtered = filter.process(raw, cutoff, 1.05).low;
        let envelope = smoothstep(t / 0.002) * (-t / 0.15).exp();
        let fade = if t > 0.44 { (1.0 - (t - 0.44) / 0.06).max(0.0) } else { 1.0 };
        let sample = filtered * envelope * fade * velocity;
        track.add(start + offset, sample * left_gain, sample * right_gain);
    }
}

/// ガラスっぽいベル: FM（変調比 2:1）で、打鍵の瞬間だけ倍音が多い
pub fn bell(track: &mut StereoBuffer, time: f32, duration: f32, note: f32, velocity: f32, pan: f32) {
    let start = seconds_to_samples(time);
    let release = 0.5;
    let length = seconds_to_samples(duration + release);
    let frequency = midi_to_frequency(note);
    let mut carrier_phase = 0.0f32;
    let mut modulator_phase = 0.0f32;
    let mut sparkle = Oscillator::new(0.0);
    let (left_gain, right_gain) = pan_gains(pan);
    for offset in 0..length {
        let t = seconds(offset);
        let index = 2.4 * (-t / 0.22).exp() + 0.35;
        modulator_phase = (modulator_phase + frequency * 2.0 / SAMPLE_RATE).fract();
        carrier_phase = (carrier_phase + frequency / SAMPLE_RATE).fract();
        let tone = (TAU * carrier_phase + index * (TAU * modulator_phase).sin()).sin();
        let shimmer = sparkle.sine(frequency * 4.0) * 0.12 * (-t / 0.18).exp();
        let decay = (-t / 1.1).exp();
        let gate = if t < duration { 1.0 } else { (-(t - duration) / 0.12).exp() };
        let envelope = smoothstep(t / 0.003) * decay * gate * velocity;
        let sample = (tone + shimmer) * envelope;
        track.add(start + offset, sample * left_gain, sample * right_gain);
    }
}

/// リード: 3 本のスーパーソウ + 1 オクターブ下の矩形。入りを少しずり上げ、伸ばすとビブラート。
pub fn lead_note(track: &mut StereoBuffer, time: f32, duration: f32, note: f32, velocity: f32) {
    const DETUNE_CENTS: [f32; 3] = [-10.0, 0.0, 10.0];
    const PANS: [f32; 3] = [-0.55, 0.0, 0.55];
    let start = seconds_to_samples(time);
    let release = 0.16;
    let length = seconds_to_samples(duration + release * 2.0);
    let frequency = midi_to_frequency(note);
    let mut voices: Vec<Oscillator> = (0..3).map(|index| Oscillator::new(index as f32 * 0.29)).collect();
    let mut octave_down = Oscillator::new(0.5);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let scoop = cents_to_ratio(-45.0 * (-t / 0.018).exp());
        let vibrato_depth = smoothstep((t - 0.2) / 0.25) * 14.0;
        let vibrato = cents_to_ratio(vibrato_depth * (TAU * 5.3 * t).sin());
        let pitch = frequency * scoop * vibrato;
        let mut left = 0.0;
        let mut right = 0.0;
        for ((oscillator, &detune), &pan) in voices.iter_mut().zip(DETUNE_CENTS.iter()).zip(PANS.iter()) {
            let sample = oscillator.saw(pitch * cents_to_ratio(detune));
            let (left_gain, right_gain) = pan_gains(pan);
            left += sample * left_gain;
            right += sample * right_gain;
        }
        let sub = octave_down.square(pitch * 0.5) * 0.25;
        let cutoff = 1_700.0 + 2_800.0 * (-t / 0.2).exp();
        let left = filter_left.process((left + sub) * 0.5, cutoff, 0.85).low;
        let right = filter_right.process((right + sub) * 0.5, cutoff, 0.85).low;
        let attack = smoothstep(t / 0.008);
        let sustain = 0.72 + 0.28 * (-t / 0.25).exp();
        let gate = if t < duration { 1.0 } else { (-(t - duration) / (release / 3.0)).exp() };
        let envelope = attack * sustain * gate * velocity;
        track.add(start + offset, left * envelope, right * envelope);
    }
}

/// ノイズのライザー: 帯域が上がりながら音量が増え、最後にすっと切れる
pub fn noise_riser(track: &mut StereoBuffer, start_time: f32, duration: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(start_time);
    let length = seconds_to_samples(duration);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    let mut tone = Oscillator::new(0.0);
    for offset in 0..length {
        let t = seconds(offset);
        let progress = t / duration;
        let cutoff = 350.0 * (9_500.0f32 / 350.0).powf(progress);
        let amplitude = progress.powf(2.2) * velocity;
        let tail = ((duration - t) / 0.01).min(1.0);
        let left = filter_left.process(rng.bipolar(), cutoff, 1.6).band;
        let right = filter_right.process(rng.bipolar(), cutoff * 1.04, 1.6).band;
        let uplifter = tone.sine(300.0 * 8f32.powf(progress)) * progress.powf(3.0) * 0.12;
        track.add(
            start + offset,
            (left + uplifter) * amplitude * tail,
            (right + uplifter) * amplitude * tail,
        );
    }
}

/// ダウンリフター: ライザーの逆（帯域が下がって消える）
pub fn downlifter(track: &mut StereoBuffer, start_time: f32, duration: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(start_time);
    let length = seconds_to_samples(duration);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let progress = t / duration;
        let cutoff = 7_000.0 * (250.0f32 / 7_000.0).powf(progress);
        let amplitude = (1.0 - progress).powf(2.0) * smoothstep(t / 0.01) * velocity;
        let left = filter_left.process(rng.bipolar(), cutoff, 1.3).band;
        let right = filter_right.process(rng.bipolar(), cutoff * 0.97, 1.3).band;
        track.add(start + offset, left * amplitude, right * amplitude);
    }
}

/// インパクト: 沈み込むサブ + 低域ノイズの一撃（残響は送りで足す）
pub fn impact(track: &mut StereoBuffer, time: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(3.2);
    let mut phase = 0.0f32;
    let mut noise_left = OnePole::default();
    let mut noise_right = OnePole::default();
    for offset in 0..length {
        let t = seconds(offset);
        let frequency = 30.0 + 75.0 * (-t / 0.32).exp();
        phase += frequency / SAMPLE_RATE;
        let boom = ((TAU * phase).sin() * (t / 0.003).min(1.0) * (-t / 1.0).exp() * 1.6).tanh();
        let noise_envelope = (-t / 0.18).exp() * 0.55;
        let left = noise_left.lowpass(rng.bipolar(), 1_400.0) * noise_envelope;
        let right = noise_right.lowpass(rng.bipolar(), 1_400.0) * noise_envelope;
        let fade = if t > 2.9 { (1.0 - (t - 2.9) / 0.3).max(0.0) } else { 1.0 };
        track.add(
            start + offset,
            (boom * 0.85 + left) * velocity * fade,
            (boom * 0.85 + right) * velocity * fade,
        );
    }
}

/// 画面の切り替えに添える風切り音。`direction` は -1（右→左）/ 1（左→右）。
pub fn whoosh(track: &mut StereoBuffer, time: f32, duration: f32, velocity: f32, direction: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(duration);
    let mut filter_left = Svf::default();
    let mut filter_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let progress = t / duration;
        let sweep = (PI * progress.powf(0.8)).sin();
        let cutoff = 320.0 + 3_600.0 * sweep * sweep;
        let amplitude = (PI * progress).sin().powf(1.6) * velocity;
        let noise = rng.bipolar();
        let left = filter_left.process(noise, cutoff, 1.4).band;
        let right = filter_right.process(noise, cutoff * 1.03, 1.4).band;
        let (left_gain, right_gain) = pan_gains(direction * (progress * 2.0 - 1.0) * 0.8);
        track.add(start + offset, left * amplitude * left_gain, right * amplitude * right_gain);
    }
}

/// キーボードの打鍵音: 高域ノイズの粒 + 底打ちの低い「コッ」
pub fn key_click(track: &mut StereoBuffer, time: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.045);
    let center = rng.range(2_400.0, 4_300.0);
    let pan = rng.range(-0.25, 0.25);
    let mut filter = Svf::default();
    let mut thump = Oscillator::new(0.0);
    let thump_pitch = rng.range(140.0, 190.0);
    for offset in 0..length {
        let t = seconds(offset);
        let tick = filter.process(rng.bipolar(), center, 1.8).band * (-t / 0.0045).exp();
        let body = thump.sine(thump_pitch) * (-t / 0.011).exp() * 0.4;
        track.add_panned(start + offset, (tick + body) * velocity, pan);
    }
}

/// UI の「ポン」: 数字や項目が出る瞬間に添える
pub fn blip(track: &mut StereoBuffer, time: f32, note: f32, velocity: f32, pan: f32) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.35);
    let frequency = midi_to_frequency(note);
    let mut fundamental = Oscillator::new(0.0);
    let mut overtone = Oscillator::new(0.0);
    for offset in 0..length {
        let t = seconds(offset);
        let envelope = smoothstep(t / 0.002) * (-t / 0.085).exp();
        let sample = (fundamental.sine(frequency) + overtone.sine(frequency * 3.0) * 0.18 * (-t / 0.03).exp())
            * envelope
            * velocity;
        track.add_panned(start + offset, sample, pan);
    }
}

/// 時計の秒針のような短い刻み（冒頭の緊張感）
pub fn tick(track: &mut StereoBuffer, time: f32, velocity: f32, pan: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.03);
    let mut tone = Oscillator::new(0.0);
    let mut filter = OnePole::default();
    for offset in 0..length {
        let t = seconds(offset);
        let envelope = (-t / 0.0035).exp();
        let sample = (tone.sine(2_600.0) * 0.6 + filter.highpass(rng.bipolar(), 5_000.0) * 0.4) * envelope * velocity;
        track.add_panned(start + offset, sample, pan);
    }
}

/// 誤送信のブザー: 不協和な矩形の二連打
pub fn error_buzz(track: &mut StereoBuffer, time: f32, velocity: f32) {
    let start = seconds_to_samples(time);
    let length = seconds_to_samples(0.3);
    let mut low = Oscillator::new(0.0);
    let mut high = Oscillator::new(0.3);
    let mut filter = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let gate = if t < 0.095 || (0.14..0.235).contains(&t) { 1.0 } else { 0.0 };
        let raw = low.square(98.0) * 0.5 + high.square(103.8) * 0.5;
        let filtered = filter.process(raw, 1_900.0, 0.9).low;
        let sample = filtered * gate * velocity;
        track.add(start + offset, sample, sample);
    }
}

/// 冒頭の持続音: 低いサインと、ゆっくり揺れる空気のノイズ
pub fn drone(track: &mut StereoBuffer, time: f32, duration: f32, note: f32, velocity: f32, rng: &mut Rng) {
    let start = seconds_to_samples(time);
    let release = 1.5;
    let length = seconds_to_samples(duration + release);
    let frequency = midi_to_frequency(note);
    let mut fundamental = Oscillator::new(0.0);
    let mut octave = Oscillator::new(0.0);
    let mut fifth = Oscillator::new(0.0);
    let mut air_left = Svf::default();
    let mut air_right = Svf::default();
    for offset in 0..length {
        let t = seconds(offset);
        let attack = smoothstep(t / 2.2);
        let gate = if t < duration { 1.0 } else { smoothstep(1.0 - (t - duration) / release) };
        let tremolo = 1.0 - 0.18 * (0.5 + 0.5 * (TAU * 0.23 * t).sin());
        let tone = fundamental.sine(frequency)
            + octave.sine(frequency * 2.0 * cents_to_ratio(3.0)) * 0.35
            + fifth.sine(frequency * 3.0) * 0.08;
        let air_cutoff = 700.0 + 500.0 * (TAU * 0.11 * t).sin();
        let air_left_sample = air_left.process(rng.bipolar(), air_cutoff, 0.8).band * 0.10;
        let air_right_sample = air_right.process(rng.bipolar(), air_cutoff * 1.1, 0.8).band * 0.10;
        let envelope = attack * gate * velocity;
        track.add(
            start + offset,
            (tone * 0.5 * tremolo + air_left_sample) * envelope,
            (tone * 0.5 * tremolo + air_right_sample) * envelope,
        );
    }
}

/// 読み込んだ短い音（モノラル）を指定時刻に置く
pub fn place_sample(track: &mut StereoBuffer, time: f32, samples: &[f32], velocity: f32, pan: f32) {
    let start = seconds_to_samples(time);
    for (offset, &sample) in samples.iter().enumerate() {
        track.add_panned(start + offset, sample * velocity, pan);
    }
}
