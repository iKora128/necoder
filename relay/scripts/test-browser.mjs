// ブラウザの成功だけでなく、ローカル Worker の未処理例外も失敗にする。
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const cli = fileURLToPath(new URL('../node_modules/@playwright/test/cli.js', import.meta.url));
const child = spawn(process.execPath, [cli, 'test', ...process.argv.slice(2)], { stdio: ['inherit', 'pipe', 'pipe'] });
let exception = false;
for (const [stream, destination] of [[child.stdout, process.stdout], [child.stderr, process.stderr]]) {
  let tail = '';
  stream.on('data', chunk => {
    const text = tail + chunk.toString();
    if (text.includes('Uncaught')) exception = true;
    tail = text.slice(-100);
    destination.write(chunk);
  });
}
child.on('error', error => { console.error(error); process.exitCode = 1; });
child.on('close', (code, signal) => {
  if (exception) console.error('Local Worker raised an uncaught exception.');
  process.exitCode = code || (signal || exception ? 1 : 0);
});
