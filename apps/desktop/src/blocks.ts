// Command blocks, as in Warp: the shell marks where each command's prompt, output and end are
// (OSC 133 A/C/D, and 633;E for the command line; zsh does it through Pitlane's startup files).
// Each finished command becomes a block: a hairline above it, a fail stripe when it failed, on
// hover its time and exit and two copies (command, output). ⌘↑ and ⌘↓ select the previous and
// next block, so ⌘C copies it. A long block keeps its command in view at the top of the pane.
import type { IDecoration, IDisposable, IMarker, Terminal } from "@xterm/xterm";
import { createSignal, type Accessor } from "solid-js";
import { t } from "./i18n";

export interface Block {
  prompt: IMarker;
  out?: IMarker;
  end?: IMarker;
  cmd: string;
  started: number;
  ms?: number;
  exit?: number;
  deco?: IDecoration;
  el?: HTMLElement;
}

/** What the pane shows on top while you read inside a long block. */
export interface Sticky {
  cmd: string;
  line: number;
  exit?: number;
  ms?: number;
  started: number;
}

const ICONS: Record<string, string> = {
  cmd: '<path d="M5 8l4 4-4 4M12 17h7" />',
  out: '<rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />',
  card: '<rect x="4" y="4" width="16" height="16" rx="2" /><path d="M12 9v6M9 12h6" />',
};

/** A failed command made into a card: the terminal's owner turns it into one on the Board. */
type CardFrom = (term: Terminal, cmd: string, output: string, exit: number) => void;
let cardFrom: CardFrom | null = null;
export function onBlockCard(fn: CardFrom) {
  cardFrom = fn;
}

/** VS Code's 633;E escaping: \\ and \xAB. */
const unescape = (s: string) => s.replace(/\\(\\|x([0-9a-f]{2}))/gi, (_, a: string, h?: string) => (h ? String.fromCharCode(parseInt(h, 16)) : a));

export function duration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms))}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.round((ms % 60_000) / 1000);
  return m < 60 ? `${m}m ${s}s` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

function linesText(term: Terminal, from: number, to: number): string {
  const b = term.buffer.active;
  const out: string[] = [];
  for (let y = from; y < to; y++) {
    const line = b.getLine(y);
    if (!line) continue;
    const text = line.translateToString(true);
    // A wrapped line continues the one before it.
    if (line.isWrapped && out.length) out[out.length - 1] += text;
    else out.push(text);
  }
  while (out.length && !out[out.length - 1]!.trim()) out.pop();
  return out.join("\n");
}

export class Blocks implements IDisposable {
  private list: Block[] = [];
  private cur: Block | null = null;
  private hovered: Block | null = null;
  private subs: IDisposable[] = [];
  private frame = 0;
  readonly sticky: Accessor<Sticky | null>;
  private setSticky: (s: Sticky | null) => void;
  /** True once the shell has marked a command: blocks are on. */
  marked = false;

  constructor(private term: Terminal) {
    [this.sticky, this.setSticky] = createSignal<Sticky | null>(null);
    const p = term.parser;
    this.subs.push(
      p.registerOscHandler(133, (d) => (this.mark(d), true)),
      p.registerOscHandler(633, (d) => {
        if (d.startsWith("E;") && this.cur) this.cur.cmd = unescape(d.slice(2).split(";")[0] ?? "").trim();
        return true;
      }),
      term.onResize(() => this.redraw()),
      term.onScroll(() => this.later()),
      term.onWriteParsed(() => this.later()),
    );
  }

  private mark(d: string) {
    const [kind, arg] = d.split(";");
    this.marked = true;
    if (kind === "A") {
      // A prompt with no command after it (Enter on an empty line) leaves no block.
      if (this.cur && !this.cur.out) this.cur.prompt.dispose();
      const prompt = this.term.registerMarker(0);
      if (!prompt) return;
      const b: Block = { prompt, cmd: "", started: 0 };
      this.cur = b;
      prompt.onDispose(() => this.drop(b));
    } else if (kind === "C" && this.cur) {
      this.cur.out = this.term.registerMarker(0);
      this.cur.started = performance.now();
      this.list.push(this.cur);
    } else if (kind === "D" && this.cur?.out) {
      const b = this.cur;
      b.end = this.term.registerMarker(0);
      b.ms = performance.now() - b.started;
      b.exit = Number(arg ?? 0) || 0;
      this.cur = null;
      this.draw(b);
    }
    this.later();
  }

  private drop(b: Block) {
    b.deco?.dispose();
    b.out?.dispose();
    b.end?.dispose();
    this.list = this.list.filter((x) => x !== b);
    if (this.cur === b) this.cur = null;
    if (this.hovered === b) this.hovered = null;
  }

  private lines(b: Block) {
    return Math.max(1, (b.end?.line ?? this.term.buffer.active.baseY + this.term.buffer.active.cursorY + 1) - b.prompt.line);
  }

