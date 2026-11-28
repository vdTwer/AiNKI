// @ts-check
// AiNKI's custom elements. Each one wraps markup the server already sent,
// so every page works without them; app.js swaps pages in, and the browser
// upgrades whatever elements arrive. Plain JavaScript, typed with JSDoc and
// checked by `tsc --checkJs` (see e2e/tsconfig.parts.json); no build step.

/** True when keys typed at `t` belong to a field, not to the page. */
function typing(/** @type {EventTarget | null} */ t) {
  return t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
}

/**
 * `<ainki-find>`: the quick find. Opens its dialog with /, Ctrl/Cmd+K, the
 * menu's Find link, or a letter typed anywhere outside a field (not while
 * studying: there letters answer the card). Results are the server's own
 * /find page; only its `#find-results` is swapped in.
 */
class AinkiFind extends HTMLElement {
  /** @type {HTMLDialogElement | null} */ dialog = null;
  /** @type {HTMLInputElement | null} */ input = null;
  /** @type {AbortController | null} */ asking = null;
  active = 0;
  timer = 0;

  connectedCallback() {
    this.dialog = this.querySelector('dialog');
    this.input = this.querySelector('input[type=search]');
    if (!this.dialog || !this.input) return;
    const { dialog, input } = this;
    document.addEventListener('keydown', this.onKey);
    document.addEventListener('click', this.onLink);
    input.addEventListener('input', () => {
      clearTimeout(this.timer);
      this.timer = window.setTimeout(() => this.ask(), 80);
    });
    input.addEventListener('keydown', (e) => this.move(e));
    dialog.addEventListener('click', (e) => { if (e.target === dialog) dialog.close(); });
    dialog.addEventListener('close', () => { input.value = ''; this.results().replaceChildren(); });
  }

  disconnectedCallback() {
    document.removeEventListener('keydown', this.onKey);
    document.removeEventListener('click', this.onLink);
  }

  /** @param {KeyboardEvent} e */
  onKey = (e) => {
    if (e.defaultPrevented || e.isComposing || typing(e.target) || this.blocked()) return;
    const k = e.key;
    if ((e.ctrlKey || e.metaKey) && !e.altKey && k.toLowerCase() === 'k') { e.preventDefault(); this.open(''); return; }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (k === '/') { e.preventDefault(); this.open(''); return; }
    if (k.length === 1 && /\p{L}/u.test(k) && !document.querySelector('main .study')) { e.preventDefault(); this.open(k); }
  };

  /** @param {MouseEvent} e */
  onLink = (e) => {
    const a = e.target instanceof Element ? e.target.closest('a[href="/find"]') : null;
    if (!a || e.ctrlKey || e.metaKey || e.shiftKey || this.blocked()) return;
    e.preventDefault();
    this.open('');
  };

  /** Quiet hours block everything, and the key sheet keeps its keys. */
  blocked() {
    return !!document.querySelector('.sleep-wall, .key-sheet:popover-open');
  }

  /** @param {string} text */
  open(text) {
    const { dialog, input } = this;
    if (!dialog || !input) return;
    if (!dialog.open) dialog.showModal();
    input.value = text;
    input.focus();
    this.ask();
  }

  results() {
    return /** @type {HTMLElement} */ (this.querySelector('#find-results'));
  }

  rows() {
    return [...this.querySelectorAll('#find-results a.row')];
  }

  async ask() {
    const q = this.input?.value.trim() ?? '';
    this.asking?.abort();
    this.asking = new AbortController();
    try {
      const res = await fetch(`/find?q=${encodeURIComponent(q)}`, { signal: this.asking.signal });
      if (!res.ok) return;
      const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
      const fresh = doc.querySelector('#find-results');
      if (fresh && q === (this.input?.value.trim() ?? '')) {
        this.results().replaceChildren(...fresh.childNodes);
        this.mark(0);
      }
    } catch (err) {
      if (!(err instanceof DOMException && err.name === 'AbortError')) throw err;
    }
  }

