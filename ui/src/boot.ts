// Runs in <head> before the stylesheet and the body (a classic script, no
// import/export): applies this device's interface size and spacing before
// anything is laid out, so the app never shows up at the wrong size first.
// The settings themselves live in uiprefs.ts (same storage keys) and
// web/viewport.ts (same viewport rule) – keep the three in step.
//
//  * Spacing: `data-density` on <html> ("tight" | "air"; normal has none).
//  * Size, browser: the viewport meta is the page zoom of phones and tablets –
//    a layout width of device width / scale at an initial scale of `scale`, so
//    every coordinate (pointer events, element boxes, canvas sizes) stays in
//    one system. Desktop app: the shell zooms the webview itself (lib.rs),
//    before the window shows.
(() => {
  const root = document.documentElement;
  let density: string | null = null;
  let scale = 1;
  try {
    density = localStorage.getItem("klangwelt.density");
    scale = Number(localStorage.getItem("klangwelt.uiScale") ?? "1");
  } catch {
    // storage blocked: defaults
  }
  if (density === "tight" || density === "air") root.dataset.density = density;
  if ("__TAURI__" in window || !(scale >= 0.4 && scale <= 2) || scale === 1) return;
  const meta = document.querySelector('meta[name="viewport"]');
  const vv = window.visualViewport;
  const deviceWidth = Math.round(vv ? vv.width * vv.scale : window.innerWidth);
  // never a layout narrower than 300 px (a large size on a phone held upright)
  scale = Math.min(scale, Math.max(1, Math.floor((deviceWidth / 300) * 20 + 1e-9) / 20));
  if (scale === 1) return;
  meta?.setAttribute(
    "content",
    `width=${(deviceWidth / scale).toFixed(2)}, initial-scale=${scale}, viewport-fit=cover, interactive-widget=resizes-content`,
  );
})();
