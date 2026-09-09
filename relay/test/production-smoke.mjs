// 公開 relay の疎通確認。ダミーのプロジェクトのみを使い、実 GUI や端末リストを変更しない。
import assert from 'node:assert/strict';
import WebSocket from 'ws';
import { Bridge } from '../host/bridge.mjs';
import { readState } from '../host/storage.mjs';
import { initiate, finish, random } from '../public/crypto.mjs';
const config = await readState('config');
assert.ok(config?.provisionToken, 'provision first');
const bridge = new Bridge(config, [], { persist: async () => {}, persistReceipts: async () => {},
  ipc: async method => { assert.equal(method, 'remote_snapshot'); return { instance_id: 'smoke-only', projects: [{ id: 'smoke', name: 'Smoke test', threads: [] }] }; },
});
await bridge.poll();
let socket, device;
try {
  const pairing = await bridge.pair('Temporary deployment check', ['smoke']);
  device = bridge.devices[0];
  const params = new URLSearchParams(new URL(pairing.url).hash.slice(1));
  const url = new URL(`/api/rooms/${device.room}/ws`, config.origin); url.protocol = 'wss:';
  socket = new WebSocket(url, ['necoder-v1', `auth.${params.get('token')}`], { origin: config.origin });
  let initial, channel, chain = Promise.resolve();
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('production handshake timed out')), 25000);
    socket.once('error', reject);
    socket.on('message', data => {
      chain = chain.then(async () => {
        const message = JSON.parse(String(data));
        if (message.type === 'peer') {
          if (message.online) { initial = await initiate(params.get('secret'), device.room); socket.send(JSON.stringify(initial.hello)); }
          return;
        }
        if (message.type === 'welcome') { channel = await finish(params.get('secret'), device.room, initial, message); return; }
        const value = await channel.open(message);
        if (value.type === 'paired') {
          assert.notEqual(value.secret, params.get('secret'));
          socket.send(JSON.stringify(await channel.seal({ type: 'confirm' })));
        } else if (value.type === 'snapshot') {
          assert.equal(value.snapshot.instance_id, 'smoke-only');
          socket.send(JSON.stringify(await channel.seal({ type: 'request', id: random(), method: 'shell', expires_at: Date.now() + 25000 })));
        } else if (value.type === 'response') {
          assert.equal(value.ok, false); assert.equal(value.error, 'invalid_request');
          clearTimeout(timeout); resolve();
        }
      }).catch(error => { clearTimeout(timeout); reject(error); });
    });
  });
  console.log('Production relay: encrypted pairing, rotated key, authenticated snapshot, forbidden command rejection passed.');
} finally {
  socket?.close();
  if (device) { const result = await bridge.revoke(device.room); assert.ok(result.relay_deleted, 'temporary room deleted'); }
  bridge.stop();
}
