// Dragging panes and tabs: where a drag would land, and the move once it does.
import { createSignal } from "solid-js";
import { produce, unwrap } from "solid-js/store";
import { insert, live, nextTabId, pane, panesOf, save, setSt, setTab, type Side, st, swap, tabOf, without } from "./model";

type Drag = { kind: "pane"; term: number } | { kind: "tab"; id: number };
type Drop = { kind: "pane"; term: number; side: Side | "center" } | { kind: "tab"; id: number; after: boolean } | { kind: "list" };
const [drag, setDrag] = createSignal<{ what: Drag; over: Drop | null } | null>(null);
export { drag };

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

export function startDrag(e: PointerEvent, what: Drag) {
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
        const id = nextTabId();
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
