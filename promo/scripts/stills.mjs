// 指定フレームの静止画をまとめて書き出し、4 枚ずつの一覧（2×2）にする（見た目の確認用）。
//   node scripts/stills.mjs ja 400 1300 1500 2000
import {bundle} from '@remotion/bundler';
import {renderStill, selectComposition} from '@remotion/renderer';
import {execFileSync} from 'node:child_process';
import {mkdirSync} from 'node:fs';
import path from 'node:path';

const [language = 'ja', ...frames] = process.argv.slice(2);
const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const outputDirectory = path.join(root, 'out', 'stills');
mkdirSync(outputDirectory, {recursive: true});
const serveUrl = await bundle({entryPoint: path.join(root, 'src', 'index.ts'), publicDir: path.join(root, 'public')});
const inputProps = {language};
const composition = await selectComposition({serveUrl, id: 'NecoderPromo', inputProps});
const files = [];
for (const frame of frames.map(Number)) {
  const output = path.join(outputDirectory, `${language}-${String(frame).padStart(5, '0')}.png`);
  await renderStill({composition, serveUrl, output, frame, inputProps, scale: 0.5});
  files.push(output);
}
for (let index = 0; index < files.length; index += 4) {
  const group = files.slice(index, index + 4);
  while (group.length < 4) group.push(group[group.length - 1]);
  const sheet = path.join(outputDirectory, `sheet-${language}-${frames[index]}.png`);
  execFileSync('ffmpeg', ['-y', '-loglevel', 'error', ...group.flatMap((file) => ['-i', file]), '-filter_complex', '[0][1]hstack[top];[2][3]hstack[bottom];[top][bottom]vstack', sheet]);
  console.log(sheet);
}
