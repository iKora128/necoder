#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { mkdir, chmod, open, unlink } from 'node:fs/promises';
import { hostname } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';
import { timingSafeEqual } from 'node:crypto';
import QRCode from 'qrcode';
import webpush from 'web-push';
import { Bridge } from './bridge.mjs';
import { ipc } from './ipc.mjs';
import { stateDir, adminSocket, readState, writeState, secureDirectory } from './storage.mjs';
import { hasLid } from './platform.mjs';
import { random, validSecret } from '../public/crypto.mjs';

const [command = 'help', ...args] = process.argv.slice(2);
async function admin(method, params = {}) {
  const config = await readState('config');
  return ipc(adminSocket, method, params, 12000, config?.adminToken);
}
async function serve() {
  const config = await readState('config');
  if (!config) throw new Error('run_init_first: init を実行してください');
  if (!config.adminToken) { config.adminToken = random(); await writeState('config', config); }
  const origin = new URL(config.origin);
  if (origin.protocol !== 'https:' && !(origin.protocol === 'http:' && ['localhost', '127.0.0.1'].includes(origin.hostname))) throw new Error('HTTPS origin required');
  try { await admin('status'); throw new Error('Host already running'); }
  catch (error) { if (!['ENOENT', 'ECONNREFUSED'].includes(error.code)) throw error; }
  await secureDirectory();
  if (process.platform !== 'win32') await unlink(adminSocket).catch(error => { if (error.code !== 'ENOENT') throw error; });
  const bridge = new Bridge(config, await readState('devices', []));
  let jobs = Promise.resolve();
  const server = net.createServer(socket => {
    socket.setEncoding('utf8'); socket.setTimeout(15_000, () => socket.destroy());
    socket.on('error', () => {});
    let body = '', handled = false;
    socket.on('data', chunk => {
      if (handled) return;
      body += chunk;
      if (body.length > 8192) { socket.destroy(); return; }
      if (!body.includes('\n')) return;
      handled = true;
      jobs = jobs.then(async () => {
        try {
          const { method, params = {}, auth } = JSON.parse(body.slice(0, body.indexOf('\n')));
          if (!validSecret(auth) || !timingSafeEqual(Buffer.from(auth), Buffer.from(config.adminToken))) throw new Error('local_auth_required');
          let result;
          if (method === 'status') result = { pid: process.pid, online: !!bridge.snapshot, origin: config.origin,
            keeps_awake: bridge.keepsAwake(), on_ac_power: bridge.onAcPower, sleeps_on_lid_close: hasLid(),
            devices: bridge.devices.filter(d => !d.revoked).map(d => ({ id: d.room, name: d.name, paired: d.confirmed, projects: d.tasks })) };
          else if (method === 'devices') result = { running: true, devices: bridge.describe() };
          else if (method === 'pair') { await bridge.poll(); result = await bridge.pair(params.name, params.tasks); }
          else if (method === 'revoke') result = await bridge.revoke(params.id);
          else if (method === 'stop') { result = { stopped: true }; setTimeout(shutdown, 100); }
          else throw new Error('unknown_command');
          socket.end(JSON.stringify({ ok: true, result }) + '\n');
        } catch (error) { socket.end(JSON.stringify({ ok: false, error: error.message }) + '\n'); }
      }).catch(error => console.error('Local control:', error.message));
    });
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(adminSocket, resolve); });
  if (process.platform !== 'win32') await chmod(adminSocket, 0o600);
  let shuttingDown = false;
  function shutdown() {
    if (shuttingDown) return; shuttingDown = true;
    bridge.stop(); server.close();
    setTimeout(() => process.exit(0), 500).unref();
  }
  process.once('SIGTERM', shutdown); process.once('SIGINT', shutdown);
  await bridge.start();
  console.log('necoder Remote host started:', config.origin);
}
/// ホストが止まっているか（admin socket が無い・応答しない）。
const hostStopped = error => ['ENOENT', 'ECONNREFUSED'].includes(error.code);
/// 止まっているホストの代わりに、保存済みの端末表を直接扱う Bridge（接続も wake lock も張らない）。
async function offlineBridge(config) {
  const bridge = new Bridge(config, await readState('devices', []));
  bridge.stopped = true;
  return bridge;
}
/// 設定画面の端末一覧。ホストが止まっていても登録は残っているので、保存済みの表から出す。
async function listDevices() {
  try { return await admin('devices'); } catch (error) { if (!hostStopped(error)) throw error; }
  const config = await readState('config');
  if (!config) return { running: false, devices: [] };
  return { running: false, devices: (await offlineBridge(config)).describe() };
}
/// 失効はホストが止まっていても行う（次に起動したホストはこの端末を認証しない）。
async function revokeDevice(id) {
  if (!validSecret(id)) throw new Error('device_not_found');
  try { return await admin('revoke', { id }); } catch (error) { if (!hostStopped(error)) throw error; }
  const config = await readState('config');
  if (!config) throw new Error('device_not_found');
  return (await offlineBridge(config)).revoke(id);
}
async function ensureStarted() {
  try { return await admin('status'); } catch (error) {
    if (!['ENOENT', 'ECONNREFUSED'].includes(error.code)) throw error;
  }
  if (!await readState('config')) throw new Error('run_init_first: init を実行してください');
  await secureDirectory();
  const log = await open(path.join(stateDir, 'host.log'), 'a', 0o600);
  const child = spawn(process.execPath, [fileURLToPath(import.meta.url), 'serve'], {
    detached: true, windowsHide: true, stdio: ['ignore', log.fd, log.fd], env: process.env,
  });
  child.unref(); await log.close();
  for (let tries = 0; tries < 40; tries++) {
    await new Promise(resolve => setTimeout(resolve, 150));
    try { return await admin('status'); } catch { /* 起動待ち。 */ }
  }
  throw new Error(`host_startup_failed: see ${path.join(stateDir, 'host.log')}`);
}
try {
  if (command === 'serve') await serve();
  else if (command === 'init') {
    // アカウントも事前登録も無い。作るのは「どのリレーを使うか」と、この Mac 固有の鍵だけ。
    const origin = new URL(args[0] || 'https://control.necoder.com').origin;
    const existing = await readState('config');
    if (existing) {
      if (existing.origin !== origin) throw new Error(`origin_conflict: 既存の設定は ${existing.origin} です`);
      throw new Error('Already initialized; existing device keys preserved');
    }
    await writeState('config', { origin, name: hostname(), adminToken: random(), vapid: webpush.generateVAPIDKeys() });
    console.log(`Initialized (${origin}). Run: ne remote pair`);
  } else if (command === 'start') console.log(await ensureStarted());
  else if (command === 'pair' || command === 'pair-json') {
    await ensureStarted();
    const result = await admin('pair', { name: args[0] || 'Phone', tasks: args.slice(1) });
    if (command === 'pair-json') {
      const qr = QRCode.create(result.url, { errorCorrectionLevel: 'M' });
      console.log(JSON.stringify({ ...result, size: qr.modules.size, modules: Array.from(qr.modules.data).join('') }));
    } else {
      console.log(await QRCode.toString(result.url, { type: 'terminal', small: true }));
      console.log(result.url);
      console.log('5分以内に読み取り、PWAで接続してください。共有対象:', result.tasks.join(', '));
      if (hasLid()) console.log('注意: この Mac は蓋を閉じるとスリープします（power assertion では止まりません）。'
        + '\n  外部ディスプレイ + 電源で clamshell にするか、蓋を開けたままにしてください。');
    }
  } else if (command === 'devices-json') console.log(JSON.stringify(await listDevices()));
  else if (command === 'revoke') console.log(JSON.stringify(await revokeDevice(args[0]), null, 2));
  else if (['status', 'stop'].includes(command)) {
    console.log(JSON.stringify(await admin(command, { id: args[0] }), null, 2));
  } else {
    console.log('necoder Remote: init [https://origin] | start | serve | pair [device-name] [task-id...] | status | devices-json | revoke <device-id> | stop');
    console.log('Mac / Windows の Node.js 22+ が必要です。pair は現在開いているプロジェクトだけを共有します。');
  }
} catch (error) { console.error(error.message); process.exitCode = 1; }
