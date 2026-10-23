// 56–72 秒: ドロップ 2。スマホ連携 → Remote SSH → Rust × GPUI → エディタの基本も全部入り。
// スマホの画面は relay/test/preview.mjs と同じ隔離（wrangler dev :8791 + 偽 GUI）で撮った実物の PWA。
// Mac 側の「スマホ連携」カードは locales の settings.remote_* の文言で組み直し、QR の中身は necoder.com（接続鍵ではない）。
import React from 'react';
import {AbsoluteFill, Easing, Img, interpolate, Sequence, spring, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';
import {Phone, Tap} from '../components/Phone';
import {PulseGrid} from '../components/PulseGrid';
import {Qr} from '../components/Qr';
import {Fade, Rise, Slam} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette} from '../theme';
import {beats, STEP} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const SideTitle: React.FC<{title: string; sub: string; at: number}> = ({title, sub, at}) => (
  <div style={{position: 'absolute', left: 110, top: 150, width: 700}}>
    <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 60, lineHeight: 1.28, color: palette.white}}>
      <Rise text={title} at={at} />
    </div>
    <Fade at={at + 20} style={{fontFamily: fonts.japanese, fontSize: 30, lineHeight: 1.6, color: palette.fg1, marginTop: 24}}>
      {sub}
    </Fade>
  </div>
);

const Points: React.FC<{items: string[]; at: number[]; top: number}> = ({items, at, top}) => (
  <div style={{position: 'absolute', left: 110, top, display: 'flex', flexDirection: 'column', gap: 20}}>
    {items.map((item, index) => (
      <Fade key={item} at={at[index]} rise={12} duration={12} style={{display: 'flex', alignItems: 'center', gap: 18, fontFamily: fonts.japanese, fontSize: 32, color: palette.fg0}}>
        <span style={{width: 36, height: 36, borderRadius: 18, border: `2px solid ${palette.fg1}`, display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 20, color: palette.white}}>✓</span>
        {item}
      </Fade>
    ))}
  </div>
);

// ── 1. スマホ連携（0–360）─────────────────────────────────────────────
const MAC_CARD = {
  ja: {heading: 'スマホ連携', hint: '3 プロジェクトを共有。QRは5分間有効です。', copy: 'ペアリングURLをコピー', hide: 'QRを隠す'},
  en: {heading: 'Phone connection', hint: 'Sharing 3 projects. Valid for 5 minutes.', copy: 'Copy pairing URL', hide: 'Hide QR'},
} as const;

const SCREENS = [
  {from: 0, name: '0-welcome'},
  {from: 92, name: '1-home'},
  {from: 186, name: '2-thread-0'},
  {from: 270, name: '2-thread-0-checked'},
  {from: 300, name: '2-thread-0-approved'},
];

