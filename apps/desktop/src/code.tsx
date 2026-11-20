// Code: VS Code (code-server, bundled with Pitlane) over the tab, on the repository or a lane's
// worktree. Files opened elsewhere in Pitlane (a terminal path, Review, a run) open here at their line.
import { Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { onVscodePlace, onVscodeReady, vscode, type Lane } from "./api";
import { prefs } from "./prefs";
import { Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

const [rootSig, setRootSig] = createSignal<string | null>(null);
let pending: { root: string; path: string; line?: number; col?: number } | null = null;

/** Opens `path` under `root` in the Code tab, at a line when one is given. The app shows the Code
 *  tab when `show` is set. */
export function openInCode(root: string, path: string, line?: number, col?: number, show = false) {
  setRootSig(root);
  pending = { root, path, line, col };
  window.dispatchEvent(new CustomEvent("pitlane-code-open"));
  if (show) window.dispatchEvent(new CustomEvent("pitlane-show-code"));
}

// ---------------------------------------------------------------- Pitlane's colours, for VS Code

/** Hex of a CSS colour (oklch included), through a 1px canvas. */
function hex(css: string): string {
  const c = document.createElement("canvas");
  c.width = c.height = 1;
  const ctx = c.getContext("2d");
  if (!ctx) return "#000000";
  ctx.fillStyle = "#000";
  ctx.fillStyle = css;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return `#${[r, g, b].map((v) => v!.toString(16).padStart(2, "0")).join("")}`;
}

/** VS Code's workbench colours from the design tokens, for one scheme. */
function colours(): Record<string, string> {
  const cs = getComputedStyle(document.documentElement);
  const t = (n: string) => hex(cs.getPropertyValue(n).trim());
  const paper = t("--color-paper"), paper2 = t("--color-paper-2"), paper3 = t("--color-paper-3"), sunk = t("--color-paper-sunk");
  const rule = t("--color-rule"), ruleStrong = t("--color-rule-strong"), ink = t("--color-ink"), ink2 = t("--color-ink-2"), muted = t("--color-muted");
  const accent = t("--color-accent"), accentInk = t("--color-accent-ink"), accentText = t("--color-accent-text"), focus = t("--color-focus");
  return {
    foreground: ink2,
    descriptionForeground: muted,
    focusBorder: focus,
    "widget.border": rule,
    "editor.background": sunk,
    "editor.foreground": ink,
    "editorLineNumber.foreground": muted,
    "editorLineNumber.activeForeground": ink2,
    "editorCursor.foreground": accent,
    "editor.lineHighlightBackground": paper,
    "editor.lineHighlightBorder": paper,
    "editorIndentGuide.background1": rule,
    "editorWidget.background": paper2,
    "editorWidget.border": ruleStrong,
    "editorGroup.border": rule,
    "editorGroupHeader.tabsBackground": paper,
    "editorGroupHeader.tabsBorder": rule,
    "tab.activeBackground": sunk,
    "tab.inactiveBackground": paper,
    "tab.activeForeground": ink,
    "tab.inactiveForeground": muted,
    "tab.border": rule,
    "tab.activeBorderTop": accent,
    "sideBar.background": paper,
    "sideBar.border": rule,
    "sideBarSectionHeader.background": paper,
    "sideBarSectionHeader.border": rule,
    "sideBarTitle.foreground": ink2,
    "activityBar.background": paper,
    "activityBar.border": rule,
    "activityBar.foreground": ink,
    "activityBar.inactiveForeground": muted,
    "activityBar.activeBorder": accent,
    "activityBarBadge.background": accent,
    "activityBarBadge.foreground": accentInk,
    "activityBarTop.foreground": ink,
    "activityBarTop.inactiveForeground": muted,
    "activityBarTop.activeBorder": accent,
    "statusBar.background": paper,
    "statusBar.border": rule,
    "statusBar.foreground": muted,
    "statusBar.noFolderBackground": paper,
    "statusBarItem.remoteBackground": paper,
    "statusBarItem.remoteForeground": muted,
    "titleBar.activeBackground": paper,
    "titleBar.border": rule,
    "panel.background": sunk,
    "panel.border": rule,
    "panelTitle.activeBorder": accent,
    "terminal.background": sunk,
    "list.activeSelectionBackground": paper3,
    "list.activeSelectionForeground": ink,
    "list.inactiveSelectionBackground": paper3,
    "list.hoverBackground": paper2,
    "list.focusOutline": focus,
    "input.background": sunk,
    "input.border": ruleStrong,
    "dropdown.background": paper2,
    "dropdown.border": ruleStrong,
    "quickInput.background": paper2,
    "button.background": accent,
    "button.foreground": accentInk,
    "button.hoverBackground": focus,
    "badge.background": paper3,
    "badge.foreground": ink,
    "progressBar.background": accent,
    "textLink.foreground": accentText,
    "textLink.activeForeground": accentText,
    "scrollbarSlider.background": `${ruleStrong}80`,
    "scrollbarSlider.hoverBackground": ruleStrong,
  };
}

/** Both schemes, read by switching the root's theme for an instant. */
function theme(): { dark: Record<string, string>; light: Record<string, string> } {
  const el = document.documentElement;
  const before = el.getAttribute("data-theme");
  el.setAttribute("data-theme", "dark");
  const dark = colours();
  el.setAttribute("data-theme", "light");
  const light = colours();
  if (before == null) el.removeAttribute("data-theme");
  else el.setAttribute("data-theme", before);
  return { dark, light };
}

let warmed: string | null = null;

/** The app's theme changed: VS Code follows. */
export function rethemeCode() {
  void vscode.theme({ ...theme(), mode: prefs().theme }).catch(() => {});
}
/** Starts VS Code on the repository in the background, once Pitlane is open: the Code tab then
 *  shows a VS Code that is already loaded. */
export function prewarmCode(repo: string) {
  if (warmed === repo || !prefs().code_prewarm) return;
  warmed = repo;
  setTimeout(() => {
    void vscode
      .theme({ ...theme(), mode: prefs().theme })
      .then(() => vscode.prewarm(repo, window.innerWidth, Math.max(300, window.innerHeight - 76), ground()))
      .catch(() => {
        warmed = null; // it says why when the tab opens
      });
  }, 300);
}

/** The editor's ground, painted under VS Code while it loads, so nothing flashes white. */
const ground = () => hex(getComputedStyle(document.documentElement).getPropertyValue("--color-paper-sunk").trim());

// ---------------------------------------------------------------- the folders VS Code offers

/** The repository and the lanes' worktrees, for the folder picker inside VS Code's side bar. */
export function setCodePlaces(repo: string, repoName: string, lanes: Lane[]) {
  const places = [
    { root: repo, label: repoName, kind: "repo" as const },
    ...lanes.filter((l) => l.worktree && !l.worktree.is_main).map((l) => ({ root: l.worktree!.path, label: l.branch, kind: "lane" as const })),
  ];
  void vscode.places(places).catch(() => {});
}

// ---------------------------------------------------------------- the tab

export function CodeScreen(props: { repo: string; repoName: string; lanes: Lane[] }) {
  let host!: HTMLDivElement;
  const root = () => rootSig() ?? props.repo;
  const [state, setState] = createSignal<"starting" | "ready" | "failed">("starting");
  // "Opening VS Code" only when it takes long enough to notice.
  const [slow, setSlow] = createSignal(false);
  const slowTimer = setTimeout(() => setSlow(true), 400);
  onCleanup(() => clearTimeout(slowTimer));
  const [why, setWhy] = createSignal("");
  let covered = false;
  let alive = true;

  const rect = () => {
    const r = host.getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  };
  const place = async () => {
    if (!alive || covered) return;
    try {
      const ready = await vscode.show(root(), rect(), ground());
      if (alive && ready) setState("ready");
    } catch (e) {
      setWhy(msg(e));
      setState("failed");
    }
  };
  const openPending = async () => {
    if (!pending) return;
    const p = pending;
    pending = null;
    if (p.root !== root()) setRootSig(p.root);
    await place();
    try {
      await vscode.open(`${p.root}/${p.path}`, p.line, p.col);
    } catch (e) {
      toast({ tone: "fail", title: `Could not open ${p.path} in VS Code`, body: msg(e) });
    }
  };

  onMount(() => {
    // VS Code takes Pitlane's colours (it follows the system between dark and light by itself).
    void vscode
      .theme({ ...theme(), mode: prefs().theme })
      .catch(() => {})
      .then(place)
      .then(openPending);
    const ro = new ResizeObserver(() => void place());
    ro.observe(host);
    // App menus, sheets and dialogs draw in the page, under VS Code's own view: step aside for them.
    const overlay = () => !!document.querySelector(".pop, dialog[open]");
    const mo = new MutationObserver(() => {
      const now = overlay();
      if (now === covered) return;
      covered = now;
      if (covered) void vscode.hide().catch(() => {});
      else void place();
    });
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["open"] });
    let offReady: (() => void) | null = null;
    let offPlace: (() => void) | null = null;
    void onVscodeReady((folder) => folder === root() && setState("ready")).then((off) => (alive ? (offReady = off) : off()));
    // The picker inside VS Code chose another folder.
    void onVscodePlace((r) => setRootSig(r)).then((off) => (alive ? (offPlace = off) : off()));
    const onRequest = () => void openPending();
    window.addEventListener("pitlane-code-open", onRequest);
    onCleanup(() => {
      alive = false;
      offReady?.();
      offPlace?.();
      ro.disconnect();
      mo.disconnect();
      window.removeEventListener("pitlane-code-open", onRequest);
      void vscode.hide().catch(() => {});
    });
  });
  createEffect(on(root, () => (setState("starting"), void place()), { defer: true }));

  return (
    <div class="vs-screen">
      <div class="vs-host" ref={host}>
        <Show when={state() === "starting" && slow()}>
          <div class="vs-opening" role="status">
            <Spinner /> Opening VS Code
          </div>
        </Show>
        <Show when={state() === "failed"}>
          <div class="code-empty">
            <div class="code-empty-title">VS Code did not start</div>
            <p class="code-empty-body selectable">{why()}</p>
            <div class="vs-actions">
              <button type="button" class="btn" onClick={() => (setState("starting"), void place())}>
                Try again
              </button>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}
