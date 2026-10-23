// QR（中身は https://necoder.com。接続用の本物ではない）。relay の qrcode で作ったモジュール配列を描く。
import React from 'react';
import qr from '../qr.json';

export const Qr: React.FC<{size: number; foreground?: string; background?: string}> = ({size, foreground = '#0b0c0f', background = '#f5f6f8'}) => {
  const quiet = 2;
  const cells = qr.size + quiet * 2;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${cells} ${cells}`} shapeRendering="crispEdges" style={{display: 'block', borderRadius: size * 0.04}}>
      <rect width={cells} height={cells} fill={background} />
      {Array.from(qr.modules).map((bit, index) =>
        bit === '1' ? <rect key={index} x={(index % qr.size) + quiet} y={Math.floor(index / qr.size) + quiet} width={1} height={1} fill={foreground} /> : null,
      )}
    </svg>
  );
};
