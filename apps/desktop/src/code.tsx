// Code: the files of the repository or of a lane's worktree, in Monaco. Open files keep their
// models (and unsaved edits) while you move between screens; ⌘S saves, ⌘P finds a file.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import type * as M from "monaco-editor";
import { files, type Lane } from "./api";
import { Icon } from "./icons";
import { loadMonaco, themeName } from "./monaco";
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
const key = (root: string, path: string) => `${root}\0${path}`;
let pending: { root: string; path: string } | null = null;

/** Opens `path` under `root` in the Code screen (from Review, Run or elsewhere). */
export function openInCode(root: string, path: string) {
  setRootSig(root);
  pending = { root, path };
  window.dispatchEvent(new CustomEvent("pitlane-code-open"));
}

export const unsavedCount = () => dirty().size;

// ---------------------------------------------------------------- the tree

interface Node {
  name: string;
  path: string;
  dir: boolean;
  kids: Node[];
}

function tree(paths: string[]): Node[] {
  const root: Node = { name: "", path: "", dir: true, kids: [] };
  for (const p of paths) {
    const parts = p.split("/");
    let at = root;
    parts.forEach((name, i) => {
      const leaf = i === parts.length - 1;
      const path = parts.slice(0, i + 1).join("/");
      let n = at.kids.find((k) => k.name === name && k.dir === !leaf);
      if (!n) {
        n = { name, path, dir: !leaf, kids: [] };
        at.kids.push(n);
      }
      at = n;
    });
  }
  const sort = (n: Node) => {
    n.kids.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
    n.kids.forEach(sort);
  };
  sort(root);
  return root.kids;
}

function TreeNodes(props: { nodes: Node[]; depth: number; open: Set<string>; toggle: (p: string) => void; pick: (p: string) => void; current: string | null }) {
  return (
    <For each={props.nodes}>
      {(n) => (
        <>
          <button
            type="button"
            class="tree-row"
            style={{ "padding-inline-start": `${8 + props.depth * 12}px` }}
            data-current={!n.dir && props.current === n.path ? "" : undefined}
            aria-expanded={n.dir ? props.open.has(n.path) : undefined}
            title={n.path}
            onClick={() => (n.dir ? props.toggle(n.path) : props.pick(n.path))}
          >
            <Show when={n.dir} fallback={<span class="tree-gap" />}>
              <Icon name="chev" small class={props.open.has(n.path) ? "" : "chev-closed"} />
            </Show>
            <span class="tree-name">{n.name}</span>
          </button>
          <Show when={n.dir && props.open.has(n.path)}>
            <TreeNodes nodes={n.kids} depth={props.depth + 1} open={props.open} toggle={props.toggle} pick={props.pick} current={props.current} />
          </Show>
        </>
      )}
    </For>
  );
}

// ---------------------------------------------------------------- the screen

