import { test } from 'node:test';
import assert from 'node:assert/strict';
import { random, initiate, accept, finish } from '../public/crypto.mjs';

async function peers(secret = random(), room = random()) {
  const initial = await initiate(secret, room);
  const host = await accept(secret, room, initial.hello);
  return { phone: await finish(secret, room, initial, host.welcome), host: host.channel };
}
test('双方向の暗号化、連番、再送と反射を拒否', async () => {
  const { phone, host } = await peers();
  const frame = await phone.seal({ text: '秘密の変更内容 🔐' });
  assert.ok(!JSON.stringify(frame).includes('秘密'));
  await assert.rejects(phone.open(frame));
  assert.deepEqual(await host.open(frame), { text: '秘密の変更内容 🔐' });
  await assert.rejects(host.open(frame));
  assert.deepEqual(await phone.open(await host.seal({ ok: true })), { ok: true });
});
test('違う鍵・部屋・改ざんされた握手を拒否', async () => {
  const secret = random(), room = random(), initial = await initiate(secret, room);
  await assert.rejects(accept(random(), room, initial.hello));
  await assert.rejects(accept(secret, random(), initial.hello));
  const host = await accept(secret, room, initial.hello);
  await assert.rejects(finish(secret, room, initial, { ...host.welcome, nonce: random() }));
});
test('再接続で鍵が変わり、古いフレームと改ざんを拒否', async () => {
  const secret = random(), room = random();
  const old = await peers(secret, room), next = await peers(secret, room);
  assert.notEqual(old.phone.session, next.phone.session);
  await assert.rejects(next.host.open(await old.phone.seal({ command: 'send' })));
  const frame = await next.phone.seal({ command: 'send' });
  await assert.rejects(next.host.open({ ...frame, ciphertext: (frame.ciphertext[0] === 'A' ? 'B' : 'A') + frame.ciphertext.slice(1) }));
  assert.deepEqual(await next.host.open(frame), { command: 'send' });
});