const PhoneScene: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].phone;
  const card = MAC_CARD[language];
  const cardIn = spring({frame, fps, config: {damping: 200, stiffness: 150}});
  const phoneIn = spring({frame: frame - 6, fps, config: {damping: 18, stiffness: 150, mass: 0.8}});
  const scanning = frame >= 68 && frame < 92;
  const recognized = interpolate(frame, [86, 90, 100], [0, 1, 0], clamp);
  const screen = [...SCREENS].reverse().find((entry) => frame >= entry.from) ?? SCREENS[0];
  const screenAge = frame - screen.from;
  const screenIn = interpolate(screenAge, [0, 6], [0.4, 1], clamp);
  const scale = 1.0;
  return (
    <AbsoluteFill>
      <SideTitle title={text.title} sub={text.sub} at={0} />
      <Points items={text.points} at={[30, 90, 150, 210]} top={560} />

      {/* Mac 側: 設定のスマホ連携（QR を発行したところ）*/}
      <div
        style={{
          position: 'absolute',
          left: 880,
          top: 250,
          width: 440,
          borderRadius: 18,
          background: palette.bg0,
          border: `1px solid ${palette.border}`,
          boxShadow: '0 40px 100px rgba(0,0,0,0.6)',
          padding: '28px 30px 26px',
          boxSizing: 'border-box',
          opacity: cardIn,
          transform: `translateY(${(1 - cardIn) * 40}px)`,
        }}
      >
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 26, color: palette.white, marginBottom: 20}}>{card.heading}</div>
        <div style={{display: 'flex', justifyContent: 'center'}}>
          <Qr size={300} />
        </div>
        <div style={{fontFamily: fonts.japanese, fontSize: 19, lineHeight: 1.6, color: palette.fg1, marginTop: 18}}>{card.hint}</div>
        <div style={{display: 'flex', gap: 10, marginTop: 16}}>
          {[card.copy, card.hide].map((label) => (
            <div key={label} style={{fontFamily: fonts.japanese, fontSize: 18, color: palette.fg0, border: `1px solid ${palette.border}`, borderRadius: 8, padding: '8px 14px'}}>
              {label}
            </div>
          ))}
        </div>
      </div>

      {/* スマホ（実物の PWA）*/}
      <div style={{position: 'absolute', left: 1400, top: 104, opacity: phoneIn, transform: `translateY(${(1 - phoneIn) * 120}px)`}}>
        <Phone scale={scale}>
          {scanning ? (
            <AbsoluteFill style={{background: '#050505', alignItems: 'center', justifyContent: 'center'}}>
              <div style={{position: 'relative', width: 280, height: 280, transform: 'rotate(-4deg)'}}>
                <div style={{position: 'absolute', left: 40, top: 40, filter: 'blur(0.6px)'}}>
                  <Qr size={200} />
                </div>
                {[0, 1, 2, 3].map((corner) => (
                  <div
                    key={corner}
                    style={{
                      position: 'absolute',
                      width: 56,
                      height: 56,
                      left: corner % 2 === 0 ? 0 : undefined,
                      right: corner % 2 === 1 ? 0 : undefined,
                      top: corner < 2 ? 0 : undefined,
                      bottom: corner >= 2 ? 0 : undefined,
                      borderColor: palette.white,
                      borderStyle: 'solid',
                      borderWidth: `${corner < 2 ? 5 : 0}px ${corner % 2 === 1 ? 5 : 0}px ${corner >= 2 ? 5 : 0}px ${corner % 2 === 0 ? 5 : 0}px`,
                      borderRadius: 10,
                    }}
                  />
                ))}
                <div style={{position: 'absolute', left: 20, right: 20, top: 30 + ((frame - 68) % 20) * 11, height: 3, background: 'rgba(255,255,255,0.8)', boxShadow: '0 0 16px rgba(255,255,255,0.8)'}} />
              </div>
            </AbsoluteFill>
          ) : (
            <Img src={staticFile(`phone/${language}-${screen.name}.png`)} style={{width: '100%', height: '100%', display: 'block', opacity: screenIn}} />
          )}
          <AbsoluteFill style={{background: palette.white, opacity: recognized * 0.8, pointerEvents: 'none'}} />
          <Tap x={195} y={498} at={60} frame={frame} scale={scale} />
          <Tap x={150} y={162} at={176} frame={frame} scale={scale} />
          <Tap x={40} y={579} at={262} frame={frame} scale={scale} />
          <Tap x={280} y={640} at={292} frame={frame} scale={scale} />
        </Phone>
      </div>
    </AbsoluteFill>
  );
};

// ── 2. Remote SSH（360–600）───────────────────────────────────────────
const Ssh: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].ssh;
  const nodeIn = (at: number) => interpolate(frame, [at, at + 12], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const line = interpolate(frame, [14, 34], [0, 1], {...clamp, easing: Easing.inOut(Easing.cubic)});
  const left = {x: 1060, y: 560};
  const right = {x: 1660, y: 560};
  const node: React.CSSProperties = {
    position: 'absolute',
    width: 300,
    height: 210,
    marginLeft: -150,
    marginTop: -105,
    borderRadius: 22,
    background: palette.bg0,
    border: `1px solid ${palette.border}`,
    boxShadow: '0 30px 80px rgba(0,0,0,0.55)',
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 12,
  };
  return (
    <AbsoluteFill>
      <SideTitle title={text.title} sub={text.sub} at={0} />
      <Points items={text.points} at={[24, 44, 64, 84]} top={560} />
      <svg width={1920} height={1080} style={{position: 'absolute', inset: 0}}>
        <line x1={left.x + 150} y1={left.y} x2={left.x + 150 + (right.x - left.x - 300) * line} y2={right.y} stroke="rgba(255,255,255,0.45)" strokeWidth={3} strokeDasharray="10 10" strokeDashoffset={-frame * 1.5} />
      </svg>
      {[0, 1, 2].map((packet) => {
        const progress = ((frame - 34) / 40 + packet / 3) % 1;
        return frame > 34 ? (
          <div
            key={packet}
            style={{
              position: 'absolute',
              left: left.x + 150 + (right.x - left.x - 300) * progress - 7,
              top: left.y - 7,
              width: 14,
              height: 14,
              borderRadius: 7,
              background: palette.white,
              boxShadow: '0 0 16px rgba(255,255,255,0.8)',
            }}
          />
        ) : null;
      })}
      <div style={{...node, left: left.x, top: left.y, opacity: nodeIn(0), transform: `scale(${0.9 + 0.1 * nodeIn(0)})`}}>
        <Img src={staticFile('brand/necoder-mark.png')} style={{width: 84, height: 84, borderRadius: 20}} />
        <div style={{fontFamily: fonts.display, fontWeight: 700, fontSize: 30, color: palette.white}}>necoder</div>
        <div style={{fontFamily: fonts.mono, fontSize: 20, color: palette.fg1}}>Mac · Windows</div>
      </div>
      <div style={{...node, left: right.x, top: right.y, opacity: nodeIn(8), transform: `scale(${0.9 + 0.1 * nodeIn(8)})`}}>
        <div
          style={{
            width: 84,
            height: 84,
            background: palette.fg0,
            WebkitMaskImage: `url(${staticFile('icons/server.svg')})`,
            maskImage: `url(${staticFile('icons/server.svg')})`,
            WebkitMaskSize: 'contain',
            maskSize: 'contain',
            WebkitMaskRepeat: 'no-repeat',
            maskRepeat: 'no-repeat',
          }}
        />
        <div style={{fontFamily: fonts.mono, fontWeight: 700, fontSize: 30, color: palette.white}}>gpu-box</div>
        <div style={{fontFamily: fonts.mono, fontSize: 20, color: palette.fg1}}>Linux</div>
      </div>
      <Fade at={30} style={{position: 'absolute', left: left.x + 150, width: right.x - left.x - 300, top: left.y - 70, textAlign: 'center', fontFamily: fonts.mono, fontSize: 26, color: palette.fg0}}>
        ssh://gpu-box/~/work
      </Fade>
      <Fade at={44} style={{position: 'absolute', left: right.x - 170, width: 340, top: right.y + 130, textAlign: 'center'}}>
        <span style={{fontFamily: fonts.mono, fontSize: 24, color: palette.white, border: `1px solid ${palette.border}`, background: palette.bg0, borderRadius: 999, padding: '10px 22px'}}>necoder-server · 2.4MB</span>
      </Fade>
    </AbsoluteFill>
  );
};

