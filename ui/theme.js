// Sets the look (light or dark) before the page renders, so there is no flash
// of the other one. app.js keeps the choice in localStorage.
(function () {
  try {
    var look = localStorage.getItem('pingtable.theme');
    if (look === 'system') look = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    document.documentElement.dataset.theme = look === 'dark' ? 'dark' : 'light';
  } catch (e) {}
})();
