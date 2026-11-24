// Every Claude Code session Pitlane starts (runs, Run all's analyses, judges), live, grouped by
// card, with the subagents each one opens. Shown in the Terminal screen and in a dock on the
// Board. You can write to a session while it works and stop it with Ctrl+C. A session closes
// itself a moment after it ends, unless you talked to it.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, type RunMeta } from "./api";
import { ensureTimeline, timelineOf } from "./store/timelines";
import { agentGroups, agentSessions, agentsWanting, agentsWorking, closeAll, keep, kindLabel, lastLine, live, look, refreshAgents, selectAgent, selected, sessions, since, stopAllAgents, subagentsOf, type Look, type Subagent } from "./store/agents";
import { Icon, type IconName } from "./icons";
import { plural, t } from "./i18n";
import { toast } from "./ui";
import { Grip, registerSize, type Size } from "./layout";
import { AgentConsole } from "./console";
import { money, msg } from "./format";


function StopAll() {
  return (
    <Show when={agentSessions().some(live)}>
      <button type="button" class="pane-btn ag-stop-all" aria-label={t("Stop all")} data-tip={t("Stop every agent Pitlane started")} onClick={() => void stopAllAgents()}>
        <Icon name="stop" small />
      </button>
    </Show>
  );
}

// ---------------------------------------------------------------- the list in the Terminal screen

/** A session's state as one colored mark (a spinner while it works); the word shows on hover. */
function StateMark(props: { s: Look["s"] | Subagent["state"]; icon: IconName; word: string }) {
  return (
    <span class="ag-state" data-s={props.s} data-tip={props.word} aria-label={props.word} role="img">
      <Show when={props.s === "work"} fallback={<Icon name={props.icon} small />}>
        <span class="ag-spin" />
      </Show>
    </span>
  );
}

const SUB_LOOK: Record<Subagent["state"], { icon: IconName; word: () => string }> = {
  work: { icon: "run", word: () => t("Working") },
  done: { icon: "done", word: () => t("Done") },
  fail: { icon: "fail", word: () => t("Failed") },
  ended: { icon: "stop", word: () => t("Ended") },
};

export function AgentList(props: { compact: boolean }) {
  return (
    <Show when={agentGroups().length > 0}>
      <Show
        when={!props.compact}
        fallback={
          <For each={agentSessions()}>
            {(m) => (
              <div class="tt ag-chip" data-active={selected()?.id === m.id ? "" : undefined}>
                <button type="button" role="tab" class="tt-main" aria-selected={selected()?.id === m.id} onClick={() => selectAgent(m.id)}>
                  <StateMark s={look(m).s} icon={look(m).icon} word={look(m).word} />
                  <span class="tt-name">
                    #{m.card} {kindLabel(m)}
                  </span>
                </button>
              </div>
            )}
          </For>
        }
      >
        <div class="ag-list" role="tablist" aria-label={t("Agents")} aria-orientation="vertical">
          <div class="ag-head">
            <span class="terms-title">{t("Agents")}</span>
            <Show when={agentsWorking() > 0}>
              <span class="ag-working" data-tip={t("{n} working", { n: agentsWorking() })}>
                <span class="ag-spin" /> {agentsWorking()}
              </span>
            </Show>
            <span class="ag-bar-fill" />
            <CloseEnded />
            <StopAll />
          </div>
          {/* Keyed by card number and by subagent id, not by the objects rebuilt on every refresh:
              rows keep their DOM while agents work, so a click is never lost to a re-render. */}
          <For each={agentGroups().map((g) => g.card)}>
            {(card) => {
              const g = () => agentGroups().find((x) => x.card === card);
              return (
                <Show when={g()}>
                  {(g) => (
                    <div class="ag-group">
                      <div class="ag-card" title={g().title}>
                        <Show when={card > 0}>
                          <span class="ag-num">#{card}</span>
                        </Show>
                        <span class="ag-title">{g().title}</span>
                      </div>
                      <For each={g().sessions}>
                        {(m) => (
                          <>
                            <SessionRow m={m} />
                            <For each={subagentsOf(m).map((s) => s.id)}>
                              {(sid) => {
                                const s = () => subagentsOf(m).find((x) => x.id === sid);
                                return <Show when={s()}>{(s) => <SubRow m={m} s={s()} />}</Show>;
                              }}
                            </For>
                          </>
                        )}
                      </For>
                    </div>
                  )}
                </Show>
              );
            }}
          </For>
        </div>
      </Show>
    </Show>
  );
}

function SessionRow(props: { m: RunMeta }) {
  const l = () => look(props.m);
  const active = () => selected()?.id === props.m.id && !selected()?.sub;
  onMount(() => void ensureTimeline(props.m.id));
  return (
    <div class="tt ag-row" data-active={active() ? "" : undefined} data-ended={live(props.m) ? undefined : ""}>
      <button type="button" role="tab" class="tt-main" aria-selected={active()} onClick={() => selectAgent(props.m.id)}>
        <StateMark s={l().s} icon={l().icon} word={l().word} />
        <span class="tt-text">
          <span class="ag-row-head">
            <span class="tt-name">{kindLabel(props.m)}</span>
            <span class="ag-time">{since(props.m)}</span>
          </span>
          <span class="tt-sub">{lastLine(props.m)}</span>
        </span>
      </button>
      <Show when={!live(props.m)}>
        <button type="button" class="tt-close" aria-label={t("Close this session")} title={t("Close")} onClick={() => close(props.m.id)}>
          <Icon name="close" small />
        </button>
      </Show>
    </div>
  );
}

