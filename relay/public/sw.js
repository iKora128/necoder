const CACHE = 'necoder-control-v1';
const ASSETS = ['/', '/index.html', '/app.mjs', '/crypto.mjs', '/storage.mjs', '/i18n.mjs', '/style.css', '/icon.svg', '/icon-192.png', '/icon-512.png', '/manifest.webmanifest'];
self.addEventListener('install', event => event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS))));
self.addEventListener('activate', event => event.waitUntil((async () => {
  for (const key of await caches.keys()) if (key.startsWith('necoder-control-') && key !== CACHE) await caches.delete(key);
  await self.clients.claim();
})()));
self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET' || url.origin !== self.location.origin || !ASSETS.includes(url.pathname)) return;
  event.respondWith((async () => {
    try {
      const response = await fetch(event.request);
      if (response.ok) { const cache = await caches.open(CACHE); await cache.put(event.request, response.clone()); }
      return response;
    } catch { return await caches.match(event.request) || Response.error(); }
  })());
});
self.addEventListener('push', event => {
  // 本文を信用してリンクやコマンドを作らない。通知は常に固定文・固定オリジン。
  event.waitUntil(self.registration.showNotification('necoder', {
    body: 'エージェントが回答を待っています / Your agent needs attention',
    tag: 'necoder-attention', icon: '/icon-192.png', badge: '/icon-192.png',
  }));
});
self.addEventListener('notificationclick', event => {
  event.notification.close();
  event.waitUntil((async () => {
    for (const client of await self.clients.matchAll({ type: 'window', includeUncontrolled: true })) {
      if (new URL(client.url).origin === self.location.origin) { await client.focus(); client.postMessage({ type: 'refresh' }); return; }
    }
    await self.clients.openWindow('/');
  })());
});
