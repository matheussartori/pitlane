// Free terminals: your own shell in the repository or a lane's worktree, in tabs that split into
// panes. Each xterm lives for the life of its shell, outside any screen, so switching screens never
// loses a line. The layout (tabs, splits, ratios, places) is kept, and comes back after a restart
// with fresh shells in the same folders.
import { For, Match, Show, Switch, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { createStore, produce, unwrap } from "solid-js/store";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { files, onTerm, openExternal, terms, type Lane, type TermInfo } from "./api";
import { AgentList, AgentPane, agentGroups, agentSelected, selectAgent } from "./agents";
import { openInCode } from "./code";
import { t } from "./i18n";
import { Icon } from "./icons";
import { dismissable, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

// ---------------------------------------------------------------- the layout

type Dir = "row" | "col";
type Side = "left" | "right" | "top" | "bottom";
interface Pane {
  kind: "pane";
  term: number;
}
interface Split {
  kind: "split";
  dir: Dir;
  sizes: number[];
  children: Node[];
}
type Node = Pane | Split;
interface Tab {
  id: number;
  /** A name you gave it; otherwise the place of its focused pane. */
  name: string | null;
  root: Node;
  focus: number;
  zoom: boolean;
}

/** What a pane is doing, said in the tab list. Tabs never move because of it. */
type Status = "idle" | "busy" | "waiting" | "bell" | "exited";
interface Meta {
  cwd: string;
  label: string;
  /** What the program calls itself (OSC 0/2): a second line, never the tab's name. */
  title: string;
  status: Status;
  /** The line that explains a mark: a notification's text, or the last line on screen. */
  note: string;
  at: number;
  exit: number | null;
  /** The agent run whose Claude Code session this pane resumed. */
  run?: string;
}

const pane = (term: number): Pane => ({ kind: "pane", term });
const has = (n: Node, term: number): boolean => (n.kind === "pane" ? n.term === term : n.children.some((c) => has(c, term)));
export const panesOf = (n: Node): number[] => (n.kind === "pane" ? [n.term] : n.children.flatMap(panesOf));

/** Same-direction children fold into their parent; a split of one is that one. */
function norm(s: Split): Node {
  const children: Node[] = [];
  const sizes: number[] = [];
  s.children.forEach((c, i) => {
    if (c.kind === "split" && c.dir === s.dir) c.children.forEach((g, j) => (children.push(g), sizes.push(s.sizes[i]! * c.sizes[j]!)));
    else (children.push(c), sizes.push(s.sizes[i]!));
  });
  if (children.length === 1) return children[0]!;
  const total = sizes.reduce((a, b) => a + b, 0) || 1;
  return { kind: "split", dir: s.dir, children, sizes: sizes.map((x) => x / total) };
}

export function without(n: Node, term: number): Node | null {
  if (n.kind === "pane") return n.term === term ? null : n;
  if (!has(n, term)) return n;
  const children: Node[] = [];
  const sizes: number[] = [];
  n.children.forEach((c, i) => {
    const r = without(c, term);
    if (r) (children.push(r), sizes.push(n.sizes[i]!));
  });
  return children.length ? norm({ ...n, children, sizes }) : null;
}

/** `p` beside the pane `target`, on `side`; it takes half of that pane's room. */
export function insert(n: Node, target: number, p: Pane, side: Side): Node {
  const dir: Dir = side === "left" || side === "right" ? "row" : "col";
  const after = side === "right" || side === "bottom";
  if (n.kind === "pane") return n.term !== target ? n : { kind: "split", dir, sizes: [0.5, 0.5], children: after ? [n, p] : [p, n] };
  if (!has(n, target)) return n;
  const i = n.children.findIndex((c) => c.kind === "pane" && c.term === target);
  if (i >= 0 && n.dir === dir) {
    const children = [...n.children];
    const sizes = [...n.sizes];
    const half = sizes[i]! / 2;
    sizes[i] = half;
    children.splice(after ? i + 1 : i, 0, p);
    sizes.splice(after ? i + 1 : i, 0, half);
    return { ...n, children, sizes };
  }
  return norm({ ...n, children: n.children.map((c) => insert(c, target, p, side)) });
}

function swap(n: Node, a: number, b: number): Node {
  if (n.kind === "pane") return n.term === a ? pane(b) : n.term === b ? pane(a) : n;
  return { ...n, children: n.children.map((c) => swap(c, a, b)) };
}

function mapTerms(n: Node, f: (t: number) => number | null): Node | null {
  if (n.kind === "pane") {
    const t = f(n.term);
    return t == null ? null : pane(t);
  }
  const children: Node[] = [];
  const sizes: number[] = [];
  n.children.forEach((c, i) => {
    const r = mapTerms(c, f);
    if (r) (children.push(r), sizes.push(n.sizes[i]!));
  });
  return children.length ? norm({ ...n, children, sizes }) : null;
}

const equal = (n: Node): Node => (n.kind === "pane" ? n : { ...n, sizes: n.children.map(() => 1 / n.children.length), children: n.children.map(equal) });

/** The split along `dir` nearest above `term`, and the index of the child that holds it. */
function owner(n: Node, term: number, dir: Dir): { split: Split; i: number } | null {
  if (n.kind === "pane") return null;
  const i = n.children.findIndex((c) => has(c, term));
  if (i < 0) return null;
  return owner(n.children[i]!, term, dir) ?? (n.dir === dir ? { split: n, i } : null);
}

// ---------------------------------------------------------------- the store

interface Live {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  el: HTMLDivElement;
  opened: boolean;
  /** The current burst of output, and the last keystroke: typing is not work. */
  first: number;
  last: number;
  typed: number;
}

const live = new Map<number, Live>();
const [st, setSt] = createStore<{ tabs: Tab[]; active: number | null; side: "left" | "top"; font: number }>({ tabs: [], active: null, side: "left", font: 13 });
const [meta, setMeta] = createStore<Record<number, Meta>>({});
const [screenOn, setScreenOn] = createSignal(false);
let nextTab = 1;

const KEY = "pitlane.terminals.v1";
interface Saved {
  tabs: Tab[];
  active: number | null;
  side: "left" | "top";
  font: number;
  places: Record<number, { cwd: string; label: string; run?: string }>;
}

function save() {
  try {
    const tabs = unwrap(st.tabs);
    const places: Saved["places"] = {};
    for (const t of tabs) for (const id of panesOf(t.root)) if (meta[id]) places[id] = { cwd: meta[id]!.cwd, label: meta[id]!.label, run: meta[id]!.run };
    localStorage.setItem(KEY, JSON.stringify({ tabs, active: st.active, side: st.side, font: st.font, places } satisfies Saved));
  } catch {
    /* no storage: the layout lives for this window */
  }
}

function loadSaved(): Saved | null {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? "null") as Saved | null;
    return s && Array.isArray(s.tabs) ? s : null;
  } catch {
    return null;
  }
}

