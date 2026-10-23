// 画面下の字幕（本文 + 副文）。下から暗くするグラデーションを敷いて、実画面の上でも読めるようにする。
import React from 'react';
import {AbsoluteFill, interpolate, useCurrentFrame} from 'remotion';
import {fonts, palette} from '../theme';
import {Fade, Rise} from './Text';

export const Caption: React.FC<{
  text: string;
  at: number;
  out?: number;
  sub?: string;
  subAt?: number;
  size?: number;
  scrim?: boolean;
}> = ({text, at, out, sub, subAt, size = 60, scrim = true}) => {
  const frame = useCurrentFrame();
  const scrimOpacity = scrim ? interpolate(frame, [at - 6, at + 8], [0, 1], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'}) : 0;
  return (
    <AbsoluteFill style={{justifyContent: 'flex-end', alignItems: 'center', pointerEvents: 'none'}}>
      <div
        style={{
          position: 'absolute',
          left: 0,
          right: 0,
          bottom: 0,
          height: 440,
          background: `linear-gradient(to top, ${palette.ink}f5 0%, ${palette.ink}d9 38%, ${palette.ink}00 100%)`,
          opacity: scrimOpacity,
        }}
      />
      <div style={{position: 'relative', textAlign: 'center', paddingBottom: sub ? 64 : 84}}>
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: size, color: palette.white, letterSpacing: '0.01em'}}>
          <Rise text={text} at={at} out={out} />
        </div>
        {sub ? (
          <Fade at={subAt ?? at + 18} out={out} style={{fontFamily: fonts.japanese, fontSize: 28, color: palette.fg1, marginTop: 14}}>
            {sub}
          </Fade>
        ) : null}
      </div>
    </AbsoluteFill>
  );
};