  /** @param {number} i */
  mark(i) {
    const rows = this.rows();
    if (!rows.length) return;
    this.active = (i + rows.length) % rows.length;
    rows.forEach((a, n) => a.classList.toggle('on', n === this.active));
    rows[this.active].scrollIntoView({ block: 'nearest' });
  }

  /** @param {KeyboardEvent} e */
  move(e) {
    const back = e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey);
    const on = e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey);
    // A search field would only clear itself on Esc; here Esc closes.
    if (e.key === 'Escape') { e.preventDefault(); this.dialog?.close(); return; }
    if (on || back) {
      e.preventDefault();
      this.mark(this.active + (on ? 1 : -1));
    } else if (e.key === 'Enter') {
      const row = this.rows()[this.active];
      if (row instanceof HTMLAnchorElement) { e.preventDefault(); row.click(); }
    }
  }
}

/**
 * `<ainki-break seconds="300">`: a break that runs on its own. It counts
 * down and then sends the "keep going" form, so the next card comes back
 * by itself. The end time survives a reload; ending early forgets it.
 */
class AinkiBreak extends HTMLElement {
  static KEY = 'ainki.break';
  tick = 0;

  connectedCallback() {
    const form = /** @type {HTMLFormElement | null} */ (document.getElementById('keep-going'));
    const length = Number(this.getAttribute('seconds')) * 1000;
    if (!form || !(length > 0)) return;
    form.addEventListener('submit', () => this.forget());
    const saved = this.saved();
    const until = saved && saved.until > Date.now() ? saved.until : Date.now() + length;
    this.store({ until, length });
    this.show(until, length, form);
    this.tick = window.setInterval(() => this.show(until, length, form), 250);
  }

  disconnectedCallback() {
    clearInterval(this.tick);
  }

  /** @returns {{ until: number, length: number } | null} */
  saved() {
    try { return JSON.parse(sessionStorage.getItem(AinkiBreak.KEY) ?? 'null'); } catch { return null; }
  }

  /** @param {{ until: number, length: number }} b */
  store(b) {
    try { sessionStorage.setItem(AinkiBreak.KEY, JSON.stringify(b)); } catch { /* storage blocked */ }
  }

  forget() {
    clearInterval(this.tick);
    try { sessionStorage.removeItem(AinkiBreak.KEY); } catch { /* storage blocked */ }
  }

  /**
   * @param {number} until
   * @param {number} length
   * @param {HTMLFormElement} form
   */
  show(until, length, form) {
    const left = Math.max(0, until - Date.now());
    const s = Math.ceil(left / 1000);
    const clock = this.querySelector('.break-clock');
    const fill = /** @type {HTMLElement | null} */ (this.querySelector('.break-fill'));
    if (clock) clock.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
    if (fill) fill.style.width = `${(100 - (left / length) * 100).toFixed(2)}%`;
    if (left <= 0) { this.forget(); form.requestSubmit(); }
  }
}

/**
 * `<ainki-card-type data-fields data-side>`: a card drawn by a type someone
 * brought (see src/types.rs). It holds the type's sandboxed frame and is the
 * only part of the page that talks to it: it hands over the card's fields
 * and the page's colors, and hears the frame's height and the learner's
 * response. A response turns the card over, as Space would; app.js says
 * when the card is turned (`ainki:reveal`), and the frame then draws the
 * solution. With `data-preview` (the card form) the fields come from the
 * form as it is written, and the Front and Back buttons pick the side.
 */
class AinkiCardType extends HTMLElement {
  /** @type {Set<AinkiCardType>} */
  static all = new Set();
  static THEME = ['ink', 'muted', 'faint', 'line', 'edge', 'track', 'bg', 'accent', 'fill', 'on-fill', 'soft', 'danger', 'sans', 'mono', 'serif', 'r', 'scale'];
  /** @type {HTMLIFrameElement | null} */ frame = null;
  /** @type {string | null} */ response = null;
  side = 'challenge';
  timer = 0;

