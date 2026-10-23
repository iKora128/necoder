// スマホの枠（iPhone 13 の画面比 390×844）。中身は子で渡す。
import React from 'react';

export const PHONE_SCREEN = {width: 390, height: 844};

export const Phone: React.FC<{scale: number; children: React.ReactNode; style?: React.CSSProperties}> = ({scale, children, style}) => {
  const bezel = 14 * scale;
  const width = PHONE_SCREEN.width * scale;
  const height = PHONE_SCREEN.height * scale;
  return (
    <div
      style={{
        position: 'relative',
        width: width + bezel * 2,
        height: height + bezel * 2,
        borderRadius: 62 * scale,
        background: 'linear-gradient(145deg, #2a2d34, #0f1013)',
        boxShadow: `0 ${50 * scale}px ${120 * scale}px rgba(0,0,0,0.7), inset 0 0 0 ${2 * scale}px #3a3e47`,
        padding: bezel,
        boxSizing: 'border-box',
        ...style,
      }}
    >
      <div style={{position: 'relative', width, height, borderRadius: 48 * scale, overflow: 'hidden', background: '#0b0c0f'}}>
        {children}
        <div
          style={{
            position: 'absolute',
            left: '50%',
            top: 11 * scale,
            width: 120 * scale,
            height: 34 * scale,
            marginLeft: -60 * scale,
            borderRadius: 20 * scale,
            background: '#000',
          }}
        />
      </div>
    </div>
  );
};

/** タップの波紋（指で押した位置）。`at` から 18 フレームで広がって消える。 */
export const Tap: React.FC<{x: number; y: number; at: number; frame: number; scale: number}> = ({x, y, at, frame, scale}) => {
  const local = frame - at;
  if (local < -8 || local > 20) {
    return null;
  }
  const press = local < 0 ? 1 + local / 8 : 1;
  const ring = Math.max(0, local) / 20;
  return (
    <div style={{position: 'absolute', left: x * scale, top: y * scale, pointerEvents: 'none'}}>
      <div
        style={{
          position: 'absolute',
          width: 44 * scale,
          height: 44 * scale,
          marginLeft: -22 * scale,
          marginTop: -22 * scale,
          borderRadius: '50%',
          background: `rgba(255,255,255,${0.45 * press * (1 - ring)})`,
        }}
      />
      <div
        style={{
          position: 'absolute',
          width: (44 + 60 * ring) * scale,
          height: (44 + 60 * ring) * scale,
          marginLeft: -(22 + 30 * ring) * scale,
          marginTop: -(22 + 30 * ring) * scale,
          borderRadius: '50%',
          border: `${2 * scale}px solid rgba(255,255,255,${0.8 * (1 - ring)})`,
          opacity: local >= 0 ? 1 : 0,
        }}
      />
    </div>
  );
};