export function CodeScreen(props: { repo: string; repoName: string; lanes: Lane[] }) {
  const places = () => [
    { root: props.repo, label: props.repoName },
    ...props.lanes.filter((l) => l.worktree && !l.worktree.is_main).map((l) => ({ root: l.worktree!.path, label: l.branch })),
  ];
  const root = () => rootSig() ?? props.repo;
  const [list, { refetch }] = createResource(root, (r) => files.list(r));
  const [filter, setFilter] = createSignal("");
  const [openDirs, setOpenDirs] = createSignal<Set<string>>(new Set());
  const nodes = createMemo(() => tree(list() ?? []));
  const found = createMemo(() => {
    const q = filter().trim().toLowerCase();
    if (!q) return [];
    const words = q.split(/\s+/);
    return (list() ?? []).filter((p) => words.every((w) => p.toLowerCase().includes(w))).slice(0, 200);
  });

  let host!: HTMLDivElement;
  let filterEl!: HTMLInputElement;
  let monaco: typeof import("monaco-editor") | null = null;
  let editor: M.editor.IStandaloneCodeEditor | null = null;
  const [ready, setReady] = createSignal(false);

  const cur = () => opened().find((o) => key(o.root, o.path) === current()) ?? null;
  const tabs = () => opened().filter((o) => o.root === root());

  async function open(path: string, at = root()) {
    const k = key(at, path);
    if (!opened().some((o) => key(o.root, o.path) === k)) {
      try {
        const f = await files.read(at, path);
        const m = monaco;
        const model =
          f.refused || !m ? null : m.editor.createModel(f.text, undefined, m.Uri.from({ scheme: "file", path: `${at}/${path}` }));
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
    select(k);
  }

  function select(k: string) {
    const before = cur();
    if (before && editor) before.view = editor.saveViewState();
    setCurrent(k);
    const o = cur();
    if (editor) {
      editor.setModel(o?.model ?? null);
      if (o?.view) editor.restoreViewState(o.view);
      editor.focus();
    }
  }

  function close(o: Open) {
    const k = key(o.root, o.path);
    if (dirty().has(k) && !window.confirm(`${o.path} has unsaved changes. Close it anyway?`)) return;
    const rest = opened().filter((x) => x !== o);
    setOpened(rest);
    const d = new Set(dirty());
    d.delete(k);
    setDirty(d);
    if (current() === k) {
      const next = rest.filter((x) => x.root === o.root).at(-1);
      if (next) select(key(next.root, next.path));
      else {
        setCurrent(null);
        editor?.setModel(null);
      }
    }
    o.model?.dispose();
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
    } catch (e) {
      toast({ tone: "fail", title: `${o.path} was not saved`, body: msg(e) });
    }
  }

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
    });
    editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => void save());
    editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyP, () => filterEl.focus());
    // Files opened before Monaco loaded have no model yet.
    for (const o of opened()) {
      if (!o.model && !o.refused) {
        const f = await files.read(o.root, o.path);
        o.model = m.editor.createModel(f.text, undefined, m.Uri.from({ scheme: "file", path: `${o.root}/${o.path}` }));
        o.saved = o.model.getAlternativeVersionId();
      }
    }
    setReady(true);
    const o = cur();
    if (o) select(key(o.root, o.path));
    if (pending) {
      const p = pending;
      pending = null;
      void open(p.path, p.root);
    }
  });
  onCleanup(() => {
    const o = cur();
    if (o && editor) o.view = editor.saveViewState();
    editor?.dispose(); // the models stay: they hold your edits
  });

  // Requests from other screens while this one is open.
  const onRequest = () => {
    if (pending && monaco) {
      const p = pending;
      pending = null;
      void open(p.path, p.root);
    }
  };
  window.addEventListener("pitlane-code-open", onRequest);
  onCleanup(() => window.removeEventListener("pitlane-code-open", onRequest));

  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey && e.key.toLowerCase() === "p") {
      e.preventDefault();
      filterEl.focus();
    }
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  // Changing the place shows that place's open files.
  createEffect(
    on(root, (r) => {
      const mine = opened().filter((o) => o.root === r);
      const last = mine.at(-1);
      if (last) select(key(last.root, last.path));
      else {
        setCurrent(null);
        editor?.setModel(null);
      }
    }, { defer: true }),
  );

  const toggle = (p: string) => {
    const s = new Set(openDirs());
    if (s.has(p)) s.delete(p);
    else s.add(p);
    setOpenDirs(s);
  };

  return (
    <div class="code">
      <aside class="code-side" aria-label="Files">
        <div class="code-side-head">
          <select class="select code-place" aria-label="Folder" value={root()} onChange={(e) => setRootSig(e.currentTarget.value)}>
            <For each={places()}>{(p) => <option value={p.root}>{p.label}</option>}</For>
          </select>
          <button type="button" class="btn btn-ghost btn-icon" aria-label="Read the file list again" title="Read the file list again" onClick={() => void refetch()}>
            <Icon name="refresh" small />
          </button>
        </div>
        <input
          ref={filterEl}
          class="input code-filter"
          type="search"
          placeholder="Find a file (⌘P)"
          aria-label="Find a file"
          value={filter()}
          onInput={(e) => setFilter(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && found()[0]) {
              void open(found()[0]!);
              setFilter("");
            }
            if (e.key === "Escape") setFilter("");
          }}
        />
        <div class="code-tree" role="tree">
          <Show when={!list.loading} fallback={<p class="loading-line"><Spinner /> Reading the files</p>}>
            <Show
              when={filter().trim()}
              fallback={<TreeNodes nodes={nodes()} depth={0} open={openDirs()} toggle={toggle} pick={(p) => void open(p)} current={cur()?.path ?? null} />}
            >
              <For each={found()} fallback={<p class="side-hint code-none">No file matches.</p>}>
                {(p) => (
                  <button type="button" class="tree-row tree-found" title={p} onClick={() => (void open(p), setFilter(""))}>
                    <span class="tree-name">{p.split("/").pop()}</span>
                    <span class="tree-dir">{p.split("/").slice(0, -1).join("/")}</span>
                  </button>
                )}
              </For>
            </Show>
          </Show>
        </div>
      </aside>
      <section class="code-main" aria-label="Editor">
        <div class="term-tabs code-tabs" role="tablist" aria-label="Open files">
          <For each={tabs()}>
            {(o) => {
              const k = key(o.root, o.path);
              return (
                <div class="term-tab" role="presentation" data-active={current() === k ? "" : undefined}>
                  <button type="button" role="tab" class="term-tab-main" aria-selected={current() === k} title={o.path} onClick={() => select(k)}>
                    <span class="term-tab-label">{o.path.split("/").pop()}</span>
                    <Show when={dirty().has(k)}>
                      <span class="code-unsaved">unsaved</span>
                    </Show>
                  </button>
                  <button type="button" class="term-tab-close" aria-label={`Close ${o.path}`} onClick={() => close(o)}>
                    <Icon name="close" small />
                  </button>
                </div>
              );
            }}
          </For>
          <span class="spacer" />
          <Show when={cur()}>
            {(o) => <span class="code-path selectable">{o().path}</span>}
          </Show>
        </div>
        <div class="code-editor" ref={host} hidden={!cur() || !!cur()?.refused} />
        <Show when={cur()?.refused}>
          {(why) => (
            <div class="empty code-empty">
              <div class="empty-title">Not shown here</div>
              <p class="empty-body">
                {cur()!.path} is {why()}.
              </p>
            </div>
          )}
        </Show>
        <Show when={!cur()}>
          <div class="empty code-empty">
            <div class="empty-title">{ready() ? "No file open" : "Loading the editor"}</div>
            <p class="empty-body">Pick a file on the left, or ⌘P to find one by name. ⌘S saves.</p>
          </div>
        </Show>
      </section>
    </div>
  );
}
