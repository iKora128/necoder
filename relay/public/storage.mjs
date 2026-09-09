import { base64, unbase64 } from './crypto.mjs';
let database;
function db() {
  if (!database) database = new Promise((resolve, reject) => {
    const request = indexedDB.open('necoder-control', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('devices', { keyPath: 'room' });
    request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
  });
  return database;
}
async function operation(mode, action) {
  const database = await db();
  return new Promise((resolve, reject) => {
    const transaction = database.transaction('devices', mode);
    const request = action(transaction.objectStore('devices'));
    transaction.oncomplete = () => resolve(request.result);
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error);
  });
}
export async function saveDevice(device) {
  // localStorage に長期秘密を置かない。非 extractable CryptoKey と暗号文を IndexedDB に保存。
  const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const payload = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(JSON.stringify(device)));
  await operation('readwrite', store => store.put({ room: device.room, key, iv: base64(iv), payload: base64(new Uint8Array(payload)) }));
}
export async function devices() {
  const rows = await operation('readonly', store => store.getAll());
  return Promise.all(rows.map(async row => JSON.parse(new TextDecoder().decode(await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: unbase64(row.iv) }, row.key, unbase64(row.payload))))));
}
export async function removeDevice(room) { await operation('readwrite', store => store.delete(room)); }
