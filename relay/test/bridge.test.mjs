import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Bridge, validateCommand, validSubscription } from '../host/bridge.mjs';
import { random } from '../public/crypto.mjs';

const device = () => ({ room: random(), tasks: ['project-a'] });
const command = () => ({ type: 'request', id: random(), method: 'send_message', expires_at: Date.now() + 25000,
  params: { instance_id: 'instance-a', task_id: 'project-a', thread_id: 'thread-a', message: 'test' } });
test('公開 API の範囲・共有対象・期限・GUI 世代を検証', () => {
  const d = device(), m = command(), snapshot = { instance_id: 'instance-a' };
  assert.doesNotThrow(() => validateCommand(m, d, snapshot));
  for (const changed of [ { method: 'shell' }, { expires_at: 0 }, { expires_at: Date.now() + 120000 },
    { params: { ...m.params, task_id: 'project-b' } }, { params: { ...m.params, instance_id: 'old' } } ]) {
    assert.throws(() => validateCommand({ ...m, ...changed }, d, snapshot));
  }
});
test('IPC 前に受付記録、同じ命令は一度だけ実行', async () => {
  let calls = 0, saved = false;
  const bridge = new Bridge({}, [], { persistReceipts: async () => { saved = true; }, ipc: async () => {
    assert.ok(saved); calls++; return { accepted: true };
  } });
  bridge.snapshot = { instance_id: 'instance-a' };
  const d = device(), m = command();
  assert.deepEqual(await bridge.execute(d, {}, m), { accepted: true });
  assert.deepEqual(await bridge.execute(d, {}, m), { accepted: true });
  assert.equal(calls, 1);
});
test('結果不明・永続化失敗・失効では再実行しない', async () => {
  let calls = 0;
  const bridge = new Bridge({}, [], { persistReceipts: async () => {}, ipc: async () => { calls++; throw new Error('timeout'); } });
  bridge.snapshot = { instance_id: 'instance-a' };
  const d = device(), m = command();
  await assert.rejects(bridge.execute(d, {}, m), /timeout/);
  await assert.rejects(bridge.execute(d, {}, m), /outcome_unknown/);
  assert.equal(calls, 1);
  bridge.persistReceipts = async () => { throw new Error('disk_full'); };
  await assert.rejects(bridge.execute(d, {}, command()), /disk_full/);
  assert.equal(calls, 1);
  d.revoked = true;
  await assert.rejects(bridge.execute(d, {}, command()), /revoked/);
  await assert.rejects(bridge.receive(d, {}, { type: 'hello' }), /revoked/);
});
test('受付記録中に失効しても IPC へ送らない', async () => {
  const d = device(); let calls = 0;
  const bridge = new Bridge({}, [], { persistReceipts: async () => { d.revoked = true; }, ipc: async () => calls++ });
  bridge.snapshot = { instance_id: 'instance-a' };
  await assert.rejects(bridge.execute(d, {}, command()), /revoked/);
  assert.equal(calls, 0);
});
test('Push 送信先を限定し SSRF を拒否', () => {
  const subscription = { endpoint: 'https://web.push.apple.com/abc', keys: { p256dh: 'a'.repeat(87), auth: 'b'.repeat(22) } };
  assert.equal(validSubscription(subscription), true);
  for (const endpoint of ['http://web.push.apple.com/a', 'https://127.0.0.1/a', 'https://push.apple.com.evil.test/a', 'https://user@fcm.googleapis.com/a', 'https://fcm.googleapis.com:444/a'])
    assert.equal(validSubscription({ ...subscription, endpoint }), false);
});
