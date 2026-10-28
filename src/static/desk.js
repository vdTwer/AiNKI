// The desktop look's extras. It runs only in the Orchid (calm) look, which
// always has JavaScript; the simple look never sees any of this, and every
// page works without it.
(function () {
  'use strict';
  var root = document.documentElement;
  if (!root.classList.contains('calm')) return;

  // Home: the number of cards to study is set in the light italic.
  var count = document.querySelector('.today h1 .count');
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

  // A ⋯ button opens the menu it controls; Escape or a click elsewhere
  // closes it.
  var toggles = document.querySelectorAll('button.more-toggle[aria-controls]');
  Array.prototype.forEach.call(toggles, function (b) {
    var menu = document.getElementById(b.getAttribute('aria-controls'));
    if (!menu) return;
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
})();
