import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, stat } from 'node:fs/promises';
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
    await assert.rejects(ipc(adminSocket, 'pair'), /local_auth_required/);
    const config = JSON.parse(await readFile(path.join(directory, 'config.json'), 'utf8'));
    assert.equal(config.adminToken.length, 43);
    if (process.platform !== 'win32') {
      assert.equal((await stat(directory)).mode & 0o777, 0o700);
      assert.equal((await stat(path.join(directory, 'config.json'))).mode & 0o777, 0o600);
    }
  } finally { await run('stop').catch(() => {}); }
});
