import { expect } from '@playwright/test';
import { authenticator } from '../support/authenticator.mjs';

// WebKit/Chromium共通のUI検証用。本人確認の署名検証は本物のサーバライブラリを通す。
export async function installPasskeyAuthenticator(page) {
  const key = authenticator('http://localhost:8791');
  await page.exposeFunction('__testPasskey', (kind, options) => kind === 'create' ? key.registration(options) : key.authentication(options));
  await page.addInitScript(() => {
    const base64 = bytes => btoa(String.fromCharCode(...new Uint8Array(bytes))).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
    const bytes = value => Uint8Array.from(atob(value.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0)).buffer;
    const run = kind => async ({ publicKey }) => {
      if (window.__cancelPasskey) throw new DOMException('Cancelled', 'NotAllowedError');
      const options = { ...publicKey, challenge: base64(publicKey.challenge) };
      const result = await window.__testPasskey(kind, options);
      const response = Object.fromEntries(Object.entries(result.response).filter(([, value]) => typeof value === 'string').map(([name, value]) => [name, bytes(value)]));
      if (kind === 'create') response.getTransports = () => ['internal'];
      return { ...result, rawId: bytes(result.rawId), response, getClientExtensionResults: () => ({}) };
    };
    if (!globalThis.PublicKeyCredential) globalThis.PublicKeyCredential = class PublicKeyCredential {};
    if (globalThis.CredentialsContainer) {
      Object.defineProperty(CredentialsContainer.prototype, 'create', { configurable: true, value: run('create') });
      Object.defineProperty(CredentialsContainer.prototype, 'get', { configurable: true, value: run('get') });
    } else Object.defineProperty(navigator, 'credentials', { configurable: true, value: { create: run('create'), get: run('get') } });
  });
}

export async function approveAndUnlock(page, request, room) {
  await expect(page.locator('#security-title')).toHaveText('パスキーを登録する');
  await page.locator('#security-verify').click();
  await expect(page.locator('#security-title')).toHaveText('PCで接続を承認してください');
  await expect(page.locator('#home-list')).toBeEmpty();
  const code = await page.locator('#security-code').textContent();
  expect(code).toMatch(/^\d{6}$/);
  const response = await request.get(`http://127.0.0.1:8792/approve?room=${room}&code=${code}`);
  expect(response.ok()).toBe(true);
  await unlock(page);
}

export async function unlock(page) {
  await expect(page.locator('#security-title')).toHaveText('本人確認して接続する', { timeout: 15000 });
  await page.locator('#security-verify').click();
  await expect(page.locator('#status')).toHaveText('接続済み');
}
