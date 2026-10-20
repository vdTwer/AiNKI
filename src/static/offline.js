// AiNKI offline support. On normal pages it keeps today's study queue and
// card images on the device and sends answers given offline. On /offline it
// runs a small study client from that queue.
(function () {
  'use strict';
  var QUEUE = 'ainki.queue';
  var PENDING = 'ainki.pending';
  var UNDO = 'ainki.undo';
  var REFRESH_SECS = 60;
  var LEARN_AHEAD_SECS = 20 * 60;

  function load(key, fallback) {
    try {
      var v = localStorage.getItem(key);
      return v ? JSON.parse(v) : fallback;
    } catch (e) { return fallback; }
  }
  function save(key, value) {
    try { localStorage.setItem(key, JSON.stringify(value)); } catch (e) { /* storage full or blocked */ }
  }
  function now() { return Math.floor(Date.now() / 1000); }
  function uid() {
    if (window.crypto && crypto.randomUUID) return crypto.randomUUID();
    return Date.now().toString(36) + '-' + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
  }
  function $(id) { return document.getElementById(id); }

  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js').catch(function () {});
  }

  var body = document.body;

  // Signup: send the browser's timezone along.
  var tz = document.getElementById('tz');
  if (tz) {
    try { tz.value = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch (e) {}
  }

  // Card form: only show the new deck name when "New deck…" is picked.
  var deckSelect = document.querySelector('[data-new-deck-select]');
  var newDeckField = document.getElementById('new-deck-field');
  if (deckSelect && newDeckField) {
    var toggle = function () {
      var isNew = deckSelect.value === 'new';
      newDeckField.hidden = !isNew;
      var input = newDeckField.querySelector('input');
      if (input) input.required = isNew;
    };
    deckSelect.addEventListener('change', function () {
      toggle();
      if (deckSelect.value === 'new') newDeckField.querySelector('input').focus();
    });
    toggle();
  }

  // Keyboard shortcuts on the (online) study pages: space shows the answer,
  // 1-4 answer. Everything also works without them.
  if (body.getAttribute('data-page') !== 'offline') {
    document.addEventListener('keydown', function (e) {
      if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') return;
      var k = e.key === ' ' || e.key === 'Enter' ? ' ' : e.key;
      if (!/^[ 1-4]$/.test(k)) return;
      var el = document.querySelector('[data-key="' + k + '"]');
      if (!el && k === ' ') el = document.querySelector('[data-key="3"]');
      if (el) { e.preventDefault(); el.click(); }
    });
  }
  var pageUser = body.getAttribute('data-user');
  var isOfflinePage = body.getAttribute('data-page') === 'offline';

  // Logged out: drop the downloaded queue. Unsent answers stay until that
  // user logs in again.
  if (document.querySelector('[data-clear-offline]')) {
    try { localStorage.removeItem(QUEUE); localStorage.removeItem(UNDO); } catch (e) {}
    if ('caches' in window) caches.delete('ainki-media').catch(function () {});
  }

  function pendingFor(userId) {
    return load(PENDING, []).filter(function (a) { return String(a.user_id) === String(userId); });
  }

  var syncing = null;
  function sync(userId) {
    if (syncing) return syncing;
    var mine = pendingFor(userId);
    if (!userId || !mine.length || !navigator.onLine) return Promise.resolve(false);
    syncing = fetch('/api/offline/sync', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ answers: mine })
    }).then(function (r) {
      if (!r.ok) throw new Error('sync failed: ' + r.status);
      return r.json();
    }).then(function (res) {
      // Everything the server looked at is settled, including stale or
      // invalid answers that would never succeed on a retry.
      var settled = {};
      res.results.forEach(function (x) { settled[x.client_id] = true; });
      save(PENDING, load(PENDING, []).filter(function (a) { return !settled[a.client_id]; }));
      return true;
    }).catch(function () { return false; }).then(function (ok) {
      syncing = null;
      return ok;
    });
    return syncing;
  }

  function warmImages(queue) {
    if (!('caches' in window)) return;
    var wanted = {};
    queue.items.forEach(function (i) {
      if (i.front_image) wanted[i.front_image] = true;
      if (i.back_image) wanted[i.back_image] = true;
    });
    caches.open('ainki-media').then(function (c) {
      c.keys().then(function (keys) {
        keys.forEach(function (k) {
          var path = new URL(k.url).pathname;
          if (!wanted[path]) c.delete(k);
        });
      });
      Object.keys(wanted).forEach(function (u) {
        c.match(u).then(function (hit) {
          if (hit) return;
          fetch(u, { credentials: 'same-origin' }).then(function (r) { if (r.ok) c.put(u, r); });
        });
      });
    }).catch(function () {});
  }

  function refresh(userId, force) {
    var q = load(QUEUE, null);
    var fresh = q && String(q.user_id) === String(userId) && now() - q.fetched_at < REFRESH_SECS;
    if (!navigator.onLine || (fresh && !force)) return Promise.resolve(q);
    // Don't replace local progress that hasn't reached the server yet.
    if (pendingFor(userId).length) return Promise.resolve(q);
    return fetch('/api/offline/queue', { credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) throw new Error('queue failed: ' + r.status);
      return r.json();
    }).then(function (data) {
      data.fetched_at = now();
      save(QUEUE, data);
      warmImages(data);
      return data;
    }).catch(function () { return q; });
  }

  function showStatus(userId) {
    var el = $('offline-status');
    if (!el) return;
    var q = load(QUEUE, null);
    var parts = [];
    if (q && String(q.user_id) === String(userId)) {
      var left = q.items.filter(function (i) { return !i.done; }).length;
      parts.push(left + ' card' + (left === 1 ? '' : 's') + ' ready for offline study');
    }
    var p = pendingFor(userId).length;
    if (p) parts.push(p + ' answer' + (p === 1 ? '' : 's') + ' waiting to sync');
    el.textContent = parts.join(' · ');
  }

  // ----- normal pages ------------------------------------------------------
  if (!isOfflinePage) {
    if (pageUser) {
      // The home page always fetches a fresh queue; elsewhere at most once a minute.
      var force = body.getAttribute('data-page') === 'home';
      sync(pageUser).then(function () { return refresh(pageUser, force); }).then(function () { showStatus(pageUser); });
      window.addEventListener('online', function () { sync(pageUser).then(function () { showStatus(pageUser); }); });
    }
    return;
  }

  // ----- the offline study client -----------------------------------------
  var queue = load(QUEUE, null);
  var current = null;
  var shownAt = 0;

  function persist() { save(QUEUE, queue); }

  function setText(id, text) { var el = $(id); if (el) el.textContent = text; }

  function connectionNote() {
    var p = queue ? pendingFor(queue.user_id).length : 0;
    var text = navigator.onLine ? 'Online' : 'Offline: answers are saved on this device';
    if (p) text += ' · ' + p + ' to sync';
    setText('o-conn', text);
  }

  function renderFace(el, html, image) {
    el.innerHTML = '';
    if (image) {
      var img = document.createElement('img');
      img.src = image;
      img.alt = '';
      el.appendChild(img);
    }
    if (html) {
      var div = document.createElement('div');
      div.className = 'md';
      // Rendered and sanitized by the server.
      div.innerHTML = html;
      el.appendChild(div);
    }
  }

  function pick() {
    var t = now();
    var open = queue.items.filter(function (i) { return !i.done; });
    var waiting = open.filter(function (i) { return i.at > 0; }).sort(function (a, b) { return a.at - b.at; });
    if (waiting.length && waiting[0].at <= t) return { item: waiting[0] };
    for (var k = 0; k < open.length; k++) if (!(open[k].at > 0)) return { item: open[k] };
    if (waiting.length && waiting[0].at <= t + LEARN_AHEAD_SECS) return { item: waiting[0] };
    if (waiting.length) return { later: waiting[0].at };
    return {};
  }

  function render() {
    connectionNote();
    if (queue && queue.font_scale) {
      document.documentElement.style.setProperty('--scale', String(queue.font_scale / 100));
    }
    if (!queue || !queue.items) {
      $('o-study').hidden = true;
      setText('o-msg', 'Nothing downloaded yet. Open AiNKI once while online to get today’s cards on this device.');
      $('o-msg').hidden = false;
      return;
    }
    var next = pick();
    current = next.item || null;
    if (!current) {
      $('o-study').hidden = true;
      var msg = 'Done for now.';
      if (next.later) {
        msg += ' Next card at ' + new Date(next.later * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) + '.';
      }
      if (now() > queue.valid_until) msg = 'These cards are from yesterday. Go online to get today’s cards.';
      setText('o-msg', msg);
      $('o-msg').hidden = false;
      return;
    }
    $('o-msg').hidden = true;
    $('o-study').hidden = false;
    var left = queue.items.filter(function (i) { return !i.done; }).length;
    setText('o-deck', current.deck);
    setText('o-left', current.state + ' · ' + left + ' left');
    renderFace($('o-front'), current.front, current.front_image);
    renderFace($('o-back'), current.back, current.back_image);
    $('o-backwrap').hidden = true;
    $('o-show').hidden = false;
    shownAt = now();
    renderButtons();
    window.scrollTo(0, 0);
  }

  function renderButtons() {
    var box = $('o-answers');
    box.innerHTML = '';
    var buttons = queue.four_buttons
      ? [['again', 'Again', '1'], ['hard', 'Hard', '2'], ['good', 'Good', '3'], ['easy', 'Easy', '4']]
      : [['again', 'Again', '1'], ['good', 'Good', '3']];
    box.className = 'answers ' + (queue.four_buttons ? 'cols-4' : 'cols-2');
    buttons.forEach(function (b) {
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'btn' + (b[0] === 'good' ? ' primary' : '');
      btn.setAttribute('data-key', b[2]);
      var label = document.createElement('span');
      label.className = 'label';
      label.textContent = b[1];
      var ivl = document.createElement('span');
      ivl.className = 'ivl';
      var p = !current.local && current.previews && current.previews[b[0]];
      ivl.textContent = p ? p.label : '';
      btn.appendChild(label);
      btn.appendChild(ivl);
      btn.addEventListener('click', function () { answer(b[0]); });
      box.appendChild(btn);
    });
  }

  function reveal() {
    if (!current) return;
    $('o-show').hidden = true;
    $('o-backwrap').hidden = false;
  }

  function answer(rating) {
    if (!current) return;
    var item = current;
    var snapshot = JSON.parse(JSON.stringify(item));
    var t = now();
    // The server's preview is exact for the first answer. After that the card
    // is in a learning step; repeat it in 10 minutes if forgotten. The server
    // works out the real schedule when the answers are synced.
    var secs;
    var p = !item.local && item.previews && item.previews[rating];
    if (p) secs = p.secs;
    else secs = (rating === 'again' || rating === 'hard') ? 600 : 86400;
    var entry = {
      client_id: uid(),
      user_id: queue.user_id,
      card_id: item.id,
      rating: rating,
      reps: item.reps,
      answered_at: t,
      duration_ms: Math.min(60, Math.max(0, t - shownAt)) * 1000
    };
    var pending = load(PENDING, []);
    pending.push(entry);
    save(PENDING, pending);
    save(UNDO, { item: snapshot, client_id: entry.client_id });

    item.reps += 1;
    item.local = true;
    if (secs < 86400) {
      item.at = t + secs;
      item.state = 'learning';
    } else {
      item.done = true;
    }
    persist();
    render();
    sync(queue.user_id).then(connectionNote);
  }

  function undo() {
    var u = load(UNDO, null);
    if (!u || !u.client_id) return flash('Nothing to undo.');
    var pending = load(PENDING, []);
    var still = pending.some(function (a) { return a.client_id === u.client_id; });
    if (!still) return flash('That answer was already synced. Use Undo on the normal study page.');
    save(PENDING, pending.filter(function (a) { return a.client_id !== u.client_id; }));
    for (var k = 0; k < queue.items.length; k++) {
      if (queue.items[k].id === u.item.id) queue.items[k] = u.item;
    }
    save(UNDO, null);
    persist();
    render();
  }

  function flash(text) {
    setText('o-msg', text);
    $('o-msg').hidden = false;
  }

  $('o-show').addEventListener('click', function (e) { e.preventDefault(); reveal(); });
  $('o-undo').addEventListener('click', undo);
  document.addEventListener('keydown', function (e) {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
    var k = e.key === ' ' || e.key === 'Enter' ? ' ' : e.key;
    if (k === ' ' && !$('o-show').hidden) { e.preventDefault(); reveal(); return; }
    if ($('o-backwrap').hidden) return;
    var btn = document.querySelector('#o-answers [data-key="' + (k === ' ' ? '3' : k) + '"]');
    if (btn) { e.preventDefault(); btn.click(); }
  });
  window.addEventListener('online', function () {
    if (!queue) return;
    sync(queue.user_id).then(connectionNote);
  });
  window.addEventListener('offline', connectionNote);
  // Learning cards become due while the page is open.
  setInterval(function () { if (!current) render(); }, 30000);

  // Opened while online: send what's waiting and get a fresh queue.
  if (navigator.onLine && queue) {
    sync(queue.user_id).then(function () { return refresh(queue.user_id, true); }).then(function (q) {
      if (q) queue = q;
      render();
    });
  } else if (navigator.onLine) {
    fetch('/api/offline/queue', { credentials: 'same-origin' }).then(function (r) {
      return r.ok ? r.json() : null;
    }).then(function (data) {
      if (data) { data.fetched_at = now(); save(QUEUE, data); warmImages(data); queue = data; }
      render();
    }).catch(render);
  }
  render();
})();