/** Tabs, for the top bar's count; and how many want you. */
export const terminalTabs = () => st.tabs;
export const terminalsWanting = () => Object.values(meta).filter((m) => m.status === "bell" || m.status === "waiting").length;

const activeTab = () => st.tabs.find((t) => t.id === st.active);
const tabOf = (term: number) => st.tabs.find((t) => has(t.root, term));
const inView = (term: number) => {
  const t = activeTab();
  return screenOn() && !!t && has(t.root, term) && (!t.zoom || t.focus === term);
};

function setTab(id: number, f: (t: Tab) => Partial<Tab>) {
  setSt("tabs", (t) => t.id === id, produce((t: Tab) => Object.assign(t, f(unwrap(t)))));
  save();
}

// ---------------------------------------------------------------- colours from the tokens

/** A CSS colour (oklch included) as #rrggbb, through a 1px canvas. */
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

const tok = (name: string) => hex(getComputedStyle(document.documentElement).getPropertyValue(name).trim());
const dark = () => getComputedStyle(document.documentElement).getPropertyValue("color-scheme").includes("dark");

function theme() {
  return {
    background: tok("--color-paper-sunk"),
    foreground: tok("--color-term-ink"),
    cursor: tok("--color-accent"),
    cursorAccent: tok("--color-paper-sunk"),
    selectionBackground: tok("--color-term-select"),
    scrollbarSliderBackground: dark() ? "#ffffff18" : "#00000018",
    scrollbarSliderHoverBackground: dark() ? "#ffffff30" : "#00000030",
    black: tok("--color-paper-3"),
    brightBlack: tok("--color-muted"),
    red: tok("--color-fail"),
    brightRed: tok("--color-fail"),
    green: tok("--color-done"),
    brightGreen: tok("--color-done"),
    yellow: tok("--code-string"),
    brightYellow: tok("--code-string"),
    blue: tok("--color-run"),
    brightBlue: tok("--code-fn"),
    magenta: tok("--code-keyword"),
    brightMagenta: tok("--code-keyword"),
    cyan: tok("--code-number"),
    brightCyan: tok("--code-number"),
    white: tok("--color-ink-2"),
    brightWhite: tok("--color-ink"),
  };
}

const findColours = () => ({
  matchBackground: dark() ? "#4a4030" : "#f3e2c0",
  activeMatchBackground: tok("--color-accent"),
  matchOverviewRuler: tok("--color-muted"),
  activeMatchColorOverviewRuler: tok("--color-accent"),
});

// ---------------------------------------------------------------- attention

