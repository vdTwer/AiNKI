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

  var tip = null;
  function showTip(e, value, label) {
    if (!tip) {
      tip = document.createElement('div');
      tip.className = 'chart-tip';
      tip.setAttribute('role', 'status');
      document.body.appendChild(tip);
    }
    tip.textContent = '';
    var v = document.createElement('strong');
    v.textContent = value;
    var l = document.createElement('span');
    l.textContent = label;
    tip.appendChild(v);
    tip.appendChild(l);
    var r = e.target.getBoundingClientRect ? e.target.getBoundingClientRect() : null;
    var x = e.clientX !== undefined && e.type !== 'focus' ? e.clientX : r.left + r.width / 2;
    var y = e.clientY !== undefined && e.type !== 'focus' ? e.clientY : r.top;
    tip.style.left = x + 'px';
    tip.style.top = y + 'px';
    tip.hidden = false;
  }
  function hideTip() { if (tip) tip.hidden = true; }
  // A mark answers the pointer and the keyboard alike.
  function hover(mark, value, label) {
    mark.setAttribute('tabindex', '0');
    mark.setAttribute('aria-label', value + ', ' + label);
    ['pointermove', 'focus'].forEach(function (t) { mark.addEventListener(t, function (e) { showTip(e, value, label); }); });
    ['pointerleave', 'blur'].forEach(function (t) { mark.addEventListener(t, hideTip); });
  }
  function weeksAgo(i, n) { var k = n - 1 - i; return k === 0 ? 'this week' : k === 1 ? 'last week' : k + ' weeks ago'; }

  var charts = {
    // Recall per week against the aim: one Orchid line, the aim dashed.
    retention: function (box) {
      var weeks = box.getAttribute('data-weeks').split(';').map(function (w) {
        var p = w.split(',');
        return { n: +p[0], v: +p[0] > 0 ? +p[1] / +p[0] : null };
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
      // The crosshair finds the week nearest the pointer.
      var cross = el('line', { y1: T, y2: H - B, 'class': 'cross', visibility: 'hidden' }, svg);
      var hit = el('rect', { x: L, y: 0, width: W - L - R, height: H - B, 'class': 'hit' }, svg);
      hit.addEventListener('pointermove', function (e) {
        var r = svg.getBoundingClientRect();
        var x = (e.clientX - r.left) / r.width * W;
        var i = Math.max(0, Math.min(weeks.length - 1, Math.round((x - L) / ((W - L - R) / (weeks.length - 1)))));
        cross.setAttribute('x1', X(i));
        cross.setAttribute('x2', X(i));
        cross.setAttribute('visibility', 'visible');
        var w = weeks[i];
        showTip(e, w.v === null ? 'No reviews' : pct(w.v), weeksAgo(i, weeks.length) + (w.n ? ', ' + w.n + ' reviews' : ''));
      });
      hit.addEventListener('pointerleave', function () { cross.setAttribute('visibility', 'hidden'); hideTip(); });
    },

    // Twelve weeks of days, darker for more reviews.
    days: function (box) {
      var days = box.getAttribute('data-days').split(',').map(Number);
      var first = new Date(box.getAttribute('data-first') + 'T12:00:00');
      var max = Math.max.apply(null, days.concat([1]));
      var cols = Math.ceil(days.length / 7);
      var svg = el('svg', { viewBox: '0 0 ' + (cols * 18 - 4) + ' 122' }, box);
      days.forEach(function (n, i) {
        var step = n === 0 ? 0 : Math.min(4, 1 + Math.floor(n / max * 3.999));
        var c = el('rect', { x: Math.floor(i / 7) * 18, y: (i % 7) * 18, width: 14, height: 14, rx: 3, 'class': 'cell h' + step }, svg);
        var day = new Date(first.getTime() + i * 86400000);
        hover(c, n + (n === 1 ? ' review' : ' reviews'), day.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' }));
      });
    },

    // The next fourteen days; today in Orchid, the rest quiet.
    forecast: function (box) {
      var vals = box.getAttribute('data-values').split(',').map(Number);
      var labels = box.getAttribute('data-labels').split(',');
      var max = Math.max.apply(null, vals.concat([1]));
      var W = 680, H = 200, B = 24, T = 22;
      var bw = W / vals.length;
      var svg = el('svg', { viewBox: '0 0 ' + W + ' ' + H }, box);
      el('line', { x1: 0, x2: W, y1: H - B + 0.5, y2: H - B + 0.5, 'class': 'grid' }, svg);
      vals.forEach(function (v, i) {
        var h = v / max * (H - B - T);
        var x = i * bw + 4;
        // A bar is rounded at its top only, anchored to the baseline.
        var r = Math.min(4, h);
        var y = H - B - h;
        var d = 'M' + x + ' ' + (H - B) + 'V' + (y + r) + 'Q' + x + ' ' + y + ' ' + (x + r) + ' ' + y + 'H' + (x + bw - 8 - r) + 'Q' + (x + bw - 8) + ' ' + y + ' ' + (x + bw - 8) + ' ' + (y + r) + 'V' + (H - B) + 'Z';
        if (v > 0) el('path', { d: d, 'class': 'bar' + (i === 0 ? ' now' : '') }, svg);
        var hit = el('rect', { x: i * bw, y: T - 18, width: bw, height: H - T + 18 - B, 'class': 'hit' }, svg);
        hover(hit, v + (v === 1 ? ' card' : ' cards'), labels[i]);
        if (i === 0) text(svg, x + (bw - 8) / 2, y - 6, String(v), 'axis-text value now', 'middle');
        if (i === 0 || i % 2 === 1) text(svg, i * bw + bw / 2, H - 6, labels[i].replace('Tomorrow', 'Tmrw').replace(/ \d+$/, ''), 'axis-text', 'middle');
      });
    },

    // Each deck: its aim as a tick, what was remembered as a dot. Below aim
    // is an open Terra ring, so it reads without colour too.
    decks: function (box) {
      var rows = box.getAttribute('data-decks').split(';').map(function (r) {
        var p = r.split('|');
        return { name: p[0], aim: +p[1], v: +p[2], id: p[3] };
      });
      var lo = Math.min.apply(null, rows.map(function (r) { return Math.min(r.aim, r.v); }).concat([0.8]));
      lo = Math.floor((lo - 0.03) * 20) / 20;
      var P = function (v) { return ((v - lo) / (1 - lo) * 100).toFixed(2) + '%'; };
      var scale = document.createElement('div');
      scale.className = 'dots-scale';
      for (var g = lo; g <= 1.0001; g += 0.05) {
        var s = document.createElement('span');
        s.style.left = P(g);
        s.textContent = pct(g);
        scale.appendChild(s);
      }
      box.appendChild(scale);
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
        hover(dot, pct(r.v) + ' remembered', r.name + ', aim ' + pct(r.aim));
        var val = document.createElement('span');
        val.className = 'dots-val' + (low ? ' low' : '');
        val.textContent = pct(r.v);
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
        return { e: +p[0], a: +p[1], n: +p[2] };
      });
      var lo = Math.min.apply(null, bins.map(function (b) { return Math.min(b.e, b.a); }).concat([0.8]));
      lo = Math.max(0, Math.floor((lo - 0.03) * 10) / 10);
      var S = 260, M = 24;
      var Q = function (v) { return M + (v - lo) / (1 - lo) * (S - 2 * M); };
      var svg = el('svg', { viewBox: '0 0 ' + S + ' ' + S, 'class': 'square' }, box);
      el('rect', { x: M, y: M, width: S - 2 * M, height: S - 2 * M, rx: 8, 'class': 'frame' }, svg);
      el('line', { x1: Q(lo), y1: S - Q(lo), x2: Q(1), y2: S - Q(1), 'class': 'aim' }, svg);
      var most = Math.max.apply(null, bins.map(function (b) { return b.n; }).concat([1]));
      bins.forEach(function (b) {
        var c = el('circle', { cx: Q(b.e), cy: S - Q(b.a), r: (4 + Math.sqrt(b.n / most) * 8).toFixed(1), 'class': 'dot' }, svg);
        hover(c, pct(b.a) + ' remembered', 'expected ' + pct(b.e) + ', ' + b.n + ' reviews');
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

  dress(document);
  breakTimer(document);
  settings(document);
  drawCharts(document);
  numbersSwitch(document);
  Array.prototype.forEach.call(document.querySelectorAll('form[action="/decks/new"] input[name=name], form[action$="/edit"] input[name=name], input[name=new_deck]'), pathField);
  // Studying swaps in the next page without a reload.
  document.addEventListener('ainki:page', function (e) { dress(e.detail || document); });
})();
