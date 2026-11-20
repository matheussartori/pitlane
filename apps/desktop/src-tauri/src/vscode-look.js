// Pitlane's look on VS Code's page. Its Content Security Policy blocks inline <style> and data:
// fonts, so the CSS goes in as a constructed stylesheet and the fonts as bytes through FontFace,
// neither of which the policy covers. The app fills in the CSS and the fonts below.
(function () {
  if (window.__pitlaneLook) return;
  window.__pitlaneLook = true;
  var css = __CSS__;
  var fonts = __FONTS__;
  function go() {
    try {
      var sheet = new CSSStyleSheet();
      sheet.replaceSync(css);
      document.adoptedStyleSheets = document.adoptedStyleSheets.concat([sheet]);
    } catch (e) {}
    fonts.forEach(function (f) {
      try {
        var bin = atob(f[1]);
        var bytes = new Uint8Array(bin.length);
        for (var i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
        var face = new FontFace(f[0], bytes.buffer, { weight: "100 900" });
        document.fonts.add(face);
        face.load();
      } catch (e) {}
    });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", go);
  else go();
})();
