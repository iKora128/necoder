// 72–82 秒: 締め。nyaco が万歳し、「necoder は necoder の中で作っている」→ 次のエディタへ → エンドカード。
import React from 'react';
import {AbsoluteFill, Easing, Img, interpolate, spring, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';
import {Mascot} from '../components/Mascot';
import {Fade, Rise} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette, projectColors} from '../theme';
import {bars, beats, cues, seconds} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const NYA_AT = seconds(cues.nya ?? 78.6) - bars(36);

export const Outro: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].outro;
  const flash = interpolate(frame, [0, 10], [0.7, 0], clamp);
  const cardAt = beats(8);
  const firstOut = interpolate(frame, [cardAt - 12, cardAt], [1, 0], clamp);
  const mascotIn = spring({frame, fps, config: {damping: 14, stiffness: 160}});
  const cardIn = spring({frame: frame - cardAt, fps, config: {damping: 200, stiffness: 110}});
  const hop = interpolate(frame, [NYA_AT - 4, NYA_AT + 4, NYA_AT + 14], [0, -26, 0], {...clamp, easing: Easing.out(Easing.quad)});
  const fadeOut = interpolate(frame, [beats(17), beats(20)], [0, 1], clamp);

  return (
    <AbsoluteFill style={{background: palette.ink}}>
      {/* 前半: nyaco と 2 つの言葉 */}
      {frame < cardAt ? (
        <AbsoluteFill style={{opacity: firstOut}}>
          <div style={{position: 'absolute', left: 170, top: 250, transform: `translateY(${(1 - mascotIn) * 80}px)`, opacity: mascotIn}}>
            <Mascot kind="celebrate" scale={7} framesPerSecond={9} />
          </div>
          <div style={{position: 'absolute', left: 760, top: 300, width: 1060}}>
            <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 70, lineHeight: 1.3, color: palette.white}}>
              <Rise text={text.inside} at={6} out={beats(3.7)} />
            </div>
            <Fade at={beats(1.2)} out={beats(3.7)} style={{fontFamily: fonts.japanese, fontSize: 36, color: palette.fg1, marginTop: 20}}>
              {text.insideSub}
            </Fade>
          </div>
          <div style={{position: 'absolute', left: 760, top: 350, width: 1060}}>
            <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 78, lineHeight: 1.3, color: palette.white}}>
              <Rise text={text.next} at={beats(4)} />
            </div>
          </div>
        </AbsoluteFill>
      ) : null}

      {/* エンドカード */}
      {frame >= cardAt ? (
        <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center', opacity: cardIn * (1 - fadeOut)}}>
          <div style={{display: 'flex', flexDirection: 'column', alignItems: 'center', transform: `translateY(${(1 - cardIn) * 30}px)`}}>
            <div style={{display: 'flex', alignItems: 'center', gap: 36}}>
              <Img src={staticFile('brand/necoder-mark.png')} style={{width: 150, height: 150, borderRadius: 34}} />
              <div style={{fontFamily: fonts.display, fontWeight: 800, fontSize: 170, color: palette.white, letterSpacing: '-0.03em', lineHeight: 1}}>necoder</div>
            </div>
            <div style={{display: 'flex', gap: 10, marginTop: 30}}>
              {projectColors.map((color) => (
                <div key={color} style={{width: 104, height: 7, borderRadius: 4, background: color}} />
              ))}
            </div>
            <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 52, color: palette.white, marginTop: 40}}>{copy[language].logo.tagline}</div>
            <div style={{fontFamily: fonts.mono, fontSize: 36, color: palette.fg0, marginTop: 34, display: 'flex', gap: 40}}>
              <span>necoder.com</span>
              <span style={{color: palette.fg2}}>·</span>
              <span>github.com/iKora128/necoder</span>
            </div>
            <div style={{fontFamily: fonts.japanese, fontSize: 28, color: palette.fg1, marginTop: 22}}>{text.meta}</div>
          </div>
          <div style={{position: 'absolute', right: 150, bottom: 150, transform: `translateY(${hop}px)`}}>
            <Mascot kind={frame >= NYA_AT - 4 && frame < NYA_AT + 40 ? 'celebrate' : 'typing'} scale={2.4} />
          </div>
          <div style={{position: 'absolute', bottom: 56, width: '100%', textAlign: 'center', fontFamily: fonts.japanese, fontSize: 20, color: palette.fg2}}>{text.credit}</div>
        </AbsoluteFill>
      ) : null}

      <AbsoluteFill style={{background: palette.white, opacity: flash, pointerEvents: 'none'}} />
    </AbsoluteFill>
  );
};
