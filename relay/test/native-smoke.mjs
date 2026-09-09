// 更新 GUI を隔離した一時プロジェクトで起動。実ユーザーの DB / 会話 / agent は触らない。
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';
import { ipc } from '../host/ipc.mjs';
const root = await mkdtemp('/tmp/ncr-');
const project = path.join(root, 'project'), state = path.join(root, 'state'), socket = path.join(root, 'gui.sock');
await mkdir(project); await mkdir(state);
await writeFile(path.join(state, 'settings.json'), JSON.stringify({ onboarded: true }));
await writeFile(path.join(project, 'example.txt'), 'before\n');
const git = args => execFileSync('git', args, { cwd: project, stdio: 'ignore' });
git(['init']); git(['add', 'example.txt']); git(['-c', 'user.name=Remote Test', '-c', 'user.email=remote-test@example.invalid', 'commit', '-m', 'fixture']);
await writeFile(path.join(project, 'example.txt'), 'after\n');
const binary = fileURLToPath(new URL('../../target/debug/necoder', import.meta.url));
let logs = '';
const child = spawn(binary, [project], { env: { ...process.env, NECODER_HOME: state, NECODER_GUI_SOCK: socket }, stdio: ['ignore', 'pipe', 'pipe'] });
child.stdout.on('data', data => { logs = (logs + data).slice(-12000); });
child.stderr.on('data', data => { logs = (logs + data).slice(-12000); });
try {
  let snapshot;
  for (let attempt = 0; attempt < 100; attempt++) {
    try { snapshot = await ipc(socket, 'remote_snapshot'); if (snapshot.projects.length) break; } catch {}
    if (child.exitCode !== null) throw new Error('GUI exited: ' + logs);
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  assert.equal(snapshot?.projects.length, 1, 'isolated project is open');
  const params = { instance_id: snapshot.instance_id, task_id: snapshot.projects[0].id, expires_at: Date.now() + 25000 };
  const diff = await ipc(socket, 'remote_get_diff', params);
  assert.ok(diff.diff.includes('+after'));
  await assert.rejects(ipc(socket, 'remote_new_thread', { ...params, expires_at: 0, agent: 'Codex' }), /command_expired/);
  await assert.rejects(ipc(socket, 'remote_new_thread', { ...params, instance_id: 'stale', agent: 'Codex' }), /stale_instance/);
  const thread = await ipc(socket, 'remote_new_thread', { ...params, agent: 'Codex' });
  const detail = await ipc(socket, 'remote_thread', { ...params, thread_id: thread.thread_id });
  assert.equal(detail.id, thread.thread_id);
  await assert.rejects(ipc(socket, 'remote_interrupt', { ...params, thread_id: detail.id, turn_id: 'old' }), /stale_turn/);
  console.log('Native GUI IPC passed: snapshot, diff, new thread, detail, expired/stale rejection. No agent API calls.');
  console.log('Isolated test data retained:', root);
} finally { child.kill('SIGTERM'); }
