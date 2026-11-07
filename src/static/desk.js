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

    // Home: the week's note stays hidden; when the coach has something to
    // say, a ! beside the heading shows it on hover or focus.
    var tip = scope.querySelector('.today-side .week .tip');
    var heading = scope.querySelector('.today h1');
    if (tip && heading && !heading.querySelector('.coach-tip')) {
      var coach = document.createElement('span');
      coach.className = 'coach-tip';
      var mark = document.createElement('button');
      mark.type = 'button';
      mark.className = 'coach-mark';
      mark.setAttribute('aria-label', 'A note from the coach');
      // The note icon: a ring with a bang, drawn like the page icons.
      mark.innerHTML = '<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5v5.5"/><path d="M12 16.4v.1"/></svg>';
      var note = document.createElement('span');
      note.className = 'coach-note';
      note.setAttribute('role', 'tooltip');
      note.appendChild(tip.firstElementChild ? tip.firstElementChild.cloneNode(true) : tip.cloneNode(true));
      coach.appendChild(mark);
      coach.appendChild(note);
      heading.appendChild(coach);
    }

    // Studying: when a card comes back, in words.
    Array.prototype.forEach.call(scope.querySelectorAll('.study .answers .ivl:not([data-remembered])'), function (e) {
      e.textContent = words(e.textContent.trim());
    });

    // A ⋯ button, or the words of Add's title, opens the menu it controls;
    // Escape or a click elsewhere closes it.
    Array.prototype.forEach.call(scope.querySelectorAll('button.more-toggle[aria-controls], button.add-pick[aria-controls]'), function (b) {
      var menu = document.getElementById(b.getAttribute('aria-controls'));
      if (!menu || b.hasAttribute('data-ready')) return;
      b.setAttribute('data-ready', '');
      // The menu hangs from the button, under it. Add's stays beside its
      // title, since a menu is not part of a heading.
      if (!b.classList.contains('add-pick')) b.parentNode.appendChild(menu);
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
    if (!start || !run || !form || start.hasAttribute('data-ready')) return;
    start.setAttribute('data-ready', '');
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
    // On the desktop a break simply starts; skipping it is one click.
    var saved = get();
    if (saved && saved.until > Date.now()) go(saved); else { put(null); start.click(); }
  }

  // A deck name with "::" reads as a path: each finished part becomes a
  // chip, the last part is typed. The real field keeps "A::B" and is sent
  // as before. Backspace at the start, or a click on a chip, turns it all
  // back into text to edit; leaving the field makes the chips again.
  function pathField(input) {
    if (input.hasAttribute('data-path')) return;
    input.setAttribute('data-path', '');
    var box = document.createElement('span');
    box.className = 'path-field';
    var chips = document.createElement('span');
    chips.className = 'path-chips';
    var typed = document.createElement('input');
    typed.type = 'text';
    typed.className = 'path-input';
    typed.placeholder = input.placeholder;
    typed.autocomplete = 'off';
    if (input.maxLength > 0) typed.maxLength = input.maxLength;
    typed.required = input.required;
    box.appendChild(chips);
    box.appendChild(typed);
    input.parentNode.insertBefore(box, input);
    var list = [];
    var editing = false;
    var render = function () {
      chips.textContent = '';
      list.forEach(function (p) {
        var c = document.createElement('button');
        c.type = 'button';
        c.className = 'chip';
        c.textContent = p;
        c.addEventListener('click', function () { unwrap(); typed.focus(); });
        var sep = document.createElement('span');
        sep.className = 'sep';
        sep.setAttribute('aria-hidden', 'true');
        sep.textContent = '›';
        chips.appendChild(c);
        chips.appendChild(sep);
      });
    };
    var sync = function () { input.value = list.concat([typed.value]).join('::'); };
    var split = function () {
      var segs = typed.value.split('::');
      if (segs.length < 2) return;
      var last = segs.pop();
      segs.forEach(function (s) { s = s.trim(); if (s) list.push(s); });
      typed.value = last.replace(/^\s+/, '');
      render();
    };
    var unwrap = function () {
      if (!list.length) return;
      var at = list.join('::').length + 2;
      typed.value = list.join('::') + '::' + typed.value;
      list = [];
      render();
      editing = true;
      typed.setSelectionRange(at, at);
    };
    typed.addEventListener('input', function () { if (!editing) split(); sync(); });
    typed.addEventListener('blur', function () { editing = false; split(); sync(); });
    typed.addEventListener('keydown', function (e) {
      if (e.key === 'Backspace' && list.length && typed.selectionStart === 0 && typed.selectionEnd === 0) {
        e.preventDefault();
        unwrap();
      }
    });
    box.addEventListener('click', function (e) { if (e.target === box) typed.focus(); });
    var focused = document.activeElement === input;
    typed.value = input.value;
    split();
    input.type = 'hidden';
    sync();
    if (focused || input.autofocus) typed.focus();
  }

  // ----- stats charts ------------------------------------------------------
  // Drawn from the numbers the server put on each .chart. Colours come from
  // classes, so dark mode follows calm.css. Every value shown on hover is
  // also in the tables behind "I love numbers".
  var SVG = 'http://www.w3.org/2000/svg';
  function el(name, attrs, parent) {
    var e = document.createElementNS(SVG, name);
    for (var k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }
  function text(parent, x, y, s, cls, anchor) {
    var t = el('text', { x: x, y: y, 'class': cls || 'axis-text', 'text-anchor': anchor || 'start' }, parent);
    t.textContent = s;
    return t;
  }
  function pct(v) { return Math.round(v * 100) + '%'; }

  // One tooltip for every chart: the value, what it is, and in a sentence
  // what it means. It stays inside the window and flips below its mark
  // when there is no room above. Its size is measured only when its words
  // change, so following the pointer costs no layout.
  var tip = null, tipKey = '', tipW = 0, tipH = 0;
  function line(tag, cls, s) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    e.textContent = s;
    return e;
  }
  function showTip(e, value, label, why) {
    if (!tip) {
      tip = document.createElement('div');
      tip.className = 'chart-tip';
      tip.setAttribute('role', 'status');
      document.body.appendChild(tip);
    }
    var key = [value, label, why].join('\n');
    tip.hidden = false;
    if (key !== tipKey) {
      tipKey = key;
      tip.textContent = '';
      if (value) tip.appendChild(line('strong', '', value));
      if (label) tip.appendChild(line('span', '', label));
      if (why) tip.appendChild(line('span', 'why', why));
      tipW = tip.offsetWidth;
      tipH = tip.offsetHeight;
    }
    var r = e.target.getBoundingClientRect ? e.target.getBoundingClientRect() : null;
    var pointer = e.clientX !== undefined && e.type !== 'focus';
    var x = pointer ? e.clientX : r.left + r.width / 2;
    var y = pointer ? e.clientY : r.top;
    var below = y - tipH - 12 < 8;
    if (below && !pointer) y = r.bottom;
    x = Math.max(8 + tipW / 2, Math.min(window.innerWidth - 8 - tipW / 2, x));
    tip.classList.toggle('below', below);
    tip.style.left = x + 'px';
    tip.style.top = y + 'px';
  }
  function hideTip() { if (tip) tip.hidden = true; }
  // A mark answers the pointer and the keyboard alike. `on` hears when it
  // is shown and hidden, to light the mark up.
  function hover(mark, value, label, why, on) {
    mark.setAttribute('tabindex', '0');
    mark.setAttribute('aria-label', value + ', ' + label + (why ? '. ' + why : ''));
    ['pointermove', 'focus'].forEach(function (t) {
      mark.addEventListener(t, function (e) { showTip(e, value, label, why); if (on) on(true); });
    });
    ['pointerleave', 'blur'].forEach(function (t) {
      mark.addEventListener(t, function () { hideTip(); if (on) on(false); });
    });
  }
  // Words on the page that explain themselves on hover (data-why from the
  // server). Their own text stays what a screen reader reads first.
  function explainers(scope) {
    Array.prototype.forEach.call(scope.querySelectorAll('.desk-stats [data-why]'), function (x) {
      var why = x.getAttribute('data-why');
      var value = x.getAttribute('data-tip') || '';
      var label = x.getAttribute('data-tip-label') || '';
      if (!x.matches('a, button, [tabindex]')) x.setAttribute('tabindex', '0');
      x.setAttribute('aria-description', [value, label, why].filter(Boolean).join('. '));
      ['pointermove', 'focus'].forEach(function (t) { x.addEventListener(t, function (e) { showTip(e, value, label, why); }); });
      ['pointerleave', 'blur'].forEach(function (t) { x.addEventListener(t, hideTip); });
    });
  }
  function weeksAgo(i, n) { var k = n - 1 - i; return k === 0 ? 'this week' : k === 1 ? 'last week' : k + ' weeks ago'; }

  var charts = {
    // Recall per week against the aim: one Orchid line, the aim dashed.
    retention: function (box) {
      var weeks = box.getAttribute('data-weeks').split(';').map(function (w) {
        var p = w.split(',');
        return { n: +p[0], p: +p[1], v: +p[0] > 0 ? +p[1] / +p[0] : null };
      });
      var aim = parseFloat(box.getAttribute('data-aim'));
      var vals = weeks.filter(function (w) { return w.v !== null; }).map(function (w) { return w.v; });
      if (!vals.length) { box.classList.add('empty'); box.textContent = 'No reviews of learned cards in the last twelve weeks.'; return; }
      var lo = Math.floor((Math.min.apply(null, vals.concat(isNaN(aim) ? [] : [aim])) - 0.03) * 20) / 20;
      lo = Math.max(0, Math.min(lo, 0.8));
      var W = 680, H = 230, L = 40, R = 8, T = 12, B = 28;
      var X = function (i) { return L + i * (W - L - R) / (weeks.length - 1); };
      var Y = function (v) { return T + (1 - v) / (1 - lo) * (H - T - B); };
      var svg = el('svg', { viewBox: '0 0 ' + W + ' ' + H }, box);
      for (var g = lo; g <= 1.0001; g += 0.05) {
        el('line', { x1: L, x2: W - R, y1: Y(g), y2: Y(g), 'class': 'grid' }, svg);
        text(svg, 0, Y(g) + 4, pct(g));
      }
      if (!isNaN(aim)) {
        el('line', { x1: L, x2: W - R, y1: Y(aim), y2: Y(aim), 'class': 'aim' }, svg);
        text(svg, W - R, Y(aim) - 7, 'aim ' + pct(aim), 'axis-text strong', 'end');
      }
      var d = '', pen = false;
      weeks.forEach(function (w, i) {
        if (w.v === null) { pen = false; return; }
        d += (pen ? 'L' : 'M') + X(i).toFixed(1) + ' ' + Y(w.v).toFixed(1);
        pen = true;
      });
      el('path', { d: d, 'class': 'line' }, svg);
      var last = null;
      weeks.forEach(function (w, i) { if (w.v !== null) last = i; });
      el('circle', { cx: X(last), cy: Y(weeks[last].v), r: 4.5, 'class': 'dot' }, svg);
      text(svg, X(last) - 8, Y(weeks[last].v) - 10, pct(weeks[last].v), 'axis-text value', 'end');
      text(svg, X(0), H - 6, '12 weeks ago');
      text(svg, X(weeks.length - 1), H - 6, 'this week', 'axis-text', 'end');
      // The crosshair and a dot find the week nearest the pointer.
      var cross = el('line', { y1: T, y2: H - B, 'class': 'cross', visibility: 'hidden' }, svg);
      var spot = el('circle', { r: 4.5, 'class': 'dot spot', visibility: 'hidden' }, svg);
      var hit = el('rect', { x: L, y: 0, width: W - L - R, height: H - B, 'class': 'hit' }, svg);
      var first = new Date(box.getAttribute('data-first') + 'T12:00:00');
      var sample = +box.getAttribute('data-sample') || 20;
      var span = function (i) {
        if (isNaN(first)) return '';
        var f = { day: 'numeric', month: 'short' };
        var a = new Date(first.getTime() + i * 7 * 86400000), b = new Date(a.getTime() + 6 * 86400000);
        return a.toLocaleDateString(undefined, f) + ' to ' + b.toLocaleDateString(undefined, f);
      };
      var why = function (w) {
        if (w.v === null) return 'No learned card came due, or you took the week off. New cards don\'t count here.';
        var s = 'You remembered ' + w.p + ' of the ' + w.n + ' learned ' + (w.n === 1 ? 'card' : 'cards') + ' that came due';
        if (!isNaN(aim)) {
          var gap = Math.round((w.v - aim) * 100);
          s += Math.abs(gap) <= 1 ? ', right at your aim.' : ', ' + Math.abs(gap) + ' points ' + (gap > 0 ? 'above' : 'below') + ' your aim.';
        } else s += '.';
        if (w.n < sample) s += ' With so few reviews, one slip moves this a lot.';
        return s;
      };
      hit.addEventListener('pointermove', function (e) {
        var r = svg.getBoundingClientRect();
        var x = (e.clientX - r.left) / r.width * W;
        var i = Math.max(0, Math.min(weeks.length - 1, Math.round((x - L) / ((W - L - R) / (weeks.length - 1)))));
        cross.setAttribute('x1', X(i));
        cross.setAttribute('x2', X(i));
        cross.setAttribute('visibility', 'visible');
        var w = weeks[i];
        if (w.v === null) spot.setAttribute('visibility', 'hidden');
        else { spot.setAttribute('cx', X(i)); spot.setAttribute('cy', Y(w.v)); spot.setAttribute('visibility', 'visible'); }
        var when = weeksAgo(i, weeks.length);
        showTip(e, w.v === null ? 'No reviews' : pct(w.v) + ' remembered', when.charAt(0).toUpperCase() + when.slice(1) + (span(i) ? ', ' + span(i) : ''), why(w));
      });
      hit.addEventListener('pointerleave', function () { cross.setAttribute('visibility', 'hidden'); spot.setAttribute('visibility', 'hidden'); hideTip(); });
      // The aim explains itself; it sits above the hit area to be found.
      if (!isNaN(aim)) {
        var aimHit = el('rect', { x: W - R - 70, y: Y(aim) - 20, width: 70, height: 26, 'class': 'hit' }, svg);
        hover(aimHit, 'Aim ' + pct(aim), 'the dashed line',
          'The share of due cards your presets aim for you to remember, averaged over your decks. FSRS spaces reviews so you land near it.');
      }
    },

    // Twelve weeks of days, darker for more reviews, and the scale below
    // them, each step with the counts it stands for.
    days: function (box) {
      var days = box.getAttribute('data-days').split(',').map(Number);
      var first = new Date(box.getAttribute('data-first') + 'T12:00:00');
      var max = Math.max.apply(null, days.concat([1]));
      var busiest = Math.max.apply(null, days);
      var cols = Math.ceil(days.length / 7);
      var w = cols * 18 - 4;
      var svg = el('svg', { viewBox: '0 0 ' + w + ' 150' }, box);
      var stepOf = function (n) { return n === 0 ? 0 : Math.min(4, 1 + Math.floor(n / max * 3.999)); };
      var reviews = function (n) { return n + (n === 1 ? ' review' : ' reviews'); };
      days.forEach(function (n, i) {
        var c = el('rect', { x: Math.floor(i / 7) * 18, y: (i % 7) * 18, width: 14, height: 14, rx: 3, 'class': 'cell h' + stepOf(n) }, svg);
        var day = new Date(first.getTime() + i * 86400000);
        var today = i === days.length - 1;
        var why = n === 0
          ? (today ? 'Nothing yet today. Your streak holds until the day ends.' : 'No reviews this day.')
          : n === busiest ? 'Your busiest day in these twelve weeks.'
          : 'Your busiest day had ' + busiest + '.';
        hover(c, reviews(n), (today ? 'Today, ' : '') + day.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' }), why);
      });
      // The scale: which counts each shade holds.
      var lo = [0, 0, 0, 0, 0], hi = [0, 0, 0, 0, 0];
      for (var n = max; n >= 1; n--) { var k = stepOf(n); lo[k] = n; if (!hi[k]) hi[k] = n; }
      var y = 7 * 18 + 10;
      var x0 = w - 5 * 18 - 30;
      text(svg, x0 - 6, y + 11, 'Less', 'axis-text', 'end');
      for (var k = 0; k <= 4; k++) {
        if (k > 0 && !lo[k]) continue;
        var s = el('rect', { x: x0 + k * 18, y: y, width: 14, height: 14, rx: 3, 'class': 'cell scale h' + k }, svg);
        hover(s, k === 0 ? 'No reviews' : lo[k] === hi[k] ? reviews(lo[k]) : lo[k] + ' to ' + reviews(hi[k]), 'in a day',
          'Each square is one day; the stronger its color, the more you reviewed.');
      }
      text(svg, x0 + 5 * 18 + 2, y + 11, 'More');
    },

    // The next fourteen days; today in Orchid, the rest quiet.
    forecast: function (box) {
      var vals = box.getAttribute('data-values').split(',').map(Number);
      var labels = box.getAttribute('data-labels').split(',');
      var max = Math.max.apply(null, vals.concat([1]));
      var W = 680, H = 200, B = 24, T = 22;
      var bw = W / vals.length;
      var sum = 0;
      var svg = el('svg', { viewBox: '0 0 ' + W + ' ' + H }, box);
      el('line', { x1: 0, x2: W, y1: H - B + 0.5, y2: H - B + 0.5, 'class': 'grid' }, svg);
      vals.forEach(function (v, i) {
        var h = v / max * (H - B - T);
        var x = i * bw + 4;
        // A bar is rounded at its top only, anchored to the baseline.
        var r = Math.min(4, h);
        var y = H - B - h;
        var d = 'M' + x + ' ' + (H - B) + 'V' + (y + r) + 'Q' + x + ' ' + y + ' ' + (x + r) + ' ' + y + 'H' + (x + bw - 8 - r) + 'Q' + (x + bw - 8) + ' ' + y + ' ' + (x + bw - 8) + ' ' + (y + r) + 'V' + (H - B) + 'Z';
        var bar = v > 0 ? el('path', { d: d, 'class': 'bar' + (i === 0 ? ' now' : '') }, svg) : null;
        var hit = el('rect', { x: i * bw, y: T - 18, width: bw, height: H - T + 18 - B, 'class': 'hit' }, svg);
        sum += v;
        var why = i === 0
          ? 'Learned cards due today, overdue ones included. New cards come on top.'
          : 'Learned cards that come due this day; ' + sum + ' from today through then. New cards come on top, and every review you do schedules its card again.';
        if (i > 0 && v === max && v > 0) why = 'The busiest day ahead. ' + why;
        hover(hit, v + (v === 1 ? ' card' : ' cards'), labels[i], why, bar && function (on) { bar.classList.toggle('hot', on); });
        if (i === 0) text(svg, x + (bw - 8) / 2, y - 6, String(v), 'axis-text value now', 'middle');
        if (i === 0 || i % 2 === 1) text(svg, i * bw + bw / 2, H - 6, labels[i].replace('Tomorrow', 'Tmrw').replace(/ \d+$/, ''), 'axis-text', 'middle');
      });
    },

    // Each deck: its aim as a tick and as the one number, what was remembered
    // as a dot (its value on hover). Below aim is an open Terra ring, so it
    // reads without colour too.
    decks: function (box) {
      var rows = box.getAttribute('data-decks').split(';').map(function (r) {
        var p = r.split('|');
        return { name: p[0], aim: +p[1], v: +p[2], id: p[3], n: +p[4] || 0 };
      });
      var lo = Math.min.apply(null, rows.map(function (r) { return Math.min(r.aim, r.v); }).concat([0.8]));
      lo = Math.floor((lo - 0.03) * 20) / 20;
      var P = function (v) { return ((v - lo) / (1 - lo) * 100).toFixed(2) + '%'; };
      var sample = +box.getAttribute('data-sample') || 20;
      rows.forEach(function (r) {
        var low = r.v < r.aim - 0.02;
        var row = document.createElement('div');
        row.className = 'dots-row';
        var a = document.createElement('a');
        a.href = '/decks/' + r.id;
        a.textContent = r.name;
        var track = document.createElement('span');
        track.className = 'dots-track';
        var span = document.createElement('i');
        span.className = 'gap' + (low ? ' low' : '');
        span.style.left = P(Math.min(r.aim, r.v));
        span.style.right = (100 - parseFloat(P(Math.max(r.aim, r.v)))).toFixed(2) + '%';
        var tick = document.createElement('i');
        tick.className = 'aim';
        tick.style.left = P(r.aim);
        var dot = document.createElement('i');
        dot.className = 'dot' + (low ? ' low' : '');
        dot.style.left = P(r.v);
        var gap = Math.round((r.v - r.aim) * 100);
        var from = r.n ? ', from ' + r.n + (r.n === 1 ? ' review' : ' reviews') + ' in the last 30 days.' : ' in the last 30 days.';
        var why = r.n && r.n < sample ? 'Only ' + r.n + (r.n === 1 ? ' review' : ' reviews') + ' in the last 30 days, too few to say much yet.'
          : Math.abs(gap) <= 2 ? 'Right at its aim' + from
          : gap < 0 ? Math.abs(gap) + ' points below its aim' + from + ' Open the deck to see which cards slip.'
          : gap + ' points above its aim' + from;
        hover(row, pct(r.v) + ' remembered', r.name + ', aim ' + pct(r.aim), why);
        var val = document.createElement('span');
        val.className = 'dots-val';
        val.textContent = pct(r.aim);
        track.appendChild(span);
        track.appendChild(tick);
        track.appendChild(dot);
        row.appendChild(a);
        row.appendChild(track);
        row.appendChild(val);
        box.appendChild(row);
      });
    },

    // Expected against remembered; on the dashed line FSRS is right.
    calibration: function (box) {
      var bins = box.getAttribute('data-bins').split(';').map(function (b) {
        var p = b.split(',');
        return { e: +p[0], a: +p[1], n: +p[2], from: p[3], to: p[4] };
      });
      var lo = Math.min.apply(null, bins.map(function (b) { return Math.min(b.e, b.a); }).concat([0.8]));
      lo = Math.max(0, Math.floor((lo - 0.03) * 10) / 10);
      var S = 260, M = 24;
      var Q = function (v) { return M + (v - lo) / (1 - lo) * (S - 2 * M); };
      var svg = el('svg', { viewBox: '0 0 ' + S + ' ' + S, 'class': 'square' }, box);
      el('rect', { x: M, y: M, width: S - 2 * M, height: S - 2 * M, rx: 8, 'class': 'frame' }, svg);
      el('line', { x1: Q(lo), y1: S - Q(lo), x2: Q(1), y2: S - Q(1), 'class': 'aim' }, svg);
      // The dashed line is wide to the pointer, under the dots.
      var fit = el('line', { x1: Q(lo), y1: S - Q(lo), x2: Q(1), y2: S - Q(1), 'class': 'hit-line' }, svg);
      hover(fit, 'A perfect fit', 'the dashed line',
        'Here FSRS would predict your memory exactly. Dots above it: you remembered more than it expected. Below: less.');
      var most = Math.max.apply(null, bins.map(function (b) { return b.n; }).concat([1]));
      bins.forEach(function (b) {
        var c = el('circle', { cx: Q(b.e), cy: S - Q(b.a), r: (4 + Math.sqrt(b.n / most) * 8).toFixed(1), 'class': 'dot' }, svg);
        var gap = Math.round((b.a - b.e) * 100);
        var band = b.from !== undefined ? b.from + ' to ' + b.to + '%' : pct(b.e);
        var why = 'Of ' + b.n + (b.n === 1 ? ' review' : ' reviews') + ' where FSRS gave you ' + band + ', you remembered ' + pct(b.a) + '. '
          + (Math.abs(gap) <= 2 ? 'Close to what it expected.'
            : gap < 0 ? Math.abs(gap) + ' points less than it expected; optimizing FSRS on your reviews narrows that.'
            : gap + ' points more than it expected.')
          + ' Bigger dots stand for more reviews.';
        hover(c, pct(b.a) + ' remembered', 'FSRS expected ' + band, why);
      });
      text(svg, S / 2, S - 4, 'expected', 'axis-text', 'middle');
      var t = text(svg, 10, S / 2, 'remembered', 'axis-text', 'middle');
      t.setAttribute('transform', 'rotate(-90 10 ' + S / 2 + ')');
      text(svg, M, M - 8, pct(lo) + ' to 100%');
    }
  };
  function drawCharts(scope) {
    Array.prototype.forEach.call(scope.querySelectorAll('.chart[data-chart]:not([data-drawn])'), function (box) {
      var draw = charts[box.getAttribute('data-chart')];
      if (!draw) return;
      box.setAttribute('data-drawn', '');
      try { draw(box); } catch (e) { box.setAttribute('data-failed', ''); }
    });
  }

  // "I love numbers" shows the exact tables under the charts, and stays on.
  function numbersSwitch(scope) {
    var sw = scope.querySelector('.numbers-toggle input');
    if (!sw) return;
    var set = function (on) { root.classList.toggle('show-numbers', on); sw.checked = on; };
    try { set(localStorage.getItem('ainki.numbers') === '1'); } catch (e) { set(false); }
    sw.addEventListener('change', function () {
      set(sw.checked);
      try { localStorage.setItem('ainki.numbers', sw.checked ? '1' : '0'); } catch (e) { /* storage blocked */ }
    });
  }

  // ----- settings ------------------------------------------------------------
  // Each part becomes a Swiss section: its title and what it is for on the
  // left, the controls on the right. Changes save by themselves. Rarely
  // touched groups fold away behind their current value. The Danger zone
  // comes last. Without this the page is the plain form with Save buttons.
  var PARTS = {
    'Preferences': ['Preferences', 'How AiNKI looks, and when your day starts.'],
    'Studying': ['Studying', 'Rest, breaks and coming back after time away.'],
    'FSRS': ['Scheduling', 'FSRS, the model that decides when cards come back.'],
    'Change password': ['Account', 'Your password and this session.']
  };
  function settings(scope) {
    var main = scope.querySelector('main');
    if (!main || !main.querySelector('form[action="/settings"]') || main.querySelector('.set')) return;
    var heads = Array.prototype.filter.call(main.children, function (c) { return c.tagName === 'H2' && PARTS[c.textContent.trim()]; });
    var sections = [];
    heads.forEach(function (h) {
      var part = PARTS[h.textContent.trim()];
      var sec = document.createElement('section');
      sec.className = 'set';
      if (h.id) { sec.id = h.id; h.removeAttribute('id'); }
      var head = document.createElement('div');
      head.className = 'set-head';
      var body = document.createElement('div');
      body.className = 'set-body';
      // Everything up to the next part belongs to this one.
      var n = h.nextElementSibling;
      while (n && !(n.tagName === 'H2' && PARTS[n.textContent.trim()]) && !n.classList.contains('build')) {
        var next = n.nextElementSibling;
        body.appendChild(n);
        n = next;
      }
      h.textContent = part[0];
      var p = document.createElement('p');
      p.className = 'small muted';
      p.textContent = part[1];
      var saved = document.createElement('p');
      saved.className = 'saved';
      saved.setAttribute('role', 'status');
      main.insertBefore(sec, h);
      head.appendChild(h);
      head.appendChild(p);
      head.appendChild(saved);
      sec.appendChild(head);
      sec.appendChild(body);
      sections.push(sec);
    });
    var account = sections[sections.length - 1];
    var logout = main.querySelector('form.logout');
    if (account && logout) account.querySelector('.set-body').appendChild(logout);

    // The Danger zone: what can't be undone, under a Terra rule.
    var danger = document.createElement('section');
    danger.className = 'set danger-zone';
    danger.innerHTML = '<div class="set-head"><h2>Danger zone</h2><p class="small muted">These can’t be undone.</p></div><div class="set-body"></div>';
    var dbody = danger.querySelector('.set-body');
    var reset = main.querySelector('details.danger');
    if (reset) {
      var line = document.createElement('div');
      line.className = 'danger-line';
      line.innerHTML = '<span><span class="lab">Go back to the default FSRS parameters</span><span class="sub">Your fitted parameters are dropped. You can optimize again any time.</span></span>';
      var form = reset.querySelector('form');
      Array.prototype.forEach.call(form.querySelectorAll('p'), function (x) { x.remove(); });
      line.appendChild(form);
      dbody.appendChild(line);
      reset.remove();
    }
    var del = document.createElement('div');
    del.className = 'danger-line';
    del.innerHTML = '<span><span class="lab">Delete my account</span><span class="sub">Not possible here yet. Export your decks first, then ask for it.</span></span>';
    dbody.appendChild(del);
    main.insertBefore(danger, main.querySelector('.build'));

    // Folds: a line that shows the current value and opens its group.
    var fold = function (sec, label, nodes, value) {
      if (!nodes.length || nodes.some(function (x) { return !x; })) return;
      var b = document.createElement('button');
      b.type = 'button';
      b.className = 'fold';
      b.setAttribute('aria-expanded', 'false');
      b.innerHTML = '<span class="lab"></span><span class="val"></span><span class="car" aria-hidden="true"></span>';
      b.querySelector('.lab').textContent = label;
      var inner = document.createElement('div');
      inner.className = 'fold-body';
      inner.hidden = true;
      nodes[0].parentNode.insertBefore(b, nodes[0]);
      b.parentNode.insertBefore(inner, b.nextSibling);
      nodes.forEach(function (x) { inner.appendChild(x); });
      var show = function () { b.querySelector('.val').textContent = value(); };
      show();
      inner.addEventListener('change', show);
      b.addEventListener('click', function () {
        var open = inner.hidden;
        inner.hidden = !open;
        b.setAttribute('aria-expanded', open ? 'true' : 'false');
      });
    };
    var q = function (sel) { return main.querySelector(sel); };
    var lineOf = function (sel) { var x = q(sel); return x && x.closest('.line, label.check'); };
    var hintAfter = function (x) { var h = x && x.nextElementSibling; return h && h.classList.contains('hint') ? h : null; };
    var opt = function (sel) { var s = q(sel); return s ? s.options[s.selectedIndex].text : ''; };
    var four = lineOf('input[name=four_buttons]');
    fold(four && four.closest('.set'), 'Answer buttons', [four, hintAfter(four)], function () {
      return q('input[name=four_buttons]').checked ? 'Four: Again, Hard, Good, Easy' : 'Two: Again and Good';
    });
    var agent = lineOf('input[name=optimize_include_agent]');
    fold(null, 'Reviews done through agents', [agent], function () {
      return q('input[name=optimize_include_agent]').checked ? 'Used when optimizing' : 'Not used';
    });
    var quiet = lineOf('select[name=quiet_from]');
    fold(null, 'Quiet hours', [quiet, hintAfter(quiet)], function () {
      var a = q('select[name=quiet_from]').value, b2 = q('select[name=quiet_to]').value;
      return a === '' || b2 === '' ? 'Off' : opt('select[name=quiet_from]') + ' to ' + opt('select[name=quiet_to]');
    });
    var cu = lineOf('input[name=catch_up]');
    var cuN = lineOf('input[name=catch_up_reviews]');
    fold(null, 'Catch-up mode', [cu, cuN, hintAfter(cuN)], function () {
      return q('input[name=catch_up]').checked ? 'On, ' + q('input[name=catch_up_reviews]').value + ' a day' : 'Off';
    });

    // Saving as you go: the form is sent in the background and the part
    // says so. Text size and the look need the page drawn again.
    Array.prototype.forEach.call(main.querySelectorAll('form[action="/settings"], form[action="/settings/study"], form[action="/settings/auto-optimize"]'), function (form) {
      var sec = form.closest('.set');
      var status = sec && sec.querySelector('.saved');
      var foot = form.querySelector('.form-foot');
      if (foot) foot.hidden = true;
      var timer = null;
      var save = function (e) {
        if (e) e.stopPropagation();
        if (!form.checkValidity()) { form.reportValidity(); return; }
        var reload = e && e.target && /^(font_scale|desktop_look)$/.test(e.target.name);
        if (status) { status.textContent = 'Saving…'; status.className = 'saved'; }
        fetch(form.action, { method: 'POST', credentials: 'same-origin', body: new URLSearchParams(new FormData(form)) })
          .then(function (r) { return r.text(); })
          .then(function (html) {
            var doc = new DOMParser().parseFromString(html, 'text/html');
            var note = doc.querySelector('.notice');
            var msg = note ? note.textContent.trim() : 'Saved.';
            if (reload && /saved/i.test(msg)) { location.reload(); return; }
            if (status) { status.textContent = /saved/i.test(msg) ? 'Saved' : msg; status.className = 'saved ' + (/saved/i.test(msg) ? 'ok' : 'err'); }
          })
          .catch(function () { if (status) { status.textContent = 'Not saved. Check your connection.'; status.className = 'saved err'; } });
      };
      form.addEventListener('change', save);
      form.addEventListener('input', function (e) {
        if (e.target.type !== 'text' && e.target.type !== 'number') return;
        clearTimeout(timer);
        timer = setTimeout(function () { save(); }, 900);
      });
      form.addEventListener('submit', function (e) { e.preventDefault(); save(); });
    });
  }

  // ----- the ? sheet -------------------------------------------------------
  // Every key, in one place, instead of key caps on the controls.
  var KEYS = [
    ['Studying', [['Space', 'Show the answer, or Done on a multicard'], ['1', 'Again'], ['3', 'Good'], ['2  4', 'Hard and Easy, when shown'], ['+', 'Ten more seconds on a timed card']]],
    ['Before the answer', [['S', 'Sure'], ['U', 'Not sure'], ['Enter', 'Check a typed answer']]],
    ['Multicards', [['1 – 9', 'Mark an item as remembered']]],
    ['Anywhere', [['A – Z or /', 'Find anything'], ['?', 'Show or hide this list'], ['Esc', 'Close']]]
  ];
  var sheet = null;
  function keySheet() {
    if (sheet) return sheet;
    sheet = document.createElement('dialog');
    sheet.className = 'key-sheet';
    sheet.setAttribute('aria-labelledby', 'key-sheet-title');
    var h = document.createElement('h2');
    h.id = 'key-sheet-title';
    h.textContent = 'Keys';
    // The title takes the focus, so no ring sits on Close at first.
    h.tabIndex = -1;
    h.setAttribute('autofocus', '');
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'link close';
    close.textContent = 'Close';
    close.addEventListener('click', function () { sheet.close(); });
    var top = document.createElement('div');
    top.className = 'key-top';
    top.appendChild(h);
    top.appendChild(close);
    var grid = document.createElement('div');
    grid.className = 'key-grid';
    KEYS.forEach(function (g) {
      var col = document.createElement('div');
      var k = document.createElement('p');
      k.className = 'kicker';
      k.textContent = g[0];
      col.appendChild(k);
      var dl = document.createElement('dl');
      g[1].forEach(function (pair) {
        var dt = document.createElement('dt');
        dt.textContent = pair[0];
        var dd = document.createElement('dd');
        dd.textContent = pair[1];
        dl.appendChild(dt);
        dl.appendChild(dd);
      });
      col.appendChild(dl);
      grid.appendChild(col);
    });
    sheet.appendChild(top);
    sheet.appendChild(grid);
    // A click on the backdrop closes it.
    sheet.addEventListener('click', function (e) { if (e.target === sheet) sheet.close(); });
    document.body.appendChild(sheet);
    return sheet;
  }
  function toggleKeys() {
    var s = keySheet();
    if (s.open) s.close(); else if (s.showModal) s.showModal();
  }
  document.addEventListener('keydown', function (e) {
    if (e.key !== '?' || e.ctrlKey || e.metaKey || e.altKey) return;
    var t = e.target;
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
    e.preventDefault();
    toggleKeys();
  });
  function keysHint(scope) {
    var b = scope.querySelector('.keys-button');
    if (!b || b.hasAttribute('data-ready')) return;
    b.setAttribute('data-ready', '');
    b.addEventListener('click', toggleKeys);
  }


  // ----- washes ----------------------------------------------------------
  // Watercolour as a style element: a few clouds of pigment that drift,
  // turn and change shape very slowly behind the page. They are blurred
  // radial gradients under one SVG filter that frays their edges like wet
  // paper. Pure decoration: hidden from screen readers, never in the way of
  // the pointer, and still when the system asks for less motion.
  var PIGMENT = {
    orchid: '168,51,109', pale: '196,120,168', rose: '222,128,150', mauve: '150,90,140',
    ochre: '228,172,92', sand: '214,170,120', olive: '150,142,92'
  };
  // x, y, width (px), pigment, seconds per turn, head start, drift x, drift y
  var HERO = [['70%', '30%', 700, 0, 36, 5, -5, 5], ['45%', '25%', 520, 1, 43, 13, 6, 4], ['58%', '62%', 560, 2, 31, 9, -4, -5], ['88%', '70%', 420, 3, 39, 21, 5, 4], ['30%', '50%', 380, 2, 47, 17, 6, -3]];
  var WIDE = [['30%', '30%', 700, 0, 34, 3, 6, 4], ['62%', '22%', 560, 1, 41, 11, -5, 6], ['40%', '68%', 720, 2, 37, 19, 5, -5], ['72%', '62%', 500, 3, 29, 7, -6, 5], ['18%', '86%', 520, 4, 45, 23, 4, -4], ['80%', '8%', 420, 2, 33, 15, -4, 6]];
  var ROUND = [['35%', '45%', 620, 0, 33, 4, 5, 4], ['60%', '40%', 560, 2, 39, 12, -5, 5], ['50%', '65%', 520, 3, 29, 8, 4, -4], ['70%', '60%', 420, 1, 44, 18, -4, -3], ['25%', '60%', 380, 4, 37, 22, 5, 3]];
  // Each place has its own set of pigments; a deck's follow from its id.
  var SETS = [
    ['orchid', 'pale', 'rose', 'ochre', 'mauve'],
    ['rose', 'ochre', 'orchid', 'sand', 'pale'],
    ['mauve', 'orchid', 'sand', 'rose', 'olive'],
    ['olive', 'ochre', 'pale', 'orchid', 'sand'],
    ['pale', 'mauve', 'ochre', 'rose', 'orchid'],
    ['orchid', 'mauve', 'pale', 'orchid', 'rose']
  ];

  function filter() {
    if (document.getElementById('wet')) return;
    var ns = 'http://www.w3.org/2000/svg';
    var svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('width', '0');
    svg.setAttribute('height', '0');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('class', 'wash-defs');
    svg.innerHTML = '<filter id="wet" x="-30%" y="-40%" width="160%" height="180%">' +
      '<feTurbulence type="fractalNoise" baseFrequency=".006 .009" numOctaves="3" seed="7" result="t"/>' +
      '<feDisplacementMap in="SourceGraphic" in2="t" scale="90" xChannelSelector="R" yChannelSelector="G"/></filter>' +
      // The same at half size, for clouds painted into a canvas (below).
      // Its region is in canvas pixels and only needs to cover the largest
      // canvas (a 720px cloud: 576 x 432). A browser may compute the noise
      // for the whole region, so a huge one can stall the page.
      '<filter id="wet-half" filterUnits="userSpaceOnUse" x="0" y="0" width="640" height="480">' +
      '<feTurbulence type="fractalNoise" baseFrequency=".012 .018" numOctaves="3" seed="7" result="t"/>' +
      '<feDisplacementMap in="SourceGraphic" in2="t" scale="45" xChannelSelector="R" yChannelSelector="G"/></filter>';
    document.body.appendChild(svg);
  }

  // A cloud painted once into a canvas at half size: a still picture the
  // GPU only has to move, in every browser. It is blurred anyway, so half
  // size looks the same and needs a quarter of the memory. The canvas
  // reaches past the cloud's box (see calm.css) so the blur and the
  // frayed edge have room. Returns null where a canvas can't filter;
  // there the cloud is a CSS-filtered element instead.
  var HALF = 0.5;
  function cloud(width, rgb) {
    var w = width, h = width / 1.5, mx = w * 0.3, my = h * 0.4;
    var c = document.createElement('canvas');
    var ctx = c.getContext && c.getContext('2d');
    if (!ctx || !('filter' in ctx) || !ctx.roundRect) return null;
    c.width = Math.round((w + 2 * mx) * HALF);
    c.height = Math.round((h + 2 * my) * HALF);
    ctx.filter = 'blur(' + (22 * HALF) + 'px) url(#wet-half)';
    if (ctx.filter === 'none') return null;
    ctx.scale(HALF, HALF);
    ctx.translate(mx, my);
    // The outline of border-radius: 42% 58% 63% 37% / 45% 40% 60% 55%.
    ctx.beginPath();
    ctx.roundRect(0, 0, w, h, [
      { x: w * 0.42, y: h * 0.45 }, { x: w * 0.58, y: h * 0.40 },
      { x: w * 0.63, y: h * 0.60 }, { x: w * 0.37, y: h * 0.55 }
    ]);
    // radial-gradient(closest-side) of a wide box is an ellipse: a circle
    // squeezed to the box's proportions.
    ctx.translate(w / 2, h / 2);
    ctx.scale(1, h / w);
    var g = ctx.createRadialGradient(0, 0, 0, 0, 0, w / 2);
    g.addColorStop(0, 'rgba(' + rgb + ',.5)');
    g.addColorStop(0.4, 'rgba(' + rgb + ',.36)');
    g.addColorStop(0.7, 'rgba(' + rgb + ',.14)');
    g.addColorStop(1, 'rgba(' + rgb + ',0)');
    ctx.fillStyle = g;
    ctx.fill();
    return c;
  }

  function wash(kind, shape, set, count, into) {
    var parent = into || document.body;
    var old = parent.querySelector(':scope > .wash');
    var key = kind + ':' + set;
    if (old && old.getAttribute('data-key') === key) return;
    if (old) old.remove();
    if (!kind) return;
    filter();
    var w = document.createElement('div');
    w.className = 'wash ' + kind;
    w.setAttribute('aria-hidden', 'true');
    w.setAttribute('data-key', key);
    var colours = SETS[set % SETS.length];
    shape.slice(0, count || shape.length).forEach(function (b) {
      var i = document.createElement('i');
      i.style.setProperty('--x', b[0]);
      i.style.setProperty('--y', b[1]);
      i.style.setProperty('--w', b[2] + 'px');
      i.style.setProperty('--c', PIGMENT[colours[b[3] % colours.length]]);
      i.style.setProperty('--d', b[4] + 's');
      i.style.setProperty('--dl', '-' + b[5] + 's');
      i.style.setProperty('--dx', b[6] + '%');
      i.style.setProperty('--dy', b[7] + '%');
      w.appendChild(i);
      // The cloud itself, painted once; its holder moves it. One cloud per
      // task, after the page is up: painting is slow without a GPU, and the
      // wash fades in anyway.
      setTimeout(function () {
        if (!w.isConnected) return;
        i.appendChild(cloud(b[2], PIGMENT[colours[b[3] % colours.length]]) || document.createElement('b'));
      }, 0);
    });
    parent.insertBefore(w, parent.firstChild);
  }

  // Quiet hours cover everything: the page under the dark screen can't be
  // reached by pointer, Tab or keys.
  function sleepWall() {
    var wall = document.querySelector('.sleep-wall');
    if (!wall) return;
    wall.removeAttribute('hidden');
    Array.prototype.forEach.call(document.querySelectorAll('body > main, body > nav.top'), function (e) { e.inert = true; });
    document.addEventListener('keydown', function (e) { e.stopImmediatePropagation(); }, true);
    wash('night', WIDE, 5, 5, wall);
  }

  function paint() {
    var page = document.body.getAttribute('data-page') || '';
    var path = location.pathname;
    var deck = /^\/decks\/(\d+)/.exec(path);
    if (!document.body.hasAttribute('data-user')) wash('side', WIDE, 0);
    else if (page === 'study') wash('', null, 0);
    else if (page === 'study-done') wash('middle', ROUND, document.querySelector('.break') ? 3 : 0);
    else if (page === 'home') wash('corner', HERO, 0);
    else if (deck) wash('corner', HERO, +deck[1] + 1);
    else if (/^\/learn/.test(path)) wash('corner small', HERO, 2, 4);
    else wash('corner faint', HERO, 4, 4);
  }
  // Log in, sign up and invites: a greeting instead of the site's name, and the field names
  // become placeholders, the two fields one group. The labels stay for
  // screen readers.
  function login() {
    var form = document.querySelector('main > form[action="/login"], main > form[action="/signup"], main > form.invite');
    if (!form || form.querySelector('.fields')) return;
    var main = form.parentNode;
    // An invite brings its own greeting: who sent it.
    if (!main.querySelector(':scope > h1.greeting')) {
      var h = document.createElement('h1');
      h.className = 'greeting';
      h.textContent = form.getAttribute('action') === '/login' ? 'Welcome back.' : 'Welcome.';
      main.insertBefore(h, main.querySelector('.notice') || form);
    }
    var fields = document.createElement('div');
    fields.className = 'fields';
    var labels = form.querySelectorAll(':scope > label');
    if (labels.length) form.insertBefore(fields, labels[0]);
    Array.prototype.forEach.call(labels, function (label) {
      var input = label.querySelector('input');
      if (!input) return;
      var name = '';
      Array.prototype.slice.call(label.childNodes).forEach(function (n) {
        if (n !== input) { name += n.textContent; n.remove(); }
      });
      var text = document.createElement('span');
      text.className = 'label-text';
      text.textContent = name.trim();
      label.insertBefore(text, input);
      input.placeholder = name.replace(/\(.*\)/, '').trim();
      fields.appendChild(label);
    });
  }


  // ----- quick find ---------------------------------------------------------
  // A sheet over the page that finds places, decks, lessons and cards as you
  // type. The server matches fuzzily (/find). It opens from the rail's first
  // dot, with /, or by simply typing a letter anywhere outside a field; while
  // studying only / opens it, since letters there answer the card.
  var finder = null;
  function findSheet() {
    if (finder) return finder;
    var d = document.createElement('dialog');
    d.className = 'find-sheet';
    d.setAttribute('aria-label', 'Find');
    d.innerHTML =
      '<div class="find-box">' +
      '<label class="find-field"><span>Go to</span>' +
      '<input type="search" autocomplete="off" spellcheck="false" role="combobox" aria-expanded="true" aria-controls="find-results" aria-autocomplete="list"></label>' +
      '<ul class="find-results" id="find-results" role="listbox" aria-label="Results"></ul>' +
      '<p class="find-none" hidden>Nothing found.</p>' +
      '</div>';
    var input = d.querySelector('input');
    var list = d.querySelector('ul');
    var none = d.querySelector('.find-none');
    var active = 0, items = [], timer = 0, ctrl = null, asked = null;

    function mark(i) {
      if (!items.length) return;
      active = (i + items.length) % items.length;
      items.forEach(function (a, n) {
        a.classList.toggle('on', n === active);
        a.setAttribute('aria-selected', n === active ? 'true' : 'false');
      });
      input.setAttribute('aria-activedescendant', items[active].id);
      items[active].scrollIntoView({ block: 'nearest' });
    }
    function show(results) {
      list.textContent = '';
      items = results.map(function (r, n) {
        var li = document.createElement('li');
        var a = document.createElement('a');
        a.href = r.href;
        a.id = 'find-' + n;
        a.setAttribute('role', 'option');
        a.className = 'find-row ' + r.kind;
        var t = document.createElement('span');
        t.className = 't';
        t.textContent = r.title;
        a.appendChild(t);
        if (r.sub) {
          var s = document.createElement('span');
          s.className = 's';
          s.textContent = r.sub;
          a.appendChild(s);
        }
        a.addEventListener('pointermove', function () { if (active !== n) mark(n); });
        li.appendChild(a);
        list.appendChild(li);
        return a;
      });
      none.hidden = items.length > 0 || !input.value.trim();
      active = 0;
      mark(0);
    }
    function ask() {
      var q = input.value.trim();
      if (q === asked) return;
      asked = q;
      if (ctrl) ctrl.abort();
      ctrl = typeof AbortController === 'function' ? new AbortController() : null;
      fetch('/find?q=' + encodeURIComponent(q), { credentials: 'same-origin', signal: ctrl ? ctrl.signal : undefined, headers: { Accept: 'application/json' } })
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (j) { if (j && j.q === input.value.trim()) show(j.results || []); })
        .catch(function () {});
    }
    input.addEventListener('input', function () {
      clearTimeout(timer);
      timer = setTimeout(ask, 80);
    });
    input.addEventListener('keydown', function (e) {
      // A search field would only clear itself on Esc; here Esc closes.
      if (e.key === 'Escape') { e.preventDefault(); d.close(); return; }
      if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey && items.length)) { e.preventDefault(); mark(active + 1); }
      else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey && items.length)) { e.preventDefault(); mark(active - 1); }
      else if (e.key === 'Enter') {
        e.preventDefault();
        clearTimeout(timer);
        if (items[active] && input.value.trim() === asked) items[active].click();
        else { ask(); }
      }
    });
    // A click beside the box closes the sheet.
    d.addEventListener('click', function (e) { if (e.target === d || e.target.classList.contains('find-box')) d.close(); });
    d.addEventListener('close', function () { input.value = ''; asked = null; list.textContent = ''; items = []; none.hidden = true; });
    d.open_with = function (text) {
      if (!d.open) d.showModal();
      input.value = text || '';
      input.focus();
      asked = null;
      ask();
    };
    document.body.appendChild(d);
    finder = d;
    return d;
  }
  function findable() {
    // Quiet hours block everything; another sheet already has the keys.
    return !document.querySelector('.sleep-wall') && !document.querySelector('dialog[open]:not(.find-sheet)');
  }
  function openFind(text) {
    if (findable()) findSheet().open_with(text);
  }
  document.addEventListener('keydown', function (e) {
    if (e.defaultPrevented || e.isComposing) return;
    var t = e.target;
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'k') { e.preventDefault(); openFind(''); return; }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '/') { e.preventDefault(); openFind(''); return; }
    // Letters start a search, but not while studying: there they answer.
    if (e.key.length === 1 && /\p{L}/u.test(e.key) && !document.querySelector('main .study')) {
      e.preventDefault();
      openFind(e.key);
    }
  });
  function findDot() {
    var b = document.querySelector('nav.top .find-dot');
    if (!b || b.hasAttribute('data-ready')) return;
    b.setAttribute('data-ready', '');
    b.addEventListener('click', function () { openFind(''); });
  }

  paint();
  sleepWall();
  login();
  dress(document);
  keysHint(document);
  findDot();
  document.addEventListener('ainki:page', function (e) { keysHint(e.detail || document); });
  breakTimer(document);
  settings(document);
  drawCharts(document);
  explainers(document);
  numbersSwitch(document);
  Array.prototype.forEach.call(document.querySelectorAll('form[action="/decks/new"] input[name=name], form[action$="/edit"] input[name=name], input[name=new_deck]'), pathField);
  // Studying swaps in the next page without a reload.
  document.addEventListener('ainki:page', function (e) { dress(e.detail || document); paint(); breakTimer(e.detail || document); });
})();
