// Free terminals: your own shell in the repository or a lane's worktree, in tabs. Each xterm lives
// for the life of its shell, outside any screen, so switching screens never loses a line.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { onTerm, terms, type Lane, type TermInfo } from "./api";
import { Icon } from "./icons";
import { toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

interface Live {
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
  opened: boolean;
}

const live = new Map<number, Live>();
const [list, setList] = createSignal<TermInfo[]>([]);
const [active, setActive] = createSignal<number | null>(null);
export { list as terminalList };

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

function theme() {
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string) => hex(cs.getPropertyValue(name).trim());
  const dark = cs.getPropertyValue("color-scheme").includes("dark");
  return {
    background: v("--color-paper-sunk"),
    foreground: v("--color-ink"),
    cursor: v("--color-accent"),
    cursorAccent: v("--color-paper-sunk"),
    selectionBackground: dark ? "#3a3f4b" : "#d6dbe4",
    black: v("--color-paper-3"),
    brightBlack: v("--color-muted"),
    red: v("--color-fail"),
    brightRed: v("--color-fail"),
    green: v("--color-done"),
    brightGreen: v("--color-done"),
    yellow: v("--code-string"),
    brightYellow: v("--code-string"),
    blue: v("--color-run"),
    brightBlue: v("--code-fn"),
    magenta: v("--code-keyword"),
    brightMagenta: v("--code-keyword"),
    cyan: v("--code-number"),
    brightCyan: v("--code-number"),
    white: v("--color-ink-2"),
    brightWhite: v("--color-ink"),
  };
}

function fontFamily() {
  return getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace";
}

// ---------------------------------------------------------------- the store

function make(info: TermInfo): Live {
  const term = new Terminal({
    fontFamily: fontFamily(),
    fontSize: 12,
    lineHeight: 1.25,
    cursorBlink: true,
    scrollback: 5000,
    macOptionIsMeta: true,
    allowProposedApi: false,
    minimumContrastRatio: 4.5, // any program's colours stay readable on our ground
    theme: theme(),
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.onData((d) => void terms.write(info.id, d).catch(() => {}));
  term.onResize(({ cols, rows }) => void terms.resize(info.id, cols, rows).catch(() => {}));
  const el = document.createElement("div");
  el.className = "term-surface";
  const l = { term, fit, el, opened: false };
  live.set(info.id, l);
  return l;
}

let started = false;
/** Once per window: follow output, and draw again any terminal that outlived a reload. */
export async function startTerminals() {
  if (started) return;
  started = true;
  await onTerm(
    (id, data) => live.get(id)?.term.write(data),
    (id, code) => {
      live.get(id)?.term.write(`\r\n\x1b[2m[the shell exited with ${code}]\x1b[0m\r\n`);
      setList((l) => l.map((t) => (t.id === id ? { ...t, exited: code } : t)));
    },
  );
  try {
    const existing = await terms.list();
    for (const t of existing) {
      const l = make(t);
      l.term.write(await terms.buffer(t.id));
    }
    setList(existing);
    if (existing.length) setActive(existing[existing.length - 1]!.id);
  } catch {
    /* no backend: no terminals */
  }
  // The theme follows the system's light and dark.
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    const t = theme();
    for (const l of live.values()) l.term.options.theme = t;
  });
}

/** Opens a terminal in `cwd` and shows it. */
export async function openTerminal(cwd: string, label: string): Promise<boolean> {
  try {
    const info = await terms.open(cwd, label, 100, 30);
    make(info);
    setList((l) => [...l, info]);
    setActive(info.id);
    return true;
  } catch (e) {
    toast({ tone: "fail", title: "No terminal", body: msg(e) });
    return false;
  }
}

export function closeTerminal(id: number) {
  void terms.close(id).catch(() => {});
  live.get(id)?.term.dispose();
  live.delete(id);
  const rest = list().filter((t) => t.id !== id);
  setList(rest);
  if (active() === id) setActive(rest.length ? rest[rest.length - 1]!.id : null);
}

