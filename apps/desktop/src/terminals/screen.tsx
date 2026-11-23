// The Terminal screen: the agents and the tab list beside the stage that shows the active tab.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import type { Lane } from "../api";
import { AgentList, AgentPane, agentGroups, agentSelected } from "../agents";
import { t } from "../i18n";
import { Icon } from "../icons";
import { dismissable } from "../ui";
import { Grip } from "../layout";
import { activeTab, AGENTS_H, closePane, equal, focusPane, fontSize, jumpToWanting, live, meta, neighbour, openTerminal, panesOf, resizeFocused, resumeInTerminal, save, screenOn, seen, selectTab, setRoots, setScreenOn, setSt, setTab, type Side, SIDE_W, split, st } from "./model";
import { drag } from "./drag";
import { TabRow } from "./tabs";
import { FindBar, NodeView, PaneView } from "./panes";

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
  createEffect(() => setRoots(places().map((p) => p.cwd)));
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
    } else if ((e.code === "ArrowUp" || e.code === "ArrowDown") && !e.shiftKey && !e.altKey && !e.ctrlKey && t) {
      // ⌘↑ / ⌘↓ walk the commands, as in Warp: the block is selected, so ⌘C copies it.
      if (live.get(t.focus)?.blocks.jump(e.code === "ArrowUp" ? -1 : 1)) take();
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
      fontSize(14);
    }
  }

  const closeFind = () => {
    setFinding(false);
    const t = tab();
    if (t) live.get(t.focus)?.term.focus();
  };

  return (
    <div
      class="terms"
      data-side={st.side}
      data-empty={st.tabs.length === 0 && agentGroups().length === 0 ? "" : undefined}
      style={st.side === "left" ? { "--side-w": `${SIDE_W.get()}px` } : undefined}
    >
      <Show when={st.tabs.length > 0 || agentGroups().length > 0}>
        <div class="terms-tabs" data-agents={agentGroups().length > 0 ? "" : undefined}>
          <div class="ag-side-wrap" style={st.side === "left" && st.tabs.length > 0 ? { height: `${AGENTS_H.get()}px` } : undefined}>
            <div class="ag-side">
              <AgentList compact={st.side === "top"} />
            </div>
            <Show when={st.side === "left" && st.tabs.length > 0}>
              <Grip size={AGENTS_H} edge="bottom" label={t("Resize agents and terminals")} />
            </Show>
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
          <Show when={st.side === "left"}>
            <Grip size={SIDE_W} edge="right" label={t("Resize the sidebar")} />
          </Show>
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
