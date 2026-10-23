// 拍に合わせて明滅する方眼（ドロップの鼓動）。無彩色。
import React from 'react';
import {AbsoluteFill, interpolate, useCurrentFrame} from 'remotion';
import {beats} from '../timeline';

export const PulseGrid: React.FC = () => {
  const frame = useCurrentFrame();
  const kick = interpolate(frame % beats(1), [0, 12], [1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
  const line = `rgba(255,255,255,${0.035 + 0.035 * kick})`;
  return (
    <AbsoluteFill
      style={{
        backgroundImage: `linear-gradient(${line} 1px, transparent 1px), linear-gradient(90deg, ${line} 1px, transparent 1px)`,
        backgroundSize: '60px 60px',
        maskImage: 'radial-gradient(ellipse 70% 65% at 50% 50%, black 30%, transparent 100%)',
        WebkitMaskImage: 'radial-gradient(ellipse 70% 65% at 50% 50%, black 30%, transparent 100%)',
      }}
    />
  );
};
