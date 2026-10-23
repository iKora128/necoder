// 8–16 秒: だから作った。拍ごとに言葉を叩き、最後はスネアロールに合わせて 5 色のレールが灯る。
import React from 'react';
import {AbsoluteFill, Easing, interpolate, spring, useCurrentFrame, useVideoConfig} from 'remotion';
import {RailItem} from '../components/RailItem';
import {Rise, Slam} from '../components/Text';
import {copy, type Language} from '../copy';
import {PulseGrid} from '../components/PulseGrid';
import {fonts, palette, projects} from '../theme';
import {beats, STEP} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

// 小節 7 のスネアロール（soundtrack の snare_roll と同じ位置・16 分単位）
const ROLL_STEPS = [0, 2, 4, 6, 8, 9, 10, 11, 12, 12.5, 13, 13.5];
const ROLL_START = beats(12);
const rollFrame = (step: number) => ROLL_START + Math.round(step * STEP);

export const Built: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].built;
  const big = {fontFamily: fonts.display, fontWeight: 800, color: palette.white, letterSpacing: '-0.02em'} as const;

  // ロール: 最初の 5 打で 1 枚ずつ灯り、残りの打で寄り集まって膨らむ
  const gather = interpolate(frame, [rollFrame(8), rollFrame(13.5) + 4], [0, 1], {...clamp, easing: Easing.in(Easing.cubic)});
  const hits = ROLL_STEPS.filter((step) => frame >= rollFrame(step));
  const lastHit = hits.length > 0 ? rollFrame(hits[hits.length - 1]) : -100;
  const pulse = interpolate(frame, [lastHit, lastHit + 6], [1, 0], clamp);
  const blackout = frame >= rollFrame(14);

  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <AbsoluteFill style={{opacity: blackout ? 0 : interpolate(frame, [beats(8), beats(10)], [0.35, 1], clamp)}}>
        <PulseGrid />
      </AbsoluteFill>
      {/* だから、エディタを作った。 */}
      <AbsoluteFill style={{justifyContent: 'center', paddingLeft: 180}}>
        <div style={{...big, fontFamily: fonts.japanese, fontWeight: 600, fontSize: 64, color: palette.fg1}}>
          <Rise text={text.so} at={0} out={beats(3.6)} />
        </div>
        <div style={{...big, fontSize: 150, marginTop: 8}}>
          <Rise text={text.built} at={beats(1)} out={beats(3.7)} />
        </div>
      </AbsoluteFill>

      {/* ゼロから。Rust で。 */}
      <AbsoluteFill style={{justifyContent: 'center', paddingLeft: 180}}>
        <div style={{...big, fontSize: 150}}>
          <Rise text={text.scratch} at={beats(4)} out={beats(7.6)} />
        </div>
        <div style={{...big, fontSize: 150}}>
          <Rise text={text.rust} at={beats(6)} out={beats(7.7)} />
        </div>
      </AbsoluteFill>

      {/* 拍ごとに叩く 4 語 */}
      <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center'}}>
        {text.slams.map((word, index) => (
          <Slam key={word} at={beats(8 + index)} out={beats(9 + index)} style={{...big, fontSize: 168, position: 'absolute'}}>
            {word}
          </Slam>
        ))}
      </AbsoluteFill>

      {/* 5 色のレールがロールで灯る */}
      {!blackout && frame >= ROLL_START ? (
        <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center'}}>
          <div style={{display: 'flex', gap: 44 * (1 - gather) + 4, transform: `scale(${1 + gather * 0.35 + pulse * 0.04})`}}>
            {projects.map((project, index) => {
              const at = rollFrame(ROLL_STEPS[index]);
              const enter = spring({frame: frame - at, fps, config: {damping: 14, stiffness: 260, mass: 0.6}});
              return (
                <div key={project.name} style={{opacity: frame >= at ? 1 : 0, transform: `scale(${enter}) translateY(${(1 - enter) * 30}px)`}}>
                  <RailItem color={project.color} monogram={project.monogram} active size={132} style={{boxShadow: `0 0 0 9px ${project.color}40, 0 0 ${50 * pulse + 10}px ${project.color}55`}} />
                </div>
              );
            })}
          </div>
        </AbsoluteFill>
      ) : null}
    </AbsoluteFill>
  );
};
