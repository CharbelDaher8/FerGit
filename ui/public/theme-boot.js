// Sets the theme before first paint, so the window doesn't flash the wrong colors while the app
// loads. A classic script, run from <head> before the page renders; the app takes over from
// src/lib/theme.svelte.ts once it starts. It must agree with `parseThemeMode` and `resolveTheme`
// there, which theme.test.ts checks.
(function () {
  var mode = null;
  try {
    mode = localStorage.getItem("fergit.theme");
  } catch (e) {
    // Storage unavailable: follow the OS.
  }
  var dark = mode === "dark" || (mode !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
})();
