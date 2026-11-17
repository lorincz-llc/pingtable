// pingtable.net: the light/dark switch, and a little life in the sample table.
// Loaded in the head so the chosen look is set before the page is drawn.

(function () {
  var root = document.documentElement;
  var saved = null;
  try { saved = localStorage.getItem('pingtable.site.theme'); } catch (e) {}
  // Light is the default; dark only when chosen here before.
  root.dataset.theme = saved === 'dark' ? 'dark' : 'light';

  document.addEventListener('DOMContentLoaded', function () {
    var button = document.getElementById('theme');
    if (button) {
      button.addEventListener('click', function () {
        var next = root.dataset.theme === 'dark' ? 'light' : 'dark';
        root.dataset.theme = next;
        try { localStorage.setItem('pingtable.site.theme', next); } catch (e) {}
      });
    }

    // The sample table: nudge the "last" figures once a second and run the
    // stopwatch, the way the app looks while it is pinging. Nothing is sent
    // anywhere; the numbers are made up around each row's typical time.
    if (window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    var cells = document.querySelectorAll('[data-base]');
    var clock = document.getElementById('clock');
    var started = Date.now();
    var two = function (n) { return (n < 10 ? '0' : '') + n; };
    setInterval(function () {
      if (document.hidden) return;
      cells.forEach(function (cell) {
        var base = parseFloat(cell.dataset.base);
        var jitter = parseFloat(cell.dataset.jitter);
        var value = Math.max(0.01, base + (Math.random() - 0.5) * 2 * jitter);
        cell.textContent = value < 10 ? value.toFixed(2) : value.toFixed(1);
      });
      if (clock) {
        var seconds = Math.floor((Date.now() - started) / 1000);
        clock.textContent = two(Math.floor(seconds / 60) % 60) + ':' + two(seconds % 60);
      }
    }, 1000);
  });
})();
