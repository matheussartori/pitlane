// Free terminals: your own shell in the repository or a lane's worktree, in tabs that split into
// panes. Each xterm lives for the life of its shell, outside any screen, so switching screens never
// loses a line. The layout (tabs, splits, ratios, places) is kept, and comes back after a restart
// with fresh shells in the same folders.
import { createSignal } from "solid-js";
import { createStore, produce, unwrap } from "solid-js/store";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import "@xterm/xterm/css/xterm.css";
import { Blocks, onBlockCard } from "../blocks";
import { loadCards, repo, yard } from "../store/repo";
import { systemNotify } from "../notify";
import { api, files, onTerm, openExternal, terms, type TermInfo } from "../api";
import { selectAgent } from "../store/agents";
import { openInCode } from "../store/code";
import { t } from "../i18n";
import { toast } from "../ui";
import { size } from "../layout";
import { msg, cssHex } from "../format";

/** The sidebar's width, and how much of it the agents take above the terminals (most of it). */
export const SIDE_W = size("terms.side", { def: 260, min: 180, max: () => Math.round(window.innerWidth * 0.45) });
export const AGENTS_H = size("terms.agents", {
  def: () => Math.round((window.innerHeight - 38) * 0.68),
  min: 80,
  max: () => window.innerHeight - 38 - 120,
});


// ---------------------------------------------------------------- the layout

type Dir = "row" | "col";
export type Side = "left" | "right" | "top" | "bottom";
export interface Pane {
  kind: "pane";
  term: number;
}
export interface Split {
  kind: "split";
  dir: Dir;
  sizes: number[];
  children: Node[];
}
export type Node = Pane | Split;
export interface Tab {
  id: number;
  /** A name you gave it; otherwise the place of its focused pane. */
  name: string | null;
  root: Node;
  focus: number;
  zoom: boolean;
}

