import { test } from 'node:test';
import assert from 'node:assert/strict';
import WebSocket from 'ws';
import { SECURITY_VERSION, IDLE_TIMEOUT, SESSION_TIMEOUT, authorized, lock, securityOptions, registerPasskey, authenticatePasskey } from '../host/passkeys.mjs';
import { Bridge } from '../host/bridge.mjs';
import { random, initiate, accept, finish } from '../public/crypto.mjs';
import { authenticator } from './support/authenticator.mjs';

const config = { origin: 'https://control.necoder.com', name: 'Test PC', keepAwake: false, vapid: { publicKey: '' } };
const persist = async () => {};
function device() { return { room: random(), tasks: ['project-a'], secret: random(), securityVersion: SECURITY_VERSION,
  paired: true, keyConfirmed: true, approved: false, approvalCode: '123456', expires: Date.now() + 300_000 }; }
async function registered() {
  const d = device(), state = { channel: {} }, key = authenticator();
  const options = await securityOptions(config, d, state);
  await registerPasskey(config, d, state, key.registration(options.options), persist);
  return { d, state, key };
}

test('登録だけ・スマホのconfirmだけでは承認されず、PC承認後も署名の検証まで利用不可', async () => {
  const { d, state, key } = await registered();
  const bridge = new Bridge(config, [d], { persist });
  bridge.renew = persist;
  assert.equal(authorized(d, state), false);
  assert.equal((await securityOptions(config, d, state)).status, 'pending');
  await assert.rejects(bridge.approve(d.room, '000000'), /code_changed/);
  await bridge.approve(d.room, '123456');
  assert.equal(authorized(d, state), false);
  const options = await securityOptions(config, d, state);
  assert.equal(options.options.userVerification, 'required');
  await authenticatePasskey(config, d, state, key.authentication(options.options), persist);
  assert.equal(authorized(d, state), true);
  assert.equal(bridge.deviceStatus()[0].state, 'approved');
});

test('PC承認の永続化が済むまで承認せず、保存失敗では権限を与えない', async () => {
  const { d } = await registered();
  let release;
  const bridge = new Bridge(config, [d], { persist: async snapshot => {
    assert.equal(d.approved, false);
    assert.equal(snapshot[0].approved, true);
    await new Promise(resolve => { release = resolve; });
  } });
  bridge.renew = persist;
  const approval = bridge.approve(d.room, '123456');
  assert.equal(d.approved, false);
  release(); await approval;
  assert.equal(d.approved, true);
  const second = await registered();
  const failed = new Bridge(config, [second.d], { persist: async () => { throw new Error('disk_full'); } });
  await assert.rejects(failed.approve(second.d.room, '123456'), /disk_full/);
  assert.equal(second.d.approved, false);
});

test('暗号化中にロックされた配信は送信直前にも拒否する', async () => {
  const d = { ...device(), approved: true, passkey: {} };
  let sent = 0;
  const state = { unlockedAt: Date.now(), activityAt: Date.now(), outbound: Promise.resolve(),
    socket: { readyState: WebSocket.OPEN, bufferedAmount: 0, send: () => sent++, close() {} } };
  state.channel = { seal: async () => { lock(state); return { ciphertext: 'not-sent' }; } };
  const bridge = new Bridge(config, [d], { persist }); bridge.connections.set(d.room, state);
  await bridge.send(state, { type: 'thread', detail: 'private' });
  assert.equal(sent, 0);
});

test('登録時に異なるOrigin・RP ID・UVなし・不正チャレンジを拒否', async () => {
  for (const changes of [{ client: { origin: 'https://evil.example' } }, { uv: false }, { client: { challenge: random() } }, { rp: true }]) {
    const d = device(), state = { channel: {} }, key = authenticator();
    const options = await securityOptions(config, d, state);
    if (changes.rp) options.options.rp.id = 'evil.example';
    await assert.rejects(registerPasskey(config, d, state, key.registration(options.options, changes), persist));
    assert.equal(d.passkey, undefined);
  }
});

test('認証時に偽署名・異なるOrigin・UVなし・別の鍵を拒否し、チャレンジを消費', async () => {
  for (const kind of ['signature', 'origin', 'uv', 'key']) {
    const { d, state, key } = await registered(); d.approved = true;
    const options = await securityOptions(config, d, state);
    const response = (kind === 'key' ? authenticator() : key).authentication(options.options,
      kind === 'origin' ? { client: { origin: 'https://evil.example' } } : kind === 'uv' ? { uv: false } : {});
    if (kind === 'signature') response.response.signature = random();
    await assert.rejects(authenticatePasskey(config, d, state, response, persist));
    assert.equal(authorized(d, state), false);
    await assert.rejects(authenticatePasskey(config, d, state, key.authentication(options.options), persist), /challenge_expired/);
  }
});

test('期限切れ・接続が替わったチャレンジ・再利用・永続化失敗では解錠しない', async () => {
  const { d, state, key } = await registered(); d.approved = true;
  let options = await securityOptions(config, d, state);
  state.challenge.expires = 0;
  await assert.rejects(authenticatePasskey(config, d, state, key.authentication(options.options), persist), /challenge_expired/);
  options = await securityOptions(config, d, state); state.channel = {};
  await assert.rejects(authenticatePasskey(config, d, state, key.authentication(options.options), persist), /challenge_expired/);
  options = await securityOptions(config, d, state);
  await assert.rejects(authenticatePasskey(config, d, state, key.authentication(options.options), async () => { throw new Error('disk_full'); }), /disk_full/);
  assert.equal(authorized(d, state), false);
  options = await securityOptions(config, d, state); const response = key.authentication(options.options);
  await authenticatePasskey(config, d, state, response, persist);
  lock(state);
  await assert.rejects(authenticatePasskey(config, d, state, response, persist), /challenge_expired/);
});

