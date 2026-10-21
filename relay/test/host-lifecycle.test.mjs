import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { locations } from '../host/platform.mjs';
import { ipc } from '../host/ipc.mjs';

test('ホスト起動・ローカル認証・停止（Windows は実際の名前付きパイプ）', { timeout: 30000 }, async () => {
  const directory = await mkdtemp(path.join(process.platform === 'win32' ? tmpdir() : '/tmp', 'ncr-host-'));
  const env = { ...process.env, NECODER_REMOTE_HOME: directory,
    NECODER_GUI_SOCK: process.platform === 'win32' ? `\\\\.\\pipe\\necoder-test-absent-${process.pid}` : path.join(directory, 'absent.sock') };
  const { adminSocket } = locations(process.platform, env, directory);
  const cli = fileURLToPath(new URL('../host/cli.mjs', import.meta.url));
  const run = (...args) => promisify(execFile)(process.execPath, [cli, ...args], { env, timeout: 20000, windowsHide: true });
  await run('init');
  try {
    await run('start');
    const status = JSON.parse((await run('status')).stdout);
    assert.equal(status.online, false);
    assert.deepEqual(status.devices, []);
    assert.deepEqual(JSON.parse((await run('devices-json')).stdout), { running: true, devices: [] });
    await assert.rejects(ipc(adminSocket, 'pair'), /local_auth_required/);
    const config = JSON.parse(await readFile(path.join(directory, 'config.json'), 'utf8'));
    assert.equal(config.adminToken.length, 43);
    if (process.platform !== 'win32') {
      assert.equal((await stat(directory)).mode & 0o777, 0o700);
      assert.equal((await stat(path.join(directory, 'config.json'))).mode & 0o777, 0o600);
    }
  } finally { await run('stop').catch(() => {}); }
});

test('ホストが止まっていても端末の一覧と失効ができる', { timeout: 30000 }, async () => {
  const directory = await mkdtemp(path.join(process.platform === 'win32' ? tmpdir() : '/tmp', 'ncr-devices-'));
  const env = { ...process.env, NECODER_REMOTE_HOME: directory,
    NECODER_GUI_SOCK: process.platform === 'win32' ? `\\\\.\\pipe\\necoder-test-absent-${process.pid}` : path.join(directory, 'absent.sock') };
  const cli = fileURLToPath(new URL('../host/cli.mjs', import.meta.url));
  const run = (...args) => promisify(execFile)(process.execPath, [cli, ...args], { env, timeout: 20000, windowsHide: true });
  // 未初期化でも失敗にしない（設定画面は開くたびに読む）。
  assert.deepEqual(JSON.parse((await run('devices-json')).stdout), { running: false, devices: [] });
  // 失効時のリレーへの削除は届かない宛先にして、ローカルの失効だけで完了することを見る。
  await run('init', 'http://127.0.0.1:9');
  const room = 'a'.repeat(43), other = 'b'.repeat(43);
  await writeFile(path.join(directory, 'devices.json'), JSON.stringify([
    { room, hostToken: 'c'.repeat(43), secret: 'd'.repeat(43), name: 'Phone', model: 'iPhone', tasks: ['p'],
      paired: true, confirmed: true, pairedAt: 1000, lastSeen: 2000 },
    { room: other, name: 'Phone', tasks: ['p'], paired: false, expires: 1 },
  ]));
  const listed = JSON.parse((await run('devices-json')).stdout);
  assert.equal(listed.running, false);
  assert.deepEqual(listed.devices, [{ id: room, name: 'Phone', model: 'iPhone', status: 'offline',
    paired_at: 1000, last_seen: 2000, expires: null, projects: ['p'] }]);
  const revoked = JSON.parse((await run('revoke', room)).stdout);
  assert.deepEqual(revoked, { revoked: true, relay_deleted: false });
  assert.deepEqual(JSON.parse((await run('devices-json')).stdout).devices, []);
  const saved = JSON.parse(await readFile(path.join(directory, 'devices.json'), 'utf8'));
  assert.equal(saved[0].revoked, true);
  assert.equal(saved[0].secret, undefined);
  await assert.rejects(run('revoke', 'not-a-device'), /device_not_found/);
});
