// テスト専用の仮想認証器。検証を差し替えず、実際のP-256署名とWebAuthnのデータを生成する。
import { generateKeyPairSync, randomBytes, createHash, sign } from 'node:crypto';
import { isoCBOR } from '@simplewebauthn/server/helpers';

const hash = value => createHash('sha256').update(value).digest();
const b64 = value => Buffer.from(value).toString('base64url');

export function authenticator(origin = 'https://control.necoder.com') {
  const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const jwk = keys.publicKey.export({ format: 'jwk' });
  const id = randomBytes(32);
  let counter = 0;
  const publicKey = isoCBOR.encode(new Map([[1, 2], [3, -7], [-1, 1], [-2, new Uint8Array(Buffer.from(jwk.x, 'base64url'))], [-3, new Uint8Array(Buffer.from(jwk.y, 'base64url'))]]));
  function clientData(type, options, changes) {
    return Buffer.from(JSON.stringify({ type, challenge: options.challenge, origin, crossOrigin: false, ...changes }));
  }
  function authData(rpID, uv, registration = false) {
    const count = Buffer.alloc(4); count.writeUInt32BE(counter);
    return Buffer.concat([hash(rpID), Buffer.from([(uv ? 5 : 1) | (registration ? 64 : 0)]), count]);
  }
  return {
    id: b64(id),
    registration(options, { uv = true, client = {} } = {}) {
      const length = Buffer.alloc(2); length.writeUInt16BE(id.length);
      const data = Buffer.concat([authData(options.rp.id, uv, true), Buffer.alloc(16), length, id, publicKey]);
      const attestation = isoCBOR.encode(new Map([['fmt', 'none'], ['attStmt', new Map()], ['authData', new Uint8Array(data)]]));
      return { id: b64(id), rawId: b64(id), type: 'public-key', authenticatorAttachment: 'platform', clientExtensionResults: {},
        response: { clientDataJSON: b64(clientData('webauthn.create', options, client)), attestationObject: b64(attestation), transports: ['internal'] } };
    },
    authentication(options, { uv = true, client = {} } = {}) {
      counter++;
      const data = authData(options.rpId, uv), clientJSON = clientData('webauthn.get', options, client);
      return { id: b64(id), rawId: b64(id), type: 'public-key', authenticatorAttachment: 'platform', clientExtensionResults: {},
        response: { clientDataJSON: b64(clientJSON), authenticatorData: b64(data), signature: b64(sign('sha256', Buffer.concat([data, hash(clientJSON)]), keys.privateKey)) } };
    },
  };
}
