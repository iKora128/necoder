// nyaco（アプリの Agent パネルに住む猫耳コーダー）。アプリ同梱のスプライト（15 コマ × 60×72）をそのまま使う。
import React from 'react';
import {staticFile, useCurrentFrame, useVideoConfig} from 'remotion';

export type MascotKind = 'typing' | 'think' | 'celebrate' | 'doze' | 'plead' | 'worry';

export const Mascot: React.FC<{kind: MascotKind; scale: number; framesPerSecond?: number; style?: React.CSSProperties}> = ({
  kind,
  scale,
  framesPerSecond = 8,
  style,
}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const index = Math.floor((frame / fps) * framesPerSecond) % 15;
  return (
    <div
      style={{
        width: 60 * scale,
        height: 72 * scale,
        backgroundImage: `url(${staticFile(`mascot/${kind}-strip.png`)})`,
        backgroundSize: `${900 * scale}px ${72 * scale}px`,
        backgroundPosition: `${-index * 60 * scale}px 0px`,
        backgroundRepeat: 'no-repeat',
        imageRendering: 'pixelated',
        ...style,
      }}
    />
  );
};
