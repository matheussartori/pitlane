// The app's preferences: read once at start, changed from Settings, saved in ~/.pitlane/desktop.json.
import { createSignal } from "solid-js";
import { DEFAULT_PREFS, api, type Prefs } from "./api";
import { retheme, setTerminalNotify } from "./terminals/model";

const [prefs, setPrefsSignal] = createSignal<Prefs>(DEFAULT_PREFS);
export { prefs };

let applied: Prefs["theme"] | undefined;

/** Light or dark on the root (the tokens follow), or the system's when "system". */
function apply() {
  const t = prefs().theme;
  if (t === "light" || t === "dark") document.documentElement.setAttribute("data-theme", t);
  else document.documentElement.removeAttribute("data-theme");
  setTerminalNotify(prefs().notify_terminals);
  const ui = prefs().ui_language;
  document.documentElement.lang = (!ui || ui === "system" ? navigator.language : ui).toLowerCase().startsWith("pt") ? "pt-BR" : "en";
  // Only when the theme itself changed: any other preference (a notification, the language)
  // must not repaint every terminal and re-send VS Code its colours.
  if (t !== applied) {
    applied = t;
    retheme();
    window.dispatchEvent(new CustomEvent("pitlane-theme"));
  }
}

export function loadPrefs(p: Partial<Prefs> | undefined) {
  setPrefsSignal({ ...DEFAULT_PREFS, ...p });
  apply();
}

export async function setPrefs(patch: Partial<Prefs>) {
  setPrefsSignal({ ...prefs(), ...patch });
  apply();
  await api.prefsSet(prefs());
}
