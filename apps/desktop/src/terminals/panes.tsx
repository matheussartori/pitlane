// A tab's panes on screen: one xterm each, the splits between them, and the find bar.
import { For, Match, Show, Switch, createSignal, onCleanup, onMount } from "solid-js";
import { produce } from "solid-js/store";
import { duration } from "../blocks";
import { t } from "../i18n";
import { Icon } from "../icons";
import { clock, closePane, findColours, focusPane, gpu, live, meta, type Node, type Pane, save, setSt, setTab, type Split, type Status, type Tab, tabOf } from "./model";
import { drag, startDrag } from "./drag";
import { STATUS } from "./tabs";

export function PaneView(props: { term: number; tab: Tab; alone: boolean }) {
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
      gpu(l);
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
      <div class="pane-body" onMouseMove={(e) => live.get(props.term)?.blocks.hover(e.clientY)} onMouseLeave={() => live.get(props.term)?.blocks.hover(null)}>
        <div class="pane-host" ref={host} />
        <Show when={live.get(props.term)?.blocks.sticky()}>
          {(s) => (
            <button type="button" class="tb-sticky" data-fail={s().exit ? "" : undefined} title={t("Back to this command")} onClick={() => (live.get(props.term)?.blocks.reveal(s().line), live.get(props.term)?.term.focus())}>
              <span class="tb-sticky-cmd">{s().cmd}</span>
              <span class="tb-sticky-info">{s().ms != null ? (s().exit ? `${t("exit")} ${s().exit} · ${duration(s().ms!)}` : duration(s().ms!)) : duration(clock() - s().started)}</span>
            </button>
          )}
        </Show>
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

export function NodeView(props: { node: Node; tab: Tab; path: number[] }) {
  return (
    <Switch>
      <Match when={props.node.kind === "pane" && (props.node as Pane)}>{(p) => <PaneView term={p().term} tab={props.tab} alone={props.path.length === 0} />}</Match>
      <Match when={props.node.kind === "split" && (props.node as Split)}>{(s) => <SplitView node={s()} tab={props.tab} path={props.path} />}</Match>
    </Switch>
  );
}

export function FindBar(props: { term: number; onClose: () => void }) {
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
