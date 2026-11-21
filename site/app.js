// pingtable.net: the light/dark switch, and the demo's frame.
// Loaded in the head so the chosen look is set before the page is drawn.

(function () {
  var root = document.documentElement;
  var saved = null;
  try { saved = localStorage.getItem('pingtable.site.theme'); } catch (e) {}
  // Light is the default; dark only when chosen here before.
  root.dataset.theme = saved === 'dark' ? 'dark' : 'light';

  // A page whose Content-Security-Policy does not allow the frame shows a
  // link to the demo's own page instead of an empty box.
  document.addEventListener('securitypolicyviolation', function (e) {
    var directive = e.effectiveDirective || e.violatedDirective || '';
    if (!/^(frame|child)-src/.test(directive)) return;
    var box = document.getElementById('demo-window');
    if (box) box.classList.add('is-blocked');
  });

  // The demo runs sandboxed, in an opaque origin: it cannot read this page or its storage, so
  // the page tells it the look. Its origin is "null", hence '*'; the look is no secret, and the
  // demo takes only {pingtableTheme: 'light' | 'dark'} from this page.
  function tellDemo(frame) {
    if (!frame || !frame.contentWindow) return;
    try { frame.contentWindow.postMessage({ pingtableTheme: root.dataset.theme === 'dark' ? 'dark' : 'light' }, '*'); } catch (e) {}
  }

  document.addEventListener('DOMContentLoaded', function () {
    var frame = document.getElementById('demo-frame');
    if (frame) {
      // Dark from its first picture on, when the page is dark; then the page's look each time it loads.
      if (root.dataset.theme === 'dark') frame.src = (frame.getAttribute('src') || '/demo/').split('?')[0] + '?theme=dark';
      frame.addEventListener('load', function () { tellDemo(frame); });
    }
    var button = document.getElementById('theme');
    if (!button) return;
    button.addEventListener('click', function () {
      var next = root.dataset.theme === 'dark' ? 'light' : 'dark';
      root.dataset.theme = next;
      try { localStorage.setItem('pingtable.site.theme', next); } catch (e) {}
      tellDemo(frame); // demo/replay.js follows it
    });
  });
})();
