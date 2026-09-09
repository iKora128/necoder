// 秘密をコマンド引数やログへ出さず、Mac の保護ファイルと Worker secret にだけ設定する。
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { hostname } from 'node:os';
import { fileURLToPath } from 'node:url';
import webpush from 'web-push';
import { random } from '../public/crypto.mjs';
import { readState, writeState } from '../host/storage.mjs';
const origin = 'https://control.necoder.com';
const existing = await readState('config');
if (existing && existing.origin !== origin) throw new Error('Existing remote configuration uses another origin; left unchanged.');
if (!existing?.provisionToken) {
  const { stdout } = await promisify(execFile)(process.execPath, ['node_modules/wrangler/bin/wrangler.js', 'secret', 'list'], {
    cwd: fileURLToPath(new URL('..', import.meta.url)), timeout: 30000,
  });
  if (JSON.parse(stdout).some(secret => secret.name === 'PROVISION_TOKEN'))
    throw new Error('Relay already provisioned. On another computer, use init with the existing NECODER_PROVISION_TOKEN; refusing to rotate other hosts out.');
}
const config = existing || { origin, name: hostname(), vapid: webpush.generateVAPIDKeys() };
config.provisionToken ||= random();
config.adminToken ||= random();
await writeState('config', config);
const child = spawn(process.execPath, ['node_modules/wrangler/bin/wrangler.js', 'secret', 'put', 'PROVISION_TOKEN'], {
  cwd: fileURLToPath(new URL('..', import.meta.url)), stdio: ['pipe', 'inherit', 'inherit'],
});
child.stdin.end(config.provisionToken + '\n');
const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
if (code !== 0) throw new Error('Cloudflare secret update failed; local secret preserved for retry.');
console.log('Provisioning configured. Existing device/VAPID keys preserved. No pairing created.');
