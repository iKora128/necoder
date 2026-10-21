// 実端末を登録せず、実際のペアリング URL と同じ長さの QR を UI 撮影に渡す。
import QRCode from 'qrcode';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { random } from '../public/crypto.mjs';
const directory = await mkdtemp('/tmp/ncr-qr-');
const expires = Date.now() + 300000;
const url = 'https://control.necoder.com/#' + new URLSearchParams({ room: random(), token: random(), secret: random(), expires });
const qr = QRCode.create(url, { errorCorrectionLevel: 'M' });
// 「登録済みの端末」の見本: 接続中・未接続・この QR の読み取り待ち。
const now = Date.now();
const devices = [
  { id: random(), model: 'iPhone', status: 'online', paired_at: now - 3 * 86400_000, last_seen: now, projects: ['a', 'b'] },
  { id: random(), model: null, status: 'offline', paired_at: now - 20 * 86400_000, last_seen: now - 5 * 3600_000, projects: ['a'] },
  { id: new URLSearchParams(url.split('#')[1]).get('room'), model: null, status: 'waiting', paired_at: null, last_seen: null, projects: ['a', 'b'] },
];
await writeFile(`${directory}/dummy.json`, JSON.stringify({ url, expires, tasks: ['a', 'b'], size: qr.modules.size, modules: Array.from(qr.modules.data).join(''), devices }));
const child = spawn('../target/debug/examples/remote_preview', [], {
  env: { ...process.env, NECODER_QR_PREVIEW_JSON: `${directory}/dummy.json`, NECODER_QR_PREVIEW_PNG: `${directory}/settings.png` }, stdio: 'inherit',
});
const code = await new Promise(resolve => child.once('exit', resolve));
if (code !== 0) throw new Error('preview failed');
console.log(`${directory}/settings.png`);
