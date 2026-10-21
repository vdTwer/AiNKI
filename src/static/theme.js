// Chooses the look before the page is drawn. The simple look is the base;
// Calm is only switched on here, so without JavaScript, on phones and on
// e-ink screens the simple look is the only one.
(function () {
  var root = document.documentElement;
  var pref = root.getAttribute('data-look');
  try {
    if (pref) localStorage.setItem('ainki.look', pref);
    else pref = localStorage.getItem('ainki.look');
  } catch (e) { /* storage blocked */ }
  if (!window.matchMedia) return;
  // The query lives once, on the Calm stylesheet's <link> (web.rs).
  var link = document.querySelector('link[data-calm]');
  var desktop = !!link && window.matchMedia(link.media).matches;
  var eink = window.matchMedia('(update: slow)').matches || window.matchMedia('(monochrome)').matches;
  if (pref === 'simple' || !desktop || eink) return;
  root.classList.add('calm');
})();
