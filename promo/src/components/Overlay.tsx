// 画面全体に重ねる質感: フィルムグレイン（毎フレーム位置をずらす）と周辺減光。どちらも無彩色。
import React from 'react';
import {AbsoluteFill, random, staticFile, useCurrentFrame} from 'remotion';

export const Grain: React.FC<{opacity?: number}> = ({opacity = 0.07}) => {
  const frame = useCurrentFrame();
  const step = Math.floor(frame / 2);
  const x = Math.floor(random(`grain-x-${step}`) * 384);
  const y = Math.floor(random(`grain-y-${step}`) * 384);
  return (
    <AbsoluteFill
      style={{
        backgroundImage: `url(${staticFile('fx/grain.png')})`,
        backgroundPosition: `${x}px ${y}px`,
        backgroundSize: '384px 384px',
        opacity,
        mixBlendMode: 'overlay',
        pointerEvents: 'none',
      }}
    />
  );
};

export const Vignette: React.FC<{strength?: number}> = ({strength = 0.55}) => (
  <AbsoluteFill
    style={{
      background: `radial-gradient(ellipse 75% 70% at 50% 48%, rgba(0,0,0,0) 55%, rgba(0,0,0,${strength}) 100%)`,
      pointerEvents: 'none',
    }}
  />
);
