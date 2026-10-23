// 48–56 秒: かゆいところに手が届く（ブレイク）。⌘⇧V で HTML / Markdown をその場でプレビュー → Chat モード。
// 画面は実機（HTML のプレビュー面だけは、offscreen では WebView が描かれないので同じ HTML を描いてはめ込んだ）。
import React from 'react';
import {AbsoluteFill, Easing, interpolate, Sequence, useCurrentFrame} from 'remotion';
import {Shot} from '../components/Shot';
import {Fade, Rise} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette} from '../theme';
import {beats} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

/** 左に見出し、右に実画面。左端は地の色へ溶かして見出しを読ませる。 */
const FeatureTitle: React.FC<{kicker: string; title: string; sub: string; at: number}> = ({kicker, title, sub, at}) => (
  <div style={{position: 'absolute', left: 110, top: 250, width: 640}}>
    <Fade at={at} style={{fontFamily: fonts.japanese, fontSize: 24, letterSpacing: '0.12em', color: palette.fg1, marginBottom: 22}}>
      {kicker}
    </Fade>
    <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 62, lineHeight: 1.3, color: palette.white}}>
      <Rise text={title} at={at + 4} />
    </div>
    <Fade at={at + 24} style={{fontFamily: fonts.japanese, fontSize: 28, lineHeight: 1.6, color: palette.fg1, marginTop: 26}}>
      {sub}
    </Fade>
  </div>
);

const LeftScrim: React.FC = () => (
  <AbsoluteFill style={{background: `linear-gradient(to right, ${palette.ink} 0%, ${palette.ink} 33%, ${palette.ink}00 52%)`, pointerEvents: 'none'}} />
);

const Key: React.FC<{label: string; pressed: number}> = ({label, pressed}) => (
  <div
    style={{
      minWidth: 64,
      height: 64,
      padding: '0 14px',
      borderRadius: 12,
      background: pressed > 0 ? palette.bg3 : palette.bg2,
      border: `1px solid ${pressed > 0 ? palette.fg1 : palette.border}`,
      boxShadow: `0 ${6 - 4 * pressed}px 0 #0a0b0e`,
      transform: `translateY(${4 * pressed}px)`,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      fontFamily: fonts.display,
      fontWeight: 600,
      fontSize: 30,
      color: palette.white,
      boxSizing: 'border-box',
    }}
  >
    {label}
  </div>
);

const Preview: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].touches;
  // Markdown: 30 で ⌘⇧V → プレビュー / HTML: 150 で ⌘⇧V → プレビュー
  const presses = [30, 150];
  const pressed = Math.max(...presses.map((at) => interpolate(frame, [at - 3, at, at + 8], [0, 1, 0], clamp)));
  const isHtml = frame >= 120;
  const flip = interpolate(frame, isHtml ? [150, 158] : [30, 38], [0, 1], clamp);
  const kind = isHtml ? 'html' : 'md';
  const cut = interpolate(frame, [120, 126], [0, 1], clamp);
  const keys = [
    {frame: 0, camera: {x: 0.37, y: 0.3, zoom: 1.4, tiltY: -8}},
    {frame: beats(8), camera: {x: 0.38, y: 0.29, zoom: 1.52, tiltY: -3}},
  ];
  return (
    <AbsoluteFill>
      <div style={{position: 'absolute', inset: 0, opacity: isHtml ? cut : 1}}>
        <Shot src={`shots/${kind}-src-${language}.png`} keys={keys} centerX={1300} />
        <div style={{position: 'absolute', inset: 0, opacity: flip}}>
          <Shot src={`shots/${kind}-${language}.png`} keys={keys} centerX={1300} />
        </div>
      </div>
      <LeftScrim />
      <FeatureTitle kicker={text.kicker} title={text.preview} sub={text.previewSub} at={0} />
      <div style={{position: 'absolute', left: 110, top: 740, display: 'flex', gap: 12, opacity: interpolate(frame, [12, 22], [0, 1], clamp)}}>
        <Key label="⌘" pressed={pressed} />
        <Key label="⇧" pressed={pressed} />
        <Key label="V" pressed={pressed} />
      </div>
    </AbsoluteFill>
  );
};

const Chat: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].touches;
  const enter = interpolate(frame, [0, 10], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  return (
    <AbsoluteFill style={{opacity: enter}}>
      <Shot
        src={`shots/chat-${language}.png`}
        keys={[
          {frame: 0, camera: {x: 0.5, y: 0.5, zoom: 1.0, tiltY: -8}},
          {frame: beats(7.5), camera: {x: 0.7, y: 0.42, zoom: 1.38, tiltY: -2}},
        ]}
        centerX={1300}
      />
      <LeftScrim />
      <FeatureTitle kicker={text.kicker} title={text.chat} sub={text.chatSub} at={0} />
    </AbsoluteFill>
  );
};

export const Touches: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  // ドロップ直前の 8 分は無音に合わせて真っ暗にする
  if (frame >= beats(15.5)) {
    return <AbsoluteFill style={{background: palette.ink}} />;
  }
  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <Sequence durationInFrames={beats(8)}>
        <Preview language={language} />
      </Sequence>
      <Sequence from={beats(8)} durationInFrames={beats(8)}>
        <Chat language={language} />
      </Sequence>
    </AbsoluteFill>
  );
};
