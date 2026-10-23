//! 合成の部品: 発振器・フィルタ・残響・ディレイ・リミッタ。
//!
//! どれも 1 サンプルずつ回す素朴な実装。曲全体でも数秒で書き出せるので、
//! ブロック処理や SIMD には寄せず、読みやすさを優先する。

use std::f32::consts::{PI, TAU};

pub const SAMPLE_RATE: f32 = 48_000.0;

pub fn seconds_to_samples(seconds: f32) -> usize {
    (seconds * SAMPLE_RATE).round().max(0.0) as usize
}

pub fn midi_to_frequency(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

pub fn cents_to_ratio(cents: f32) -> f32 {
    2f32.powf(cents / 1200.0)
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// 等パワーのパン。`pan` は -1（左）〜 1（右）。
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * PI * 0.25;
    (angle.cos(), angle.sin())
}

/// 曲全体の長さを持つステレオのトラック。
#[derive(Clone)]
pub struct StereoBuffer {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl StereoBuffer {
    pub fn new(length: usize) -> Self {
        Self {
            left: vec![0.0; length],
            right: vec![0.0; length],
        }
    }

    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn add(&mut self, index: usize, left: f32, right: f32) {
        if index < self.left.len() {
            self.left[index] += left;
            self.right[index] += right;
        }
    }

    pub fn add_panned(&mut self, index: usize, sample: f32, pan: f32) {
        let (left_gain, right_gain) = pan_gains(pan);
        self.add(index, sample * left_gain, sample * right_gain);
    }

    pub fn mix_from(&mut self, other: &StereoBuffer, gain: f32) {
        for index in 0..self.len().min(other.len()) {
            self.left[index] += other.left[index] * gain;
            self.right[index] += other.right[index] * gain;
        }
    }

    pub fn apply_gain_curve(&mut self, curve: &[f32]) {
        for index in 0..self.len().min(curve.len()) {
            self.left[index] *= curve[index];
            self.right[index] *= curve[index];
        }
    }

    pub fn peak(&self) -> f32 {
        self.left
            .iter()
            .chain(self.right.iter())
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }
}

/// xorshift64*。毎回同じ曲になるよう種は固定で渡す。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0.0 〜 1.0
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// -1.0 〜 1.0
    pub fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }
}

fn poly_blep(phase: f32, increment: f32) -> f32 {
    if phase < increment {
        let position = phase / increment;
        position + position - position * position - 1.0
    } else if phase > 1.0 - increment {
        let position = (phase - 1.0) / increment;
        position * position + position + position + 1.0
    } else {
        0.0
    }
}

/// PolyBLEP で折り返しを抑えた発振器。
pub struct Oscillator {
    phase: f32,
}

impl Oscillator {
    pub fn new(phase: f32) -> Self {
        Self {
            phase: phase.rem_euclid(1.0),
        }
    }

    fn advance(&mut self, increment: f32) {
        self.phase += increment;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }

    pub fn saw(&mut self, frequency: f32) -> f32 {
        let increment = (frequency / SAMPLE_RATE).clamp(0.0, 0.5);
        let value = 2.0 * self.phase - 1.0 - poly_blep(self.phase, increment);
        self.advance(increment);
        value
    }

    pub fn square(&mut self, frequency: f32) -> f32 {
        let increment = (frequency / SAMPLE_RATE).clamp(0.0, 0.5);
        let mut value = if self.phase < 0.5 { 1.0 } else { -1.0 };
        value += poly_blep(self.phase, increment);
        value -= poly_blep((self.phase + 0.5).rem_euclid(1.0), increment);
        self.advance(increment);
        value
    }

    pub fn sine(&mut self, frequency: f32) -> f32 {
        let value = (TAU * self.phase).sin();
        self.advance((frequency / SAMPLE_RATE).clamp(0.0, 0.5));
        value
    }
}

pub struct SvfOutput {
    pub low: f32,
    /// 中心周波数で利得 1 に揃えたバンドパス
    pub band: f32,
    pub high: f32,
}

/// Cytomic（Zavalishin）の TPT 状態変数フィルタ。カットオフを毎サンプル動かしても暴れない。
#[derive(Default, Clone, Copy)]
pub struct Svf {
    first_state: f32,
    second_state: f32,
}

