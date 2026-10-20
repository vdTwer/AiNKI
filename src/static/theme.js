// Chooses the look before the page is drawn. The simple look is the base;
// Calm is only switched on here, so without JavaScript, on phones and on
// e-ink screens the simple look is the only one.
(function () {
  var root = document.documentElement;
  var pref = root.getAttribute('data-look');
  var rail = null;
  try {
    if (pref) localStorage.setItem('ainki.look', pref);
    else pref = localStorage.getItem('ainki.look');
    rail = localStorage.getItem('ainki.rail');
  } catch (e) { /* storage blocked */ }
  if (!window.matchMedia) return;
  var desktop = window.matchMedia('(min-width: 60em) and (hover: hover) and (pointer: fine)').matches;
  var eink = window.matchMedia('(update: slow)').matches || window.matchMedia('(monochrome)').matches;
  if (pref === 'simple' || !desktop || eink) return;
  root.classList.add('calm');
  // The side rail starts collapsed while studying, open elsewhere, unless
  // the person chose otherwise.
  var studying = location.pathname === '/study' || location.pathname === '/offline';
  if (rail === 'closed' || (rail === null && studying)) root.classList.add('rail-collapsed');
})();
