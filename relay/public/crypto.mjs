// Web Crypto の P-256 / HKDF / HMAC / AES-GCM のみを使う、ブラウザとホスト共通の通信契約。
// 接続ごとに ephemeral ECDH。長期鍵は相互認証に使い、送受信鍵/連番は方向ごとに分離する。
const encoder = new TextEncoder();
export const VERSION = 1;
export const MAX_PLAIN = 2_000_000;
export function base64(bytes) {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 8192)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
export function unbase64(value) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]+$/.test(value)) throw new Error('invalid_encoding');
  return Uint8Array.from(atob(value.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0));
}
export function random(size = 32) { return base64(crypto.getRandomValues(new Uint8Array(size))); }
export function validSecret(value) { return typeof value === 'string' && /^[A-Za-z0-9_-]{43}$/.test(value); }
export async function digest(value) { return base64(new Uint8Array(await crypto.subtle.digest('SHA-256', encoder.encode(value)))); }
async function hmacKey(secret) {
  if (!validSecret(secret)) throw new Error('invalid_key');
  return crypto.subtle.importKey('raw', unbase64(secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
}
async function sign(secret, value) {
  return base64(new Uint8Array(await crypto.subtle.sign('HMAC', await hmacKey(secret), encoder.encode(value))));
}
async function verify(secret, value, proof) {
  return crypto.subtle.verify('HMAC', await hmacKey(secret), unbase64(proof), encoder.encode(value));
}
async function ephemeral() {
  const keys = await crypto.subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, false, ['deriveBits']);
  return { privateKey: keys.privateKey, publicKey: base64(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey))) };
}
const transcript = (room, nonce, publicKey) => JSON.stringify(['necoder-control', VERSION, room, nonce, publicKey]);
export async function initiate(secret, room) {
  const keys = await ephemeral();
  const nonce = random();
  const value = transcript(room, nonce, keys.publicKey);
  return { keys, value, hello: { type: 'hello', version: VERSION, nonce, publicKey: keys.publicKey, proof: await sign(secret, value) } };
}
export async function accept(secret, room, hello) {
  if (hello.type !== 'hello' || hello.version !== VERSION || !validSecret(hello.nonce)) throw new Error('invalid_hello');
  const value = transcript(room, hello.nonce, hello.publicKey);
  if (!await verify(secret, value, hello.proof)) throw new Error('authentication_failed');
  const keys = await ephemeral();
  const nonce = random();
  const context = JSON.stringify([value, nonce, keys.publicKey]);
  const welcome = { type: 'welcome', version: VERSION, nonce, publicKey: keys.publicKey, proof: await sign(secret, context) };
  return { welcome, channel: await channel(secret, room, keys.privateKey, hello.publicKey, context, 'host') };
}
export async function finish(secret, room, initiation, welcome) {
  if (welcome.type !== 'welcome' || welcome.version !== VERSION || !validSecret(welcome.nonce)) throw new Error('invalid_welcome');
  const context = JSON.stringify([initiation.value, welcome.nonce, welcome.publicKey]);
  if (!await verify(secret, context, welcome.proof)) throw new Error('authentication_failed');
  return channel(secret, room, initiation.keys.privateKey, welcome.publicKey, context, 'phone');
}
async function channel(secret, room, privateKey, publicKey, context, role) {
  const peer = await crypto.subtle.importKey('raw', unbase64(publicKey), { name: 'ECDH', namedCurve: 'P-256' }, false, []);
  const shared = await crypto.subtle.deriveBits({ name: 'ECDH', public: peer }, privateKey, 256);
  const material = await crypto.subtle.importKey('raw', shared, 'HKDF', false, ['deriveKey']);
  const session = await digest(context);
  async function key(direction) {
    return crypto.subtle.deriveKey({ name: 'HKDF', hash: 'SHA-256', salt: unbase64(secret),
      info: encoder.encode(JSON.stringify([room, session, direction])) }, material,
    { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
  }
  const tx = role === 'phone' ? 'phone-host' : 'host-phone';
  const rx = role === 'phone' ? 'host-phone' : 'phone-host';
  const [txKey, rxKey] = await Promise.all([key(tx), key(rx)]);
  let sent = 0, received = 0;
  const iv = sequence => { const bytes = new Uint8Array(12); new DataView(bytes.buffer).setBigUint64(4, BigInt(sequence)); return bytes; };
  const aad = (direction, sequence) => encoder.encode(JSON.stringify([VERSION, room, session, direction, sequence]));
  return {
    session,
    async seal(value) {
      const plain = encoder.encode(JSON.stringify(value));
      if (plain.length > MAX_PLAIN) throw new Error('message_too_large');
      const sequence = ++sent;
      if (!Number.isSafeInteger(sequence)) throw new Error('session_exhausted');
      const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv: iv(sequence), additionalData: aad(tx, sequence) }, txKey, plain);
      return { type: 'sealed', session, sequence, ciphertext: base64(new Uint8Array(ciphertext)) };
    },
    async open(frame) {
      if (frame.type !== 'sealed' || frame.session !== session || frame.sequence !== received + 1
          || typeof frame.ciphertext !== 'string' || frame.ciphertext.length > 2_700_000) throw new Error('invalid_frame');
      const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: iv(frame.sequence), additionalData: aad(rx, frame.sequence) }, rxKey, unbase64(frame.ciphertext));
      received = frame.sequence;
      return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(plain));
    },
  };
}