/** The last line worth reading on screen: what a waiting program last said. */
function lastLine(t: Terminal): string {
  const b = t.buffer.active;
  const end = b.baseY + b.cursorY;
  for (let y = end; y >= Math.max(0, end - 40); y--) {
    const s = (b.getLine(y)?.translateToString(true) ?? "").replace(/^[\s│┃|╭╰─>❯$%#]+|[\s│┃|╮╯─]+$/g, "").trim();
    if (s.length > 3 && !/^[─━═╌\-–\s]+$/.test(s) && !/for shortcuts$/.test(s)) return s.slice(0, 140);
  }
  return "";
}

function placeName(term: number) {
  const tb = tabOf(term);
  return tb?.name || meta[term]?.label || t("A terminal");
}

async function systemNotify(title: string, body: string) {
  try {
    let ok = await isPermissionGranted();
    if (!ok) ok = (await requestPermission()) === "granted";
    if (ok) sendNotification({ title, body });
  } catch {
    /* the browser preview has no notifications */
  }
}

let notifyOn = true;
/** Settings: whether a terminal that wants you sends a system notification (the mark stays). */
export function setTerminalNotify(on: boolean) {
  notifyOn = on;
}

/** Settings: the tab list's side and the font size, read and changed. */
export const terminalPrefs = () => ({ side: st.side, font: st.font });
export function setTerminalSide(side: "left" | "top") {
  setSt("side", side);
  save();
}
export function setTerminalFont(size: number) {
  fontSize(size);
}
/** The colours again, after the app's theme changed. */
export function retheme() {
  const t = theme();
  for (const l of live.values()) l.term.options.theme = t;
}

/** One event, one signal: a mark on the tab, and a notification only when you cannot see it. */
function attend(term: number, status: "bell" | "waiting", note: string) {
  const m = meta[term];
  if (!m) return;
  if (inView(term) && document.hasFocus()) return;
  if ((m.status === "bell" || m.status === "waiting") && Date.now() - m.at < 3000) return;
  setMeta(term, { status, note, at: Date.now() });
  if (!document.hasFocus() && notifyOn) {
    void systemNotify(status === "bell" ? t("{name} needs you", { name: placeName(term) }) : t("{name} is waiting", { name: placeName(term) }), note || t("The program stopped and may be waiting for you."));
  }
}

/** Marks clear once you look at the tab. */
function seen(tab: Tab | undefined) {
  if (!tab) return;
  for (const id of panesOf(tab.root)) {
    const s = meta[id]?.status;
    if (s === "bell" || s === "waiting") setMeta(id, { status: "idle", note: "" });
  }
}

let ticker: ReturnType<typeof setInterval> | undefined;
/** Busy for a while, then quiet: the program finished or stopped to ask. */
function tick() {
  const now = performance.now();
  for (const [id, l] of live) {
    if (meta[id]?.status !== "busy" || now - l.last < 3500) continue;
    if (l.last - l.first > 8000 && !inView(id)) attend(id, "waiting", lastLine(l.term));
    else setMeta(id, { status: "idle" });
  }
}

function output(id: number) {
  const l = live.get(id);
  if (!l) return;
  const now = performance.now();
  if (now - l.typed < 1500) return; // the echo of your own typing
  if (now - l.last > 2000) l.first = now;
  l.last = now;
  const s = meta[id]?.status;
  if (s !== "busy" && s !== "exited" && now - l.first > 3000) setMeta(id, { status: "busy" });
}

// ---------------------------------------------------------------- paths that open in Code

/** The folders Code can open: the repository and every lane's worktree (set by the screen). */
let roots: string[] = [];
const fileSets = new Map<string, { at: number; files: Promise<Set<string>> }>();

function filesOf(root: string): Promise<Set<string>> {
  const c = fileSets.get(root);
  if (c && Date.now() - c.at < 20_000) return c.files;
  const f = files.list(root).then((l) => new Set(l), () => new Set<string>());
  fileSets.set(root, { at: Date.now(), files: f });
  return f;
}

function normalise(path: string): string {
  const out: string[] = [];
  for (const part of path.split("/")) {
    if (part === "..") out.pop();
    else if (part && part !== ".") out.push(part);
  }
  return "/" + out.join("/");
}

/** `src/a.ts:12:5`, `./x.py`, `/abs/path/y.rs:3`: a file of a known folder, with its line. */
const PATH = /(?:\.{0,2}\/)?(?:[\w.@~+-]+\/)*[\w.@+-]+\.[A-Za-z][A-Za-z0-9]{0,9}(?::(\d+)(?::(\d+))?|\((\d+),(\d+)\))?/g;

async function resolvePath(text: string, cwd: string): Promise<{ root: string; rel: string } | null> {
  const bare = text.replace(/(?::\d+(?::\d+)?|\(\d+,\d+\))$/, "");
  const sorted = [...roots].sort((a, b) => b.length - a.length);
  const tries = bare.startsWith("/") ? [normalise(bare)] : [normalise(`${cwd}/${bare}`), ...sorted.map((r) => normalise(`${r}/${bare}`))];
  for (const abs of tries) {
    const root = sorted.find((r) => abs.startsWith(r + "/"));
    if (!root) continue;
    const rel = abs.slice(root.length + 1);
    if ((await filesOf(root)).has(rel)) return { root, rel };
  }
  return null;
}

function linkFiles(term: Terminal, id: number) {
  term.registerLinkProvider({
    provideLinks(y, callback) {
      const line = term.buffer.active.getLine(y - 1)?.translateToString(true) ?? "";
      const cwd = meta[id]?.cwd ?? "";
      const found = [...line.matchAll(PATH)].slice(0, 24);
      if (!found.length || !roots.length) return callback(undefined);
      void Promise.all(
        found.map(async (m) => {
          const hit = await resolvePath(m[0], cwd);
          if (!hit) return null;
          const ln = Number(m[1] ?? m[3]) || undefined;
          const col = Number(m[2] ?? m[4]) || undefined;
          const start = (m.index ?? 0) + 1;
          return {
            range: { start: { x: start, y }, end: { x: start + m[0].length - 1, y } },
            text: m[0],
            decorations: { pointerCursor: true, underline: true },
            activate: (e: MouseEvent) => {
              if (e.metaKey) openInCode(hit.root, hit.rel, ln, col, true);
            },
            hover: (e: MouseEvent) => e.target instanceof HTMLElement && (e.target.title = "⌘-click to open in Code"),
            leave: (e: MouseEvent) => e.target instanceof HTMLElement && e.target.removeAttribute("title"),
          };
        }),
      ).then((links) => callback(links.filter((l) => l != null)));
    },
  });
}

// ---------------------------------------------------------------- terminals

function make(info: TermInfo, run?: string): Live {
  const term = new Terminal({
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace",
    fontSize: st.font,
    lineHeight: 1.35, // Warp-like air between lines
    cursorBlink: true,
    scrollback: 10000,
    macOptionIsMeta: true,
    allowProposedApi: true, // search decorations
    minimumContrastRatio: 4.5, // any program's colours stay readable on our ground
    theme: theme(),
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  // ⌘-click, as in iTerm2 and VS Code: a plain click selects text.
  term.loadAddon(new WebLinksAddon((e, uri) => e.metaKey && void openExternal(uri).catch(() => {})));
  linkFiles(term, info.id);
  const l: Live = { term, fit, search, el: document.createElement("div"), opened: false, first: 0, last: 0, typed: 0 };
  l.el.className = "term-surface";
  term.onData((d) => {
    l.typed = performance.now();
    void terms.write(info.id, d).catch(() => {});
  });
  term.onResize(({ cols, rows }) => void terms.resize(info.id, cols, rows).catch(() => {}));
  term.onTitleChange((title) => setMeta(info.id, "title", title.trim()));
  term.onBell(() => attend(info.id, "bell", lastLine(term)));
  // Notifications programs send: OSC 9 (iTerm2; 9;4 is a progress bar, not a message), 777 and 99.
  term.parser.registerOscHandler(9, (d) => (d.startsWith("4;") ? true : (attend(info.id, "bell", d), true)));
  term.parser.registerOscHandler(777, (d) => {
    const [kind, title, ...body] = d.split(";");
    if (kind === "notify") attend(info.id, "bell", body.join(";") || title || "");
    return true;
  });
  term.parser.registerOscHandler(99, (d) => (attend(info.id, "bell", d.slice(d.indexOf(";") + 1)), true));
  // The folder the shell is in (OSC 7), so a split opens where you are.
  term.parser.registerOscHandler(7, (d) => {
    try {
      const path = decodeURIComponent(new URL(d).pathname);
      if (path) setMeta(info.id, "cwd", path);
    } catch {
      /* not a file URL */
    }
    return true;
  });
  live.set(info.id, l);
  setMeta(info.id, { cwd: info.cwd, label: info.label, title: "", status: info.exited != null ? "exited" : "idle", note: "", at: 0, exit: info.exited, run });
  return l;
}

function dispose(id: number) {
  live.get(id)?.term.dispose();
  live.delete(id);
  setMeta(produce((m) => void delete m[id]));
}

let started = false;
/** Once per window: follow output, draw again any terminal that outlived a reload, and bring back
 *  the saved layout (fresh shells where the old ones are gone). */
export async function startTerminals() {
  if (started) return;
  started = true;
  await onTerm(
    (id, data) => {
      live.get(id)?.term.write(data);
      output(id);
    },
    (id, code) => {
      if (code === 0 && tabOf(id)) {
        // A clean exit closes its pane, as in any terminal.
        closePane(id);
        return;
      }
      live.get(id)?.term.write(`\r\n\x1b[2m[the shell exited with ${code}]\x1b[0m\r\n`);
      setMeta(id, { status: "exited", exit: code });
    },
  );
  const saved = loadSaved();
  if (saved) setSt({ side: saved.side === "top" ? "top" : "left", font: Math.min(20, Math.max(9, !saved.font || saved.font === 12 ? 13 : saved.font)) }); // 12 was the old default: it moves to 13 with the Warp-like pass
  let existing: TermInfo[] = [];
  try {
    existing = await terms.list();
    for (const t of existing) make(t).term.write(await terms.buffer(t.id));
  } catch {
    /* no backend: no terminals */
  }
  const byId = new Map(existing.map((t) => [t.id, t]));
  const placed = new Set<number>();
  const tabs: Tab[] = [];
  let reopened = 0;
  for (const t of saved?.tabs ?? []) {
    // A pane keeps its shell when the shell is still there in the same folder; otherwise a fresh one.
    const ids = panesOf(t.root);
    const map = new Map<number, number>();
    for (const old of ids) {
      const place = saved!.places[old];
      const same = byId.get(old);
      if (same && !placed.has(old) && (!place || same.cwd === place.cwd || same.label === place.label)) {
        map.set(old, old);
        placed.add(old);
      } else if (place && existing.length === 0 && reopened < 16) {
        try {
          // A resumed Claude Code session comes back resumed; a shell comes back a shell.
          const info = place.run ? await terms.resume(place.run, 100, 30).catch(() => terms.open(place.cwd, place.label, 100, 30)) : await terms.open(place.cwd, place.label, 100, 30);
          make(info, info.session ? place.run : undefined);
          map.set(old, info.id);
          placed.add(info.id);
          reopened++;
        } catch {
          /* the folder is gone: the pane goes too */
        }
      }
    }
    const root = mapTerms(t.root, (x) => map.get(x) ?? null);
    if (!root) continue;
    const focus = map.get(t.focus) ?? panesOf(root)[0]!;
    tabs.push({ id: nextTab++, name: t.name ?? null, root, focus, zoom: false });
    if (t.id === saved?.active) setSt("active", tabs[tabs.length - 1]!.id);
  }
  for (const t of existing) if (!placed.has(t.id)) tabs.push({ id: nextTab++, name: null, root: pane(t.id), focus: t.id, zoom: false });
  setSt("tabs", tabs);
  if (!activeTab() && tabs.length) setSt("active", tabs[tabs.length - 1]!.id);
  save();
  ticker ??= setInterval(tick, 1000);
  // The theme follows the system's light and dark.
  const retheme = () => {
    const t = theme();
    for (const l of live.values()) l.term.options.theme = t;
  };
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", retheme);
  // And the Appearance setting (light, dark or system), once its tokens are on the page.
  window.addEventListener("pitlane-theme", () => requestAnimationFrame(retheme));
}

/** Opens a terminal in `cwd`, in a new tab, and shows it. */
export async function openTerminal(cwd: string, label: string): Promise<boolean> {
  selectAgent(null);
  try {
    const info = await terms.open(cwd, label, 100, 30);
    make(info);
    const id = nextTab++;
    setSt(produce((s) => (s.tabs.push({ id, name: null, root: pane(info.id), focus: info.id, zoom: false }), (s.active = id))));
    save();
    return true;
  } catch (e) {
    toast({ tone: "fail", title: t("No terminal"), body: msg(e) });
    return false;
  }
}

/** Continue in Claude Code: the agent run's session, resumed by the official CLI in a new tab. */
export async function resumeInTerminal(run: string): Promise<boolean> {
  selectAgent(null);
  try {
    const info = await terms.resume(run, 100, 30);
    make(info, run);
    const id = nextTab++;
    setSt(produce((s) => (s.tabs.push({ id, name: null, root: pane(info.id), focus: info.id, zoom: false }), (s.active = id))));
    save();
    return true;
  } catch (e) {
    toast({ tone: "fail", title: t("Could not open it in Claude Code"), body: msg(e) });
    return false;
  }
}

async function split(dir: Dir) {
  const tb = activeTab();
  if (!tb) return;
  const from = meta[tb.focus];
  try {
    const info = await terms.open(from?.cwd ?? "", from?.label ?? "terminal", 100, 30);
    make(info);
    // As in Warp: the panes along the split share the room equally, however many there are.
    setTab(tb.id, (x) => {
      const root = insert(x.root, x.focus, pane(info.id), dir === "row" ? "right" : "bottom");
      const o = owner(root, info.id, dir);
      if (o) o.split.sizes = o.split.children.map(() => 1 / o.split.children.length);
      return { root, focus: info.id, zoom: false };
    });
  } catch (e) {
    toast({ tone: "fail", title: t("No terminal"), body: msg(e) });
  }
}

/** Closes a pane, and its tab with its last pane. `kill`: also end the shell (it may have ended). */
export function closePane(term: number, kill = true) {
  if (kill) void terms.close(term).catch(() => {});
  const t = tabOf(term);
  dispose(term);
  if (!t) return;
  const before = panesOf(t.root);
  const root = without(unwrap(t.root), term);
  if (!root) {
    const i = st.tabs.findIndex((x) => x.id === t.id);
    setSt(
      produce((s) => {
        s.tabs.splice(i, 1);
        if (s.active === t.id) s.active = s.tabs[Math.min(i, s.tabs.length - 1)]?.id ?? null;
      }),
    );
    save();
    return;
  }
  const left = panesOf(root);
  const next = t.focus === term ? left[Math.min(Math.max(0, before.indexOf(term) - 1), left.length - 1)]! : t.focus;
  setTab(t.id, () => ({ root, focus: next, zoom: false }));
}

export function closeTab(id: number) {
  const t = st.tabs.find((x) => x.id === id);
  if (t) for (const term of panesOf(t.root)) closePane(term);
}

function selectTab(id: number) {
  selectAgent(null);
  setSt("active", id);
  save();
}

function focusPane(term: number) {
  const t = tabOf(term);
  if (!t) return;
  if (st.active !== t.id) selectTab(t.id);
  setTab(t.id, () => ({ focus: term }));
  requestAnimationFrame(() => live.get(term)?.term.focus());
}

/** The pane nearest the focused one in a direction, by where they sit on screen. */
function neighbour(side: Side): number | null {
  const t = activeTab();
  if (!t) return null;
  const rect = (id: number) => document.querySelector(`[data-pane="${id}"]`)?.getBoundingClientRect();
  const a = rect(t.focus);
  if (!a) return null;
  let best: { id: number; d: number } | null = null;
  for (const id of panesOf(t.root)) {
    const b = rect(id);
    if (id === t.focus || !b) continue;
    const ok =
      side === "left" ? b.right <= a.left + 2 : side === "right" ? b.left >= a.right - 2 : side === "top" ? b.bottom <= a.top + 2 : b.top >= a.bottom - 2;
    if (!ok) continue;
    const d = Math.hypot(b.left + b.width / 2 - (a.left + a.width / 2), b.top + b.height / 2 - (a.top + a.height / 2));
    if (!best || d < best.d) best = { id, d };
  }
  return best?.id ?? null;
}

function resizeFocused(side: Side) {
  const t = activeTab();
  if (!t) return;
  const dir: Dir = side === "left" || side === "right" ? "row" : "col";
  const sign = side === "right" || side === "bottom" ? 1 : -1;
  setTab(t.id, (x) => {
    const root = structuredClone(x.root);
    const o = owner(root, x.focus, dir);
    if (!o) return {};
    const { split: s, i } = o;
    const [a, b] = i < s.sizes.length - 1 ? [i, i + 1] : [i - 1, i];
    const d = 0.05 * sign;
    const min = 0.08;
    const na = Math.min(Math.max(s.sizes[a]! + d, min), s.sizes[a]! + s.sizes[b]! - min);
    s.sizes[b] = s.sizes[a]! + s.sizes[b]! - na;
    s.sizes[a] = na;
    return { root };
  });
}

function fontSize(next: number) {
  setSt("font", Math.min(20, Math.max(9, next)));
  for (const l of live.values()) {
    l.term.options.fontSize = st.font;
    try {
      l.fit.fit();
    } catch {
      /* hidden */
    }
  }
  save();
}

/** The tab that most recently asked for you, if any. */
function jumpToWanting() {
  let best: { id: number; at: number } | null = null;
  for (const [id, m] of Object.entries(meta)) if ((m.status === "bell" || m.status === "waiting") && (!best || m.at > best.at)) best = { id: Number(id), at: m.at };
  if (best) focusPane(best.id);
}

// ---------------------------------------------------------------- dragging panes and tabs

type Drag = { kind: "pane"; term: number } | { kind: "tab"; id: number };
type Drop = { kind: "pane"; term: number; side: Side | "center" } | { kind: "tab"; id: number; after: boolean } | { kind: "list" };
const [drag, setDrag] = createSignal<{ what: Drag; over: Drop | null } | null>(null);

function dropAt(x: number, y: number, what: Drag): Drop | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  const p = el?.closest<HTMLElement>("[data-pane]");
  if (p && what.kind === "pane") {
    const term = Number(p.dataset.pane);
    if (term === what.term) return null;
    const r = p.getBoundingClientRect();
    const fx = (x - r.left) / r.width;
    const fy = (y - r.top) / r.height;
    if (fx > 0.3 && fx < 0.7 && fy > 0.3 && fy < 0.7) return { kind: "pane", term, side: "center" };
    const edges: [Side, number][] = [["left", fx], ["right", 1 - fx], ["top", fy], ["bottom", 1 - fy]];
    return { kind: "pane", term, side: edges.sort((a, b) => a[1] - b[1])[0]![0] };
  }
  const row = el?.closest<HTMLElement>("[data-tabrow]");
  if (row) {
    const id = Number(row.dataset.tabrow);
    const r = row.getBoundingClientRect();
    const after = st.side === "left" ? y > r.top + r.height / 2 : x > r.left + r.width / 2;
    if (what.kind === "tab" && id === what.id) return null;
    return { kind: "tab", id, after };
  }
  if (el?.closest("[data-tablist]") && what.kind === "pane") return { kind: "list" };
  return null;
}

function startDrag(e: PointerEvent, what: Drag) {
  if (e.button !== 0) return;
  const x0 = e.clientX;
  const y0 = e.clientY;
  let on = false;
  const move = (m: PointerEvent) => {
    if (!on && Math.hypot(m.clientX - x0, m.clientY - y0) < 5) return;
    on = true;
    setDrag({ what, over: dropAt(m.clientX, m.clientY, what) });
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    const d = drag();
    setDrag(null);
    if (on && d?.over) land(d.what, d.over);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

function land(what: Drag, to: Drop) {
  if (what.kind === "tab") {
    if (to.kind !== "tab") return;
    setSt(
      produce((s) => {
        const from = s.tabs.findIndex((t) => t.id === what.id);
        const [t] = s.tabs.splice(from, 1);
        const at = s.tabs.findIndex((x) => x.id === to.id) + (to.after ? 1 : 0);
        s.tabs.splice(at, 0, t!);
      }),
    );
    save();
    return;
  }
  const src = tabOf(what.term);
  if (!src) return;
  const term = what.term;
  if (to.kind === "pane" && to.side === "center") {
    setTab(src.id, (x) => ({ root: swap(x.root, term, to.term) }));
    return;
  }
  // Out of its tab first, then into its new place; a tab left empty goes.
  const rest = without(unwrap(src.root), term);
  if ((to.kind === "tab" && to.id === src.id) || (to.kind === "list" && !rest)) return;
  const destId = to.kind === "pane" ? tabOf(to.term)?.id : to.kind === "tab" ? to.id : undefined;
  setSt(
    produce((s) => {
      const from = s.tabs.find((t) => t.id === src.id)!;
      if (rest) Object.assign(from, { root: rest, focus: from.focus === term ? panesOf(rest)[0]! : from.focus, zoom: false });
      if (to.kind === "list") {
        const id = nextTab++;
        s.tabs.push({ id, name: null, root: pane(term), focus: term, zoom: false });
        s.active = id;
      } else {
        const d = s.tabs.find((t) => t.id === destId);
        if (!d) return;
        const base = unwrap(d.root);
        d.root = to.kind === "pane" ? insert(base, to.term, pane(term), to.side as Side) : insert(base, d.focus, pane(term), "right");
        d.focus = term;
        d.zoom = false;
        s.active = d.id;
      }
      if (!rest) s.tabs.splice(s.tabs.findIndex((t) => t.id === src.id), 1);
    }),
  );
  save();
  requestAnimationFrame(() => live.get(term)?.term.focus());
}

// ---------------------------------------------------------------- the screen

const place = (cwd: string) => cwd.replace(/^\/Users\/[^/]+/, "~");

function statusOf(tab: Tab): { s: Status; note: string } {
  const all = panesOf(tab.root).map((id) => meta[id]).filter(Boolean) as Meta[];
  for (const s of ["bell", "waiting", "busy", "exited"] as const) {
    const m = all.find((x) => x.status === s);
    if (m) return { s, note: m.note };
  }
  return { s: "idle", note: "" };
}

const STATUS: Record<Exclude<Status, "idle">, { icon: string; word: string }> = {
  bell: { icon: "needs", word: "Needs you" },
  waiting: { icon: "needs", word: "Waiting" },
  busy: { icon: "run", word: "Working" },
  exited: { icon: "fail", word: "Exited" },
};

function TabRow(props: { tab: Tab; index: number }) {
  const [editing, setEditing] = createSignal(false);
  const focusMeta = () => meta[props.tab.focus];
  const name = () => props.tab.name || focusMeta()?.label || t("terminal");
  const sub = () => {
    const m = focusMeta();
    if (!m) return "";
    return m.title && m.title !== name() ? m.title : place(m.cwd);
  };
  const status = () => statusOf(props.tab);
  const count = () => panesOf(props.tab.root).length;
  const over = () => {
    const d = drag()?.over;
    return d?.kind === "tab" && d.id === props.tab.id ? (drag()!.what.kind === "pane" ? "into" : d.after ? "after" : "before") : undefined;
  };
  const rename = (v: string) => {
    setEditing(false);
    setTab(props.tab.id, () => ({ name: v.trim() || null }));
  };
  return (
    <div
      class="tt"
      role="presentation"
      data-tabrow={props.tab.id}
      data-active={st.active === props.tab.id && !agentSelected() ? "" : undefined}
      data-drop={over()}
      data-dragging={drag()?.what.kind === "tab" && (drag()!.what as { id: number }).id === props.tab.id ? "" : undefined}
    >
      <Show
        when={!editing()}
        fallback={
          <input
            class="input input-mono tt-rename"
            value={props.tab.name ?? name()}
            aria-label={t("Tab name")}
            ref={(el) => requestAnimationFrame(() => (el.focus(), el.select()))}
            onKeyDown={(e) => {
              if (e.key === "Enter") rename(e.currentTarget.value);
              if (e.key === "Escape") setEditing(false);
              e.stopPropagation();
            }}
            onBlur={(e) => rename(e.currentTarget.value)}
          />
        }
      >
        <button
          type="button"
          role="tab"
          class="tt-main"
          aria-selected={st.active === props.tab.id}
          title={`${focusMeta()?.cwd ?? ""}${props.index < 9 ? `  ⌘${props.index + 1}` : ""}\n${t("Double-click to rename")}`}
          onPointerDown={(e) => startDrag(e, { kind: "tab", id: props.tab.id })}
          onClick={() => selectTab(props.tab.id)}
          onDblClick={() => setEditing(true)}
        >
          <Icon name={focusMeta()?.run ? "spark" : props.tab.name || !focusMeta()?.cwd.includes("/wt/") ? "term" : "branch"} small />
          <span class="tt-text">
            <span class="tt-name">{name()}</span>
            <Show when={st.side === "left" && sub()}>
              <span class="tt-sub">{sub()}</span>
            </Show>
          </span>
          <Show when={status().s !== "idle"}>
            <span class="tt-status" data-s={status().s} title={status().note || undefined}>
              <Icon name={STATUS[status().s as Exclude<Status, "idle">].icon} small />
              <Show when={st.side === "left"}>{t(STATUS[status().s as Exclude<Status, "idle">].word)}</Show>
            </span>
          </Show>
          <Show when={count() > 1 && status().s === "idle"}>
            <span class="tt-count" title={t("{n} panes", { n: count() })}>{count()}</span>
          </Show>
        </button>
      </Show>
      <button type="button" class="tt-close" aria-label={t("Close {name}", { name: name() })} title={t("Close tab")} onClick={() => closeTab(props.tab.id)}>
        <Icon name="close" small />
      </button>
    </div>
  );
}

function PaneView(props: { term: number; tab: Tab; alone: boolean }) {
  let host!: HTMLDivElement;
  const m = () => meta[props.term];
  const focused = () => props.tab.focus === props.term;
  const over = () => {
    const d = drag()?.over;
    return d?.kind === "pane" && d.term === props.term ? d.side : undefined;
  };
  const fit = () => {
    const l = live.get(props.term);
    try {
      l?.fit.fit();
    } catch {
      /* not laid out yet */
    }
  };
  onMount(() => {
    const l = live.get(props.term);
    if (!l) return;
    host.appendChild(l.el);
    if (!l.opened) {
      l.term.open(l.el);
      l.opened = true;
      const id = props.term;
      l.term.textarea?.addEventListener("focus", () => {
        const t = tabOf(id);
        if (t && t.focus !== id) setTab(t.id, () => ({ focus: id }));
      });
    }
    requestAnimationFrame(() => {
      fit();
      l.term.refresh(0, l.term.rows - 1);
      if (focused()) l.term.focus();
    });
    const ro = new ResizeObserver(() => fit());
    ro.observe(host);
    onCleanup(() => ro.disconnect());
  });
  return (
    <div class="pane" data-pane={props.term} data-focus={focused() ? "" : undefined} data-drop={over()} onPointerDown={() => !focused() && focusPane(props.term)}>
      <Show when={!props.alone || props.tab.zoom}>
        <div class="pane-bar" onPointerDown={(e) => startDrag(e, { kind: "pane", term: props.term })} title={t("Drag onto another pane to move it")}>
          <span class="pane-name" title={m()?.cwd}>{m()?.label}</span>
          <span class="pane-place">{m()?.title}</span>
          <Show when={m() && m()!.status !== "idle"}>
            <span class="tt-status" data-s={m()!.status}>
              <Icon name={STATUS[m()!.status as Exclude<Status, "idle">].icon} small />
            </span>
          </Show>
          <button
            type="button"
            class="pane-btn"
            aria-label={props.tab.zoom ? t("Show all panes") : t("Zoom this pane")}
            title={props.tab.zoom ? t("Show all panes (⌘⇧↩)") : t("Zoom (⌘⇧↩)")}
            aria-pressed={props.tab.zoom}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => setTab(props.tab.id, (x) => ({ zoom: !x.zoom, focus: props.term }))}
          >
            <Icon name={props.tab.zoom ? "unzoom" : "zoom"} small />
          </button>
          <button type="button" class="pane-btn" aria-label={t("Close pane")} title={t("Close pane (⌘W)")} onPointerDown={(e) => e.stopPropagation()} onClick={() => closePane(props.term)}>
            <Icon name="close" small />
          </button>
        </div>
      </Show>
      <div class="pane-body">
        <div class="pane-host" ref={host} />
      </div>
      <Show when={over()}>
        <div class="pane-drop" data-side={over()} aria-hidden="true" />
      </Show>
    </div>
  );
}

function SplitView(props: { node: Split; tab: Tab; path: number[] }) {
  let el!: HTMLDivElement;
  const resize = (e: PointerEvent, i: number) => {
    if (e.button !== 0) return;
    // No preventDefault here: it would swallow the double-click that shares the room equally.
    const cells = [...el.children].filter((c) => c.classList.contains("split-cell")) as HTMLElement[];
    const a = cells[i - 1]!;
    const b = cells[i]!;
    const row = props.node.dir === "row";
    const total = row ? el.getBoundingClientRect().width : el.getBoundingClientRect().height;
    const sa = props.node.sizes[i - 1]!;
    const sb = props.node.sizes[i]!;
    const start = row ? e.clientX : e.clientY;
    const min = Math.min(0.45 * (sa + sb), 120 / total);
    let na = sa;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    document.body.dataset.resizing = row ? "row" : "col";
    const move = (m: PointerEvent) => {
      na = Math.min(Math.max(sa + ((row ? m.clientX : m.clientY) - start) / total, min), sa + sb - min);
      a.style.flexGrow = String(na);
      b.style.flexGrow = String(sa + sb - na);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      delete document.body.dataset.resizing;
      setSizes(props.tab.id, props.path, (s) => ((s[i - 1] = na), (s[i] = sa + sb - na)));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div class="split" data-dir={props.node.dir} ref={el}>
      <For each={props.node.children}>
        {(child, i) => (
          <>
            <Show when={i() > 0}>
              <div
                class="split-handle"
                role="separator"
                aria-orientation={props.node.dir === "row" ? "vertical" : "horizontal"}
                title={t("Drag to resize, double-click to share the room equally")}
                onPointerDown={(e) => resize(e, i())}
                onDblClick={() => setSizes(props.tab.id, props.path, (s) => s.forEach((_, j) => (s[j] = 1 / s.length)))}
              />
            </Show>
            <div class="split-cell" style={{ "flex-grow": props.node.sizes[i()] }}>
              <NodeView node={child} tab={props.tab} path={[...props.path, i()]} />
            </div>
          </>
        )}
      </For>
    </div>
  );
}

/** Sizes change in place, so no pane is drawn again while you resize. */
function setSizes(tab: number, path: number[], f: (s: number[]) => void) {
  setSt(
    "tabs",
    (t) => t.id === tab,
    produce((t: Tab) => {
      let n: Node = t.root;
      for (const i of path) n = (n as Split).children[i]!;
      if (n.kind === "split") f(n.sizes);
    }),
  );
  save();
}

function NodeView(props: { node: Node; tab: Tab; path: number[] }) {
  return (
    <Switch>
      <Match when={props.node.kind === "pane" && (props.node as Pane)}>{(p) => <PaneView term={p().term} tab={props.tab} alone={props.path.length === 0} />}</Match>
      <Match when={props.node.kind === "split" && (props.node as Split)}>{(s) => <SplitView node={s()} tab={props.tab} path={props.path} />}</Match>
    </Switch>
  );
}

function FindBar(props: { term: number; onClose: () => void }) {
  const [q, setQ] = createSignal("");
  const [res, setRes] = createSignal<{ i: number; n: number } | null>(null);
  const l = () => live.get(props.term);
  onMount(() => {
    const sub = l()?.search.onDidChangeResults((r) => setRes(r.resultCount ? { i: r.resultIndex + 1, n: r.resultCount } : { i: 0, n: 0 }));
    onCleanup(() => {
      sub?.dispose();
      l()?.search.clearDecorations();
    });
  });
  const go = (back = false) => {
    const opts = { incremental: !back, decorations: findColours() };
    if (!q()) return setRes(null);
    if (back) l()?.search.findPrevious(q(), opts);
    else l()?.search.findNext(q(), opts);
  };
  return (
    <div class="find" role="search">
      <Icon name="search" small />
      <input
        class="find-input"
        placeholder={t("Find in this pane")}
        aria-label={t("Find in this pane")}
        value={q()}
        ref={(el) => requestAnimationFrame(() => el.focus())}
        onInput={(e) => (setQ(e.currentTarget.value), go())}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.preventDefault(), go(e.shiftKey));
          if (e.key === "Escape") props.onClose();
          e.stopPropagation();
        }}
      />
      <span class="find-count" aria-live="polite">
        {res() ? (res()!.n ? t("{i} of {n}", { i: res()!.i, n: res()!.n }) : t("No match")) : ""}
      </span>
      <button type="button" class="pane-btn" aria-label={t("Previous match")} title={t("Previous (⇧↩)")} onClick={() => go(true)}>
        <Icon name="up" small />
      </button>
      <button type="button" class="pane-btn" aria-label={t("Next match")} title={t("Next (↩)")} onClick={() => go()}>
        <Icon name="chev" small />
      </button>
      <button type="button" class="pane-btn" aria-label={t("Close find")} title={t("Close (Esc)")} onClick={props.onClose}>
        <Icon name="close" small />
      </button>
    </div>
  );
}

export function TerminalScreen(props: { repo: string; repoName: string; lanes: Lane[]; onOpenRun: (id: string) => void }) {
  const [menu, setMenu] = createSignal(false);
  let menuBox: HTMLDivElement | undefined;
  dismissable(() => menuBox, menu, () => setMenu(false));
  const [finding, setFinding] = createSignal(false);
  const places = () => [
    { cwd: props.repo, label: props.repoName, sub: "Repository", icon: "folder" },
    ...props.lanes
      .filter((l) => l.worktree && !l.worktree.is_main)
      .map((l) => ({ cwd: l.worktree!.path, label: l.branch, sub: "Lane worktree", icon: "branch" })),
  ];
  const tab = activeTab;
  createEffect(() => (roots = places().map((p) => p.cwd)));
  const open = async (cwd: string, label: string) => {
    setMenu(false);
    await openTerminal(cwd, label);
  };

  onMount(() => {
    setScreenOn(true);
    seen(activeTab());
    const onFocus = () => seen(activeTab());
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      setScreenOn(false);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey, true);
    });
  });

  // Looking at a tab clears its marks.
  createEffect(on([() => st.active, screenOn], () => screenOn() && seen(activeTab())));

  // The Ghostty and iTerm2 keys. Taken before the terminal sees them; Ctrl keys stay the shell's.
  function onKey(e: KeyboardEvent) {
    if (!e.metaKey || e.ctrlKey && !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Equal"].includes(e.code)) return;
    // An agent on screen: only the keys that leave it for a shell apply.
    if (agentSelected() && !(e.key.toLowerCase() === "t" && !e.shiftKey) && !/^Digit[1-9]$/.test(e.code)) return;
    if (e.target instanceof HTMLInputElement && e.target.closest(".find")) {
      if (e.key.toLowerCase() !== "f") return;
    }
    const t = tab();
    const k = e.key.toLowerCase();
    const take = () => (e.preventDefault(), e.stopPropagation());
    const arrows: Record<string, Side> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "top", ArrowDown: "bottom" };
    if (k === "t" && !e.shiftKey) {
      take();
      const m = t ? meta[t.focus] : undefined;
      void openTerminal(m?.cwd ?? props.repo, m?.label ?? props.repoName);
    } else if (k === "d" && t) {
      take();
      void split(e.shiftKey ? "col" : "row");
    } else if (k === "w" && t) {
      take();
      closePane(t.focus);
    } else if (e.ctrlKey && arrows[e.code] && t) {
      take();
      resizeFocused(arrows[e.code]!);
    } else if (e.ctrlKey && e.code === "Equal" && t) {
      take();
      setTab(t.id, (x) => ({ root: equal(x.root) }));
    } else if (e.altKey && arrows[e.code] && t) {
      take();
      const n = neighbour(arrows[e.code]!);
      if (n != null) focusPane(n);
    } else if ((e.code === "BracketLeft" || e.code === "BracketRight") && t) {
      take();
      const step = e.code === "BracketRight" ? 1 : -1;
      if (e.shiftKey) {
        const i = st.tabs.findIndex((x) => x.id === t.id);
        selectTab(st.tabs[(i + step + st.tabs.length) % st.tabs.length]!.id);
      } else {
        const all = panesOf(t.root);
        focusPane(all[(all.indexOf(t.focus) + step + all.length) % all.length]!);
      }
    } else if (/^Digit[1-9]$/.test(e.code) && !e.shiftKey && !e.altKey) {
      take();
      const n = Number(e.code.slice(5));
      const target = n === 9 ? st.tabs[st.tabs.length - 1] : st.tabs[n - 1];
      if (target) selectTab(target.id);
    } else if (k === "enter" && e.shiftKey && t) {
      take();
      setTab(t.id, (x) => ({ zoom: !x.zoom }));
    } else if (k === "k" && !e.shiftKey && t) {
      take();
      live.get(t.focus)?.term.clear();
    } else if (k === "f" && t) {
      take();
      setFinding(!finding());
      if (finding() === false) live.get(t.focus)?.term.focus();
    } else if (k === "u" && e.shiftKey) {
      take();
      jumpToWanting();
    } else if ((k === "=" || k === "+") && !e.ctrlKey) {
      take();
      fontSize(st.font + 1);
    } else if (k === "-") {
      take();
      fontSize(st.font - 1);
    } else if (k === "0") {
      take();
      fontSize(12);
    }
  }

  const closeFind = () => {
    setFinding(false);
    const t = tab();
    if (t) live.get(t.focus)?.term.focus();
  };

  return (
    <div class="terms" data-side={st.side} data-empty={st.tabs.length === 0 && agentGroups().length === 0 ? "" : undefined}>
      <Show when={st.tabs.length > 0 || agentGroups().length > 0}>
        <div class="terms-tabs" data-agents={agentGroups().length > 0 ? "" : undefined}>
          <div class="ag-side">
            <AgentList compact={st.side === "top"} />
          </div>
          <div class="terms-head">
            <Show when={st.side === "left"}>
              <span class="terms-title">{t("Terminals")}</span>
            </Show>
            <div class="anchor" ref={menuBox}>
              <button type="button" class="btn btn-ghost btn-icon" aria-label={t("New terminal")} aria-expanded={menu()} title={t("New terminal (⌘T)")} onClick={() => setMenu(!menu())}>
                <Icon name="plus" small />
              </button>
              <Show when={menu()}>
                <div class="pop term-menu" role="menu">
                  <For each={places()}>
                    {(p) => (
                      <button type="button" class="menu-item" role="menuitem" onClick={() => void open(p.cwd, p.label)}>
                        <Icon name={p.icon} small />
                        <span class="repo-entry">
                          <span class="repo-name">{p.label}</span>
                          <span class="repo-path">{t(p.sub)}</span>
                        </span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </div>
            <button
              type="button"
              class="btn btn-ghost btn-icon"
              aria-label={st.side === "left" ? t("Put the tabs on top") : t("Put the tabs at the side")}
              title={st.side === "left" ? t("Tabs on top") : t("Tabs at the side")}
              onClick={() => (setSt("side", st.side === "left" ? "top" : "left"), save())}
            >
              <Icon name={st.side === "left" ? "tabs-top" : "tabs-side"} small />
            </button>
          </div>
          <div class="terms-list" role="tablist" aria-label={t("Terminals")} aria-orientation={st.side === "left" ? "vertical" : "horizontal"} data-tablist data-drop={drag()?.over?.kind === "list" ? "" : undefined}>
            <For each={st.tabs}>{(t, i) => <TabRow tab={t} index={i()} />}</For>
          </div>
        </div>
      </Show>
      <div class="terms-stage">
        <Show when={!agentSelected()} fallback={<AgentPane onOpenRun={props.onOpenRun} onContinue={resumeInTerminal} />}>
        <Show
          when={tab()}
          keyed
          fallback={
            <div class="terms-empty">
              <div class="terms-empty-title">{t("No terminal open")}</div>
              <p class="terms-empty-body">{t("Your own shell, in the repository or in a lane’s worktree.")}</p>
              <div class="terms-places">
                <For each={places()}>
                  {(p) => (
                    <button type="button" class="terms-place" onClick={() => void open(p.cwd, p.label)}>
                      <Icon name={p.icon} small />
                      <span class="terms-place-name">{p.label}</span>
                      <span class="terms-place-sub">{t(p.sub)}</span>
                    </button>
                  )}
                </For>
              </div>
              <p class="terms-empty-keys">
                <span class="kbd">⌘T</span> {t("new tab")} <span class="kbd">⌘D</span> {t("split")} <span class="kbd">⌘⇧D</span> {t("split down")}
              </p>
            </div>
          }
        >
          {(t) => (
            <div class="terms-panes" data-dragging={drag() ? "" : undefined} data-bars={t.zoom || t.root.kind === "split" ? "" : undefined}>
              <Show when={t.zoom} fallback={<NodeView node={t.root} tab={t} path={[]} />}>
                <PaneView term={t.focus} tab={t} alone />
              </Show>
              <Show when={finding()}>
                <FindBar term={t.focus} onClose={closeFind} />
              </Show>
            </div>
          )}
        </Show>
        </Show>
      </div>
    </div>
  );
}
