// WebAuthn の検証はPCで行う。スマホの自己申告や画面上のロックを権限として扱わない。
import { generateRegistrationOptions, verifyRegistrationResponse, generateAuthenticationOptions, verifyAuthenticationResponse } from '@simplewebauthn/server';
import { base64, unbase64 } from '../public/crypto.mjs';

export const SECURITY_VERSION = 2;
export const IDLE_TIMEOUT = 5 * 60_000;
export const SESSION_TIMEOUT = 15 * 60_000;
export const CHALLENGE_TIMEOUT = 60_000;

export function authorized(device, state, now = Date.now()) {
  return !device.revoked && device.securityVersion === SECURITY_VERSION && device.approved === true && !!device.passkey
    && !!state.channel && !!state.unlockedAt && now - state.unlockedAt < SESSION_TIMEOUT
    && now - state.activityAt < IDLE_TIMEOUT;
}

export function lock(state) {
  state.securityGeneration = (state.securityGeneration || 0) + 1;
  state.unlockedAt = 0; state.activityAt = 0; state.challenge = null;
  state.selection = null; state.lastThread = '';
}

function enrollmentAllowed(device) {
  if (device.revoked || device.securityVersion !== SECURITY_VERSION) throw new Error('pair_again_security_upgrade');
  if (!device.approved && device.expires <= Date.now()) throw new Error('pairing_expired');
}

export async function securityOptions(config, device, state) {
  enrollmentAllowed(device);
  const origin = new URL(config.origin);
  if (device.passkey && !device.approved) return { type: 'security', status: 'pending', code: device.approvalCode, expires: device.expires };
  const purpose = device.passkey ? 'authenticate' : 'register';
  const challenge = { purpose, generation: state.securityGeneration || 0, channel: state.channel };
  const options = purpose === 'register'
    ? await generateRegistrationOptions({
      rpName: 'necoder Remote', rpID: origin.hostname, userID: unbase64(device.room),
      userName: `necoder-${device.room.slice(0, 12)}`, userDisplayName: String(config.name || 'necoder').slice(0, 80),
      attestationType: 'none', timeout: CHALLENGE_TIMEOUT,
      authenticatorSelection: { residentKey: 'preferred', userVerification: 'required', authenticatorAttachment: 'platform' },
      supportedAlgorithmIDs: [-7, -257],
    })
    : await generateAuthenticationOptions({
      rpID: origin.hostname, timeout: CHALLENGE_TIMEOUT, userVerification: 'required',
      allowCredentials: [{ id: device.passkey.id, transports: device.passkey.transports }],
    });
  Object.assign(challenge, { value: options.challenge, expires: Date.now() + CHALLENGE_TIMEOUT });
  stillValid(device, state, challenge);
  state.challenge = challenge;
  return { type: 'security', status: purpose === 'register' ? 'register' : 'locked', options, expires: state.challenge.expires };
}

function consumeChallenge(device, state, purpose) {
  enrollmentAllowed(device);
  const challenge = state.challenge;
  state.challenge = null; // 成否に関わらず1回限り。リプレイ・並行した使い回しを拒否。
  if (!challenge || challenge.purpose !== purpose) throw new Error('passkey_challenge_expired');
  stillValid(device, state, challenge);
  return challenge;
}

function stillValid(device, state, challenge) {
  enrollmentAllowed(device);
  // ロックは進行中の検証・保存も取り消す。同じ暗号チャネルが残っていても再利用しない。
  if (challenge.channel !== state.channel || challenge.expires <= Date.now()
    || challenge.generation !== (state.securityGeneration || 0)) throw new Error('passkey_challenge_expired');
}

export async function registerPasskey(config, device, state, response, persist) {
  const challenge = consumeChallenge(device, state, 'register');
  if (device.passkey || device.approved) throw new Error('passkey_already_registered');
  const result = await verifyRegistrationResponse({
    response, expectedChallenge: challenge.value, expectedOrigin: new URL(config.origin).origin,
    expectedRPID: new URL(config.origin).hostname, requireUserVerification: true,
    supportedAlgorithmIDs: [-7, -257],
  });
  stillValid(device, state, challenge);
  if (!result.verified || !result.registrationInfo) throw new Error('passkey_verification_failed');
  const { credential } = result.registrationInfo;
  device.passkey = { id: credential.id, publicKey: base64(credential.publicKey), counter: credential.counter,
    transports: credential.transports?.filter(value => ['internal', 'hybrid', 'usb', 'nfc', 'ble'].includes(value)) };
  await persist();
  stillValid(device, state, challenge);
}

export async function authenticatePasskey(config, device, state, response, persist) {
  const challenge = consumeChallenge(device, state, 'authenticate');
  if (!device.approved || !device.passkey) throw new Error('pc_approval_required');
  const result = await verifyAuthenticationResponse({
    response, expectedChallenge: challenge.value, expectedOrigin: new URL(config.origin).origin,
    expectedRPID: new URL(config.origin).hostname, requireUserVerification: true,
    credential: { ...device.passkey, publicKey: unbase64(device.passkey.publicKey) },
  });
  stillValid(device, state, challenge);
  if (!result.verified || !device.approved) throw new Error('passkey_verification_failed');
  device.passkey.counter = result.authenticationInfo.newCounter;
  await persist();
  stillValid(device, state, challenge);
  if (!device.approved) throw new Error('pc_approval_required');
  state.unlockedAt = Date.now(); state.activityAt = state.unlockedAt;
}
