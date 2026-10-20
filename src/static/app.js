// AiNKI's browser script. Everything works without it; with it, studying
// never reloads the page: the answer side is revealed in place, and answers
// and undo are sent in the background with the next card swapped in.
(function () {
  'use strict';

  // Offline studying was removed; take down the service worker and the
  // caches that earlier versions left behind.
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.getRegistrations().then(function (regs) {
      regs.forEach(function (r) { r.unregister(); });
    }).catch(function () {});
  }
  if ('caches' in window) {
    ['ainki-media', 'ainki-shell-v4'].forEach(function (k) { caches.delete(k).catch(function () {}); });
  }
  try {
    ['ainki.queue', 'ainki.pending', 'ainki.undo', 'ainki.progress'].forEach(function (k) { localStorage.removeItem(k); });
  } catch (e) { /* storage blocked */ }

  var root = document.documentElement;
  var calm = root.classList.contains('calm');
  var motion = !(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);

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

  // Mark the current section in the menu.
  (function () {
    var path = location.pathname;
    var section =
      path === '/' || path.indexOf('/decks/') === 0 && path !== '/decks/new' ? 'decks' :
      path === '/study' ? 'study' :
      path === '/cards/new' || path === '/decks/new' || path === '/import' ? 'add' :
      path === '/stats' ? 'stats' :
      path === '/settings' || path.indexOf('/presets') === 0 || path === '/tokens' ? 'settings' : null;
    if (!section) return;
    var links = document.querySelectorAll('nav.top a[data-nav="' + section + '"]');
    for (var i = 0; i < links.length; i++) links[i].setAttribute('aria-current', 'page');
  })();

  // ----- page details (run again after every swap) --------------------------

  // Buttons are small cards: while the pointer is over one it tilts toward
  // it (the side under the pointer dips away), then springs back. Small
  // ones get a closer perspective and a steeper angle, or the tilt would
  // be too faint to read as one.
  function tilt(el) {
    if (el.closest('.quiet-links')) return;
    var row = el.matches('a.row');
    var base = row ? 3 : el.closest('.answers') ? 10 : 8;
    el.addEventListener('pointerenter', function () {
      var w = el.getBoundingClientRect().width;
      el.style.setProperty('--persp', (row ? 700 : Math.max(240, Math.min(700, w * 3))).toFixed(0) + 'px');
    });
    el.addEventListener('pointermove', function (e) {
      var r = el.getBoundingClientRect();
      var max = row ? base : base * Math.max(1, Math.min(1.8, 220 / r.width));
      var x = (e.clientX - r.left) / r.width;
      var y = (e.clientY - r.top) / r.height;
      var s = el.style;
      s.setProperty('--dur', '90ms');
      s.setProperty('--spring', 'linear');
      s.setProperty('--rx', ((0.5 - y) * max).toFixed(2) + 'deg');
      s.setProperty('--ry', ((x - 0.5) * max).toFixed(2) + 'deg');
      s.setProperty('--sx', ((0.5 - x) * 10).toFixed(1) + 'px');
      s.setProperty('--sy', (2 + (0.5 - y) * 8).toFixed(1) + 'px');
      s.setProperty('--gx', (x * 100).toFixed(1) + '%');
      s.setProperty('--gy', (y * 100).toFixed(1) + '%');
    });
    el.addEventListener('pointerleave', function () {
      var s = el.style;
      s.removeProperty('--dur');
      s.removeProperty('--spring');
      s.setProperty('--rx', '0deg');
      s.setProperty('--ry', '0deg');
      s.setProperty('--sx', '0px');
      s.setProperty('--sy', '2px');
    });
  }

  function setup(scope) {
    // Long card text is set smaller and left-aligned.
    var faces = scope.querySelectorAll('section.face');
    for (var f = 0; f < faces.length; f++) {
      if ((faces[f].textContent || '').trim().length > 90) faces[f].classList.add('long');
    }
    if (calm && motion) {
      // Hidden buttons have no size yet; they are set up when shown.
      var els = scope.querySelectorAll('.btn, button:not(.link), a.row');
      for (var t = 0; t < els.length; t++) {
        if (!els[t].closest('[hidden]') && !els[t].hasAttribute('data-tilt')) {
          els[t].setAttribute('data-tilt', '');
          tilt(els[t]);
        }
      }
    }
  }
  setup(document);

  // ----- studying without reloads ---------------------------------------------

  function main() { return document.querySelector('main'); }
  function studying() { return !!document.querySelector('main .study, main .session-end'); }

  function reveal() {
    var study = document.querySelector('.study:not(.revealed)');
    if (!study) return false;
    study.classList.add('revealed');
    var hidden = study.querySelectorAll('[hidden]');
    for (var i = 0; i < hidden.length; i++) hidden[i].hidden = false;
    var show = study.querySelector('.reveal');
    if (show) show.hidden = true;
    setup(study);
    return true;
  }

  var busy = false;

  // Put the page the server answered with in place of the current one.
  function swap(html, url) {
    var doc = new DOMParser().parseFromString(html, 'text/html');
    var next = doc.querySelector('main');
    if (!next) throw new Error('no page');
    var oldBar = document.querySelector('.study-foot .bar');
    var from = oldBar ? oldBar.getAttribute('data-progress') : null;
    var wasStudying = !!document.querySelector('main .study');
    document.title = doc.title;
    var page = doc.body.getAttribute('data-page');
    if (page) document.body.setAttribute('data-page', page); else document.body.removeAttribute('data-page');
    main().replaceWith(next);
    // A card that arrives already turned over never offers "Show answer".
    var shown = next.querySelector('.study.revealed .reveal');
    if (shown) shown.hidden = true;
    if (url) history.replaceState(null, '', url.replace(/#.*$/, ''));
    window.scrollTo(0, 0);
    setup(next);
    // The progress line glides on from where it was.
    var bar = next.querySelector('.study-foot .bar');
    if (bar && from !== null && calm) {
      var to = bar.getAttribute('data-progress');
      bar.style.transition = 'none';
      bar.style.width = from + '%';
      void bar.offsetWidth;
      bar.style.transition = '';
      requestAnimationFrame(function () { bar.style.width = to + '%'; });
    }
    if (next.querySelector('.study')) next.querySelector('.study').classList.add('entering');
    // The closing animation is for the answer that ended the session, not
    // for opening the page again later.
    var end = next.querySelector('.session-end');
    if (end && wasStudying) end.classList.add('finishing');
  }

  function send(form, submitter) {
    if (busy) return;
    busy = true;
    var data = new URLSearchParams(new FormData(form));
    if (submitter && submitter.name) data.append(submitter.name, submitter.value);
    var area = document.querySelector('.study .card-area');
    if (area) area.classList.add('leaving');
    fetch(form.action, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: data
    }).then(function (r) {
      if (!r.ok || r.redirected && new URL(r.url).pathname !== '/study') throw new Error('unexpected answer');
      return r.text().then(function (html) { swap(html, r.url); });
    }).catch(function () {
      // Something unusual (logged out, server error): do it the plain way.
      if (submitter && submitter.name) {
        var hidden = document.createElement('input');
        hidden.type = 'hidden';
        hidden.name = submitter.name;
        hidden.value = submitter.value;
        form.appendChild(hidden);
      }
      form.submit();
    }).then(function () { busy = false; });
  }

  document.addEventListener('click', function (e) {
    var show = e.target.closest && e.target.closest('.study .reveal a.show');
    if (show && reveal()) e.preventDefault();
  });

  document.addEventListener('submit', function (e) {
    var form = e.target;
    var action = form.getAttribute('action');
    if (!studying() || (action !== '/study/answer' && action !== '/study/undo')) return;
    e.preventDefault();
    send(form, e.submitter);
  });

  // Keyboard: space shows the answer (then means Good), 1-4 answer.
  document.addEventListener('keydown', function (e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') return;
    if (!document.querySelector('main .study')) return;
    var k = e.key === ' ' || e.key === 'Enter' ? ' ' : e.key;
    if (!/^[ 1-4]$/.test(k)) return;
    e.preventDefault();
    if (busy) return;
    if (k === ' ' && reveal()) return;
    var visible = function (key) {
      var els = document.querySelectorAll('main [data-key="' + key + '"]');
      for (var i = 0; i < els.length; i++) if (!els[i].closest('[hidden]')) return els[i];
      return null;
    };
    var el = visible(k) || (k === ' ' ? visible('3') : null);
    if (el) el.click();
  });
})();
