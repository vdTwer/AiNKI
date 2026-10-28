// The desktop look's extras. It runs only in the Orchid (calm) look, which
// always has JavaScript; the simple look never sees any of this, and every
// page works without it.
(function () {
  'use strict';
  var root = document.documentElement;
  if (!root.classList.contains('calm')) return;

  var UNITS = { m: 'minute', h: 'hour', d: 'day', mo: 'month', y: 'year' };

  // "6d" becomes "in 6 days", "<1m" "in under a minute".
  function words(t) {
    if (t === '<1m') return 'in under a minute';
    var m = /^(\d+(?:\.\d+)?)(mo|m|h|d|y)$/.exec(t);
    if (!m) return t;
    var n = m[1].replace(/\.0$/, '');
    return 'in ' + n + ' ' + UNITS[m[2]] + (n === '1' ? '' : 's');
  }

  function dress(scope) {
    // Home: the number of cards to study is set in the light italic.
    var count = scope.querySelector('.today h1 .count');
    if (count && count.firstChild && count.firstChild.nodeType === 3) {
      var m = /^(\d[\d,.\s]*)(.*)$/.exec(count.firstChild.nodeValue);
      if (m) {
        var n = document.createElement('span');
        n.className = 'n';
        n.textContent = m[1].trim();
        count.firstChild.nodeValue = ' ' + m[2].trim();
        count.insertBefore(n, count.firstChild);
      }
    }

    // Studying: when a card comes back, in words.
    Array.prototype.forEach.call(scope.querySelectorAll('.study .answers .ivl:not([data-remembered])'), function (e) {
      e.textContent = words(e.textContent.trim());
    });

    // A ⋯ button opens the menu it controls; Escape or a click elsewhere
    // closes it.
    Array.prototype.forEach.call(scope.querySelectorAll('button.more-toggle[aria-controls]'), function (b) {
      var menu = document.getElementById(b.getAttribute('aria-controls'));
      if (!menu || b.hasAttribute('data-ready')) return;
      b.setAttribute('data-ready', '');
      // The menu hangs from the button, under it.
      b.parentNode.appendChild(menu);
      var set = function (open) {
        menu.classList.toggle('open', open);
        b.setAttribute('aria-expanded', open ? 'true' : 'false');
      };
      b.addEventListener('click', function (e) {
        e.stopPropagation();
        set(!menu.classList.contains('open'));
        if (menu.classList.contains('open')) { var first = menu.querySelector('a, button'); if (first) first.focus(); }
      });
      document.addEventListener('click', function (e) { if (!menu.contains(e.target)) set(false); });
      document.addEventListener('keydown', function (e) { if (e.key === 'Escape' && menu.classList.contains('open')) { set(false); b.focus(); } });
    });
  }

  // A break counts down here and then sends the "keep going" form, so the
  // next card comes back by itself. The end time survives a reload.
  function breakTimer(scope) {
    var start = scope.querySelector('.break-start [data-break]');
    var run = scope.querySelector('.break-run');
    var form = document.getElementById('keep-going');
    if (!start || !run || !form) return;
    var section = run.closest('.break');
    var clock = run.querySelector('.break-clock');
    var fill = run.querySelector('.break-fill');
    var KEY = 'ainki.break';
    var tick = null;
    var get = function () { try { return JSON.parse(sessionStorage.getItem(KEY)); } catch (e) { return null; } };
    var put = function (v) { try { if (v) sessionStorage.setItem(KEY, JSON.stringify(v)); else sessionStorage.removeItem(KEY); } catch (e) { /* storage blocked */ } };
    var show = function (b) {
      var left = Math.max(0, b.until - Date.now());
      var s = Math.ceil(left / 1000);
      clock.textContent = Math.floor(s / 60) + ':' + ('0' + (s % 60)).slice(-2);
      fill.style.width = (100 - left / b.length * 100).toFixed(2) + '%';
      if (left <= 0) { clearInterval(tick); put(null); form.requestSubmit ? form.requestSubmit() : form.submit(); }
    };
    var go = function (b) {
      section.classList.add('running');
      run.hidden = false;
      show(b);
      tick = setInterval(function () { show(b); }, 250);
    };
    start.addEventListener('click', function () {
      var ms = +start.getAttribute('data-break') * 1000;
      var b = { until: Date.now() + ms, length: ms };
      put(b);
      go(b);
    });
    // Ending the break early, or skipping it, forgets it.
    form.addEventListener('submit', function () { clearInterval(tick); put(null); });
    var saved = get();
    if (saved && saved.until > Date.now()) go(saved); else put(null);
  }

  dress(document);
  breakTimer(document);
  // Studying swaps in the next page without a reload.
  document.addEventListener('ainki:page', function (e) { dress(e.detail || document); });
})();
