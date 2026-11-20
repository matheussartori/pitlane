// Code: the files of the repository or of a lane's worktree, in Monaco. Open files keep their
// models (and unsaved edits) while you move between screens. Around Monaco, the few things it does
// not have: quick open (⌘P), search across files (⇧⌘F), the lane's changes in the margin and side
// by side, tabs that come back, and the standard keys.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import type * as M from "monaco-editor";
import { files, vscode, type FileChanges, type Lane, type SearchHit, type VscodeStatus } from "./api";
import { Icon } from "./icons";
import { loadMonaco, themeName } from "./monaco";
import { ensureLsp, familyName, familyOf, installLsp, lspStateFor } from "./lsp";
import { Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

interface Open {
  root: string;
  path: string;
  model: M.editor.ITextModel | null;
  /** Why it cannot be shown, if it cannot. */
  refused: string | null;
  saved: number;
  view: M.editor.ICodeEditorViewState | null;
}

// Module state: open files survive the screen.
const [opened, setOpened] = createSignal<Open[]>([]);
const [current, setCurrent] = createSignal<string | null>(null); // key = root + "\0" + path
const [dirty, setDirty] = createSignal<Set<string>>(new Set());
const [rootSig, setRootSig] = createSignal<string | null>(null);
const [changes, setChanges] = createSignal<Record<string, FileChanges>>({});
const key = (root: string, path: string) => `${root}\0${path}`;
let pending: { root: string; path: string; line?: number; col?: number } | null = null;
/** Recently closed, for ⇧⌘T; recently shown, for ⌘P with nothing typed. */
const closed: { root: string; path: string }[] = [];
const recent: string[] = [];

const SAVED = "pitlane.code.v1";
function remember() {
  try {
    localStorage.setItem(SAVED, JSON.stringify({ open: opened().map((o) => [o.root, o.path]), current: current() }));
  } catch {
    /* no storage */
  }
}

/** Opens `path` under `root` in the Code screen (from Review, Run, a terminal…), at a line when
 *  one is given. The app shows the Code screen when `show` is set. */
export function openInCode(root: string, path: string, line?: number, col?: number, show = false) {
  setRootSig(root);
  pending = { root, path, line, col };
  window.dispatchEvent(new CustomEvent("pitlane-code-open"));
  if (show) window.dispatchEvent(new CustomEvent("pitlane-show-code"));
}

export const unsavedCount = () => dirty().size;

// ---------------------------------------------------------------- the tree

interface Node {
  name: string;
  path: string;
  dir: boolean;
  kids: Node[];
  /** Ignored by git: greyed, and a folder's contents are read when you open it. */
  ignored: boolean;
}

/** The tree of `paths`, plus `ignored` entries (folders end with "/"), greyed. */
function tree(paths: string[], ignored: string[] = []): Node[] {
  const root: Node = { name: "", path: "", dir: true, kids: [], ignored: false };
  const add = (p: string, grey: boolean) => {
    const isDir = p.endsWith("/");
    const parts = p.replace(/\/$/, "").split("/");
    let at = root;
    parts.forEach((name, i) => {
      const leaf = i === parts.length - 1;
      const dir = !leaf || isDir;
      const path = parts.slice(0, i + 1).join("/");
      let n = at.kids.find((k) => k.name === name && k.dir === dir);
      if (!n) {
        n = { name, path, dir, kids: [], ignored: grey && leaf };
        at.kids.push(n);
      }
      if (at.ignored) n.ignored = true;
      at = n;
    });
  };
  for (const p of paths) add(p, false);
  for (const p of ignored) add(p, true);
  const sort = (n: Node) => {
    n.kids.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
    n.kids.forEach(sort);
  };
  sort(root);
  return root.kids;
}

function TreeNodes(props: { nodes: Node[]; depth: number; open: Set<string>; toggle: (p: string) => void; pick: (p: string) => void; current: string | null; changed: Set<string> }) {
  return (
    <For each={props.nodes}>
      {(n) => (
        <>
          <button
            type="button"
            class="tree-row"
            style={{ "padding-inline-start": `${8 + props.depth * 12}px` }}
            data-current={!n.dir && props.current === n.path ? "" : undefined}
            data-changed={!n.dir && props.changed.has(n.path) ? "" : undefined}
            data-ignored={n.ignored ? "" : undefined}
            aria-expanded={n.dir ? props.open.has(n.path) : undefined}
            title={n.ignored ? `${n.path} (ignored by git)` : n.path}
            onClick={() => (n.dir ? props.toggle(n.path) : props.pick(n.path))}
          >
            <Show when={n.dir} fallback={<span class="tree-gap" />}>
              <Icon name="chev" small class={props.open.has(n.path) ? "" : "chev-closed"} />
            </Show>
            <span class="tree-name">{n.name}</span>
          </button>
          <Show when={n.dir && props.open.has(n.path)}>
            <TreeNodes nodes={n.kids} depth={props.depth + 1} open={props.open} toggle={props.toggle} pick={props.pick} current={props.current} changed={props.changed} />
          </Show>
        </>
      )}
    </For>
  );
}

// ---------------------------------------------------------------- quick open

/** Subsequence match, scored: the file name over the folders, runs and word starts over gaps. */
export function fuzzy(q: string, path: string): number {
  if (!q) return 1;
  const p = path.toLowerCase();
  const name = p.slice(p.lastIndexOf("/") + 1);
  let score = 0;
  let at = 0;
  let run = 0;
  for (const ch of q.toLowerCase()) {
    if (ch === " ") continue;
    const i = p.indexOf(ch, at);
    if (i < 0) return 0;
    run = i === at ? run + 1 : 0;
    const start = i === 0 || "/._-".includes(p[i - 1]!);
    score += 1 + run * 2 + (start ? 3 : 0) + (i >= p.length - name.length ? 2 : 0);
    at = i + 1;
  }
  if (name.startsWith(q.toLowerCase())) score += 10;
  return score - p.length / 100;
}

function QuickOpen(props: { list: string[]; onPick: (path: string, line?: number) => void; onClose: () => void }) {
  const [q, setQ] = createSignal("");
  const [sel, setSel] = createSignal(0);
  const parsed = () => {
    const m = /^(.*?)(?::(\d+))?$/.exec(q().trim())!;
    return { text: m[1] ?? "", line: m[2] ? Number(m[2]) : undefined };
  };
  const results = createMemo(() => {
    const { text } = parsed();
    if (!text) {
      const rec = recent.filter((p) => props.list.includes(p));
      return [...rec, ...props.list.filter((p) => !rec.includes(p))].slice(0, 50);
    }
    return props.list
      .map((p) => [p, fuzzy(text, p)] as const)
      .filter(([, s]) => s > 0)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 50)
      .map(([p]) => p);
  });
  createEffect(on(q, () => setSel(0)));
  const pick = (p: string | undefined) => {
    if (p) props.onPick(p, parsed().line);
    props.onClose();
  };
  return (
    <div class="qo-scrim" onPointerDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="qo" role="dialog" aria-label="Open a file">
        <input
          class="qo-input"
          placeholder="Open a file by name, path:line to jump"
          aria-label="File name"
          value={q()}
          ref={(el) => requestAnimationFrame(() => el.focus())}
          onInput={(e) => setQ(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") (e.preventDefault(), setSel(Math.min(sel() + 1, results().length - 1)));
            if (e.key === "ArrowUp") (e.preventDefault(), setSel(Math.max(sel() - 1, 0)));
            if (e.key === "Enter") pick(results()[sel()]);
            if (e.key === "Escape") props.onClose();
            e.stopPropagation();
          }}
        />
        <div class="qo-list" role="listbox">
          <For each={results()} fallback={<p class="qo-none">No file matches.</p>}>
            {(p, i) => (
              <button
                type="button"
                class="qo-row"
                role="option"
                aria-selected={i() === sel()}
                ref={(el) => createEffect(() => i() === sel() && el.scrollIntoView({ block: "nearest" }))}
                onPointerMove={() => setSel(i())}
                onClick={() => pick(p)}
              >
                <span class="qo-name">{p.split("/").pop()}</span>
                <span class="qo-dir">{p.split("/").slice(0, -1).join("/")}</span>
              </button>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- search across files

function SearchPanel(props: { root: string; onPick: (path: string, line: number, col: number) => void; focus: number }) {
  let input!: HTMLInputElement;
  const [q, setQ] = createSignal("");
  const [hits, setHits] = createSignal<SearchHit[] | null>(null);
  const [busy, setBusy] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const run = (text: string) => {
    clearTimeout(timer);
    if (!text.trim()) return setHits(null);
    timer = setTimeout(async () => {
      setBusy(true);
      try {
        setHits(await files.search(props.root, text));
      } catch (e) {
        toast({ tone: "fail", title: "Search failed", body: msg(e) });
      } finally {
        setBusy(false);
      }
    }, 180);
  };
  createEffect(on(() => props.focus, () => requestAnimationFrame(() => (input.focus(), input.select()))));
  createEffect(on(() => props.root, () => run(q()), { defer: true }));
  const groups = createMemo(() => {
    const m = new Map<string, SearchHit[]>();
    for (const h of hits() ?? []) m.set(h.path, [...(m.get(h.path) ?? []), h]);
    return [...m.entries()];
  });
  const mark = (h: SearchHit) => {
    const t = h.text.replace(/^\s+/, "");
    const cut = h.text.length - t.length;
    const at = Math.max(0, h.col - 1 - cut);
    return [t.slice(0, at), t.slice(at, at + q().length), t.slice(at + q().length)];
  };
  return (
    <div class="code-search">
      <label class="search-field code-filter">
        <Icon name="search" small />
      <input
        ref={input}
        type="search"
        placeholder="Search in files"
        aria-label="Search in files"
        value={q()}
        onInput={(e) => (setQ(e.currentTarget.value), run(e.currentTarget.value))}
        onKeyDown={(e) => {
          if (e.key === "Enter" && hits()?.[0]) props.onPick(hits()![0]!.path, hits()![0]!.line, hits()![0]!.col);
          if (e.key === "Escape") (setQ(""), run(""));
          e.stopPropagation();
        }}
      />
      </label>
      <div class="code-tree" role="list">
        <Show when={hits()} fallback={<p class="side-hint code-none">{busy() ? "Searching" : "Text across this folder’s files. A capital letter makes it match case."}</p>}>
          <p class="side-hint code-none">
            {hits()!.length === 0 ? "Nothing found." : `${hits()!.length >= 400 ? "400+" : hits()!.length} in ${groups().length} ${groups().length === 1 ? "file" : "files"}`}
          </p>
          <For each={groups()}>
            {([path, hs]) => (
              <div class="sr-file" role="listitem">
                <div class="sr-head" title={path}>
                  <span class="tree-name">{path.split("/").pop()}</span>
                  <span class="tree-dir">{path.split("/").slice(0, -1).join("/")}</span>
                </div>
                <For each={hs.slice(0, 30)}>
                  {(h) => (
                    <button type="button" class="sr-line" onClick={() => props.onPick(h.path, h.line, h.col)}>
                      <span class="sr-num">{h.line}</span>
                      <span class="sr-text">
                        {mark(h)[0]}
                        <mark>{mark(h)[1]}</mark>
                        {mark(h)[2]}
                      </span>
                    </button>
                  )}
                </For>
              </div>
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- the folder: the repository or a lane

/** The same picker as the top bar's repository switch: a quiet button, a menu of places. */
function PlacePicker(props: { places: { root: string; label: string }[]; root: string; repo: string; onPick: (root: string) => void }) {
  const [open, setOpen] = createSignal(false);
  let box!: HTMLDivElement;
  const here = () => props.places.find((p) => p.root === props.root) ?? props.places[0];
  const icon = (root: string) => (root === props.repo ? "folder" : "branch");
  const away = (e: PointerEvent) => !box.contains(e.target as globalThis.Node) && setOpen(false);
  createEffect(() => {
    if (open()) document.addEventListener("pointerdown", away, true);
    else document.removeEventListener("pointerdown", away, true);
  });
  onCleanup(() => document.removeEventListener("pointerdown", away, true));
  const pick = (r: string) => {
    props.onPick(r);
    setOpen(false);
  };
  return (
    <div class="anchor code-place" ref={box}>
      <button type="button" class="repo code-place-btn" aria-haspopup="menu" aria-expanded={open()} title={here()?.root} onClick={() => setOpen(!open())} onKeyDown={(e) => e.key === "Escape" && setOpen(false)}>
        <Icon name={icon(props.root)} small />
        <span class="code-place-name">{here()?.label}</span>
        <Icon name="chev" small />
      </button>
      <Show when={open()}>
        <div class="pop" role="menu" onKeyDown={(e) => e.key === "Escape" && setOpen(false)}>
          <For each={props.places}>
            {(p) => (
              <div class="menu-item" role="menuitem" tabindex="0" aria-current={p.root === props.root} onClick={() => pick(p.root)} onKeyDown={(e) => e.key === "Enter" && pick(p.root)}>
                <Icon name={icon(p.root)} small />
                <span class="repo-entry">
                  <span class="repo-name">{p.label}</span>
                  <span class="repo-path">{p.root === props.repo ? "Repository" : "Lane worktree"}</span>
                </span>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------- the screen

let openerSet = false;
/** Set while the Code screen is open: where Monaco's go-to-definition lands. */
let openHere: ((root: string, path: string, line?: number, col?: number) => void) | null = null;

/** Monaco asks this when a jump (definition, reference) leads to another file. */
export function openFromMonaco(resource: M.Uri, at?: { lineNumber: number; column: number }): boolean {
  const full = resource.path;
  const roots = [...new Set([...opened().map((x) => x.root), rootSig() ?? ""])].filter(Boolean).sort((a, b) => b.length - a.length);
  const root = roots.find((r) => full.startsWith(r + "/"));
  if (!root || !openHere) return false;
  openHere(root, full.slice(root.length + 1), at?.lineNumber, at?.column);
  return true;
}

function LightEditor(props: { repo: string; repoName: string; lanes: Lane[]; onVscode: () => void }) {
  const places = () => [
    { root: props.repo, label: props.repoName },
    ...props.lanes.filter((l) => l.worktree && !l.worktree.is_main).map((l) => ({ root: l.worktree!.path, label: l.branch })),
  ];
  const root = () => rootSig() ?? props.repo;
  const [list] = createResource(root, (r) => files.list(r));
  const [side, setSide] = createSignal<"files" | "search" | null>("files");
  const [searchFocus, setSearchFocus] = createSignal(0);
  const [quick, setQuick] = createSignal(false);
  const [diff, setDiff] = createSignal(false);
  const [openDirs, setOpenDirs] = createSignal<Set<string>>(new Set());
  const [ignored] = createResource(root, (r) => files.ignored(r).catch(() => [] as string[]));
  // Inside ignored folders, what you have opened so far.
  const [inside, setInside] = createSignal<Record<string, string[]>>({});
  createEffect(on(root, () => setInside({}), { defer: true }));
  const nodes = createMemo(() => tree(list() ?? [], [...(ignored() ?? []), ...Object.values(inside()).flat()]));
  // The open file's folders stay open in the tree.
  createEffect(
    on(current, () => {
      const p = cur()?.path;
      if (!p) return;
      const parts = p.split("/").slice(0, -1);
      const s = new Set(openDirs());
      parts.forEach((_, i) => s.add(parts.slice(0, i + 1).join("/")));
      setOpenDirs(s);
    }),
  );

  let host!: HTMLDivElement;
  let diffHost!: HTMLDivElement;
  let monaco: typeof import("monaco-editor") | null = null;
  let editor: M.editor.IStandaloneCodeEditor | null = null;
  let diffEditor: M.editor.IStandaloneDiffEditor | null = null;
  let baseModel: M.editor.ITextModel | null = null;
  let marks: M.editor.IEditorDecorationsCollection | null = null;
  const [ready, setReady] = createSignal(false);
  let markersSub: { dispose(): void } | null = null;

  const cur = () => opened().find((o) => key(o.root, o.path) === current()) ?? null;
  const tabs = () => opened().filter((o) => o.root === root());
  const curChanges = () => {
    const o = cur();
    return o ? changes()[key(o.root, o.path)] : undefined;
  };
  const changeCount = () => {
    const c = curChanges();
    return c ? c.added.length + c.changed.length + c.removed.length : 0;
  };
  const changedFiles = createMemo(
    () =>
      new Set(
        Object.entries(changes())
          .filter(([k, c]) => k.startsWith(root() + "\0") && c.added.length + c.changed.length + c.removed.length > 0)
          .map(([k]) => k.split("\0")[1]!),
      ),
  );

  async function open(path: string, at = root(), line?: number, col?: number) {
    const k = key(at, path);
    if (!opened().some((o) => key(o.root, o.path) === k)) {
      try {
        const f = await files.read(at, path);
        const m = monaco;
        const uri = m?.Uri.file(`${at}/${path}`);
        const model = f.refused || !m || !uri ? null : (m.editor.getModel(uri) ?? m.editor.createModel(f.text, undefined, uri));
        const entry: Open = { root: at, path, model, refused: f.refused, saved: model?.getAlternativeVersionId() ?? 0, view: null };
        model?.onDidChangeContent(() => {
          const d = new Set(dirty());
          if (model.getAlternativeVersionId() !== entry.saved) d.add(k);
          else d.delete(k);
          setDirty(d);
        });
        setOpened([...opened(), entry]);
      } catch (e) {
        toast({ tone: "fail", title: `Could not open ${path}`, body: msg(e) });
        return;
      }
    }
    if (root() !== at) setRootSig(at);
    select(k);
    if (line && editor) {
      const pos = { lineNumber: line, column: col ?? 1 };
      editor.setPosition(pos);
      editor.revealPositionInCenter(pos);
    }
  }

  function select(k: string) {
    const before = cur();
    if (before && editor && !diff()) before.view = editor.saveViewState();
    setDiff(false);
    setCurrent(k);
    const o = cur();
    if (o) {
      const i = recent.indexOf(o.path);
      if (i >= 0) recent.splice(i, 1);
      recent.unshift(o.path);
      recent.length = Math.min(recent.length, 30);
    }
    if (editor) {
      editor.setModel(o?.model ?? null);
      if (o?.view) editor.restoreViewState(o.view);
      editor.focus();
    }
    remember();
    void loadChanges();
    if (monaco && o?.model) void ensureLsp(monaco, o.root, o.path);
    countProblems();
  }

  function close(o: Open) {
    const k = key(o.root, o.path);
    if (dirty().has(k) && !window.confirm(`${o.path} has unsaved changes. Close it anyway?`)) return;
    const rest = opened().filter((x) => x !== o);
    setOpened(rest);
    const d = new Set(dirty());
    d.delete(k);
    setDirty(d);
    closed.push({ root: o.root, path: o.path });
    if (current() === k) {
      const next = rest.filter((x) => x.root === o.root).at(-1);
      if (next) select(key(next.root, next.path));
      else {
        setCurrent(null);
        setDiff(false);
        editor?.setModel(null);
      }
    }
    o.model?.dispose();
    remember();
  }

  async function save() {
    const o = cur();
    if (!o?.model) return;
    try {
      await files.write(o.root, o.path, o.model.getValue());
      o.saved = o.model.getAlternativeVersionId();
      const d = new Set(dirty());
      d.delete(key(o.root, o.path));
      setDirty(d);
      void loadChanges();
    } catch (e) {
      toast({ tone: "fail", title: `${o.path} was not saved`, body: msg(e) });
    }
  }

  // ------------------------------------------------ language servers and problems

  const [problems, setProblems] = createSignal({ errors: 0, warnings: 0 });
  function countProblems() {
    const o = cur();
    if (!monaco || !o?.model) return setProblems({ errors: 0, warnings: 0 });
    const ms = monaco.editor.getModelMarkers({ resource: o.model.uri });
    setProblems({ errors: ms.filter((x) => x.severity === monaco!.MarkerSeverity.Error).length, warnings: ms.filter((x) => x.severity === monaco!.MarkerSeverity.Warning).length });
  }
  const lsp = () => {
    const o = cur();
    return o ? lspStateFor(o.root, o.path) : undefined;
  };
  async function install() {
    const o = cur();
    const st = lsp();
    if (!monaco || !o || st?.s !== "missing") return;
    if (!window.confirm(`${st.hint}\n\nInstall it now?`)) return;
    await installLsp(monaco, o.root, o.path);
  }

  // ------------------------------------------------ changes against the lane's base

  async function loadChanges() {
    const o = cur();
    if (!o?.model) return paintMarks();
    try {
      const c = await files.changes(o.root, o.path);
      setChanges({ ...changes(), [key(o.root, o.path)]: c });
    } catch {
      /* not a git checkout: no marks */
    }
    paintMarks();
    if (diff()) showDiff();
  }

  function paintMarks() {
    if (!editor || !monaco) return;
    const c = curChanges();
    const m = monaco;
    const cs = getComputedStyle(document.documentElement);
    const ruler = (v: string) => ({ color: cs.getPropertyValue(v).trim(), position: m.editor.OverviewRulerLane.Left });
    const line = (a: number, b: number, cls: string, v: string): M.editor.IModelDeltaDecoration => ({
      range: new m.Range(a, 1, b, 1),
      options: { isWholeLine: true, linesDecorationsClassName: `gut ${cls}`, overviewRuler: ruler(v) },
    });
    const deco = c
      ? [
          ...c.added.map(([a, b]) => line(a, b, "gut-add", "--color-done")),
          ...c.changed.map(([a, b]) => line(a, b, "gut-mod", "--color-run")),
          ...c.removed.map((a) => line(a, a, "gut-del", "--color-fail")),
        ]
      : [];
    marks ??= editor.createDecorationsCollection();
    marks.set(deco);
  }

  /** F7 / ⇧F7: the next or previous change in the file. */
  function nextChange(back: boolean) {
    const c = curChanges();
    if (!c || !editor) return;
    const starts = [...c.added.map(([a]) => a), ...c.changed.map(([a]) => a), ...c.removed].sort((a, b) => a - b);
    if (!starts.length) return;
    const line = editor.getPosition()?.lineNumber ?? 1;
    const to = back ? ([...starts].reverse().find((l) => l < line) ?? starts.at(-1)!) : (starts.find((l) => l > line) ?? starts[0]!);
    editor.setPosition({ lineNumber: to, column: 1 });
    editor.revealLineInCenter(to);
  }

  function showDiff() {
    const o = cur();
    const c = curChanges();
    if (!monaco || !o?.model || !c) return;
    const m = monaco;
    if (!diffEditor) {
      diffEditor = m.editor.createDiffEditor(diffHost, {
        theme: themeName(),
        fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim(),
        fontSize: 12.5,
        lineHeight: 20,
        automaticLayout: true,
        renderSideBySide: true,
        useInlineViewWhenSpaceIsLimited: true,
        renderSideBySideInlineBreakpoint: 900,
        originalEditable: false,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        renderOverviewRuler: false,
        hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 4, revealLineCount: 10 },
        padding: { top: 8 },
      });
      diffEditor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => void save());
    }
    baseModel?.dispose();
    baseModel = m.editor.createModel(c.original ?? "", o.model.getLanguageId());
    diffEditor.setModel({ original: baseModel, modified: o.model });
  }

  createEffect(
    on(
      diff,
      (shown) => {
        if (shown) requestAnimationFrame(showDiff);
        else {
          diffEditor?.setModel(null);
          const o = cur();
          if (editor && o) {
            editor.setModel(o.model);
            if (o.view) editor.restoreViewState(o.view);
            editor.focus();
          }
        }
      },
      { defer: true },
    ),
  );

  // ------------------------------------------------ Monaco

  onMount(async () => {
    const m = await loadMonaco();
    monaco = m;
    const mono = getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim();
    editor = m.editor.create(host, {
      model: null,
      theme: themeName(),
      fontFamily: mono,
      fontSize: 12.5,
      lineHeight: 20,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderLineHighlight: "line",
      smoothScrolling: true,
      automaticLayout: true,
      padding: { top: 8 },
      fixedOverflowWidgets: true,
      lineDecorationsWidth: 10,
      glyphMargin: false,
      overviewRulerBorder: false,
      stickyScroll: { enabled: true, maxLineCount: 3 },
      guides: { indentation: true, bracketPairs: false },
      renderWhitespace: "selection",
      "semanticHighlighting.enabled": true,
    });
    editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => void save());
    markersSub = m.editor.onDidChangeMarkers(() => countProblems());
    editor.addCommand(m.KeyCode.F7, () => nextChange(false));
    editor.addCommand(m.KeyMod.Shift | m.KeyCode.F7, () => nextChange(true));
    // Go to definition (and peek, references) into another file opens it here, like a tab.
    openHere = (r, p, line, col) => void open(p, r, line, col);
    if (!openerSet) {
      openerSet = true;
      m.editor.registerEditorOpener({
        openCodeEditor: (_src, resource, at) => openFromMonaco(resource, at && "startLineNumber" in at ? { lineNumber: at.startLineNumber, column: at.startColumn } : at),
      });
    }
    // Files kept from the last session, or opened before Monaco loaded, get their models now.
    if (!opened().length) {
      try {
        const s = JSON.parse(localStorage.getItem(SAVED) ?? "null") as { open: [string, string][]; current: string | null } | null;
        for (const [r, p] of s?.open ?? []) if (r === root()) await open(p, r);
        if (s?.current && opened().some((o) => key(o.root, o.path) === s.current)) select(s.current);
      } catch {
        /* nothing kept */
      }
    }
    for (const o of opened()) {
      if (!o.model && !o.refused) {
        const f = await files.read(o.root, o.path);
        o.model = m.editor.createModel(f.text, undefined, m.Uri.file(`${o.root}/${o.path}`));
        o.saved = o.model.getAlternativeVersionId();
      }
    }
    setReady(true);
    const o = cur();
    if (o) select(key(o.root, o.path));
    if (pending) {
      const p = pending;
      pending = null;
      void open(p.path, p.root, p.line, p.col);
    }
  });
  onCleanup(() => {
    const o = cur();
    if (o && editor && !diff()) o.view = editor.saveViewState();
    openHere = null;
    marks = null;
    markersSub?.dispose();
    editor?.dispose(); // the models stay: they hold your edits
    diffEditor?.dispose();
    baseModel?.dispose();
  });

  // Requests from other screens while this one is open.
  const onRequest = () => {
    if (pending && monaco) {
      const p = pending;
      pending = null;
      void open(p.path, p.root, p.line, p.col);
    }
  };
  window.addEventListener("pitlane-code-open", onRequest);
  onCleanup(() => window.removeEventListener("pitlane-code-open", onRequest));
  // Coming back to the window: an agent may have changed the file on disk.
  const onFocus = () => void loadChanges();
  window.addEventListener("focus", onFocus);
  onCleanup(() => window.removeEventListener("focus", onFocus));

  // VS Code's keys, taken before Monaco sees them.
  const onKey = (e: KeyboardEvent) => {
    if (!e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key.toLowerCase();
    const take = () => (e.preventDefault(), e.stopPropagation());
    if (k === "p" && !e.shiftKey) {
      take();
      setQuick(true);
    } else if (k === "f" && e.shiftKey) {
      take();
      setSide("search");
      setSearchFocus(searchFocus() + 1);
    } else if (k === "e" && e.shiftKey) {
      take();
      setSide("files");
    } else if (k === "b" && !e.shiftKey) {
      take();
      setSide(side() ? null : "files");
    } else if (k === "w" && !e.shiftKey) {
      take();
      const o = cur();
      if (o) close(o);
    } else if (k === "t" && e.shiftKey) {
      take();
      const c = closed.pop();
      if (c) void open(c.path, c.root);
    }
  };
  window.addEventListener("keydown", onKey, true);
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  // Changing the place shows that place's open files.
  createEffect(
    on(
      root,
      (r) => {
        const last = opened().filter((o) => o.root === r).at(-1);
        if (last) {
          if (current() !== key(last.root, last.path) && cur()?.root !== r) select(key(last.root, last.path));
        } else {
          setCurrent(null);
          setDiff(false);
          editor?.setModel(null);
        }
      },
      { defer: true },
    ),
  );

  const isIgnoredDir = (p: string) => (ignored() ?? []).includes(`${p}/`) || Object.values(inside()).flat().includes(`${p}/`);
  const toggle = (p: string) => {
    const s = new Set(openDirs());
    if (s.has(p)) s.delete(p);
    else {
      s.add(p);
      if (isIgnoredDir(p) && !inside()[p]) {
        const r = root();
        void files.dir(r, p).then(
          (names) => root() === r && setInside({ ...inside(), [p]: names.map((n) => `${p}/${n}`) }),
          (e) => toast({ tone: "fail", title: `Could not read ${p}`, body: msg(e) }),
        );
      }
    }
    setOpenDirs(s);
  };

  return (
    <div class="code-screen" data-side={side() ? "" : undefined}>
      <Show when={side()}>
        <aside class="code-side" aria-label={side() === "search" ? "Search" : "Files"}>
          <div class="code-side-head">
            <PlacePicker places={places()} root={root()} repo={props.repo} onPick={setRootSig} />
            <button type="button" class="btn btn-ghost btn-icon" aria-label="Open in VS Code" title="VS Code, with extensions" onClick={() => props.onVscode()}>
              <Icon name="code" small />
            </button>
            <button type="button" class="btn btn-ghost btn-icon" aria-label="Files" title="Files (⇧⌘E)" aria-pressed={side() === "files"} onClick={() => setSide("files")}>
              <Icon name="folder" small />
            </button>
            <button
              type="button"
              class="btn btn-ghost btn-icon"
              aria-label="Search in files"
              title="Search in files (⇧⌘F)"
              aria-pressed={side() === "search"}
              onClick={() => (setSide("search"), setSearchFocus(searchFocus() + 1))}
            >
              <Icon name="search" small />
            </button>
          </div>
          <Show
            when={side() === "search"}
            fallback={
              <div class="code-tree" role="tree">
                <Show when={!list.loading} fallback={<p class="loading-line code-none"><Spinner /> Reading the files</p>}>
                  <TreeNodes nodes={nodes()} depth={0} open={openDirs()} toggle={toggle} pick={(p) => void open(p)} current={cur()?.path ?? null} changed={changedFiles()} />
                </Show>
              </div>
            }
          >
            <SearchPanel root={root()} focus={searchFocus()} onPick={(p, line, col) => void open(p, root(), line, col)} />
          </Show>
        </aside>
      </Show>
      <section class="code-main" aria-label="Editor">
        <div class="term-tabs code-tabs" role="tablist" aria-label="Open files">
          <For each={tabs()}>
            {(o) => {
              const k = key(o.root, o.path);
              return (
                <div class="term-tab" role="presentation" data-active={current() === k ? "" : undefined}>
                  <button type="button" role="tab" class="term-tab-main" aria-selected={current() === k} title={o.path} onClick={() => select(k)} onAuxClick={(e) => e.button === 1 && close(o)}>
                    <span class="term-tab-label">{o.path.split("/").pop()}</span>
                    <Show when={dirty().has(k)}>
                      <span class="code-unsaved">unsaved</span>
                    </Show>
                  </button>
                  <button type="button" class="term-tab-close" aria-label={`Close ${o.path}`} title="Close (⌘W)" onClick={() => close(o)}>
                    <Icon name="close" small />
                  </button>
                </div>
              );
            }}
          </For>
          <span class="spacer" />
          <Show when={cur()}>
            {(o) => (
              <>
                <span class="code-path selectable" title={`${o().root}/${o().path}`}>
                  {o().path}
                </span>
                <Show when={problems().errors + problems().warnings > 0}>
                  <button
                    type="button"
                    class="btn btn-ghost code-problems"
                    data-errors={problems().errors > 0 ? "" : undefined}
                    title="Go to the next problem (F8)"
                    onClick={() => (editor?.focus(), editor?.trigger("pitlane", "editor.action.marker.next", null))}
                  >
                    <Icon name={problems().errors ? "fail" : "needs"} small />
                    {problems().errors + problems().warnings} {problems().errors + problems().warnings === 1 ? "problem" : "problems"}
                  </button>
                </Show>
                <Show when={lsp()}>
                  {(st) => {
                    const lang = () => familyName(familyOf(o().path) ?? "");
                    return (
                      <Show
                        when={st().s === "missing"}
                        fallback={
                          <span class="code-lsp" data-s={st().s} title={st().s === "failed" ? (st() as { why: string }).why : st().s === "ready" ? `${lang()} through ${(st() as { name: string }).name}: definitions, references, types and problems across the project` : undefined}>
                            <Show when={st().s === "starting" || st().s === "installing"}>
                              <Spinner />
                            </Show>
                            {st().s === "ready" ? (st() as { name: string }).name : st().s === "starting" ? `Starting ${lang()}` : st().s === "installing" ? `Installing ${lang()} support` : `${lang()} support off`}
                          </span>
                        }
                      >
                        <button type="button" class="btn btn-ghost code-lsp-add" title={(st() as { hint: string }).hint} onClick={() => void install()}>
                          Add {lang()} support
                        </button>
                      </Show>
                    );
                  }}
                </Show>
                <Show when={changeCount() > 0}>
                  <button
                    type="button"
                    class="btn btn-ghost code-changes"
                    aria-pressed={diff()}
                    title={`Compared with ${curChanges()?.base ?? "the base"}, where this lane left production. F7 moves between changes.`}
                    onClick={() => setDiff(!diff())}
                  >
                    {diff() ? "Back to the file" : `${changeCount()} ${changeCount() === 1 ? "change" : "changes"}`}
                  </button>
                </Show>
              </>
            )}
          </Show>
        </div>
        <div class="code-stage">
          <div class="code-editor" ref={host} hidden={!cur() || !!cur()?.refused || diff()} />
          <div class="code-editor" ref={diffHost} hidden={!diff()} />
          <Show when={cur()?.refused}>
            {(why) => (
              <div class="code-empty">
                <div class="code-empty-title">Not shown here</div>
                <p class="code-empty-body">
                  {cur()!.path} is {why()}.
                </p>
              </div>
            )}
          </Show>
          <Show when={!cur()}>
            <div class="code-empty">
              <div class="code-empty-title">{ready() ? "No file open" : "Loading the editor"}</div>
              <p class="code-empty-body">Open one from the tree, or by name.</p>
              <p class="terms-empty-keys">
                <span class="kbd">⌘P</span> open <span class="kbd">⇧⌘F</span> search <span class="kbd">⌘B</span> sidebar
              </p>
            </div>
          </Show>
        </div>
      </section>
      <Show when={quick()}>
        <QuickOpen list={list() ?? []} onClose={() => (setQuick(false), editor?.focus())} onPick={(p, line) => void open(p, root(), line)} />
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------- VS Code in the tab

const MODE = "pitlane.code.mode";
const [mode, setModeSig] = createSignal<"vscode" | "light">((() => {
  try {
    return localStorage.getItem(MODE) === "light" ? "light" : "vscode";
  } catch {
    return "vscode";
  }
})());
function setMode(m: "vscode" | "light") {
  setModeSig(m);
  try {
    localStorage.setItem(MODE, m);
  } catch {
    /* no storage */
  }
}

/** The Code tab: VS Code (code-server) when it is installed and chosen, else the light editor. */
export function CodeScreen(props: { repo: string; repoName: string; lanes: Lane[] }) {
  const [status, { refetch }] = createResource(() => vscode.status().catch(() => ({ installed: false, version: null, running: false }) as VscodeStatus));
  return (
    <Show when={mode() === "vscode" && status()} fallback={<LightEditor {...props} onVscode={() => setMode("vscode")} />}>
      {(st) => (
        <Show when={st().installed} fallback={<VscodeInstall onDone={() => void refetch()} onLight={() => setMode("light")} />}>
          <VscodeView {...props} onLight={() => setMode("light")} />
        </Show>
      )}
    </Show>
  );
}

function VscodeInstall(props: { onDone: () => void; onLight: () => void }) {
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const install = async () => {
    setBusy(true);
    setErr(null);
    try {
      await vscode.install();
      props.onDone();
    } catch (e) {
      setErr(msg(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="code-empty vs-install">
      <div class="code-empty-title">VS Code, inside Pitlane</div>
      <p class="code-empty-body">
        The full editor, with extensions, on the repository or any lane’s worktree. It runs code-server, the open-source VS Code by Coder: about 235 MB, downloaded once into
        ~/.pitlane/code-server. Extensions come from Open VSX.
      </p>
      <div class="vs-actions">
        <button type="button" class="btn btn-primary" disabled={busy()} aria-busy={busy()} onClick={() => void install()}>
          <Show when={busy()} fallback={<Icon name="code" small />}>
            <Spinner />
          </Show>
          {busy() ? "Downloading code-server" : "Install VS Code"}
        </button>
        <button type="button" class="btn btn-ghost" onClick={props.onLight}>
          Use the light editor
        </button>
      </div>
      <Show when={err()}>
        <p class="vs-error">{err()}</p>
      </Show>
    </div>
  );
}

function VscodeView(props: { repo: string; repoName: string; lanes: Lane[]; onLight: () => void }) {
  let host!: HTMLDivElement;
  const places = () => [
    { root: props.repo, label: props.repoName },
    ...props.lanes.filter((l) => l.worktree && !l.worktree.is_main).map((l) => ({ root: l.worktree!.path, label: l.branch })),
  ];
  const root = () => rootSig() ?? props.repo;
  const [state, setState] = createSignal<"starting" | "ready" | "failed">("starting");
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
      await vscode.show(root(), rect());
      if (alive) setState("ready");
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
    void place().then(openPending);
    const ro = new ResizeObserver(() => void place());
    ro.observe(host);
    // App menus, sheets and dialogs draw in the page, under VS Code's own view: step aside for them.
    const overlay = () => !!document.querySelector(".pop, dialog[open], .qo-scrim");
    const mo = new MutationObserver(() => {
      const now = overlay();
      if (now === covered) return;
      covered = now;
      if (covered) void vscode.hide().catch(() => {});
      else void place();
    });
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["open"] });
    const onRequest = () => void openPending();
    window.addEventListener("pitlane-code-open", onRequest);
    onCleanup(() => {
      alive = false;
      ro.disconnect();
      mo.disconnect();
      window.removeEventListener("pitlane-code-open", onRequest);
      void vscode.hide().catch(() => {});
    });
  });
  createEffect(on(root, () => void place(), { defer: true }));

  return (
    <div class="vs-screen">
      <div class="vs-bar">
        <PlacePicker places={places()} root={root()} repo={props.repo} onPick={setRootSig} />
        <span class="spacer" />
        <Show when={state() === "starting"}>
          <span class="code-lsp">
            <Spinner /> Starting VS Code
          </span>
        </Show>
        <button type="button" class="btn btn-ghost vs-switch" title="Pitlane's own editor: lighter, with the lane's changes in the margin" onClick={props.onLight}>
          Light editor
        </button>
      </div>
      <div class="vs-host" ref={host}>
        <Show when={state() === "failed"}>
          <div class="code-empty">
            <div class="code-empty-title">VS Code did not start</div>
            <p class="code-empty-body selectable">{why()}</p>
            <div class="vs-actions">
              <button type="button" class="btn" onClick={() => (setState("starting"), void place())}>
                Try again
              </button>
              <button type="button" class="btn btn-ghost" onClick={props.onLight}>
                Use the light editor
              </button>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}