function SubRow(props: { m: RunMeta; s: Subagent }) {
  const active = () => selected()?.id === props.m.id && selected()?.sub === props.s.id;
  return (
    <div class="tt ag-row ag-sub" style={{ "--depth": String(props.s.depth) }} data-active={active() ? "" : undefined}>
      <button type="button" role="tab" class="tt-main" aria-selected={active()} onClick={() => selectAgent(props.m.id, props.s.id)}>
        <StateMark s={props.s.state} icon={SUB_LOOK[props.s.state].icon} word={SUB_LOOK[props.s.state].word()} />
        <span class="tt-text">
          <span class="tt-name ag-sub-name">{props.s.label}</span>
        </span>
      </button>
    </div>
  );
}

function close(id: string) {
  closeAll([id]);
}

/** Close every session that ended, in one go. */
function CloseEnded() {
  const ended = () => agentSessions().filter((m) => !live(m));
  return (
    <Show when={ended().length > 0}>
      <button type="button" class="pane-btn" aria-label={t("Close ended")} data-tip={t("Close every session that ended")} onClick={() => closeAll(ended().map((m) => m.id))}>
        <Icon name="close" small />
      </button>
    </Show>
  );
}

// ---------------------------------------------------------------- one session, as a terminal

export function AgentPane(props: { onOpenRun: (id: string) => void; onContinue?: (id: string) => Promise<boolean> }) {
  const m = () => sessions().find((x) => x.id === selected()?.id) ?? null;
  // Continue in Claude Code. A live session is stopped first (and asked once): two Claude Codes
  // must never write the same session.
  const [taking, setTaking] = createSignal<"ask" | "stopping" | null>(null);
  let askTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(askTimer));
  createEffect(on(() => selected()?.id, () => setTaking(null)));
  const takeOver = async () => {
    const mm = m();
    if (!mm || !props.onContinue || taking() === "stopping") return;
    if (live(mm) && taking() !== "ask") {
      setTaking("ask");
      clearTimeout(askTimer);
      askTimer = setTimeout(() => taking() === "ask" && setTaking(null), 5000);
      return;
    }
    const id = mm.id;
    if (live(mm)) {
      setTaking("stopping");
      try {
        await api.runStop(id);
      } catch (e) {
        setTaking(null);
        toast({ tone: "fail", title: t("Could not stop it"), body: msg(e) });
        return;
      }
      const until = Date.now() + 20_000;
      while (Date.now() < until && sessions().some((x) => x.id === id && live(x))) {
        await new Promise((r) => setTimeout(r, 300));
        await refreshAgents().catch(() => {});
      }
      if (sessions().some((x) => x.id === id && live(x))) {
        setTaking(null);
        toast({ tone: "fail", title: t("It is still stopping"), body: t("Try again once the agent has stopped.") });
        return;
      }
    }
    setTaking(null);
    keep(id, true);
    await props.onContinue(id);
  };

  return (
    <Show when={m()}>
      {(mm) => (
        <div class="ag-pane">
          <div class="ag-bar">
            <span class="ag-dot" data-s={look(mm()).s} />
            <span class="ag-bar-title">
              <span class="ag-num">#{mm().card}</span> {mm().card_title}
            </span>
            <span class="ag-bar-sep">·</span>
            <span class="ag-bar-kind">{kindLabel(mm())}</span>
            <span class="ag-bar-meta">
              {[since(mm()), money(mm().cost_usd)].filter(Boolean).join("  ·  ")}
            </span>
            <span class="ag-bar-fill" />
            <Show when={props.onContinue && mm().session_id}>
              <button
                type="button"
                class="btn btn-ghost"
                data-confirm={taking() === "ask" ? "" : undefined}
                disabled={taking() === "stopping"}
                title={live(mm()) ? t("Stop the agent here and go on with this session in Claude Code, in a terminal tab") : t("Resume this session in Claude Code, in a terminal tab")}
                onClick={() => void takeOver()}
              >
                <Icon name="spark" small />{" "}
                {taking() === "ask" ? t("Stop it and continue?") : taking() === "stopping" ? t("Stopping…") : t("Continue in Claude Code")}
              </button>
            </Show>
            <Show when={(mm().kind ?? "run") === "run"}>
              <button type="button" class="btn btn-ghost" onClick={() => props.onOpenRun(mm().id)}>
                {t("Open run")}
              </button>
            </Show>
            <Show
              when={live(mm())}
              fallback={
                <button type="button" class="btn btn-ghost" onClick={() => close(mm().id)}>
                  {t("Close")}
                </button>
              }
            >
              <button type="button" class="btn btn-ghost" title={t("Stop this agent (Ctrl+C)")} onClick={() => void api.runStop(mm().id).catch((e) => toast({ tone: "fail", title: t("Could not stop it"), body: msg(e) }))}>
                <Icon name="stop" small /> {t("Stop")}
              </button>
            </Show>
          </div>
          <AgentConsole
            m={mm()}
            events={timelineOf(mm().id) ?? []}
            sub={selected()?.sub ?? null}
            onSub={(s) => selectAgent(mm().id, s)}
            kind={kindLabel(mm())}
            ended={look(mm()).word}
            onTalked={() => (keep(mm().id, true), void refreshAgents())}
            onResumed={(next) => {
              keep(next.id, true);
              void refreshAgents().then(() => selectAgent(next.id));
            }}
          />
        </div>
      )}
    </Show>
  );
}

