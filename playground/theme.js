// Demo kit v1 (contract: /demo-kit/v1/): sets a demo's color scheme. Load it
// as a classic script in <head>, before tokens.css, so the scheme is set before
// first paint. The first match wins:
// 1. the embedding portfolio page's <html data-theme>, which it follows live;
// 2. a #light or #dark URL fragment;
// 3. the system preference (no data-theme attribute).
// It sets <html data-theme> and, inside a frame, <html data-embedded>, exposes
// window.demoTheme() ("light" or "dark"), and fires a "demo-theme" event on
// window when the scheme changes, for canvas apps that draw their own colors.
(function () {
    "use strict";
    var root = document.documentElement;
    var embedded = window.parent !== window;
    var dark = window.matchMedia("(prefers-color-scheme: dark)");

    function scheme(value) {
        return value === "light" || value === "dark" ? value : null;
    }
    function hostTheme() {
        try {
            return embedded ? scheme(window.parent.document.documentElement.getAttribute("data-theme")) : null;
        } catch (error) {
            return null; // A cross-origin host cannot be read.
        }
    }
    function current() {
        return scheme(root.getAttribute("data-theme")) || (dark.matches ? "dark" : "light");
    }

    var last = null;
    function apply() {
        var chosen = hostTheme() || scheme(window.location.hash.slice(1));
        if (chosen) root.setAttribute("data-theme", chosen);
        else root.removeAttribute("data-theme");
        if (current() !== last) {
            last = current();
            window.dispatchEvent(new CustomEvent("demo-theme", { detail: last }));
        }
    }

    if (embedded) root.setAttribute("data-embedded", "");
    window.demoTheme = current;
    apply();
    window.addEventListener("hashchange", apply);
    dark.addEventListener("change", apply);
    try {
        if (embedded) {
            new MutationObserver(apply).observe(window.parent.document.documentElement, {
                attributes: true,
                attributeFilter: ["data-theme"],
            });
        }
    } catch (error) {
        // A cross-origin host keeps the fragment or system preference.
    }
})();
