// 120 BPM・60fps。1 拍 = 30 フレーム・1 小節 = 120 フレーム。
// 場面の切れ目はサウンドトラック（soundtrack/src/main.rs の表）と同じ小節に置く。
import cues from './cues.json';

export const FPS = 60;
export const BEAT = 30;
export const BAR = BEAT * 4;
export const STEP = BEAT / 4;
export const TOTAL_BARS = 41;
export const DURATION = TOTAL_BARS * BAR;

export const bars = (count: number) => Math.round(count * BAR);
export const beats = (count: number) => Math.round(count * BEAT);
export const seconds = (value: number) => Math.round(value * FPS);

export const scenes = {
  lost: {from: bars(0), length: bars(4)},
  built: {from: bars(4), length: bars(4)},
  logo: {from: bars(8), length: bars(2)},
  color: {from: bars(10), length: bars(6)},
  agents: {from: bars(16), length: bars(8)},
  touches: {from: bars(24), length: bars(4)},
  power: {from: bars(28), length: bars(8)},
  outro: {from: bars(36), length: bars(5)},
} as const;

export {cues};
