// 20–32 秒: 色。実機の 5 プロジェクトを拍ごとに切り替え → 色の流れ先に寄る → ⌘O で全体を見る。
import React from 'react';
import {AbsoluteFill, Easing, interpolate, useCurrentFrame} from 'remotion';
import {Caption} from '../components/Caption';
import {Shot, type Camera} from '../components/Shot';
import {copy, type Language} from '../copy';
import {fonts, palette, projects, threadColors} from '../theme';
import {beats} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const WIDE: Camera = {x: 0.5, y: 0.53, zoom: 0.88, tiltX: 5};

// 画像内の座標（0〜1）。hero-*.png（3360×2000）で測った位置。
const TARGETS = {
  rail: {x: 0.0145, y: 0.138},
  tab: {x: 0.366, y: 0.041},
  caret: {x: 0.188, y: 0.535},
  chip: {x: 0.87, y: 0.741},
};

const FocusRing: React.FC<{left: number; top: number; width: number; height: number; color: string; at: number; label: string; side: 'left' | 'right' | 'below'}> = ({
  left,
  top,
  width,
  height,
  color,
  at,
  label,
  side,
}) => {
  const frame = useCurrentFrame();
  const enter = interpolate(frame, [at, at + 12], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const leave = interpolate(frame, [at + 48, at + 58], [0, 1], clamp);
  const opacity = enter * (1 - leave);
  if (opacity <= 0) {
    return null;
  }
  const pulse = 1 + 0.04 * Math.sin((frame - at) / 5);
  const labelStyle: React.CSSProperties = {
    position: 'absolute',
    fontFamily: fonts.japanese,
    fontWeight: 700,
    fontSize: 40,
    color: palette.white,
    background: `${palette.ink}e6`,
    border: `2px solid ${color}`,
    borderRadius: 12,
    padding: '10px 22px',
    whiteSpace: 'nowrap',
    transform: `translateY(${(1 - enter) * 16}px)`,
  };
  const placement: React.CSSProperties =
    side === 'right'
      ? {left: left + width + 40, top: top + height / 2 - 34}
      : side === 'left'
        ? {right: 1920 - left + 40, top: top + height / 2 - 34}
        : {left: left + width / 2 - 90, top: top + height + 36};
  return (
    <div style={{position: 'absolute', inset: 0, opacity}}>
      <div
        style={{
          position: 'absolute',
          left: left - 10,
          top: top - 10,
          width: width + 20,
          height: height + 20,
          borderRadius: 14,
          border: `3px solid ${color}`,
          boxShadow: `0 0 30px ${color}66`,
          transform: `scale(${pulse})`,
        }}
      />
      <div style={{...labelStyle, ...placement}}>{label}</div>
    </div>
  );
};

export const Color: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].color;
  const shot = (name: string) => `shots/${name}-${language}.png`;

  // ── A: 拍ごとにプロジェクトを切り替える（0–240）
  const order = ['hero', 'project1', 'project2', 'project3', 'project4', 'hero'];
  const switchIndex = Math.min(order.length - 1, Math.floor(frame / beats(1)));
  const projectIndex = [0, 1, 2, 3, 4, 0][switchIndex];
  const project = projects[projectIndex];
  const bump = interpolate(frame % beats(1), [0, 8], [1.012, 1], clamp);

  // ── B: 色の流れ先に寄る（240–480）
  const keysB: {frame: number; camera: Camera}[] = [
    {frame: beats(8), camera: WIDE},
    {frame: beats(9), camera: {x: 0.075, y: 0.16, zoom: 2.05}},
    {frame: beats(10), camera: {x: 0.075, y: 0.16, zoom: 2.05}},
    {frame: beats(11), camera: {x: 0.34, y: 0.11, zoom: 2.05}},
    {frame: beats(12), camera: {x: 0.34, y: 0.11, zoom: 2.05}},
    {frame: beats(13), camera: {x: 0.26, y: 0.53, zoom: 2.05}},
    {frame: beats(14), camera: {x: 0.26, y: 0.53, zoom: 2.05}},
    {frame: beats(15), camera: {x: 0.8, y: 0.7, zoom: 2.05}},
    {frame: beats(16), camera: {x: 0.8, y: 0.7, zoom: 2.3, opacity: 0}},
  ];

  // ── C: ⌘O スイッチャー（480–720）
  const keysC: {frame: number; camera: Camera}[] = [
    {frame: beats(16), camera: {x: 0.5, y: 0.45, zoom: 0.7, tiltX: 28, tiltY: -22, roll: 4, opacity: 0}},
    {frame: beats(16) + 10, camera: {x: 0.5, y: 0.45, zoom: 0.74, tiltX: 24, tiltY: -19, roll: 3.4, opacity: 1}},
    {frame: beats(20), camera: {x: 0.5, y: 0.3, zoom: 1.32, tiltX: 0, tiltY: 0, roll: 0}},
    {frame: beats(24), camera: {x: 0.5, y: 0.3, zoom: 1.4}},
  ];

  return (
    <AbsoluteFill style={{background: palette.ink}}>
      {frame < beats(8) ? (
        <>
          <div style={{position: 'absolute', inset: 0, transform: `scale(${bump})`}}>
            <Shot src={shot(order[switchIndex])} keys={[{frame: 0, camera: {...WIDE, zoom: 0.84}}, {frame: beats(8), camera: WIDE}]} />
          </div>
          <Caption text={text.title} at={6} out={beats(7.6)} />
          <div
            style={{
              position: 'absolute',
              right: 150,
              top: 52,
              display: 'flex',
              alignItems: 'center',
              gap: 14,
              fontFamily: fonts.mono,
              fontSize: 30,
              color: palette.white,
              opacity: interpolate(frame, [4, 14, beats(7.4), beats(7.8)], [0, 1, 1, 0], clamp),
            }}
          >
            <div style={{width: 18, height: 18, borderRadius: 9, background: project.color}} />
            {project.name}
            <span style={{color: palette.fg1}}>⎇ {project.branch}</span>
          </div>
        </>
      ) : null}

      {frame >= beats(8) && frame < beats(16) ? (
        <>
          <Shot src={shot('hero')} keys={keysB}>
            {(layout) => {
              const at = (target: {x: number; y: number}) => ({left: layout.left + target.x * layout.width, top: layout.top + target.y * layout.height});
              const unit = layout.width / 3360;
              const rail = at(TARGETS.rail);
              const tab = at(TARGETS.tab);
              const caret = at(TARGETS.caret);
              const chip = at(TARGETS.chip);
              return (
                <>
                  <FocusRing left={rail.left - 30 * unit} top={rail.top - 170 * unit} width={60 * unit} height={340 * unit} color={projects[0].color} at={beats(9)} label={text.labels.rail} side="right" />
                  <FocusRing left={tab.left - 170 * unit} top={tab.top - 8 * unit} width={340 * unit} height={70 * unit} color={projects[0].color} at={beats(11)} label={text.labels.tab} side="below" />
                  <FocusRing left={caret.left - 12 * unit} top={caret.top - 28 * unit} width={560 * unit} height={56 * unit} color={projects[0].color} at={beats(13)} label={text.labels.caret} side="below" />
                  <FocusRing left={chip.left - 380 * unit} top={chip.top - 26 * unit} width={760 * unit} height={52 * unit} color={threadColors[0]} at={beats(15)} label={text.labels.chip} side="left" />
                </>
              );
            }}
          </Shot>
          <Caption text={text.flow} at={beats(8) + 8} out={beats(15.6)} />
        </>
      ) : null}

      {frame >= beats(16) ? (
        <>
          <Shot src={shot('switcher')} keys={keysC} easing={Easing.out(Easing.cubic)} />
          <Caption text={text.glance} at={beats(17)} />
        </>
      ) : null}
    </AbsoluteFill>
  );
};
