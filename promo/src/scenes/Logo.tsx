// 16–20 秒: ロゴ。ドロップの頭（インパクト）で白く弾け、名前と一言が立つ。
import React from 'react';
import {AbsoluteFill, Easing, Img, interpolate, spring, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';
import {Fade, Rise} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette, projectColors} from '../theme';
import {beats} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

export const Logo: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].logo;
  const flash = interpolate(frame, [0, 10], [0.9, 0], clamp);
  const mark = spring({frame, fps, config: {damping: 16, stiffness: 180, mass: 0.8}});
  const tracking = interpolate(frame, [0, 26], [0.5, -0.03], {...clamp, easing: Easing.out(Easing.exp)});
  const wordOpacity = interpolate(frame, [2, 12], [0, 1], clamp);
  const exit = interpolate(frame, [beats(7.2), beats(8)], [0, 1], {...clamp, easing: Easing.in(Easing.cubic)});
  const drift = interpolate(frame, [0, beats(8)], [1, 1.04]);

  return (
    <AbsoluteFill style={{background: palette.ink, alignItems: 'center', justifyContent: 'center'}}>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          transform: `scale(${drift + exit * 0.5})`,
          opacity: 1 - exit,
          filter: `blur(${exit * 16}px)`,
        }}
      >
        <div style={{display: 'flex', alignItems: 'center', gap: 44}}>
          <Img
            src={staticFile('brand/necoder-mark.png')}
            style={{width: 190, height: 190, borderRadius: 44, transform: `scale(${mark})`, boxShadow: '0 20px 60px rgba(0,0,0,0.6)'}}
          />
          <div
            style={{
              fontFamily: fonts.display,
              fontWeight: 800,
              fontSize: 210,
              letterSpacing: `${tracking}em`,
              color: palette.white,
              opacity: wordOpacity,
              lineHeight: 1,
            }}
          >
            necoder
          </div>
        </div>
        <div style={{display: 'flex', gap: 10, marginTop: 36}}>
          {projectColors.map((color, index) => {
            const grow = interpolate(frame, [6 + index * 3, 22 + index * 3], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
            return <div key={color} style={{width: 120 * grow, height: 8, borderRadius: 4, background: color}} />;
          })}
        </div>
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 76, color: palette.white, marginTop: 48}}>
          <Rise text={text.tagline} at={beats(2)} />
        </div>
        <Fade at={beats(4)} style={{fontFamily: fonts.japanese, fontSize: 34, color: palette.fg1, marginTop: 18}}>
          {text.sub}
        </Fade>
      </div>
      <AbsoluteFill style={{background: palette.white, opacity: flash, pointerEvents: 'none'}} />
    </AbsoluteFill>
  );
};