impl Svf {
    pub fn process(&mut self, input: f32, cutoff: f32, quality: f32) -> SvfOutput {
        let cutoff = cutoff.clamp(20.0, SAMPLE_RATE * 0.45);
        let gain = (PI * cutoff / SAMPLE_RATE).tan();
        let damping = 1.0 / quality.max(0.05);
        let coefficient_one = 1.0 / (1.0 + gain * (gain + damping));
        let coefficient_two = gain * coefficient_one;
        let coefficient_three = gain * coefficient_two;
        let difference = input - self.second_state;
        let band = coefficient_one * self.first_state + coefficient_two * difference;
        let low = self.second_state + coefficient_two * self.first_state + coefficient_three * difference;
        self.first_state = 2.0 * band - self.first_state;
        self.second_state = 2.0 * low - self.second_state;
        SvfOutput {
            low,
            band: band * damping,
            high: input - damping * band - low,
        }
    }
}

/// 1 次の平滑化フィルタ。音色の角を丸める程度に使う。
#[derive(Default, Clone, Copy)]
pub struct OnePole {
    state: f32,
}

impl OnePole {
    pub fn lowpass(&mut self, input: f32, cutoff: f32) -> f32 {
        let coefficient = 1.0 - (-TAU * cutoff / SAMPLE_RATE).exp();
        self.state += coefficient * (input - self.state);
        self.state
    }

    pub fn highpass(&mut self, input: f32, cutoff: f32) -> f32 {
        input - self.lowpass(input, cutoff)
    }
}

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    filter_store: f32,
}

impl Comb {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
            filter_store: 0.0,
        }
    }

    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.buffer[self.index];
        self.filter_store = output * (1.0 - damping) + self.filter_store * damping;
        self.buffer[self.index] = input + self.filter_store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
}

struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = buffered - input;
        self.buffer[self.index] = input + buffered * 0.5;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
}

/// Freeverb（Jezar のチューニングを 48kHz へ伸ばしたもの）+ プリディレイ。
pub struct Reverb {
    combs_left: Vec<Comb>,
    combs_right: Vec<Comb>,
    allpasses_left: Vec<Allpass>,
    allpasses_right: Vec<Allpass>,
    pre_delay: Vec<f32>,
    pre_delay_index: usize,
    feedback: f32,
    damping: f32,
    input_highpass: OnePole,
    input_lowpass: OnePole,
}

impl Reverb {
    pub fn new(room_size: f32, damping: f32, pre_delay_seconds: f32) -> Self {
        const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
        const STEREO_SPREAD: usize = 23;
        let scale = SAMPLE_RATE / 44_100.0;
        let scaled = |length: usize| (length as f32 * scale).round() as usize;
        Self {
            combs_left: COMB_TUNING.iter().map(|&length| Comb::new(scaled(length))).collect(),
            combs_right: COMB_TUNING
                .iter()
                .map(|&length| Comb::new(scaled(length + STEREO_SPREAD)))
                .collect(),
            allpasses_left: ALLPASS_TUNING.iter().map(|&length| Allpass::new(scaled(length))).collect(),
            allpasses_right: ALLPASS_TUNING
                .iter()
                .map(|&length| Allpass::new(scaled(length + STEREO_SPREAD)))
                .collect(),
            pre_delay: vec![0.0; seconds_to_samples(pre_delay_seconds).max(1)],
            pre_delay_index: 0,
            feedback: room_size * 0.28 + 0.7,
            damping: damping * 0.4,
            input_highpass: OnePole::default(),
            input_lowpass: OnePole::default(),
        }
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mono = (left + right) * 0.5;
        let filtered = self.input_lowpass.lowpass(self.input_highpass.highpass(mono, 280.0), 7_500.0);
        let delayed = self.pre_delay[self.pre_delay_index];
        self.pre_delay[self.pre_delay_index] = filtered;
        self.pre_delay_index = (self.pre_delay_index + 1) % self.pre_delay.len();

        let input = delayed * 0.015;
        let mut output_left = 0.0;
        let mut output_right = 0.0;
        for comb in &mut self.combs_left {
            output_left += comb.process(input, self.feedback, self.damping);
        }
        for comb in &mut self.combs_right {
            output_right += comb.process(input, self.feedback, self.damping);
        }
        for allpass in &mut self.allpasses_left {
            output_left = allpass.process(output_left);
        }
        for allpass in &mut self.allpasses_right {
            output_right = allpass.process(output_right);
        }
        (output_left * 3.0, output_right * 3.0)
    }
}