test('認証の保存待ちにロックされたら、同じ接続でも古い認証で解錠しない', async () => {
  const { d, state, key } = await registered(); d.approved = true;
  const options = await securityOptions(config, d, state);
  await assert.rejects(authenticatePasskey(config, d, state, key.authentication(options.options), async () => {
    lock(state);
  }), /challenge_expired/);
  assert.equal(authorized(d, state), false);
  const fresh = await securityOptions(config, d, state);
  await authenticatePasskey(config, d, state, key.authentication(fresh.options), persist);
  assert.equal(authorized(d, state), true);
});

test('チャレンジ生成中のロックでも、古いチャレンジを発行しない', async () => {
  const d = device(), state = { channel: {} };
  const issuing = securityOptions(config, d, state);
  lock(state);
  await assert.rejects(issuing, /challenge_expired/);
  assert.equal(state.challenge, null);
  assert.equal((await securityOptions(config, d, state)).status, 'register');
});

test('解錠通知も暗号化中にロックされたら送らない', async () => {
  const d = { ...device(), approved: true, passkey: {} };
  let sent = 0, closed = false;
  const state = { unlockedAt: Date.now(), activityAt: Date.now(), outbound: Promise.resolve(),
    socket: { readyState: WebSocket.OPEN, bufferedAmount: 0, send: () => sent++, close() { closed = true; } } };
  state.channel = { seal: async () => { lock(state); return { ciphertext: 'not-sent' }; } };
  const bridge = new Bridge(config, [d], { persist }); bridge.connections.set(d.room, state);
  await bridge.send(state, { type: 'unlocked' });
  assert.equal(sent, 0);
  assert.equal(closed, true);
});

test('無操作・最大期限・未承認・旧端末・失効をホストで拒否', async () => {
  const { d, state, key } = await registered(); d.approved = true;
  const options = await securityOptions(config, d, state);
  await authenticatePasskey(config, d, state, key.authentication(options.options), persist);
  const bridge = new Bridge(config, [d], { persist });
  const request = { type: 'request', method: 'snapshot', id: random(), expires_at: Date.now() + 25_000 };
  state.activityAt = Date.now() - IDLE_TIMEOUT;
  await assert.rejects(bridge.execute(d, state, request), /authentication_required/);
  state.activityAt = Date.now(); state.unlockedAt = Date.now() - SESSION_TIMEOUT;
  await assert.rejects(bridge.execute(d, state, request), /authentication_required/);
  state.unlockedAt = Date.now(); d.approved = false;
  await assert.rejects(bridge.execute(d, state, request), /authentication_required/);
  d.approved = true; d.securityVersion = 1;
  await assert.rejects(bridge.execute(d, state, request), /authentication_required/);
  d.securityVersion = SECURITY_VERSION; d.revoked = true;
  await assert.rejects(bridge.execute(d, state, request), /device_revoked/);
});

test('confirm・heartbeat・pollでも認証前はsnapshotも会話も配信しない', async () => {
  const d = device(), initial = await initiate(d.secret, d.room), accepted = await accept(d.secret, d.room, initial.hello);
  const phone = await finish(d.secret, d.room, initial, accepted.welcome), sent = [];
  const state = { channel: accepted.channel, outbound: Promise.resolve(), socket: { readyState: WebSocket.OPEN, bufferedAmount: 0, send: frame => sent.push(JSON.parse(frame)), close() {} } };
  const snapshot = { instance_id: 'test', projects: [{ id: 'project-a', name: 'secret project', threads: [] }] };
  const bridge = new Bridge(config, [d], { persist, ipc: async () => snapshot, readPower: async () => false });
  bridge.connections.set(d.room, state); bridge.snapshot = snapshot;
  await bridge.receive(d, state, await phone.seal({ type: 'confirm' }));
  await bridge.receive(d, state, await phone.seal({ type: 'heartbeat' }));
  await bridge.sendSnapshot(d, state); await bridge.poll();
  const messages = []; for (const frame of sent) messages.push(await phone.open(frame));
  assert(!messages.some(value => ['snapshot', 'thread'].includes(value.type)));
  assert.equal(d.approved, false); assert.equal(d.confirmed, undefined);
});

test('接続解除はリレー不通でも先にローカル失効し、再操作を拒否', async () => {
  const { d, state, key } = await registered(); d.approved = true;
  const options = await securityOptions(config, d, state);
  await authenticatePasskey(config, d, state, key.authentication(options.options), persist);
  const bridge = new Bridge(config, [d], { persist });
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async () => { throw new Error('offline'); };
    const result = await bridge.revoke(d.room);
    assert.equal(result.revoked, true); assert.equal(result.relay_deleted, false);
    assert.equal(authorized(d, state), false); assert.equal(d.passkey, undefined);
    assert.deepEqual(bridge.deviceStatus(), []);
  } finally { globalThis.fetch = original; }
});