// ── 3. Rust × GPUI（600–840）──────────────────────────────────────────
const Performance: React.FC<{language: Language}> = ({language}) => {
  const text = copy[language].power;
  return (
    <AbsoluteFill>
      <PulseGrid />
      <AbsoluteFill style={{alignItems: 'center'}}>
        <div style={{position: 'absolute', top: 170, width: '100%', display: 'flex', justifyContent: 'center'}}>
          <Slam at={0} style={{textAlign: 'center'}}>
            <div style={{fontFamily: fonts.display, fontWeight: 900, fontSize: 210, color: palette.white, letterSpacing: '-0.03em', lineHeight: 1}}>
              Rust <span style={{fontWeight: 300, color: palette.fg1}}>×</span> GPUI
            </div>
          </Slam>
        </div>
        <Fade at={12} style={{position: 'absolute', top: 420, width: '100%', textAlign: 'center', fontFamily: fonts.japanese, fontSize: 38, color: palette.fg1}}>
          {text.sub}
        </Fade>
        <div style={{position: 'absolute', top: 560, width: '100%', display: 'flex', justifyContent: 'center', gap: 110}}>
          {text.items.map((item, index) => {
            const at = beats(1) + index * beats(1);
            return (
              <Slam key={item.value} at={at} from={1.25} style={{textAlign: 'center', minWidth: 380}}>
                <div style={{fontFamily: fonts.display, fontWeight: 800, fontSize: 124, color: palette.white, letterSpacing: '-0.02em', lineHeight: 1.1}}>{item.value}</div>
                <div style={{fontFamily: fonts.japanese, fontSize: 30, color: palette.fg1, marginTop: 14}}>{item.label}</div>
              </Slam>
            );
          })}
        </div>
      </AbsoluteFill>
    </AbsoluteFill>
  );
};

// ── 4. エディタの基本も全部入り（840–960）────────────────────────────
const Extras: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].extras;
  return (
    <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center'}}>
      <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 70, color: palette.white, marginBottom: 56}}>
        <Rise text={text.title} at={0} />
      </div>
      <div style={{display: 'flex', flexWrap: 'wrap', justifyContent: 'center', gap: 18, width: 1500}}>
        {text.chips.map((chip, index) => {
          const at = 4 + Math.round(index * STEP * 0.5);
          const enter = spring({frame: frame - at, fps, config: {damping: 13, stiffness: 260, mass: 0.5}});
          return (
            <div
              key={chip}
              style={{
                fontFamily: /^[\x00-\x7F]+$/.test(chip) ? fonts.mono : fonts.japanese,
                fontSize: 32,
                color: palette.fg0,
                padding: '14px 28px',
                borderRadius: 999,
                border: `1px solid ${palette.border}`,
                background: palette.bg0,
                opacity: frame >= at ? 1 : 0,
                transform: `scale(${enter}) translateY(${(1 - enter) * 20}px)`,
              }}
            >
              {chip}
            </div>
          );
        })}
      </div>
    </AbsoluteFill>
  );
};

export const Power: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const fill = interpolate(frame, [beats(31.5), beats(32)], [0, 0.6], clamp);
  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <AbsoluteFill style={{opacity: 1 - fill}}>
        <Sequence durationInFrames={beats(12)}>
          <PhoneScene language={language} />
        </Sequence>
        <Sequence from={beats(12)} durationInFrames={beats(8)}>
          <Ssh language={language} />
        </Sequence>
        <Sequence from={beats(20)} durationInFrames={beats(8)}>
          <Performance language={language} />
        </Sequence>
        <Sequence from={beats(28)} durationInFrames={beats(4)}>
          <Extras language={language} />
        </Sequence>
      </AbsoluteFill>
    </AbsoluteFill>
  );
};