  private draw(b: Block) {
    b.deco?.dispose();
    b.el = undefined;
    if (b.prompt.isDisposed) return;
    const deco = this.term.registerDecoration({ marker: b.prompt, width: this.term.cols, height: this.lines(b), layer: "top" });
    if (!deco) return;
    b.deco = deco;
    deco.onRender((el) => {
      if (b.el === el) return;
      b.el = el;
      el.classList.add("tb");
      if (b.exit) el.dataset.fail = "";
      const bar = document.createElement("div");
      bar.className = "tb-bar";
      const info = document.createElement("span");
      info.className = "tb-info";
      info.textContent = b.exit ? `${t("exit")} ${b.exit} · ${duration(b.ms ?? 0)}` : duration(b.ms ?? 0);
      bar.append(info, this.button(t("Copy command"), "cmd", () => b.cmd), this.button(t("Copy output"), "out", () => this.output(b)));
      // A failure is work: one click puts it on the Board with the command, the place and the output.
      if (b.exit && cardFrom) bar.append(this.action(t("Make a card to fix this"), "card", () => cardFrom?.(this.term, b.cmd, this.output(b), b.exit!)));
      el.append(bar);
      if (this.hovered === b) el.dataset.hover = "";
    });
  }

  private button(label: string, kind: string, text: () => string) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "tb-btn";
    btn.dataset.kind = kind;
    btn.title = label;
    btn.setAttribute("aria-label", label);
    btn.innerHTML = `<svg class="icon icon-sm" viewBox="0 0 24 24" aria-hidden="true">${ICONS[kind]}</svg>`;
    btn.addEventListener("mousedown", (e) => (e.preventDefault(), e.stopPropagation()));
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      void navigator.clipboard.writeText(text()).then(() => {
        btn.dataset.done = "";
        setTimeout(() => delete btn.dataset.done, 900);
      });
      this.term.focus();
    });
    return btn;
  }

  private action(label: string, kind: string, run: () => void) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "tb-btn";
    btn.dataset.kind = kind;
    btn.title = label;
    btn.setAttribute("aria-label", label);
    btn.innerHTML = `<svg class="icon icon-sm" viewBox="0 0 24 24" aria-hidden="true">${ICONS[kind]}</svg>`;
    btn.addEventListener("mousedown", (e) => (e.preventDefault(), e.stopPropagation()));
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      run();
      btn.dataset.done = "";
      setTimeout(() => delete btn.dataset.done, 900);
    });
    return btn;
  }

  /** The command's output, without its prompt line. */
  output(b: Block): string {
    if (!b.out) return "";
    return linesText(this.term, b.out.line + (b.out.line === b.prompt.line ? 1 : 0), b.end?.line ?? b.out.line);
  }

  private redraw() {
    for (const b of this.list) if (b.end) this.draw(b);
    this.later();
  }

  /** The finished or running block under a buffer line. */
  at(line: number): Block | null {
    for (let i = this.list.length - 1; i >= 0; i--) {
      const b = this.list[i]!;
      if (b.prompt.line <= line && (!b.end || line < b.end.line)) return b;
    }
    return null;
  }

  /** Hover from the pane: the pointer's y in the terminal's screen. */
  hover(clientY: number | null) {
    let b: Block | null = null;
    const screen = this.term.element?.querySelector<HTMLElement>(".xterm-screen");
    if (clientY != null && screen) {
      const r = screen.getBoundingClientRect();
      const row = Math.floor(((clientY - r.top) / r.height) * this.term.rows);
      if (row >= 0 && row < this.term.rows) b = this.at(this.term.buffer.active.viewportY + row);
      if (b && !b.end) b = null;
    }
    if (b === this.hovered) return;
    if (this.hovered?.el) delete this.hovered.el.dataset.hover;
    this.hovered = b;
    if (b?.el) b.el.dataset.hover = "";
  }

  /** ⌘↑ / ⌘↓: select the previous or next command with its output, and bring it into view. */
  jump(step: -1 | 1) {
    const done = this.list.filter((b) => b.end && !b.prompt.isDisposed);
    if (!done.length) return false;
    const sel = this.term.getSelectionPosition();
    const from = sel ? sel.start.y : step < 0 ? Number.MAX_SAFE_INTEGER : -1;
    const next = step < 0 ? [...done].reverse().find((b) => b.prompt.line < from) : done.find((b) => b.prompt.line > from);
    if (!next) {
      if (step > 0) {
        this.term.clearSelection();
        this.term.scrollToBottom();
      }
      return true;
    }
    this.term.selectLines(next.prompt.line, Math.max(next.prompt.line, next.end!.line - 1));
    const b = this.term.buffer.active;
    if (next.prompt.line < b.viewportY || next.end!.line > b.viewportY + this.term.rows) this.term.scrollToLine(Math.max(0, next.prompt.line - 1));
    return true;
  }

  /** Back to a block's prompt, from the sticky head. */
  reveal(line: number) {
    this.term.scrollToLine(Math.max(0, line - 1));
  }

  private later() {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.updateSticky();
    });
  }

  private updateSticky() {
    const top = this.term.buffer.active.viewportY;
    const b = this.at(top);
    // Only when the prompt is above the view and the block still has lines below the head.
    if (!b || !b.out || b.prompt.line >= top || (b.end && b.end.line <= top + 1)) return this.setSticky(null);
    const s = this.sticky();
    if (s && s.line === b.prompt.line && s.exit === b.exit) return;
    this.setSticky({ cmd: b.cmd || linesText(this.term, b.prompt.line, b.prompt.line + 1).trim(), line: b.prompt.line, exit: b.exit, ms: b.ms, started: b.started });
  }

  dispose() {
    cancelAnimationFrame(this.frame);
    for (const s of this.subs) s.dispose();
    for (const b of this.list) b.deco?.dispose();
    this.list = [];
  }
}
