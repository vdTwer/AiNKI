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
  // Card text longer than this is set smaller and left-aligned, when
  // writing and when studying alike.
  var LONG_TEXT = 90;

  // Signup: send the browser's timezone along.
  var tz = document.getElementById('tz');
  if (tz) {
    try { tz.value = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch (e) {}
  }

  // Invite links: a Copy button, where the browser lets us copy.
  Array.prototype.forEach.call(document.querySelectorAll('button[data-copy]'), function (b) {
    var from = document.getElementById(b.getAttribute('data-copy'));
    if (!from || !navigator.clipboard) return;
    b.hidden = false;
    b.addEventListener('click', function () {
      navigator.clipboard.writeText(from.textContent.trim()).then(function () {
        b.textContent = 'Copied';
        setTimeout(function () { b.textContent = 'Copy'; }, 1600);
      });
    });
  });

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
  // Card form: while you write, the server says which ways the back can be
  // learned item by item (only those are offered), and Preview shows the
  // card as it will be studied. Both use the server's Markdown, so what you
  // see is what you get.
  var multiPick = document.querySelector('.card-form .multi-pick');
  var previewBox = document.querySelector('.card-form .card-preview');
  var previewToggle = document.querySelector('.card-form .preview-toggle');
  var frontText = document.getElementById('front');
  var backText = document.getElementById('back');
  if (multiPick && previewBox && previewToggle && frontText && backText) {
    var sheet = previewBox.previousElementSibling;
    // The coach's hints while writing: a long list, or a card like this one.
    var hintBox = document.querySelector('.card-form .write-hints');
    var previewing = false;
    var picked = multiPick.querySelector('input[name=multi]:checked:not([value=""])') !== null;
    var asked = 0;
    var chosen = function () {
      var r = multiPick.querySelector('input[name=multi]:checked');
      return r ? r.value : '';
    };
    var refresh = function () {
      var n = ++asked;
      var body = new URLSearchParams({ front: frontText.value, back: backText.value, multi: chosen() });
      var editing = multiPick.closest('form').getAttribute('data-card');
      if (editing) body.append('card', editing);
      fetch('/cards/preview', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: body
      }).then(function (r) {
        if (!r.ok) throw new Error('preview');
        return r.json();
      }).then(function (v) {
        if (n !== asked) return;
        var any = false;
        Array.prototype.forEach.call(multiPick.querySelectorAll('[data-split]'), function (label) {
          var input = label.querySelector('input');
          var ok = v.splits.indexOf(label.getAttribute('data-split')) >= 0;
          // A choice already made stays visible; saving says what's wrong.
          label.hidden = !ok && !input.checked;
          any = any || !label.hidden;
        });
        multiPick.hidden = !any;
        // Marking words means learning them one by one: picked for you
        // until you pick something yourself.
        var marked = multiPick.querySelector('[data-split=marked] input');
        if (marked && !picked && v.splits.indexOf('marked') >= 0 && !marked.checked) {
          marked.checked = true;
          refresh();
          return;
        }
        previewBox.innerHTML = v.html;
        setup(previewBox);
        if (hintBox && typeof v.hints === 'string') hintBox.innerHTML = v.hints;
      }).catch(function () {});
    };
    var timer = null;
    var later = function () {
      clearTimeout(timer);
      timer = setTimeout(refresh, 250);
    };
    frontText.addEventListener('input', later);
    backText.addEventListener('input', later);
    multiPick.addEventListener('change', function () { picked = true; refresh(); });
    // Ctrl/Cmd+M marks the selected words on the back, or unmarks them.
    backText.addEventListener('keydown', function (e) {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.key.toLowerCase() !== 'm') return;
      e.preventDefault();
      var a = backText.selectionStart, b = backText.selectionEnd, v = backText.value;
      var words = v.slice(a, b);
      var lead = words.length - words.trimStart().length, tail = words.length - words.trimEnd().length;
      a += lead; b -= tail; words = v.slice(a, b);
      if (v.slice(a - 2, a) === '{{' && v.slice(b, b + 2) === '}}') {
        backText.setRangeText(words, a - 2, b + 2, 'select');
      } else if (words) {
        backText.setRangeText('{{' + words + '}}', a, b, 'end');
      } else {
        backText.setRangeText('{{}}', a, b, 'start');
        backText.selectionStart = backText.selectionEnd = a + 2;
      }
      backText.dispatchEvent(new Event('input', { bubbles: true }));
    });
    previewToggle.hidden = false;
    previewToggle.addEventListener('click', function () {
      previewing = !previewing;
      if (previewing) refresh();
      sheet.hidden = previewing;
      previewBox.hidden = !previewing;
      previewToggle.textContent = previewing ? 'Write' : 'Preview';
      if (!previewing) backText.focus();
    });
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
        if (!CSS.supports('field-sizing', 'content')) {
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
    if (key) {
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

  // A form marked data-autosubmit saves as soon as it changes (a switch);
  // without JavaScript it has its own button.
  document.addEventListener('change', function (e) {
    var form = e.target.closest && e.target.closest('form[data-autosubmit]');
    if (!form) return;
    if (form.requestSubmit) form.requestSubmit(); else form.submit();
  });

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

  function setup(scope) {
    // Long card text is set smaller and left-aligned.
    var faces = scope.querySelectorAll('section.face');
    for (var f = 0; f < faces.length; f++) {
      if ((faces[f].textContent || '').trim().length > LONG_TEXT) faces[f].classList.add('long');
    }
  }
  setup(document);

  // ----- studying without reloads ---------------------------------------------

  function main() { return document.querySelector('main'); }
  function studying() { return !!document.querySelector('main .study, main .session-end'); }

  // Elements marked data-stay keep their own hidden state on the reveal:
  // the timer, the timeout form, the typed answer's echo.
  function reveal() {
    var study = document.querySelector('.study:not(.revealed)');
    if (!study) return false;
    stopTimer();
    turned();
    study.classList.add('revealed');
    var hidden = study.querySelectorAll('[hidden]:not([data-stay])');
    for (var i = 0; i < hidden.length; i++) {
      if (!hidden[i].closest('[data-stay][hidden]')) hidden[i].hidden = false;
    }
    var shows = study.querySelectorAll('.reveal, .timer');
    for (var j = 0; j < shows.length; j++) shows[j].hidden = true;
    setup(study);
    return true;
  }

  // ----- timed cards ------------------------------------------------------------

  // A timed card counts down under the card. "+10 s" adds time once; when
  // time runs out the answer is shown and the card counts as forgotten.
  var timer = null;

  function stopTimer() {
    if (timer) cancelAnimationFrame(timer.frame);
    timer = null;
  }

  function startTimer(scope) {
    stopTimer();
    var box = scope.querySelector('.study:not(.revealed) .timer');
    if (!box) return;
    var fill = box.querySelector('.timer-fill');
    var secs = box.querySelector('.timer-secs');
    var more = box.querySelector('.extend');
    var total = +box.getAttribute('data-limit') * 1000;
    var t = { end: Date.now() + total, total: total, frame: 0, more: more };
    timer = t;
    if (more) more.hidden = false;
    var tick = function () {
      if (timer !== t) return;
      if (!box.isConnected) { stopTimer(); return; }
      var left = t.end - Date.now();
      if (left <= 0) { stopTimer(); timeUp(); return; }
      fill.style.width = (left / t.total * 100).toFixed(2) + '%';
      box.classList.toggle('low', left <= 3000);
      secs.textContent = Math.ceil(left / 1000) + ' s';
      t.frame = requestAnimationFrame(tick);
    };
    t.tick = tick;
    tick();
  }

  function extendTimer() {
    if (!timer || !timer.more || timer.more.hidden) return;
    timer.end += 10000;
    timer.total = Math.max(timer.total, timer.end - Date.now());
    timer.more.hidden = true;
  }

  function timeUp() {
    var study = document.querySelector('.study:not(.revealed)');
    if (!study || !reveal()) return;
    study.classList.add('timed-out');
    var answers = study.querySelectorAll('form.answers, .list-answer .answers');
    for (var i = 0; i < answers.length; i++) answers[i].hidden = true;
    var form = study.querySelector('.timeout-form');
    if (form) {
      form.hidden = false;
      var go = form.querySelector('button');
      if (go) go.focus({ preventScroll: true });
    }
  }

  // ----- answer timing ------------------------------------------------------

  // How long each card took, to the millisecond, sent along with the
  // rating: from the front showing to the answer showing (think), and from
  // there to the rating (answer). Time with the tab hidden is kept apart
  // (away), so it never passes for slowness. Pages without JavaScript and
  // cards that arrive already turned over send what they can, or nothing.
  var clock = null;
  var lastInput = null;
  function ms() { return performance.now(); }

  function startClock(scope) {
    var study = scope.querySelector('.study');
    if (!study) { clock = null; return; }
    var c = { front: null, back: null, away: 0, paused: 0, idleAtBack: 0, hiddenSince: null, pausedSince: null };
    clock = c;
    var turnedOver = study.classList.contains('revealed');
    // The first frame drawn is when the card can be seen. A hidden tab draws
    // nothing, so the clock waits until the card is really on screen.
    requestAnimationFrame(function () {
      if (clock !== c) return;
      if (turnedOver) c.back = ms(); else if (c.back === null) c.front = ms();
    });
  }

  function idle(c) { return c.away + c.paused; }

  // The answer side has just come up.
  function turned() {
    var c = clock;
    if (!c || c.front === null || c.back !== null) return;
    c.back = ms();
    c.idleAtBack = idle(c);
  }

  // What goes along with the rating; the card's clock stops for good.
  function timing() {
    var c = clock;
    clock = null;
    if (!c) return {};
    var t = ms();
    var out = { away_ms: c.away, paused_ms: c.paused };
    if (c.front !== null && c.back !== null) out.think_ms = c.back - c.front - c.idleAtBack;
    if (c.back !== null) out.answer_ms = t - c.back - (idle(c) - c.idleAtBack);
    if (lastInput) out.input = lastInput;
    return out;
  }

  document.addEventListener('visibilitychange', function () {
    var c = clock;
    // Paused time is counted as paused, even with the tab hidden.
    if (!c || (c.front === null && c.back === null) || c.pausedSince !== null) return;
    if (document.hidden) c.hiddenSince = ms();
    else if (c.hiddenSince !== null) { c.away += ms() - c.hiddenSince; c.hiddenSince = null; }
  });
  // A page brought back from the browser's history has lost its clock.
  window.addEventListener('pageshow', function (e) { if (e.persisted) clock = null; });
  // How the rating was given: by key, mouse, finger or pen.
  document.addEventListener('keydown', function () { lastInput = 'key'; }, true);
  document.addEventListener('pointerdown', function (e) { lastInput = e.pointerType || 'mouse'; }, true);


  // ----- pausing (desktop) ----------------------------------------------------

  // A break of your own in the middle of a card: the card is covered, so
  // the pause can't be used to think, and its clock and timer stop.
  function paused() { return !!document.querySelector('main .study.paused'); }

  function pause() {
    var study = document.querySelector('main .study');
    var cover = study && study.querySelector('.pause-cover');
    if (!cover || paused() || busy) return;
    var c = clock;
    if (c && c.pausedSince === null) {
      // A hidden tab right now is the pause's from here on.
      if (c.hiddenSince !== null) { c.away += ms() - c.hiddenSince; c.hiddenSince = null; }
      c.pausedSince = ms();
    }
    if (timer) { timer.left = timer.end - Date.now(); cancelAnimationFrame(timer.frame); }
    study.classList.add('paused');
    cover.hidden = false;
    var go = cover.querySelector('[data-resume]');
    if (go) go.focus({ preventScroll: true });
  }

  function resume() {
    var study = document.querySelector('main .study.paused');
    if (!study) return;
    var c = clock;
    if (c && c.pausedSince !== null) {
      c.paused += ms() - c.pausedSince;
      c.pausedSince = null;
      if (document.hidden) c.hiddenSince = ms();
    }
    if (timer && timer.left !== undefined) {
      timer.end = Date.now() + timer.left;
      timer.left = undefined;
      timer.frame = requestAnimationFrame(timer.tick);
    }
    var cover = study.querySelector('.pause-cover');
    // Let go of Resume at once, or the next space would press it again
    // instead of showing the answer.
    var held = document.activeElement;
    if (held && (cover.contains(held) || held.closest('.pause-study'))) held.blur();
    study.classList.remove('paused');
    cover.hidden = true;
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
    place(doc, next, url);
    // The progress line glides on from where it was; a miss slips back.
    var bar = next.querySelector('.study-foot .bar');
    if (bar && from !== null) {
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
    begin(next);
  }

  // A card's front has just come up: the clock starts, the answer box
  // takes the keys.
  function begin(scope) {
    startClock(scope);
    startTimer(scope);
    var box = scope.querySelector('.study:not(.revealed) .typed-answer input');
    if (box) box.focus({ preventScroll: true });
  }
  begin(document);

  function send(form, submitter) {
    if (busy) return;
    busy = true;
    var data = new URLSearchParams(new FormData(form));
    if (submitter && submitter.name) data.append(submitter.name, submitter.value);
    if (form.getAttribute('action') === '/study/answer') {
      var took = timing();
      Object.keys(took).forEach(function (k) {
        var v = took[k];
        data.append(k, typeof v === 'number' ? String(Math.max(0, Math.round(v))) : v);
      });
    }
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
    if (e.target.closest && e.target.closest('.study .pause-study')) { pause(); return; }
    if (e.target.closest && e.target.closest('.study .pause-cover [data-resume]')) { resume(); return; }
    var show = e.target.closest && e.target.closest('.study .reveal a.show');
    if (show && reveal()) e.preventDefault();
    if (e.target.closest && e.target.closest('.timer .extend')) extendTimer();
  });

  // The reveal as a form (typed answers, the sure check): what was typed
  // goes above the answer, and sure or not goes along with the rating.
  function revealFrom(form, submitter) {
    var sure = submitter && submitter.name === 'sure' ? submitter.value : '';
    var fields = document.querySelectorAll('main .study input.sure-field');
    for (var i = 0; i < fields.length; i++) fields[i].value = sure;
    var typed = form.querySelector('input[name=typed]');
    var echo = document.querySelector('main .study .typed-echo');
    if (typed && echo) {
      var text = typed.value.trim();
      echo.querySelector('.typed-text').textContent = text;
      echo.hidden = !text;
    }
    reveal();
  }

  // A list card's Done says how many you remembered.
  document.addEventListener('change', function (e) {
    var box = e.target;
    if (!box.matches || !box.matches(':is(.list-items, .marked) input[type=checkbox]')) return;
    var boxes = box.form.querySelectorAll(':is(.list-items, .marked) input[type=checkbox]');
    var got = box.form.querySelectorAll(':is(.list-items, .marked) input[type=checkbox]:checked').length;
    var count = box.form.querySelector('[data-remembered]');
    if (count) count.textContent = got === boxes.length ? 'All ' + got + ' remembered' : got + ' of ' + boxes.length + ' remembered';
  });

  document.addEventListener('submit', function (e) {
    var form = e.target;
    if (form.matches('.study .reveal-form')) {
      e.preventDefault();
      revealFrom(form, e.submitter);
      return;
    }
    var action = form.getAttribute('action');
    if (!studying() || (action !== '/study/answer' && action !== '/study/undo')) return;
    e.preventDefault();
    send(form, e.submitter);
  });

  // Keyboard: space shows the answer (then means Good, or Done on a list
  // card), 1-4 answer; on a list card 1-9 mark items as remembered. With
  // the sure check S and U show the answer, and + adds time to a timed card.
  document.addEventListener('keydown', function (e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    var tag = e.target.tagName;
    if ((tag === 'INPUT' && e.target.type !== 'checkbox') || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (!document.querySelector('main .study')) return;
    // Paused, P, Esc, space or Enter carry on and every other key waits.
    if (paused()) {
      if (/^(p|P|Escape| |Enter)$/.test(e.key)) resume();
      e.preventDefault();
      return;
    }
    if (e.key === 'p' || e.key === 'P') { pause(); return; }
    // Enter and space on a focused link or button do what it says.
    if ((e.key === ' ' || e.key === 'Enter') && (tag === 'A' || tag === 'BUTTON') && !e.target.matches('.reveal a.show')) return;
    var k = e.key === ' ' || e.key === 'Enter' ? ' ' : e.key.toLowerCase();
    if (!/^[ 1-9su+]$/.test(k)) return;
    var visible = function (key) {
      var els = document.querySelectorAll('main [data-key="' + key + '"]');
      for (var i = 0; i < els.length; i++) if (!els[i].closest('[hidden]')) return els[i];
      return null;
    };
    var el = visible(k) || (k === ' ' ? visible('3') : null);
    if (!el && !/^[ 1-9]$/.test(k)) return;
    e.preventDefault();
    if (busy || !el) return;
    el.click();
  });
})();
