// AiNKI service worker: keeps the offline study page, styles and card images
// available without a connection.
var SHELL = 'ainki-shell-v3';
var MEDIA = 'ainki-media';
var SHELL_URLS = [
  '/offline', '/style.css', '/offline.js', '/theme.js', '/sketch.css', '/paper.svg', '/scribble.svg',
  '/fonts/alegreya-sans-400-latin.woff2', '/fonts/alegreya-sans-700-latin.woff2', '/fonts/patrick-hand-400-latin.woff2'
];

self.addEventListener('install', function (e) {
  e.waitUntil(
    caches.open(SHELL)
      .then(function (c) { return c.addAll(SHELL_URLS); })
      .then(function () { return self.skipWaiting(); })
  );
});

self.addEventListener('activate', function (e) {
  e.waitUntil(
    caches.keys()
      .then(function (keys) {
        return Promise.all(keys
          .filter(function (k) { return k !== SHELL && k !== MEDIA; })
          .map(function (k) { return caches.delete(k); }));
      })
      .then(function () { return self.clients.claim(); })
  );
});

self.addEventListener('fetch', function (e) {
  var req = e.request;
  if (req.method !== 'GET') return;
  var url = new URL(req.url);
  if (url.origin !== self.location.origin) return;

  // Images never change (new image = new id): cache first.
  if (url.pathname.indexOf('/media/') === 0) {
    e.respondWith(caches.open(MEDIA).then(function (c) {
      return c.match(req.url).then(function (hit) {
        return hit || fetch(req).then(function (r) {
          if (r.ok) c.put(req.url, r.clone());
          return r;
        });
      });
    }));
    return;
  }

  // App shell: network first, cached copy when offline.
  if (SHELL_URLS.indexOf(url.pathname) >= 0) {
    e.respondWith(fetch(req).then(function (r) {
      if (r.ok) {
        var copy = r.clone();
        caches.open(SHELL).then(function (c) { c.put(url.pathname, copy); });
      }
      return r;
    }).catch(function () { return caches.match(url.pathname); }));
    return;
  }

  // Any other page while offline: the offline study page.
  if (req.mode === 'navigate') {
    e.respondWith(fetch(req).catch(function () { return caches.match('/offline'); }));
  }
});
