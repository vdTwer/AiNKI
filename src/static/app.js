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
  // Card text longer than this is set smaller and left-aligned, when
  // writing and when studying alike.
  var LONG_TEXT = 90;
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
      var isNew = deckSelect.value === deckSelect.getAttribute('data-new-deck-select');
      newDeckField.hidden = !isNew;
      var input = newDeckField.querySelector('input');
      if (input) input.required = isNew;
    };
    deckSelect.addEventListener('change', function () {
      toggle();
      if (newDeckField.hidden === false) newDeckField.querySelector('input').focus();
    });
    toggle();
  }

  // Files: every file field takes a dropped (or pasted) file the same
  // way. The first file that fits goes into the input as if it had been
  // picked, so the field's own change handler shows it.
  // Card form: offer "Learn item by item" once the back has a numbered
  // list with two or more items (the same rule as the server's).
  var listToggle = document.querySelector('.card-form .list-toggle');
  var backText = document.getElementById('back');
  if (listToggle && backText) {
    var listBox = listToggle.querySelector('input');
    var hasList = function () {
      var n = 0;
      backText.value.split(/\r?\n/).forEach(function (line) { if (/^ {0,3}\d{1,3}[.)][ \t]+\S/.test(line)) n++; });
      return n >= 2;
    };
    var offer = function () {
      var show = hasList();
      listToggle.hidden = !show;
      listBox.disabled = !show;
    };
    backText.addEventListener('input', offer);
    offer();
  }

  function putFile(input, files, fits) {
    var f = files && Array.prototype.filter.call(files, fits)[0];
    if (!f || typeof DataTransfer === 'undefined') return false;
    var dt = new DataTransfer();
    dt.items.add(f);
    input.files = dt.files;
    input.dispatchEvent(new Event('change'));
    return true;
  }

  // The zone lights up while files are dragged over it. A drop is always
  // caught, so a file that doesn't fit is ignored instead of the browser
  // leaving the page to open it.
  function dropZone(zone, input, fits) {
    var hasFiles = function (e) {
      return e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types, 'Files') >= 0;
    };
    zone.addEventListener('dragover', function (e) {
      if (!hasFiles(e)) return;
      e.preventDefault();
      zone.classList.add('dropping');
    });
    zone.addEventListener('dragleave', function (e) {
      if (!zone.contains(e.relatedTarget)) zone.classList.remove('dropping');
    });
    zone.addEventListener('drop', function (e) {
      zone.classList.remove('dropping');
      if (!hasFiles(e)) return;
      e.preventDefault();
      putFile(input, e.dataTransfer.files, fits);
    });
  }

  var isImage = function (f) { return /^image\//.test(f.type); };
  var anyFile = function () { return true; };

  // Card form: the sides grow with their text, long text is set as it
  // will be when studying, an image can be picked, dropped or pasted onto
  // a side, and Ctrl/Cmd+Enter adds the card.
  var cardForm = document.querySelector('form.card-form');
  if (cardForm) {
    var faces = cardForm.querySelectorAll('.face-edit');
    Array.prototype.forEach.call(faces, function (face) {
      var text = face.querySelector('textarea');
      var file = face.querySelector('input[type=file]');
      var preview = face.querySelector('.face-image.preview');
      var fit = function () {
        text.classList.toggle('long', text.value.trim().length > LONG_TEXT);
        if (calm && !CSS.supports('field-sizing', 'content')) {
          text.style.height = 'auto';
          text.style.height = text.scrollHeight + 'px';
        }
      };
      text.addEventListener('input', fit);
      fit();
      var show = function () {
        var f = file.files && file.files[0];
        var img = preview.querySelector('img');
        if (img.src) URL.revokeObjectURL(img.src);
        if (f) {
          img.src = URL.createObjectURL(f);
          img.alt = f.name;
        } else {
          img.removeAttribute('src');
        }
        preview.hidden = !f;
        face.classList.toggle('has-image', !!f);
      };
      file.addEventListener('change', show);
      preview.querySelector('[data-clear-image]').addEventListener('click', function () {
        file.value = '';
        show();
        file.focus();
      });
      text.addEventListener('paste', function (e) {
        if (e.clipboardData && putFile(file, e.clipboardData.files, isImage)) e.preventDefault();
      });
      dropZone(face, file, isImage);
    });
    var key = cardForm.querySelector('.submit-key');
    if (key && calm) {
      if (/Mac|iPhone|iPad/.test(navigator.platform || '')) key.textContent = '⌘ ⏎';
      key.hidden = false;
    }
    cardForm.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (cardForm.requestSubmit) cardForm.requestSubmit(); else cardForm.submit();
      }
    });
  }

  // Picking a file to import: the field names the file and takes drops.
  Array.prototype.forEach.call(document.querySelectorAll('form:not(.card-form) label.file'), function (label) {
    var input = label.querySelector('input[type=file]');
    var text = label.querySelector('.drop-text, .attach-text');
    if (!input || !text) return;
    var empty = text.textContent;
    input.addEventListener('change', function () {
      var f = input.files && input.files[0];
      text.textContent = f ? f.name : empty;
      label.classList.toggle('chosen', !!f);
    });
    dropZone(label, input, anyFile);
  });

  // ----- page details (run again after every swap) --------------------------

  // Buttons are small cards: while the pointer is over one it tilts toward
  // it (the side under the pointer dips away), then springs back. Small
  // ones get a closer perspective and a steeper angle, or the tilt would
  // be too faint to read as one.
  //
  // Leaving is forgiving: a card stays tilted and lit (.held) while the
  // pointer is within a small margin around it, and only springs back once
  // it is clearly gone. Entering has no margin, and entering another card
  // lets the held one go at once.
  var held = [];

  function grace() { return Math.min(24, Math.max(10, window.innerWidth * 0.02)); }

  function lean(el, e) {
    var r = el.getBoundingClientRect();
    var row = el.matches('.row');
    var base = row ? 3 : el.closest('.answers') ? 10 : 8;
    var max = row ? base : base * Math.max(1, Math.min(1.8, 220 / r.width));
    var x = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
    var y = Math.max(0, Math.min(1, (e.clientY - r.top) / r.height));
    var s = el.style;
    s.setProperty('--dur', '90ms');
    s.setProperty('--spring', 'linear');
    s.setProperty('--rx', ((0.5 - y) * max).toFixed(2) + 'deg');
    s.setProperty('--ry', ((x - 0.5) * max).toFixed(2) + 'deg');
    s.setProperty('--sx', ((0.5 - x) * 10).toFixed(1) + 'px');
    s.setProperty('--sy', (2 + (0.5 - y) * 8).toFixed(1) + 'px');
    s.setProperty('--gx', (x * 100).toFixed(1) + '%');
    s.setProperty('--gy', (y * 100).toFixed(1) + '%');
  }

  function rest(el) {
    el.classList.remove('held');
    var s = el.style;
    s.removeProperty('--dur');
    s.removeProperty('--spring');
    s.setProperty('--rx', '0deg');
    s.setProperty('--ry', '0deg');
    s.setProperty('--sx', '0px');
    s.setProperty('--sy', '2px');
  }

  function letGo(except) {
    held = held.filter(function (el) {
      if (el === except) return true;
      rest(el);
      return false;
    });
  }

  function tilt(el) {
    if (el.closest('.quiet-links')) return;
    var row = el.matches('.row');
    // An icon tilts on its own, with the close perspective of the rail.
    var icon = el.matches('.icon-link');
    el.addEventListener('pointerenter', function () {
      letGo(el);
      el.classList.remove('held');
      held = held.filter(function (h) { return h !== el; });
      var w = el.getBoundingClientRect().width;
      el.style.setProperty('--persp', (icon ? 240 : row ? 700 : Math.max(240, Math.min(700, w * 3))).toFixed(0) + 'px');
    });
    el.addEventListener('pointermove', function (e) { lean(el, e); });
    el.addEventListener('pointerleave', function (e) {
      if (e.pointerType !== 'mouse' || !el.isConnected) { rest(el); return; }
      el.classList.add('held');
      held.push(el);
    });
  }

  document.addEventListener('pointermove', function (e) {
    if (!held.length) return;
    var g = grace();
    held = held.filter(function (el) {
      var r = el.getBoundingClientRect();
      var inside = el.isConnected &&
        e.clientX >= r.left - g && e.clientX <= r.right + g &&
        e.clientY >= r.top - g && e.clientY <= r.bottom + g;
      if (inside) lean(el, e); else rest(el);
      return inside;
    });
  });
  // The pointer left the window, or the page scrolled under it.
  document.addEventListener('mouseout', function (e) { if (!e.relatedTarget) letGo(null); });
  window.addEventListener('scroll', function () { letGo(null); }, { passive: true });

  // The side rail: dots far away, icons up close. --near goes from 0 to 1
  // as the pointer comes within reach of the rail.
  (function () {
    var nav = document.querySelector('nav.top');
    if (!calm || !nav) return;
    var far = 160, close = 56, frame = 0, px = -1, py = -1;
    function update() {
      frame = 0;
      var near = 0;
      if (px >= 0) {
        var r = nav.getBoundingClientRect();
        var dx = Math.max(r.left - px, 0, px - r.right);
        var dy = Math.max(r.top - py, 0, py - r.bottom);
        var d = Math.sqrt(dx * dx + dy * dy);
        near = Math.max(0, Math.min(1, (far - d) / (far - close)));
      }
      nav.style.setProperty('--near', near.toFixed(3));
    }
    function later() { if (!frame) frame = requestAnimationFrame(update); }
    document.addEventListener('pointermove', function (e) {
      if (e.pointerType !== 'mouse') return;
      px = e.clientX; py = e.clientY;
      later();
    });
    document.addEventListener('mouseout', function (e) {
      if (!e.relatedTarget) { px = -1; later(); }
    });
  })();

  function setup(scope) {
    // Long card text is set smaller and left-aligned.
    var faces = scope.querySelectorAll('section.face');
    for (var f = 0; f < faces.length; f++) {
      if ((faces[f].textContent || '').trim().length > LONG_TEXT) faces[f].classList.add('long');
    }
    if (calm && motion) {
      // Hidden buttons have no size yet; they are set up when shown.
      var els = scope.querySelectorAll('.btn, button:not(.link), a.row, label.row, nav.top a:not(.brand), .icon-link');
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
  // Returns a promise: the last answer of a session first fills the line.
  function swap(html, url) {
    var doc = new DOMParser().parseFromString(html, 'text/html');
    var next = doc.querySelector('main');
    if (!next) throw new Error('no page');
    var oldBar = document.querySelector('.study-foot .bar');
    var from = oldBar ? oldBar.getAttribute('data-progress') : null;
    var wasStudying = !!document.querySelector('main .study');
    var ending = wasStudying && !!next.querySelector('.session-end .end-dot');
    if (ending && oldBar && calm && motion) {
      // The line fills up, then rises and closes into the dot.
      oldBar.closest('.study').classList.add('closing');
      oldBar.style.width = '100%';
      return new Promise(function (done) { setTimeout(done, 380); }).then(function () {
        var line = oldBar.parentNode.getBoundingClientRect();
        place(doc, next, url);
        var end = next.querySelector('.session-end');
        end.classList.add('finishing', 'from-line');
        var dot = end.querySelector('.end-dot div');
        var d = dot.getBoundingClientRect();
        var dx = line.left + line.width / 2 - (d.left + d.width / 2);
        var dy = line.top + line.height / 2 - (d.top + d.height / 2);
        if (!dot.animate) return;
        dot.animate([
          { width: line.width + 'px', height: '2px', borderRadius: '2px', transform: 'translate(' + dx + 'px,' + dy + 'px)' },
          { width: '6px', height: '2px', borderRadius: '2px', transform: 'translate(0,0)', offset: 0.6 },
          { width: '7px', height: '7px', borderRadius: '50%', transform: 'translate(0,0)' }
        ], { duration: 680, easing: 'cubic-bezier(.6, 0, .2, 1)' });
      });
    }
    place(doc, next, url);
    // The progress line glides on from where it was; a miss slips back.
    var bar = next.querySelector('.study-foot .bar');
    if (bar && from !== null && calm) {
      var to = bar.getAttribute('data-progress');
      bar.style.transition = 'none';
      bar.style.width = from + '%';
      if (+to < +from) bar.classList.add('slip');
      void bar.offsetWidth;
      bar.style.transition = '';
      requestAnimationFrame(function () {
        bar.style.width = to + '%';
        bar.classList.remove('slip');
      });
    }
    if (next.querySelector('.study')) next.querySelector('.study').classList.add('entering');
    // The closing animation is for the answer that ended the session, not
    // for opening the page again later.
    if (ending) next.querySelector('.session-end').classList.add('finishing');
    return Promise.resolve();
  }

  function place(doc, next, url) {
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
      return r.text().then(function (html) { return swap(html, r.url); });
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

  // A list card's Done says how many you remembered.
  document.addEventListener('change', function (e) {
    var box = e.target;
    if (!box.matches || !box.matches('.list-items input[type=checkbox]')) return;
    var boxes = box.form.querySelectorAll('.list-items input[type=checkbox]');
    var got = box.form.querySelectorAll('.list-items input[type=checkbox]:checked').length;
    var count = box.form.querySelector('[data-remembered]');
    if (count) count.textContent = got === boxes.length ? 'All ' + got + ' remembered' : got + ' of ' + boxes.length + ' remembered';
  });

  document.addEventListener('submit', function (e) {
    var form = e.target;
    var action = form.getAttribute('action');
    if (!studying() || (action !== '/study/answer' && action !== '/study/undo')) return;
    e.preventDefault();
    send(form, e.submitter);
  });

  // Keyboard: space shows the answer (then means Good, or Done on a list
  // card), 1-4 answer; on a list card 1-9 mark items as remembered.
  document.addEventListener('keydown', function (e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    var tag = e.target.tagName;
    if ((tag === 'INPUT' && e.target.type !== 'checkbox') || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (!document.querySelector('main .study')) return;
    var k = e.key === ' ' || e.key === 'Enter' ? ' ' : e.key;
    if (!/^[ 1-9]$/.test(k)) return;
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
