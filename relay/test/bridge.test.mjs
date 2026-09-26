import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Bridge, leadingZeroBits, validateCommand, validSubscription } from '../host/bridge.mjs';
import { random } from '../public/crypto.mjs';
import { createHash } from 'node:crypto';
import webpush from 'web-push';

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
test('PoW は難易度 0 で無効・要求されれば Worker と同じ規則で解く', async () => {
  const room = random();
  // difficulty 0（既定）は計算せず素通り。旧 provisioning token の代わりに何も要求しない。
  const off = new Bridge({ origin: 'http://relay.test' }, [], {});
  global.fetch = async () => ({ ok: true, json: async () => ({ version: 1, difficulty: 0 }) });
  assert.equal(await off.proveWork(room), null);
  // リレーが到達不能でも pair を止めない（部屋作成の失敗として POST 側で出る）。
  global.fetch = async () => { throw new Error('offline'); };
  assert.equal(await off.proveWork(room), null);
  // 要求されたら、Worker の provenWork と同じ「先頭ゼロ bit」を満たす nonce を返す。
  global.fetch = async () => ({ ok: true, json: async () => ({ version: 1, difficulty: 8 }) });
  const nonce = await off.proveWork(room);
  assert.match(nonce, /^[A-Za-z0-9_-]{1,64}$/);
  const digest = createHash('sha256').update(`necoder-room|${room}|${nonce}`).digest();
  assert.ok(leadingZeroBits(digest) >= 8);
  // 天井を超える難易度は解こうとせず即座に諦める（永遠に回る pair を作らない）。
  global.fetch = async () => ({ ok: true, json: async () => ({ version: 1, difficulty: 64 }) });
  await assert.rejects(off.proveWork(room), /proof_of_work_too_hard/);
});
test('leadingZeroBits は Worker と同じ数え方をする', () => {
  assert.equal(leadingZeroBits(Buffer.from([0xff])), 0);
  assert.equal(leadingZeroBits(Buffer.from([0x7f])), 1);
  assert.equal(leadingZeroBits(Buffer.from([0x01])), 7);
  assert.equal(leadingZeroBits(Buffer.from([0x00, 0x80])), 8);
  assert.equal(leadingZeroBits(Buffer.from([0x00, 0x00])), 16);
});
test('ペアリング済み端末がある間だけ Mac を眠らせない', { skip: process.platform !== 'darwin' }, () => {
  const paired = () => ({ room: random(), tasks: [], paired: true });
  // 未ペア（QR を出しただけ）では掴まない — 使われないまま Mac が起き続けるのを避ける。
  const idle = new Bridge({}, [{ room: random(), tasks: [], paired: false }], {});
  idle.updateWakeLock();
  assert.equal(idle.caffeinate, null);
  // revoke 済みも数えない。
  const revoked = new Bridge({}, [{ ...paired(), revoked: true }], {});
  revoked.updateWakeLock();
  assert.equal(revoked.caffeinate, null);
  // 明示的に切っていれば掴まない。
  const disabled = new Bridge({ keepAwake: false }, [paired()], {});
  disabled.updateWakeLock();
  assert.equal(disabled.caffeinate, null);
  // ペアリング済みが 1 台あれば掴み、stop() で放す。
  const live = new Bridge({}, [paired()], {});
  live.updateWakeLock();
  assert.ok(live.caffeinate?.pid, 'caffeinate should hold the assertion');
  live.updateWakeLock(); // 冪等（二重に起こさない）
  assert.ok(live.caffeinate?.pid);
  live.stop();
  assert.equal(live.caffeinate, null);
});
test('電池駆動の間は掴まず、電源に挿し直すと掴み直す', async () => {
  if (process.platform !== 'darwin') return;
  let onAc = false;
  const bridge = new Bridge({}, [{ room: random(), tasks: [], paired: true }], { readPower: async () => onAc });
  // 電池のうちはペアリング済みでも掴まない（鞄の中でノートの電池を溶かさない）。
  await bridge.refreshPower();
  assert.equal(bridge.caffeinate, null);
  assert.equal(bridge.keepsAwake(), false);
  // 挿し直したら掴む。30 秒の絞りがあるので計測時刻を戻してから見る。
  onAc = true; bridge.powerCheckedAt = 0;
  await bridge.refreshPower();
  assert.ok(bridge.caffeinate?.pid);
  // 抜いたら放す。
  onAc = false; bridge.powerCheckedAt = 0;
  await bridge.refreshPower();
  assert.equal(bridge.caffeinate, null);
  bridge.stop();
});
test('電源の実測は 30 秒に 1 回だけ', async () => {
  let reads = 0;
  const bridge = new Bridge({}, [], { readPower: async () => { reads++; return true; } });
  await bridge.refreshPower();
  await bridge.refreshPower();
  await bridge.refreshPower();
  assert.equal(reads, 1);
  bridge.powerCheckedAt = 0;
  await bridge.refreshPower();
  assert.equal(reads, 2);
  bridge.stop();
});
test('走っていたスレッドが終わったら完了を 1 回だけ push・待ちは attention', async () => {
  const sent = [];
  const original = webpush.sendNotification;
  webpush.sendNotification = async (_subscription, payload) => { sent.push(JSON.parse(payload).type); };
  try {
    const bridge = new Bridge({ origin: 'http://relay.test', vapid: { publicKey: 'p', privateKey: 'q' } }, [],
      { persist: async () => {} });
    const d = { ...device(), subscription: { endpoint: 'https://web.push.apple.com/abc' } };
    const snapshot = (...threads) => ({ projects: [{ id: 'project-a', threads },
      { id: 'project-b', threads: [{ id: 'other', running: false }] }] });
    await bridge.maybeNotify(d, snapshot({ id: 't', running: true }));
    await bridge.maybeNotify(d, snapshot({ id: 't', running: true }));
    assert.deepEqual(sent, [], '走っている間は知らせない');
    await bridge.maybeNotify(d, snapshot({ id: 't', running: false }));
    await bridge.maybeNotify(d, snapshot({ id: 't', running: false }));
    assert.deepEqual(sent, ['done'], '終わった時に 1 回だけ');
    // 質問で止まった＝完了ではなく attention。答えて走り、終われば done。
    await bridge.maybeNotify(d, snapshot({ id: 't', running: true }));
    await bridge.maybeNotify(d, snapshot({ id: 't', running: true, blocked: true, question_pending: true }));
    await bridge.maybeNotify(d, snapshot({ id: 't', running: false, blocked: true, question_pending: true }));
    assert.deepEqual(sent, ['done', 'attention']);
    await bridge.maybeNotify(d, snapshot({ id: 't', running: true }));
    await bridge.maybeNotify(d, snapshot({ id: 't', running: false, session_lost: true }));
    assert.deepEqual(sent, ['done', 'attention'], '切断は完了として知らせない');
    // 共有していないプロジェクトのスレッドは見ない。
    const shared = { ...device(), subscription: d.subscription };
    await bridge.maybeNotify(shared, { projects: [{ id: 'project-b', threads: [{ id: 'x', running: true }] }] });
    await bridge.maybeNotify(shared, { projects: [{ id: 'project-b', threads: [{ id: 'x', running: false }] }] });
    assert.deepEqual(sent, ['done', 'attention']);
  } finally { webpush.sendNotification = original; }
});