/// 左右に跳ねるディレイ。帰還路で高域を落として、繰り返すほど遠くなるようにする。
pub struct PingPongDelay {
    left: Vec<f32>,
    right: Vec<f32>,
    index: usize,
    feedback: f32,
    damping_left: OnePole,
    damping_right: OnePole,
}

impl PingPongDelay {
    pub fn new(delay_seconds: f32, feedback: f32) -> Self {
        let length = seconds_to_samples(delay_seconds).max(1);
        Self {
            left: vec![0.0; length],
            right: vec![0.0; length],
            index: 0,
            feedback,
            damping_left: OnePole::default(),
            damping_right: OnePole::default(),
        }
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let delayed_left = self.left[self.index];
        let delayed_right = self.right[self.index];
        let input = (left + right) * 0.5;
        self.left[self.index] = input + self.damping_right.lowpass(delayed_right, 4_500.0) * self.feedback;
        self.right[self.index] = self.damping_left.lowpass(delayed_left, 4_500.0) * self.feedback;
        self.index = (self.index + 1) % self.left.len();
        (delayed_left, delayed_right)
    }
}

/// 先読みピークリミッタ。先読み窓の最小ゲインを取り、同じ長さの移動平均で角を丸める
/// （平均は窓内の最小値を超えないので、ピークの位置では必ず天井に収まる）。
pub fn limit(buffer: &mut StereoBuffer, ceiling: f32, lookahead_seconds: f32, release_seconds: f32) {
    let length = buffer.len();
    let lookahead = seconds_to_samples(lookahead_seconds).max(1);
    let required: Vec<f32> = (0..length)
        .map(|index| {
            let peak = buffer.left[index].abs().max(buffer.right[index].abs());
            if peak > ceiling {
                ceiling / peak
            } else {
                1.0
            }
        })
        .collect();

    // 窓 [index, index + lookahead] の最小値（単調キュー）
    let mut window_minimum = vec![1.0f32; length];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for index in (0..length).rev() {
        while let Some(&back) = queue.back() {
            if required[back] >= required[index] {
                queue.pop_back();
            } else {
                break;
            }
        }
        queue.push_back(index);
        while let Some(&front) = queue.front() {
            if front > index + lookahead {
                queue.pop_front();
            } else {
                break;
            }
        }
        if let Some(&front) = queue.front() {
            window_minimum[index] = required[front];
        }
    }

    let release_coefficient = 1.0 - (-1.0 / (release_seconds * SAMPLE_RATE)).exp();
    let mut released = vec![1.0f32; length];
    let mut current = 1.0f32;
    for index in 0..length {
        let target = window_minimum[index];
        current = if target < current {
            target
        } else {
            current + (target - current) * release_coefficient
        };
        released[index] = current;
    }

    let mut running_sum = 0.0f64;
    for index in 0..length {
        running_sum += released[index] as f64;
        if index >= lookahead {
            running_sum -= released[index - lookahead] as f64;
        }
        let count = (index + 1).min(lookahead) as f64;
        let gain = (running_sum / count) as f32;
        buffer.left[index] *= gain;
        buffer.right[index] *= gain;
    }

    // 移動平均の端数で天井をわずかに越えた分だけを最後に軟らかく丸める
    for index in 0..length {
        buffer.left[index] = soft_ceiling(buffer.left[index], ceiling);
        buffer.right[index] = soft_ceiling(buffer.right[index], ceiling);
    }
}

fn soft_ceiling(sample: f32, ceiling: f32) -> f32 {
    let knee = ceiling * 0.9;
    let magnitude = sample.abs();
    if magnitude <= knee {
        return sample;
    }
    let over = (magnitude - knee) / (ceiling - knee);
    let shaped = knee + (ceiling - knee) * over.tanh();
    shaped.copysign(sample)
}