/** What a pane is doing, said in the tab list. Tabs never move because of it. */
export type Status = "idle" | "busy" | "waiting" | "bell" | "exited";
export interface Meta {
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

export const pane = (term: number): Pane => ({ kind: "pane", term });
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

export function swap(n: Node, a: number, b: number): Node {
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

export const equal = (n: Node): Node => (n.kind === "pane" ? n : { ...n, sizes: n.children.map(() => 1 / n.children.length), children: n.children.map(equal) });

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
  blocks: Blocks;
  /** Drawn on the GPU, as Warp does: crisp glyphs on the cell grid. Gone after a lost context. */
  gl?: WebglAddon;
  el: HTMLDivElement;
  opened: boolean;
  /** The current burst of output, and the last keystroke: typing is not work. */
  first: number;
  last: number;
  typed: number;
  /** An agent here reports its own turns (the warp://cli-agent protocol): no guessing from quiet. */
  agent?: boolean;
}

export const live = new Map<number, Live>();

// A failed command becomes a card: what ran, where, how it ended and the end of its output.
onBlockCard((term, cmd, output, exit) => {
  const path = repo();
  if (!path) return;
  const id = [...live.entries()].find(([, l]) => l.term === term)?.[0];
  const cwd = (id != null && meta[id]?.cwd) || path;
  const lane = yard()?.lanes.find((l) => l.worktree && (cwd === l.worktree.path || cwd.startsWith(`${l.worktree.path}/`)));
  const lines = output.replace(/\s+$/, "").split("\n");
  const tail = lines.slice(-80).join("\n");
  const short = cmd.length > 60 ? `${cmd.slice(0, 59)}…` : cmd;
  const title = t("Fix: {cmd} fails", { cmd: short || t("a command") });
  const where = lane ? t("in the lane {branch}", { branch: lane.branch }) : t("in the repository");
  const body = [
    t("`{cmd}` exited with {code} {where} ({cwd}).", { cmd, code: exit, where, cwd }),
    "",
    t("Make it pass, and say what was wrong."),
    "",
    lines.length > 80 ? t("The last 80 of {n} lines of its output:", { n: lines.length }) : t("Its output:"),
    "",
    "```",
    tail,
    "```",
  ].join("\n");
  void api
    .cardAdd(path, title, body)
    .then((c) => {
      void loadCards();
      toast({ tone: "done", title: t("#{n} {title} is on the Board", { n: c.id, title: c.title }), action: { label: t("Open"), run: () => window.dispatchEvent(new CustomEvent("pitlane-open-card", { detail: c.id })) } });
    })
    .catch((e) => toast({ tone: "fail", title: t("The card was not made"), body: String(e) }));
});
/** Faces a terminal can wear. Hack is Warp's default face, bundled so every Mac draws the same. */
export const TERMINAL_FACES = [
  { v: "hack", label: "Hack", css: 'Hack, Menlo, monospace' },
  { v: "menlo", label: "Menlo", css: 'Menlo, "SF Mono", monospace' },
  { v: "sf", label: "SF Mono", css: 'ui-monospace, "SF Mono", Menlo, monospace' },
  { v: "geist", label: "Geist Mono", css: '"Geist Mono Variable", Menlo, monospace' },
] as const;
type Face = (typeof TERMINAL_FACES)[number]["v"];
const faceCss = (f: Face) => (TERMINAL_FACES.find((x) => x.v === f) ?? TERMINAL_FACES[0]).css;
/** How glyphs reach the screen. Smooth: WebKit draws the text as Warp does (grayscale, no stem
 * thickening), soft to read for hours. Sharp: the GPU atlas, whose canvas glyphs macOS thickens. */
type Render = "smooth" | "gpu";
/** The terminal's text size until you change it: Warp's. */
export const DEFAULT_FONT = 13;

export const [st, setSt] = createStore<{ tabs: Tab[]; active: number | null; side: "left" | "top"; fold: boolean; font: number; face: Face; render: Render }>({ tabs: [], active: null, side: "left", fold: false, font: DEFAULT_FONT, face: "hack", render: "smooth" });
const [meta, setMeta] = createStore<Record<number, Meta>>({});
export { meta };
export const [screenOn, setScreenOn] = createSignal(false);
let nextTab = 1;
/** A new tab's id. */
export const nextTabId = () => nextTab++;

const KEY = "pitlane.terminals.v1";
interface Saved {
  tabs: Tab[];
  active: number | null;
  side: "left" | "top";
  fold?: boolean; // the side list folded to a rail of marks
  font: number;
  font13?: boolean; // saved since 13px became the size (Warp's): before it, 14 was only the default
  face?: Face;
  face2?: Face; // the face chosen since Hack became the default (an old "menlo" was only the default)
  render?: Render;
  places: Record<number, { cwd: string; label: string; run?: string }>;
}

export function save() {
  try {
    const tabs = unwrap(st.tabs);
    const places: Saved["places"] = {};
    for (const t of tabs) for (const id of panesOf(t.root)) if (meta[id]) places[id] = { cwd: meta[id]!.cwd, label: meta[id]!.label, run: meta[id]!.run };
    localStorage.setItem(KEY, JSON.stringify({ tabs, active: st.active, side: st.side, fold: st.fold, font: st.font, font13: true, face: st.face, face2: st.face, render: st.render, places } satisfies Saved));
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

/** How many terminals want you, for the top bar's count. */
export const terminalsWanting = () => Object.values(meta).filter((m) => m.status === "bell" || m.status === "waiting").length;

export const activeTab = () => st.tabs.find((t) => t.id === st.active);
export const tabOf = (term: number) => st.tabs.find((t) => has(t.root, term));
const inView = (term: number) => {
  const t = activeTab();
  return screenOn() && !!t && has(t.root, term) && (!t.zoom || t.focus === term);
};

export function setTab(id: number, f: (t: Tab) => Partial<Tab>) {
  setSt("tabs", (t) => t.id === id, produce((t: Tab) => Object.assign(t, f(unwrap(t)))));
  save();
}

// ---------------------------------------------------------------- colours from the tokens


const tok = (name: string) => cssHex(getComputedStyle(document.documentElement).getPropertyValue(name).trim());
const dark = () => getComputedStyle(document.documentElement).getPropertyValue("color-scheme").includes("dark");

function theme() {
  return {
    background: tok("--color-term-paper"),
    foreground: tok("--color-term-ink"),
    cursor: tok("--color-accent"),
    cursorAccent: tok("--color-term-paper"),
    selectionBackground: tok("--color-term-select"),
    scrollbarSliderBackground: dark() ? "#ffffff18" : "#00000018",
    scrollbarSliderHoverBackground: dark() ? "#ffffff30" : "#00000030",
    black: tok("--ansi-black"),
    brightBlack: tok("--ansi-bright-black"),
    red: tok("--ansi-red"),
    brightRed: tok("--ansi-bright-red"),
    green: tok("--ansi-green"),
    brightGreen: tok("--ansi-bright-green"),
    yellow: tok("--ansi-yellow"),
    brightYellow: tok("--ansi-bright-yellow"),
    blue: tok("--ansi-blue"),
    brightBlue: tok("--ansi-bright-blue"),
    magenta: tok("--ansi-magenta"),
    brightMagenta: tok("--ansi-bright-magenta"),
    cyan: tok("--ansi-cyan"),
    brightCyan: tok("--ansi-bright-cyan"),
    white: tok("--ansi-white"),
    brightWhite: tok("--ansi-bright-white"),
  };
}

export const findColours = () => ({
  matchBackground: dark() ? "#4a4030" : "#f3e2c0",
  activeMatchBackground: tok("--color-accent"),
  matchOverviewRuler: tok("--color-muted"),
  activeMatchColorOverviewRuler: tok("--color-accent"),
});

// ---------------------------------------------------------------- keys

/** What Warp sends for the editing keys of a Mac: ⌘ moves or deletes to the line's ends,
 *  ⌥ by words, ⇧↵ a new line (Claude Code reads ESC-CR as one). Anything else stays xterm's. */
function editKey(e: KeyboardEvent): string | null {
  const only = (m: boolean) => m && !(e.metaKey && e.altKey);
  if (only(e.metaKey) && !e.shiftKey) {
    if (e.key === "ArrowLeft") return "\x01"; // start of line
    if (e.key === "ArrowRight") return "\x05"; // end of line
    if (e.key === "Backspace") return "\x15"; // delete to start
    if (e.key === "Delete") return "\x0b"; // delete to end
  }
  if (only(e.altKey) && !e.shiftKey) {
    if (e.key === "ArrowLeft" || e.code === "ArrowLeft") return "\x1bb"; // word back
    if (e.key === "ArrowRight" || e.code === "ArrowRight") return "\x1bf"; // word forward
    if (e.code === "Backspace") return "\x17"; // delete word back
    if (e.code === "Delete") return "\x1bd"; // delete word forward
  }
  if (e.shiftKey && !e.metaKey && !e.altKey && e.key === "Enter") return "\x1b\r";
  return null;
}

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

let notifyOn = true;
/** Settings: whether a terminal that wants you sends a system notification (the mark stays). */
export function setTerminalNotify(on: boolean) {
  notifyOn = on;
}

/** Settings: the tab list's side and the font size, read and changed. */
export const terminalPrefs = () => ({ side: st.side, font: st.font, face: st.face, render: st.render });
export function setTerminalRender(render: Render) {
  setSt("render", render);
  for (const l of live.values()) {
    if (!l.opened) continue;
    if (render === "gpu" && !l.gl) gpu(l);
    if (render === "smooth" && l.gl) {
      l.gl.dispose();
      l.gl = undefined;
    }
    l.term.refresh(0, l.term.rows - 1);
  }
  save();
}
export function setTerminalFace(face: Face) {
  setSt("face", face);
  void document.fonts.load(`${st.font}px ${faceCss(face)}`).finally(() => {
    for (const l of live.values()) {
      l.term.options.fontFamily = faceCss(face);
      l.gl?.clearTextureAtlas();
      try {
        l.fit.fit();
      } catch {
        /* hidden */
      }
    }
  });
  save();
}
/** Folds the side list to a rail of marks, or opens it again (⌘\ on the Terminal screen). */
export function toggleFold(fold = !st.fold) {
  if (st.side === "top" && fold) setSt("side", "left");
  setSt("fold", fold);
  save();
}
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

/** One event, one signal: a mark on the tab, and a notification only when you cannot see it.
 *  A guess (output that went quiet) only marks the tab: macOS hears only what a program said. */
function attend(term: number, status: "bell" | "waiting", note: string, o: { title?: string; guess?: boolean } = {}) {
  const m = meta[term];
  if (!m) return;
  if (inView(term) && document.hasFocus()) return;
  if ((m.status === "bell" || m.status === "waiting") && Date.now() - m.at < 3000) return;
  setMeta(term, { status, note, at: Date.now() });
  if (!document.hasFocus() && notifyOn && !o.guess) {
    void systemNotify(o.title ?? (status === "bell" ? t("{name} needs you", { name: placeName(term) }) : t("{name} is waiting", { name: placeName(term) })), note || t("The program stopped and may be waiting for you."));
  }
}

/** One line worth reading from an agent's text: its first sentence, no markdown. */
function gist(s: unknown): string {
  if (typeof s !== "string") return "";
  const line = s.split("\n").map((x) => x.replace(/^[#>*\-\s`]+|[*`]+/g, "").trim()).find((x) => x.length > 2) ?? "";
  return line.length > 160 ? line.slice(0, 157) + "…" : line;
}

/** Claude Code's Warp plugin reports every turn (OSC 777, title warp://cli-agent, a JSON body).
 *  Most are progress: they set the tab's state. Only a decision, an error or the end of a turn
 *  is worth a notification. */
function agentEvent(id: number, body: string) {
  let e: Record<string, unknown>;
  try {
    e = JSON.parse(body);
  } catch {
    return;
  }
  const l = live.get(id);
  if (l) l.agent = true;
  const who = typeof e.project === "string" && e.project ? e.project : placeName(id);
  switch (e.event) {
    case "prompt_submit":
    case "tool_complete":
      if (meta[id]?.status !== "busy") setMeta(id, { status: "busy" });
      return;
    case "permission_request":
      attend(id, "bell", gist(e.summary) || t("Claude asks to use {tool}", { tool: String(e.tool_name ?? "a tool") }), { title: t("{name}: Claude needs your approval", { name: who }) });
      return;
    case "agent_needs_input":
    case "question":
      attend(id, "bell", gist(e.summary) || t("Claude is asking you something."), { title: t("{name}: Claude has a question", { name: who }) });
      return;
    case "stop":
      setMeta(id, { status: "idle" });
      attend(id, "waiting", gist(e.response) || t("Claude finished its turn."), { title: t("{name}: Claude finished", { name: who }) });
      return;
    case "stop_failure":
      setMeta(id, { status: "idle" });
      attend(id, "bell", gist(e.response) || String(e.error_type ?? t("Claude stopped with an error.")), { title: t("{name}: Claude stopped with an error", { name: who }) });
      return;
    default:
      // session_start, idle_prompt (a reminder of a stop already told), anything newer: quiet.
      return;
  }
}

/** Marks clear once you look at the tab. */
export function seen(tab: Tab | undefined) {
  if (!tab) return;
  for (const id of panesOf(tab.root)) {
    const s = meta[id]?.status;
    if (s === "bell" || s === "waiting") setMeta(id, { status: "idle", note: "" });
  }
}

let ticker: ReturnType<typeof setInterval> | undefined;
/** Busy for a while, then quiet: the program finished or stopped to ask. */
/** A second hand for running blocks' clocks. */
const [clock, setClock] = createSignal(performance.now());
export { clock };
function tick() {
  const now = performance.now();
  setClock(now);
  for (const [id, l] of live) {
    if (l.agent || meta[id]?.status !== "busy" || now - l.last < 3500) continue;
    if (l.last - l.first > 8000 && !inView(id)) attend(id, "waiting", lastLine(l.term), { guess: true });
    else setMeta(id, { status: "idle" });
  }
}

function output(id: number) {
  const l = live.get(id);
  if (!l) return;
  const now = performance.now();
  if (l.agent || now - l.typed < 1500) return; // an agent says when it works; your own typing echoes
  if (now - l.last > 2000) l.first = now;
  l.last = now;
  const s = meta[id]?.status;
  if (s !== "busy" && s !== "exited" && now - l.first > 3000) setMeta(id, { status: "busy" });
}

// ---------------------------------------------------------------- paths that open in Code

/** The folders Code can open: the repository and every lane's worktree (set by the screen). */
let roots: string[] = [];
export function setRoots(r: string[]) {
  roots = r;
}
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
            hover: (e: MouseEvent) => e.target instanceof HTMLElement && (e.target.title = t("⌘-click to open in Code")),
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
    fontFamily: faceCss(st.face),
    fontSize: st.font,
    fontWeight: "normal",
    fontWeightBold: "bold",
    lineHeight: 1, // the face's own line, as Warp sets it: 13px Hack on rows of about 15.5px
    cursorStyle: "bar",
    cursorWidth: 2,
    cursorInactiveStyle: "outline",
    cursorBlink: true,
    drawBoldTextInBrightColors: false,
    rescaleOverlappingGlyphs: true,
    scrollback: 10000,
    // Warp scrolls by the trackpad's own pixels; xterm's wheel feels short of that.
    scrollSensitivity: 1.6,
    fastScrollSensitivity: 6, // ⌥ while scrolling
    macOptionIsMeta: true,
    allowProposedApi: true, // search decorations
    minimumContrastRatio: 4.5, // any program's colours stay readable on our ground
    theme: theme(),
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  // Emoji and wide glyphs (Claude Code's marks) take the cells they really take.
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  // ⌘-click, as in iTerm2 and VS Code: a plain click selects text.
  term.loadAddon(new WebLinksAddon((e, uri) => e.metaKey && void openExternal(uri).catch(() => {})));
  linkFiles(term, info.id);
  const l: Live = { term, fit, search, blocks: new Blocks(term), el: document.createElement("div"), opened: false, first: 0, last: 0, typed: 0 };
  l.el.className = "term-surface";
  // Warp's (and macOS's) text keys, sent as the readline keys every shell and TUI knows.
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown" || e.ctrlKey) return true;
    const seq = editKey(e);
    if (seq == null) return true;
    e.preventDefault();
    l.typed = performance.now();
    void terms.write(info.id, seq).catch(() => {});
    return false;
  });
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
    if (kind === "notify" && title === "warp://cli-agent") agentEvent(info.id, body.join(";"));
    else if (kind === "notify") attend(info.id, "bell", body.join(";") || title || "", title ? { title } : {});
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
  live.get(id)?.blocks.dispose();
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
  // Terminals being drawn again from their kept output: what arrives meanwhile waits, and only
  // what the kept output does not hold yet is written after it.
  const restoring = new Map<number, [string, number][]>();
  await onTerm(
    (id, data, seq) => {
      const waiting = restoring.get(id);
      if (waiting) return void waiting.push([data, seq]);
      live.get(id)?.term.write(data);
      output(id);
    },
    (id, code) => {
      if (code === 0 && tabOf(id)) {
        // A clean exit closes its pane, as in any terminal.
        closePane(id);
        return;
      }
      live.get(id)?.term.write(`\r\n\x1b[2m[${t("the shell exited with {code}", { code: String(code) })}]\x1b[0m\r\n`);
      setMeta(id, { status: "exited", exit: code });
    },
  );
  const saved = loadSaved();
  const face = saved?.face2 ?? (saved?.face === "menlo" ? undefined : saved?.face);
  if (face && TERMINAL_FACES.some((f) => f.v === face)) setSt("face", face);
  if (saved?.render === "gpu") setSt("render", "gpu");
  // Measure cells with the real face, never a fallback: a late font leaves gaps between letters.
  await Promise.all(["normal", "bold"].map((w) => document.fonts.load(`${w} ${DEFAULT_FONT}px ${faceCss(st.face)}`))).catch(() => {});
  if (saved) setSt({ side: saved.side === "top" ? "top" : "left", fold: saved.fold === true, font: Math.min(20, Math.max(9, !saved.font || (!saved.font13 && saved.font <= 14) ? DEFAULT_FONT : saved.font)) }); // 12, 13 and 14 were defaults once: they move to today's
  let existing: TermInfo[] = [];
  try {
    existing = await terms.list();
    for (const t of existing) {
      restoring.set(t.id, []);
      try {
        const term = make(t).term;
        const kept = await terms.buffer(t.id);
        term.write(kept.text);
        for (const [data, seq] of restoring.get(t.id) ?? []) if (seq > kept.seq) term.write(data);
      } finally {
        restoring.delete(t.id);
      }
    }
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
          const [cols, rows] = guessSize(undefined, share(t.root, old) ?? undefined);
          const info = place.run ? await terms.resume(place.run, cols, rows).catch(() => terms.open(place.cwd, place.label, cols, rows)) : await terms.open(place.cwd, place.label, cols, rows);
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

/** The size a new shell will have, before its pane exists. A shell born wider than its pane draws
 *  zsh's partial-line mark (PROMPT_SP: a "%" and a row of spaces) across two rows, and the "%"
 *  stays on screen; so the guess leans narrow, and the real size follows once the pane is laid out. */
/** A pane's share of its tab, across and down. */
function share(n: Node, term: number): [number, number] | null {
  if (n.kind === "pane") return n.term === term ? [1, 1] : null;
  for (let i = 0; i < n.children.length; i++) {
    const f = share(n.children[i]!, term);
    if (f) return n.dir === "row" ? [f[0] * n.sizes[i]!, f[1]] : [f[0], f[1] * n.sizes[i]!];
  }
  return null;
}

function guessSize(split?: Dir, part?: [number, number]): [number, number] {
  const c = document.createElement("canvas").getContext("2d");
  let cw = st.font * 0.6;
  if (c) {
    c.font = `${st.font}px ${faceCss(st.face)}`;
    cw = c.measureText("W".repeat(20)).width / 20 || cw;
  }
  const ch = Math.ceil(st.font * 1.2) * 1.35;
  const t = activeTab();
  const focused = split && t ? document.querySelector<HTMLElement>(`[data-pane="${t.focus}"] .pane-host`) : null;
  const box = focused ?? document.querySelector<HTMLElement>(".terms-stage");
  const r = box?.getBoundingClientRect();
  // Off screen: the window less the rail, the tab list and the frame.
  let w = r?.width || window.innerWidth - 330;
  let h = r?.height || window.innerHeight - 70;
  if (!focused) (w -= 14), (h -= 10); // the pane body's padding
  if (split === "row") w = w / 2 - 14;
  if (split === "col") h = h / 2 - 10 - 28;
  if (split && t && panesOf(t.root).length === 1) h -= 28; // bars appear once a tab splits
  if (part && (part[0] < 1 || part[1] < 1)) (w *= part[0]), (h = h * part[1] - 28);
  return [Math.max(20, Math.floor(w / cw) - 1), Math.max(4, Math.floor(h / ch))];
}

/** Opens a terminal in `cwd`, in a new tab, and shows it. */
export async function openTerminal(cwd: string, label: string): Promise<boolean> {
  selectAgent(null);
  try {
    const info = await terms.open(cwd, label, ...guessSize());
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

/** Dev servers the terminals printed (Vite's "Local: http://localhost:5173/", Next's, Rails'…):
 *  the newest line first, one per address, with the folder of the terminal that printed it. */
export function localServers(): { url: string; cwd: string; label: string }[] {
  const re = /\bhttps?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]|[a-z0-9-]+\.localhost)(?::\d{2,5})?(?:\/[^\s'"<>)\]]*)?/gi;
  const found: { url: string; cwd: string; label: string; at: number }[] = [];
  for (const [id, l] of live) {
    const m = meta[id];
    if (!m || m.status === "exited" || m.run) continue;
    const buf = l.term.buffer.active;
    const end = buf.length;
    for (let i = Math.max(0, end - 400); i < end; i++) {
      const text = buf.getLine(i)?.translateToString(true) ?? "";
      for (const hit of text.matchAll(re)) {
        const url = hit[0].replace(/[.,;:]+$/, "").replace("0.0.0.0", "localhost");
        found.push({ url, cwd: m.cwd, label: placeName(id), at: (l.last || 0) * 1e6 + i });
      }
    }
  }
  const seen = new Set<string>();
  return found
    .sort((a, b) => b.at - a.at)
    .filter((f) => {
      const origin = (() => {
        try {
          return new URL(f.url).origin;
        } catch {
          return f.url;
        }
      })();
      if (seen.has(origin)) return false;
      seen.add(origin);
      return true;
    })
    .map(({ url, cwd, label }) => ({ url, cwd, label }));
}

/** A terminal in that folder: the open tab already there (its focused pane, still running), else a new one. */
export async function terminalAt(cwd: string, label: string): Promise<boolean> {
  const there = st.tabs.find((tb) => meta[tb.focus]?.cwd === cwd && meta[tb.focus]?.status !== "exited" && !meta[tb.focus]?.run);
  if (there) {
    selectAgent(null);
    selectTab(there.id);
    return true;
  }
  return openTerminal(cwd, label);
}

/** Continue in Claude Code: the agent run's session, resumed by the official CLI in a new tab. */
export async function resumeInTerminal(run: string): Promise<boolean> {
  selectAgent(null);
  try {
    const info = await terms.resume(run, ...guessSize());
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

export async function split(dir: Dir) {
  const tb = activeTab();
  if (!tb) return;
  const from = meta[tb.focus];
  try {
    const info = await terms.open(from?.cwd ?? "", from?.label ?? "terminal", ...guessSize(dir));
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

export function selectTab(id: number) {
  selectAgent(null);
  setSt("active", id);
  save();
}

export function focusPane(term: number) {
  const t = tabOf(term);
  if (!t) return;
  if (st.active !== t.id) selectTab(t.id);
  setTab(t.id, () => ({ focus: term }));
  requestAnimationFrame(() => live.get(term)?.term.focus());
}

/** The pane nearest the focused one in a direction, by where they sit on screen. */
export function neighbour(side: Side): number | null {
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

export function resizeFocused(side: Side) {
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

/** The GPU renderer, falling back to the DOM one when WebKit takes the context back. */
export function gpu(l: Live) {
  try {
    const gl = new WebglAddon();
    gl.onContextLoss(() => {
      gl.dispose();
      l.gl = undefined;
    });
    l.term.loadAddon(gl);
    l.gl = gl;
  } catch {
    /* no WebGL: the DOM renderer draws */
  }
}

export function fontSize(next: number) {
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
export function jumpToWanting() {
  let best: { id: number; at: number } | null = null;
  for (const [id, m] of Object.entries(meta)) if ((m.status === "bell" || m.status === "waiting") && (!best || m.at > best.at)) best = { id: Number(id), at: m.at };
  if (best) focusPane(best.id);
}
