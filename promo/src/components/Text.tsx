// 文字の動き。どれも「シーン内のフレーム」で時刻を受け取る（Sequence の中で使う前提）。
import React from 'react';
import {Easing, interpolate, spring, useCurrentFrame, useVideoConfig} from 'remotion';
import {palette} from '../theme';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const hasJapanese = (text: string) => /[　-ヿ㐀-鿿＀-￯]/.test(text);

type RiseProps = {
  text: string;
  at: number;
  out?: number;
  stagger?: number;
  style?: React.CSSProperties;
};

/**
 * 1 行を下からせり上げる。日本語は 1 文字ずつ、英語は 1 語ずつ、マスクの中から出す。
 */
export const Rise: React.FC<RiseProps> = ({text, at, out, stagger, style}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const japanese = hasJapanese(text);
  const step = stagger ?? (japanese ? 1.2 : 2.5);
  let order = 0;
  const renderUnit = (unit: string, key: string) => {
    if (unit.length === 0) {
      return null;
    }
    if (/^\s+$/.test(unit)) {
      return <span key={key}>{unit}</span>;
    }
    const position = order++;
    const enter = spring({
      frame: frame - at - position * step,
      fps,
      config: {damping: 200, stiffness: 140, mass: 0.8},
    });
    const leave =
      out === undefined
        ? 0
        : interpolate(frame, [out + position * step * 0.4, out + position * step * 0.4 + 12], [0, 1], {
            ...clamp,
            easing: Easing.in(Easing.cubic),
          });
    const offset = (1 - enter) * 110 - leave * 110;
    return (
      <span
        key={key}
        style={{display: 'inline-block', overflow: 'hidden', verticalAlign: 'bottom', padding: '0.06em 0.02em 0.14em', margin: '-0.06em -0.02em -0.14em'}}
      >
        <span style={{display: 'inline-block', transform: `translateY(${offset}%)`, opacity: Math.min(1, enter * 1.6) * (1 - leave)}}>{unit}</span>
      </span>
    );
  };
  const lines = text.split('\n');
  return (
    <span style={{display: 'inline-block', whiteSpace: 'pre', ...style}}>
      {lines.map((line, lineIndex) => {
        const units = japanese ? Array.from(line) : line.split(/(\s+)/);
        return (
          <span key={lineIndex} style={{display: lines.length > 1 ? 'block' : 'inline'}}>
            {units.map((unit, index) => renderUnit(unit, `${lineIndex}-${index}`))}
          </span>
        );
      })}
    </span>
  );
};

type FadeProps = {at: number; out?: number; children: React.ReactNode; style?: React.CSSProperties; rise?: number; duration?: number};

/** ふわっと出る（副文・注記用） */
export const Fade: React.FC<FadeProps> = ({at, out, children, style, rise = 18, duration = 18}) => {
  const frame = useCurrentFrame();
  const enter = interpolate(frame, [at, at + duration], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const leave = out === undefined ? 0 : interpolate(frame, [out, out + 12], [0, 1], {...clamp, easing: Easing.in(Easing.cubic)});
  return (
    <div style={{opacity: enter * (1 - leave), transform: `translateY(${(1 - enter) * rise - leave * rise * 0.5}px)`, ...style}}>
      {children}
    </div>
  );
};

type SlamProps = {at: number; out?: number; children: React.ReactNode; style?: React.CSSProperties; from?: number};

/** 拍に合わせて叩きつける（大きく・ぼけから合焦） */
export const Slam: React.FC<SlamProps> = ({at, out, children, style, from = 1.35}) => {
  const frame = useCurrentFrame();
  if (frame < at || (out !== undefined && frame >= out)) {
    return null;
  }
  const local = frame - at;
  const scale = interpolate(local, [0, 9], [from, 1], {...clamp, easing: Easing.out(Easing.exp)});
  const blur = interpolate(local, [0, 7], [14, 0], clamp);
  const opacity = interpolate(local, [0, 3], [0, 1], clamp);
  return <div style={{transform: `scale(${scale})`, filter: `blur(${blur}px)`, opacity, ...style}}>{children}</div>;
};

type TypeProps = {
  text: string;
  at: number;
  interval: number;
  caretColor?: string;
  caret?: boolean;
  style?: React.CSSProperties;
};

/** 1 文字ずつ打つ。打鍵音（cues.json）と同じ間隔で文字が増える。 */
export const Typewriter: React.FC<TypeProps> = ({text, at, interval, caretColor = palette.white, caret = true, style}) => {
  const frame = useCurrentFrame();
  const characters = Array.from(text);
  const count = frame < at ? 0 : Math.min(characters.length, Math.floor((frame - at) / interval) + 1);
  const typing = frame >= at && count < characters.length;
  const blink = typing || Math.floor(frame / 33) % 2 === 0;
  return (
    <span style={{whiteSpace: 'pre', ...style}}>
      {characters.slice(0, count).join('')}
      {caret ? (
        <span
          style={{
            display: 'inline-block',
            width: '0.09em',
            minWidth: 3,
            height: '1.08em',
            marginLeft: '0.06em',
            verticalAlign: '-0.16em',
            background: caretColor,
            opacity: blink ? 1 : 0,
          }}
        />
      ) : null}
    </span>
  );
};

type CounterProps = {to: number; at: number; duration: number; decimals?: number; style?: React.CSSProperties};

/** 数字を 0 から回して止める */
export const Counter: React.FC<CounterProps> = ({to, at, duration, decimals = 0, style}) => {
  const frame = useCurrentFrame();
  const value = interpolate(frame, [at, at + duration], [0, to], {...clamp, easing: Easing.out(Easing.cubic)});
  const text = value.toLocaleString('en-US', {minimumFractionDigits: decimals, maximumFractionDigits: decimals});
  return <span style={{fontVariantNumeric: 'tabular-nums', ...style}}>{text}</span>;
};
