// Which folder the Code tab shows, and the file asked to open there: any screen can send a file
// to Code (a terminal path, Review, a run) without knowing the tab.
import { createSignal } from "solid-js";

const [rootSig, setRootSig] = createSignal<string | null>(null);
export { setRootSig as setCodeRoot };
/** The folder Code shows (a lane's worktree or the repository), once it was opened. */
export const codeRoot = () => rootSig();
/** Another repository: Code goes back to showing the repository itself, not a folder of the last one. */
export const resetCodeRoot = () => setRootSig(null);
let pending: { root: string; path: string; line?: number; col?: number } | null = null;

/** Opens `path` under `root` in the Code tab, at a line when one is given. The app shows the Code
 *  tab when `show` is set. */
export function openInCode(root: string, path: string, line?: number, col?: number, show = false) {
  setRootSig(root);
  pending = { root, path, line, col };
  window.dispatchEvent(new CustomEvent("pitlane-code-open"));
  if (show) window.dispatchEvent(new CustomEvent("pitlane-show-code"));
}

/** The file waiting to be opened, handed over once: the Code tab takes it when it is ready. */
export function takeCodeOpen() {
  const p = pending;
  pending = null;
  return p;
}
