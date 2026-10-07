// Chooses the color scheme before the page paints. A same-origin page that
// embeds the playground, such as the author's portfolio, may set
// `data-theme="light"` or `"dark"` on its root element; the playground follows
// it, including later changes. Otherwise it follows the system setting.
(() => {
  const root = document.documentElement;
  const system = matchMedia('(prefers-color-scheme: dark)');
  let host = null;
  try {
    if (window.parent !== window) host = window.parent.document.documentElement;
  } catch {
    // A cross-origin host cannot be read; follow the system instead.
  }
  root.classList.toggle('embedded', window.parent !== window);

  function apply() {
    const chosen = host?.getAttribute('data-theme');
    root.dataset.theme = chosen === 'light' || chosen === 'dark' ? chosen : system.matches ? 'dark' : 'light';
  }

  apply();
  system.addEventListener('change', apply);
  if (host) new MutationObserver(apply).observe(host, { attributes: true, attributeFilter: ['data-theme'] });
})();