// ---------------------------------------------------------------- the dock on the Board

const DOCK_KEY = "pitlane.agents.dock";
const [dockOpen, setDockOpen] = createSignal(readDock());
function readDock(): boolean {
  try {
    return localStorage.getItem(DOCK_KEY) !== "closed";
  } catch {
    return true;
  }
}

const DOCK_H_KEY = "pitlane.agents.dock.h";
const DOCK_H = { min: 64, def: 132 };
const dockMax = () => Math.round(window.innerHeight * 0.6);
const [dockH, setDockH] = createSignal(readDockH());
registerSize(DOCK_H_KEY, () => setDockH(DOCK_H.def));
function readDockH(): number {
  try {
    const v = Number(localStorage.getItem(DOCK_H_KEY));
    return Number.isFinite(v) && v >= DOCK_H.min ? v : DOCK_H.def;
  } catch {
    return DOCK_H.def;
  }
}
function saveDockH(h: number) {
  const v = Math.round(Math.max(DOCK_H.min, Math.min(dockMax(), h)));
  setDockH(v);
  try {
    localStorage.setItem(DOCK_H_KEY, String(v));
  } catch {
    /* fine */
  }
}

/** The dock's height as the divider on its top edge sets it. */
const DOCK_SIZE: Size = { get: dockH, set: saveDockH, reset: () => saveDockH(DOCK_H.def), min: DOCK_H.min, max: dockMax, def: () => DOCK_H.def };

export function AgentDock(props: { onOpen: (id: string) => void; onOpenAll: () => void }) {
  const toggle = () => {
    setDockOpen(!dockOpen());
    try {
      localStorage.setItem(DOCK_KEY, dockOpen() ? "open" : "closed");
    } catch {
      /* fine */
    }
  };
  return (
    <Show when={agentSessions().length > 0}>
      <section class="ag-dock" aria-label={t("Agents")} data-open={dockOpen() ? "" : undefined}>
        <Grip size={DOCK_SIZE} edge="top" label={t("Resize the agents dock")} class="ag-dock-grip" cursor="ns-resize" bare onGrab={() => !dockOpen() && setDockOpen(true)} />
        <div class="ag-dock-head">
          <button type="button" class="ag-dock-toggle" aria-expanded={dockOpen()} onClick={toggle}>
            <Icon name="chev" small class={dockOpen() ? "ag-chev-open" : undefined} />
            <span class="terms-title">{t("Agents")}</span>
            <span class="ag-working">
              {agentsWorking() > 0 ? t("{n} working", { n: agentsWorking() }) : t("all ended")}
              {agentsWanting() > 0 ? ` · ${t("{n} need you", { n: agentsWanting() })}` : ""}
            </span>
          </button>
          <span class="ag-bar-fill" />
          <CloseEnded />
          <StopAll />
          <button type="button" class="btn btn-ghost" onClick={props.onOpenAll}>
            <Icon name="term" small /> {t("Open in Terminal")}
          </button>
        </div>
        <Show when={dockOpen()}>
          <div class="ag-dock-list" style={{ height: `${Math.min(dockH(), dockMax())}px` }}>
            <For each={agentSessions()}>
              {(m) => {
                onMount(() => void ensureTimeline(m.id));
                const subs = () => subagentsOf(m).filter((s) => s.state === "work").length;
                return (
                  <button type="button" class="ag-cell" data-ended={live(m) ? undefined : ""} onClick={() => props.onOpen(m.id)}>
                    <span class="ag-cell-head">
                      <StateMark s={look(m).s} icon={look(m).icon} word={look(m).word} />
                      <span class="ag-num">#{m.card}</span>
                      <span class="ag-cell-kind">{kindLabel(m)}</span>
                      <span class="ag-bar-fill" />
                    </span>
                    <span class="ag-cell-title">{m.card_title}</span>
                    <span class="ag-cell-last">
                      {subs() > 0 ? `${plural(subs(), "{n} subagent", "{n} subagents")} · ` : ""}
                      {lastLine(m)}
                    </span>
                  </button>
                );
              }}
            </For>
          </div>
        </Show>
      </section>
    </Show>
  );
}
