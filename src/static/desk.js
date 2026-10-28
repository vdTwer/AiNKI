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
})();