export const activeTerminal = active;

// ---------------------------------------------------------------- the screen

export function TerminalScreen(props: { repo: string; repoName: string; lanes: Lane[] }) {
  let host!: HTMLDivElement;
  const [menu, setMenu] = createSignal(false);
  const places = () => [
    { cwd: props.repo, label: props.repoName, sub: "repository" },
    ...props.lanes
      .filter((l) => l.worktree && !l.worktree.is_main)
      .map((l) => ({ cwd: l.worktree!.path, label: l.branch, sub: "lane worktree" })),
  ];

  // Shows the active terminal in the host: its element moves here, opened the first time.
  const show = () => {
    const id = active();
    if (!host) return;
    for (const child of [...host.children]) host.removeChild(child);
    if (id == null) return;
    const l = live.get(id);
    if (!l) return;
    host.appendChild(l.el);
    if (!l.opened) {
      l.term.open(l.el);
      l.opened = true;
    }
    requestAnimationFrame(() => {
      try {
        l.fit.fit();
      } catch {
        /* not laid out yet */
      }
      l.term.focus();
    });
  };

  // Terminals that arrive after the screen (a reload, ⌘T elsewhere) show as soon as they exist.
  createEffect(on([active, () => list().length], () => show(), { defer: true }));

  onMount(() => {
    show();
    const ro = new ResizeObserver(() => {
      const l = active() != null ? live.get(active()!) : undefined;
      try {
        l?.fit.fit();
      } catch {
        /* hidden */
      }
    });
    ro.observe(host);
    onCleanup(() => ro.disconnect());
  });

  const pick = (id: number) => {
    setActive(id);
    show();
  };
  const open = async (cwd: string, label: string) => {
    setMenu(false);
    if (await openTerminal(cwd, label)) show();
  };

  return (
    <div class="terms">
      <div class="term-tabs" role="tablist" aria-label="Terminals">
        <For each={list()}>
          {(t) => (
            <div class="term-tab" role="presentation" data-active={active() === t.id ? "" : undefined}>
              <button
                type="button"
                role="tab"
                class="term-tab-main"
                aria-selected={active() === t.id}
                title={t.cwd}
                onClick={() => pick(t.id)}
              >
                <Icon name="term" small />
                <span class="term-tab-label">{t.label}</span>
                <Show when={t.exited != null}>
                  <span class="term-tab-exit">exited</span>
                </Show>
              </button>
              <button type="button" class="term-tab-close" aria-label={`Close ${t.label}`} onClick={() => (closeTerminal(t.id), show())}>
                <Icon name="close" small />
              </button>
            </div>
          )}
        </For>
        <div class="anchor">
          <button type="button" class="btn btn-ghost btn-icon" aria-label="New terminal" aria-expanded={menu()} title="New terminal (⌘T)" onClick={() => setMenu(!menu())}>
            <Icon name="plus" small />
          </button>
          <Show when={menu()}>
            <div class="pop term-menu" role="menu">
              <For each={places()}>
                {(p) => (
                  <button type="button" class="menu-item" role="menuitem" onClick={() => void open(p.cwd, p.label)}>
                    <Icon name={p.sub === "repository" ? "folder" : "branch"} small />
                    <span class="repo-entry">
                      <span class="repo-name">{p.label}</span>
                      <span class="repo-path">{p.sub}</span>
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </div>
      </div>
      <Show when={list().length === 0}>
        <div class="empty term-empty">
          <div class="empty-title">No terminal open</div>
          <p class="empty-body">Your own shell, in the repository or in any lane’s worktree. ⌘T opens one where you are.</p>
          <button type="button" class="btn btn-primary" onClick={() => void open(props.repo, props.repoName)}>
            <Icon name="term" small />
            Open in {props.repoName}
          </button>
        </div>
      </Show>
      <div class="term-view" ref={host} hidden={list().length === 0} />
    </div>
  );
}
