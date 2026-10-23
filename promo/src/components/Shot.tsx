// 実機スクリーンショットをカメラで見せる。ズームは transform ではなく実寸で描き直す
// （transform で拡大すると Chrome が縮小済みのラスタを引き伸ばしてぼける）。
import React from 'react';
import {Easing, Img, interpolate, staticFile, useCurrentFrame} from 'remotion';
import {palette} from '../theme';

export type Camera = {x: number; y: number; zoom: number; tiltX?: number; tiltY?: number; roll?: number; opacity?: number};

const SHOT_ASPECT = 2000 / 3360;

const mix = (from: number, to: number, progress: number) => from + (to - from) * progress;

export const cameraAt = (keys: {frame: number; camera: Camera}[], frame: number, easing = Easing.inOut(Easing.cubic)): Camera => {
  if (frame <= keys[0].frame) {
    return keys[0].camera;
  }
  for (let index = 0; index < keys.length - 1; index++) {
    const start = keys[index];
    const end = keys[index + 1];
    if (frame <= end.frame) {
      const progress = easing(interpolate(frame, [start.frame, end.frame], [0, 1]));
      return {
        x: mix(start.camera.x, end.camera.x, progress),
        y: mix(start.camera.y, end.camera.y, progress),
        zoom: mix(start.camera.zoom, end.camera.zoom, progress),
        tiltX: mix(start.camera.tiltX ?? 0, end.camera.tiltX ?? 0, progress),
        tiltY: mix(start.camera.tiltY ?? 0, end.camera.tiltY ?? 0, progress),
        roll: mix(start.camera.roll ?? 0, end.camera.roll ?? 0, progress),
        opacity: mix(start.camera.opacity ?? 1, end.camera.opacity ?? 1, progress),
      };
    }
  }
  return keys[keys.length - 1].camera;
};

/**
 * `camera.x / y` は画像内の注視点（0〜1）、`zoom` は画面幅 1600px を 1 とした倍率。
 * 注視点が画面中央（960, 540）に来るように置く。
 */
export const Shot: React.FC<{
  src: string;
  keys: {frame: number; camera: Camera}[];
  baseWidth?: number;
  easing?: (value: number) => number;
  /** 注視点を置く画面上の位置（既定は画面中央）。見出しを左に置く場面で右へ寄せる。 */
  centerX?: number;
  centerY?: number;
  children?: (layout: {left: number; top: number; width: number; height: number}) => React.ReactNode;
}> = ({src, keys, baseWidth = 1600, easing, centerX = 960, centerY = 540, children}) => {
  const frame = useCurrentFrame();
  const camera = cameraAt(keys, frame, easing);
  const width = baseWidth * camera.zoom;
  const height = width * SHOT_ASPECT;
  const left = centerX - camera.x * width;
  const top = centerY - camera.y * height;
  return (
    <div style={{position: 'absolute', inset: 0, perspective: 2400, opacity: camera.opacity ?? 1}}>
      <div
        style={{
          position: 'absolute',
          inset: 0,
          transformStyle: 'preserve-3d',
          transformOrigin: `${centerX}px ${centerY}px`,
          transform: `rotateX(${camera.tiltX ?? 0}deg) rotateY(${camera.tiltY ?? 0}deg) rotateZ(${camera.roll ?? 0}deg)`,
        }}
      >
        <div
          style={{
            position: 'absolute',
            left,
            top,
            width,
            height,
            borderRadius: 10 * camera.zoom,
            overflow: 'hidden',
            boxShadow: `0 ${40 * camera.zoom}px ${120 * camera.zoom}px rgba(0,0,0,0.65), 0 0 0 1px ${palette.border}`,
          }}
        >
          <Img src={staticFile(src)} style={{width: '100%', height: '100%', display: 'block'}} />
        </div>
        {children ? children({left, top, width, height}) : null}
      </div>
    </div>
  );
};
