// Code: VS Code (code-server, bundled with Pitlane) over the tab, on the repository or a lane's
// worktree. Files opened elsewhere in Pitlane (a terminal path, Review, a run) open here at their line.
import { Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { api, onVscodePlace, onVscodeReady, vscode, type Lane } from "./api";
import { loadCards, repo, yard } from "./store/repo";
import { t } from "./i18n";
import { prefs } from "./prefs";
import { Spinner, toast } from "./ui";
import { msg, cssHex } from "./format";


const [rootSig, setRootSig] = createSignal<string | null>(null);
/** The folder Code shows (a lane's worktree or the repository), once it was opened. */
export const codeRoot = () => rootSig();
let pending: { root: string; path: string; line?: number; col?: number } | null = null;

/** Opens `path` under `root` in the Code tab, at a line when one is given. The app shows the Code
 *  tab when `show` is set. */
export function openInCode(root: string, path: string, line?: number, col?: number, show = false) {
  setRootSig(root);
  pending = { root, path, line, col };
  window.dispatchEvent(new CustomEvent("pitlane-code-open"));
  if (show) window.dispatchEvent(new CustomEvent("pitlane-show-code"));
}

// ---------------------------------------------------------------- "Ask Claude about this" (⌘L in VS Code)

/** What the bridge extension sends: the ask, and where in which file. */
interface CodeAsk {
  ask: string;
  file: string;
  root: string;
  from: number;
  to: number;
  language: string;
  text: string;
}

/** The ask becomes a Backlog card: what you asked, the file and lines, and the code you pointed at. */
async function cardFromCode(a: CodeAsk) {
  const path = repo();
  if (!path) return;
  const root = a.root || path;
  const rel = a.file.startsWith(`${root}/`) ? a.file.slice(root.length + 1) : a.file;
  const lane = yard()?.lanes.find((l) => l.worktree?.path === root);
  const lines = a.from === a.to ? t("line {n}", { n: a.from }) : t("lines {from} to {to}", { from: a.from, to: a.to });
  const where = lane ? t("in the lane {branch}", { branch: lane.branch }) : t("in the repository");
  const first = a.ask.split("\n")[0]!.trim();
  const title = first.length > 80 ? `${first.slice(0, 79)}…` : first;
  const fence = a.text.includes("```") ? "~~~" : "```";
  const body = [a.ask, "", t("In `{file}`, {lines}, {where}:", { file: rel, lines, where }), "", `${fence}${a.language || ""}`, a.text.replace(/\s+$/, ""), fence].join("\n");
  try {
    const c = await api.cardAdd(path, title, body);
    void loadCards();
    toast({
      tone: "done",
      title: t("#{n} {title} is on the Board", { n: c.id, title: c.title }),
      body: `${rel}:${a.from}${a.to !== a.from ? `-${a.to}` : ""}`,
      action: { label: t("Open"), run: () => window.dispatchEvent(new CustomEvent("pitlane-open-card", { detail: c.id })) },
    });
  } catch (e) {
    toast({ tone: "fail", title: t("The card was not made"), body: msg(e) });
  }
}

if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) void listen<CodeAsk>("vscode-ask", (e) => void cardFromCode(e.payload));

// ---------------------------------------------------------------- Pitlane's colours, for VS Code


/** VS Code's workbench colours from the design tokens, for one scheme. */
function colours(): Record<string, string> {
  const cs = getComputedStyle(document.documentElement);
  const tok = (n: string) => cssHex(cs.getPropertyValue(n).trim());
  const paper = tok("--color-paper"), paper2 = tok("--color-paper-2"), paper3 = tok("--color-paper-3"), sunk = tok("--color-paper-sunk");
  const rule = tok("--color-rule"), ruleStrong = tok("--color-rule-strong"), ink = tok("--color-ink"), ink2 = tok("--color-ink-2"), muted = tok("--color-muted");
  const accent = tok("--color-accent"), accentInk = tok("--color-accent-ink"), accentText = tok("--color-accent-text"), focus = tok("--color-focus");
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
const ground = () => cssHex(getComputedStyle(document.documentElement).getPropertyValue("--color-paper-sunk").trim());

// ---------------------------------------------------------------- the folders VS Code offers

/** The repository and the lanes' worktrees, for the folder picker inside VS Code's side bar. */
export function setCodePlaces(repo: string, repoName: string, lanes: Lane[], head?: string | null) {
  // The repository's own checkout is named by the branch it has out (its name is already on top).
  const main = head ?? lanes.find((l) => l.worktree?.is_main)?.branch ?? repoName;
  const places = [
    { root: repo, label: main, kind: "repo" as const },
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
      // Left (or covered) while the view was being shown: its hide may have landed first.
      if (!alive || covered) return void vscode.hide().catch(() => {});
      if (ready) setState("ready");
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
    if (!p.path) return; // the folder alone: it is the place now
    try {
      await vscode.open(`${p.root}/${p.path}`, p.line, p.col);
    } catch (e) {
      toast({ tone: "fail", title: t("Could not open {path} in VS Code", { path: p.path }), body: msg(e) });
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
    // Tooltips and toasts stay with VS Code in view: its view gets a hole where each one draws.
    let sent = "[]";
    const cutHoles = () => {
      const area = host.getBoundingClientRect();
      const holes: [number, number, number, number, number][] = [];
      for (const el of document.querySelectorAll<HTMLElement>(".tip[data-on], .toasts > .toast")) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.right <= area.left || r.left >= area.right || r.bottom <= area.top || r.top >= area.bottom) continue;
        holes.push([r.left, r.top, r.width, r.height, parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0]);
      }
      const now = JSON.stringify(holes);
      if (now === sent) return;
      sent = now;
      void vscode.holes(holes).catch(() => {});
    };
    let frame = 0;
    const holesSoon = () => {
      if (!frame) frame = requestAnimationFrame(() => ((frame = 0), cutHoles()));
    };
    const ho = new MutationObserver(holesSoon);
    ho.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-on", "style"] });
    window.addEventListener("resize", holesSoon);
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
      ho.disconnect();
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", holesSoon);
      if (sent !== "[]") void vscode.holes([]).catch(() => {});
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
            <Spinner /> {t("Opening VS Code")}
          </div>
        </Show>
        <Show when={state() === "failed"}>
          <div class="code-empty">
            <div class="code-empty-title">{t("VS Code did not start")}</div>
            <p class="code-empty-body selectable">{why()}</p>
            <div class="vs-actions">
              <button type="button" class="btn" onClick={() => (setState("starting"), void place())}>
                {t("Try again")}
              </button>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}