  connectedCallback() {
    this.frame = this.querySelector('iframe');
    if (!this.frame) return;
    this.side = this.dataset.side === 'solution' ? 'solution' : 'challenge';
    AinkiCardType.all.add(this);
    this.closest('.study')?.addEventListener('ainki:reveal', this.onReveal);
    const form = this.closest('form');
    if (this.hasAttribute('data-preview') && form) {
      form.addEventListener('input', this.onInput);
      for (const b of this.querySelectorAll('button[data-side]')) b.addEventListener('click', this.onSide);
    }
    this.show();
  }

  disconnectedCallback() {
    AinkiCardType.all.delete(this);
    this.closest('.study')?.removeEventListener('ainki:reveal', this.onReveal);
  }

  /** The frame is told what to draw; it draws a side only once. */
  show() {
    const style = getComputedStyle(document.documentElement);
    /** @type {Record<string, string>} */
    const theme = {};
    for (const k of AinkiCardType.THEME) {
      const v = style.getPropertyValue('--' + k).trim();
      if (v) theme[k] = v;
    }
    let fields = {};
    try { fields = JSON.parse(this.dataset.fields ?? '{}'); } catch { /* left empty */ }
    this.frame?.contentWindow?.postMessage({ type: 'show', side: this.side, fields, response: this.response, theme }, '*');
  }

  onReveal = () => {
    this.side = 'solution';
    this.show();
  };

  /** @param {Event} e */
  onSide = (e) => {
    const b = /** @type {HTMLElement} */ (e.currentTarget);
    this.side = b.dataset.side === 'solution' ? 'solution' : 'challenge';
    for (const o of this.querySelectorAll('button[data-side]')) o.setAttribute('aria-pressed', String(o === b));
    this.show();
  };

  onInput = () => {
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => {
      const form = this.closest('form');
      if (!form) return;
      /** @type {Record<string, string>} */
      const fields = {};
      for (const [k, v] of new FormData(form)) {
        if (k.startsWith('f.') && typeof v === 'string' && v.trim()) fields[k.slice(2)] = v.trim();
      }
      this.dataset.fields = JSON.stringify(fields);
      this.show();
    }, 250);
  };

  /** @param {unknown} m */
  heard(m) {
    if (!m || typeof m !== 'object' || !this.frame) return;
    const msg = /** @type {Record<string, unknown>} */ (m);
    if (msg.type === 'ready') {
      this.show();
    } else if (msg.type === 'height' && typeof msg.height === 'number') {
      this.frame.style.height = `${Math.max(0, Math.min(4000, Math.round(msg.height)))}px`;
    } else if (msg.type === 'response' && typeof msg.response === 'string' && msg.response.length <= 4096) {
      if (this.side !== 'challenge' || this.hasAttribute('data-preview')) return;
      this.response = msg.response;
      const study = this.closest('.study');
      for (const f of study?.querySelectorAll('input[name=response]') ?? []) {
        /** @type {HTMLInputElement} */ (f).value = msg.response;
      }
      document.dispatchEvent(new CustomEvent('ainki:respond'));
    }
  }
}

// Messages from frames: each goes to the element whose frame sent it.
addEventListener('message', (e) => {
  for (const el of AinkiCardType.all) {
    if (el.frame && e.source === el.frame.contentWindow) el.heard(e.data);
  }
});

customElements.define('ainki-find', AinkiFind);
customElements.define('ainki-break', AinkiBreak);
customElements.define('ainki-card-type', AinkiCardType);

// The ? key shows or hides the key sheet (a popover the ? hint opens too).
document.addEventListener('keydown', (e) => {
  if (e.key !== '?' || e.ctrlKey || e.metaKey || e.altKey || typing(e.target)) return;
  const sheet = document.getElementById('keys');
  if (!sheet) return;
  e.preventDefault();
  sheet.togglePopover();
});
