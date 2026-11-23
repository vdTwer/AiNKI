// The bootstrap of a brought card type's frame (see src/types.rs). It runs
// inside the sandbox, next to the type's own module, and is the only code
// there that AiNKI wrote. It loads the module, and passes messages:
//
//   from the study page  {type: 'show', side: 'challenge' | 'solution',
//                         fields, response, theme}
//   to the study page    {type: 'ready'}, {type: 'height', height},
//                        {type: 'response', response}
//
// The module exports two functions, each drawing into `el`:
//
//   challenge(fields, el, respond)  respond(value) hands over what the
//                                   learner did (any JSON value, 4 KB at
//                                   most) and turns the card over
//   solution(fields, response, el)  response is that value, or null when
//                                   the learner turned the card over
//                                   themselves

const root = /** @type {HTMLElement} */ (document.getElementById('card'));
const send = (/** @type {object} */ msg) => parent.postMessage(msg, '*');
const loaded = import(new URL(document.body.dataset.module || '', location.href).href);
let shown = '';
let run = 0;

/** Says so in the frame, and in the console, when the type fails. */
function failed(/** @type {unknown} */ err) {
  console.error('card type:', err);
  const p = document.createElement('p');
  p.className = 'type-failed';
  p.textContent = 'This card type could not draw the card.';
  root.replaceChildren(p);
}

addEventListener('message', async (e) => {
  if (e.source !== parent) return;
  const m = e.data;
  if (!m || m.type !== 'show' || (m.side !== 'challenge' && m.side !== 'solution')) return;
  const key = JSON.stringify([m.side, m.fields, m.response]);
  if (key === shown) return;
  shown = key;
  const mine = ++run;
  for (const [k, v] of Object.entries(m.theme || {})) {
    if (typeof v === 'string') document.documentElement.style.setProperty('--' + k, v);
  }
  let mod;
  try {
    mod = await loaded;
  } catch (err) {
    failed(err);
    return;
  }
  if (mine !== run) return;
  const el = document.createElement('div');
  root.replaceChildren(el);
  const respond = (/** @type {unknown} */ value) => {
    if (mine !== run || m.side !== 'challenge') return;
    const json = JSON.stringify(value === undefined ? null : value);
    if (json.length > 4096) {
      console.error('card type: a response has at most 4096 characters of JSON');
      return;
    }
    send({ type: 'response', response: json });
  };
  try {
    if (m.side === 'challenge') await mod.challenge(m.fields, el, respond);
    else await mod.solution(m.fields, m.response == null ? null : JSON.parse(m.response), el);
  } catch (err) {
    if (mine === run) failed(err);
  }
});

let last = -1;
new ResizeObserver(() => {
  const height = Math.ceil(document.body.getBoundingClientRect().height);
  if (height !== last) send({ type: 'height', height: (last = height) });
}).observe(document.body);

send({ type: 'ready' });
