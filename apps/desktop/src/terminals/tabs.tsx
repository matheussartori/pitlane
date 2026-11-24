// The tab list's rows: a tab's place, its status, renaming, closing, dragging.
import { Show, createSignal } from "solid-js";
import { agentSelected } from "../store/agents";
import { t } from "../i18n";
import { tilde } from "../format";
import { Icon, type IconName } from "../icons";
import { closeTab, type Meta, meta, panesOf, selectTab, setTab, st, type Status, type Tab } from "./model";
import { drag, startDrag } from "./drag";


function statusOf(tab: Tab): { s: Status; note: string } {
  const all = panesOf(tab.root).map((id) => meta[id]).filter(Boolean) as Meta[];
  for (const s of ["bell", "waiting", "busy", "exited"] as const) {
    const m = all.find((x) => x.status === s);
    if (m) return { s, note: m.note };
  }
  return { s: "idle", note: "" };
}

export const STATUS: Record<Exclude<Status, "idle">, { icon: string; word: string }> = {
  bell: { icon: "needs", word: "Needs you" },
  waiting: { icon: "needs", word: "Waiting" },
  busy: { icon: "run", word: "Working" },
  exited: { icon: "fail", word: "Exited" },
};

/** A tab's icon: an agent's session continued here, a lane's worktree, or a shell. */
function tabIcon(tab: Tab): IconName {
  const m = meta[tab.focus];
  return m?.run ? "spark" : tab.name || !m?.cwd.includes("/wt/") ? "term" : "branch";
}

/** A tab in the folded sidebar: its icon and a dot for its status; the rest shows on hover. */
export function TabMark(props: { tab: Tab; index: number }) {
  const m = () => meta[props.tab.focus];
  const name = () => props.tab.name || m()?.label || t("terminal");
  const status = () => statusOf(props.tab);
  const active = () => st.active === props.tab.id && !agentSelected();
  const tip = () => {
    const where = m() ? (m()!.title && m()!.title !== name() ? m()!.title : tilde(m()!.cwd)) : "";
    const s = status().s;
    const word = s === "idle" ? "" : t(STATUS[s].word);
    return [name(), where, [word, props.index < 9 ? `⌘${props.index + 1}` : ""].filter(Boolean).join(" · ")].filter(Boolean).join("\n");
  };
  return (
    <button type="button" role="tab" class="rail-btn" aria-selected={active()} data-active={active() ? "" : undefined} data-tip={tip()} aria-label={name()} onClick={() => selectTab(props.tab.id)}>
      <Icon name={tabIcon(props.tab)} small />
      <Show when={status().s !== "idle"}>
        <span class="rail-badge" data-s={status().s} />
      </Show>
    </button>
  );
}

export function TabRow(props: { tab: Tab; index: number }) {
  const [editing, setEditing] = createSignal(false);
  const focusMeta = () => meta[props.tab.focus];
  const name = () => props.tab.name || focusMeta()?.label || t("terminal");
  const sub = () => {
    const m = focusMeta();
    if (!m) return "";
    return m.title && m.title !== name() ? m.title : tilde(m.cwd);
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
          <Icon name={tabIcon(props.tab)} small />
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
