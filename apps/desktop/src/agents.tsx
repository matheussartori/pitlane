// Every Claude Code session Pitlane starts (runs, Run all's analyses, judges), live, grouped by
// card, with the subagents each one opens. Shown in the Terminal screen and in a dock on the
// Board. You can write to a session while it works and stop it with Ctrl+C. A session closes
// itself a moment after it ends, unless you talked to it.
import { For, Show, createEffect, createMemo, createRoot, createSignal, on, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api, isLive, type RunEvent, type RunMeta } from "./api";
import { dropTimelines, ensureTimeline, resetTimelines, startTimelines, timelineOf } from "./store/timelines";
import { Icon, type IconName } from "./icons";
import { plural, t, tx } from "./i18n";
import { toast } from "./ui";
import { registerSize } from "./layout";
import { AgentConsole } from "./console";
import { msg } from "./format";


/** How long an ended session stays in view (so you see how it ended) before it closes. */
const LINGER_MS = 20_000;
const KEPT_KEY = "pitlane.agents.kept";

const [repo, setRepo] = createSignal<string | null>(null);
// A store reconciled by id: rows keep their DOM while the list refreshes.
const [store, setStore] = createStore<{ list: RunMeta[] }>({ list: [] });
const sessions = () => store.list;
const setSessions = (all: RunMeta[]) => setStore("list", reconcile(all, { key: "id" }));
const [kept, setKept] = createSignal<string[]>(readKept());
const [selected, setSelected] = createSignal<{ id: string; sub: string | null } | null>(null);
const [watching, setWatching] = createSignal(false);
const [now, setNow] = createSignal(Date.now());
/** Ended sessions you closed by hand. */
const [closed, setClosed] = createSignal<string[]>([]);

function readKept(): string[] {
  try {
    return JSON.parse(localStorage.getItem(KEPT_KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
}
function keep(id: string, on: boolean) {
  const next = on ? [...new Set([...kept(), id])] : kept().filter((x) => x !== id);
  setKept(next.slice(-50));
  try {
    localStorage.setItem(KEPT_KEY, JSON.stringify(kept()));
  } catch {
    /* private window: kept for this launch only */
  }
}

const live = (m: RunMeta) => isLive(m.state);
/** A live session the person opened that answered and now waits for their next message: its
 *  process is still there, but it is not working. */
const idle = (m: RunMeta) => live(m) && m.waiting.length === 0 && (timelineOf(m.id) ?? []).at(-1)?.kind === "finished";
const endedMs = (m: RunMeta) => (m.ended_at ?? m.started_at) * 1000;

/** What the Terminal screen and the dock show: live sessions, ones you talked to, the one you
 *  are looking at, and ones that ended a moment ago. Oldest first, so rows do not jump.
 *  Kept for the app's whole life, in a root of its own (a memo outside one is never owned). */
export const agentSessions = createRoot(() => createMemo(() => {
  const sel = selected()?.id;
  const k = kept();
  return sessions()
    .filter((m) => live(m) || (!closed().includes(m.id) && (k.includes(m.id) || m.id === sel || now() - endedMs(m) < LINGER_MS)))
    .sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));
}));

export interface AgentGroup {
  card: number;
  title: string;
  sessions: RunMeta[];
}

export const agentGroups = createRoot(() => createMemo<AgentGroup[]>(() => {
  const out: AgentGroup[] = [];
  for (const m of agentSessions()) {
    let g = out.find((x) => x.card === m.card);
    if (!g) out.push((g = { card: m.card, title: m.card_title, sessions: [] }));
    g.sessions.push(m);
  }
  return out;
}));

export const agentsWorking = () => agentSessions().filter((m) => live(m) && !idle(m)).length;
export const agentsWanting = () => agentSessions().filter((m) => live(m) && m.waiting.length > 0).length;
export const agentSelected = () => (selected() && agentSessions().some((m) => m.id === selected()!.id) ? selected() : null);
/** Stop all: every session the daemon runs, and analyses still waiting for a slot. */
export async function stopAllAgents(): Promise<void> {
  try {
    const n = await api.runsStopAll();
    toast({ tone: "info", title: n > 0 ? plural(n, "Stopping {n} agent", "Stopping {n} agents") : t("Nothing was running"), body: n > 0 ? t("Each ends its current step; anything still there after a few seconds is killed.") : undefined });
    void refreshAgents();
  } catch (e) {
    toast({ tone: "fail", title: t("Could not stop the agents"), body: msg(e) });
  }
}

function StopAll() {
  return (
    <Show when={agentSessions().some(live)}>
      <button type="button" class="pane-btn ag-stop-all" aria-label={t("Stop all")} data-tip={t("Stop every agent Pitlane started")} onClick={() => void stopAllAgents()}>
        <Icon name="stop" small />
      </button>
    </Show>
  );
}

export function selectAgent(id: string | null, sub: string | null = null) {
  setSelected(id ? { id, sub } : null);
  if (id) void ensureTimeline(id);
}
/** Run all is on: look for new sessions often, even before the first one shows up. */
export const setAgentsWatch = (on: boolean) => setWatching(on);

// ---------------------------------------------------------------- loading

let gen = 0;
export async function refreshAgents() {
  const path = repo();
  if (!path) return;
  const g = ++gen;
  try {
    const all = await api.sessions(path);
    if (g !== gen || path !== repo()) return;
    setSessions(all.slice(0, 200));
    // One you closed that came back (an assistant you wrote to again) is open again.
    const back = closed().filter((id) => all.some((m) => m.id === id && live(m)));
    if (back.length) setClosed(closed().filter((id) => !back.includes(id)));
    for (const m of all) {
      if (live(m)) {
        void ensureTimeline(m.id);
        void api.runFollow(m.id, 0).catch(() => {});
      }
    }
    // What left the list (ended a while ago, closed) gives its log back.
    const listed = new Set(agentSessions().map((m) => m.id));
    dropTimelines((id) => listed.has(id));
  } catch {
    /* no daemon yet, or the browser preview */
  }
}

export function setAgentsRepo(path: string | null) {
  if (path === repo()) return;
  setRepo(path);
  setSessions([]);
  setSelected(null);
  resetTimelines();
  void refreshAgents();
}

let started = false;
/** Once, at launch: listens to the daemon's events and keeps the list fresh. */
export async function startAgents() {
  if (started) return;
  started = true;
  // Preview: ?agent=<id> opens a session, for screenshots.
  if (import.meta.env.DEV) {
    const a = new URLSearchParams(location.search).get("agent");
    if (a) setTimeout(() => selectAgent(a), 300);
  }
  await startTimelines(
    (_run, e) => {
      if (e.kind === "approval" || e.kind === "decided" || e.kind === "started") void refreshAgents();
    },
    () => void refreshAgents(),
  );
  let last = 0;
  // One poll at a time: when the daemon answers slower than the pace (a loaded Mac), a new poll
  // would only drop the one on its way, and the list would never update.
  let polling = false;
  setInterval(() => {
    setNow(Date.now());
    const busy = watching() || sessions().some(live);
    const every = busy ? 1500 : 15_000;
    if (document.visibilityState === "visible" && !polling && Date.now() - last >= every) {
      last = Date.now();
      polling = true;
      void refreshAgents().finally(() => (polling = false));
    }
  }, 500);
}

// ---------------------------------------------------------------- reading a session

/** A session's kind in words (English keys; drawn through `tx(…, "kind")`). */
export const KIND: Record<string, string> = { run: "Run", analysis: "Analysis", judge: "Judge", finish: "Closing step", assist: "Assistant", aside: "Aside" };
export function kindLabel(m: RunMeta): string {
  const k = tx(KIND[m.kind ?? "run"] ?? "Run", "kind");
  return m.kind === "run" || !m.kind ? ((m.round ?? 0) > 0 ? t("{kind} · round {n}", { kind: k, n: (m.round ?? 0) + 1 }) : k) : k;
}

type Look = { icon: IconName; word: string; s: "work" | "needs" | "done" | "fail" | "stopped" };
export function look(m: RunMeta): Look {
  if (live(m) && m.waiting.length > 0) return { icon: "needs", word: t("Needs you"), s: "needs" };
  if (m.state === "verifying") return { icon: "verify", word: t("Checking"), s: "work" };
  // A session the person opened stays open between messages: answered, it waits for them.
  if (idle(m)) return { icon: "done", word: t("Waiting for you"), s: "done" };
  if (live(m)) return { icon: "run", word: t("Working"), s: "work" };
  if (m.state === "done") return { icon: "done", word: t("Done"), s: "done" };
  if (m.state === "stopped") return { icon: "stop", word: t("Stopped"), s: "stopped" };
  return { icon: "fail", word: m.state === "interrupted" ? t("Interrupted") : t("Failed"), s: "fail" };
}

const isSpawn = (e: RunEvent): e is Extract<RunEvent, { kind: "tool" }> => e.kind === "tool" && (e.name === "Task" || e.name === "Agent");

export interface Subagent {
  id: string;
  label: string;
  depth: number;
  state: "work" | "done" | "fail" | "ended";
}

/** The subagents a session opened (Task/Agent tool calls), nested by who opened them. */
export function subagentsOf(m: RunMeta): Subagent[] {
  const t = timelineOf(m.id) ?? [];
  const done = new Map<string, boolean>();
  for (const e of t) if (e.kind === "tool-done") done.set(e.id, e.ok);
  const parentOf = new Map<string, string | undefined>();
  const out: Subagent[] = [];
  for (const e of t) {
    if (!isSpawn(e)) continue;
    parentOf.set(e.id, e.parent);
    let depth = 0;
    for (let p = e.parent; p && depth < 4; p = parentOf.get(p)) depth++;
    const ok = done.get(e.id);
    out.push({ id: e.id, label: e.detail || e.name, depth, state: ok === undefined ? (live(m) ? "work" : "ended") : ok ? "done" : "fail" });
  }
  return out;
}

/** The last thing a session did, in one line. */
export function lastLine(m: RunMeta): string {
  const tl = timelineOf(m.id) ?? [];
  for (let i = tl.length - 1; i >= 0; i--) {
    const e = tl[i]!;
    if (e.kind === "text") return e.text.trim().split("\n").pop() ?? "";
    if (e.kind === "tool") return `${e.name} ${e.detail}`.trim();
    if (e.kind === "note") return e.text;
    if (e.kind === "said") return t("you: {text}", { text: e.text });
  }
  return m.state === "starting" ? t("Preparing the worktree") : t("Starting");
}

function since(m: RunMeta): string {
  const end = live(m) ? now() : endedMs(m);
  const s = Math.max(0, Math.round(end / 1000 - m.started_at));
  return s >= 3600 ? `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
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

/** Takes ended sessions out of view (live ones only stop, never hide: nothing runs unseen). */
function closeAll(ids: string[]) {
  if (!ids.length) return;
  setKept(kept().filter((x) => !ids.includes(x)));
  try {
    localStorage.setItem(KEPT_KEY, JSON.stringify(kept()));
  } catch {
    /* private window: kept for this launch only */
  }
  if (ids.includes(selected()?.id ?? "")) setSelected(null);
  setClosed([...closed(), ...ids]);
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
              {[since(mm()), mm().cost_usd != null ? `$${mm().cost_usd!.toFixed(2)}` : null].filter(Boolean).join("  ·  ")}
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

/** The dock's top edge: drag to resize, double click for the default, arrows from the keyboard. */
function DockGrip() {
  let from = 0;
  let start = 0;
  const move = (e: PointerEvent) => saveDockH(start + (from - e.clientY));
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    document.body.style.removeProperty("cursor");
  };
  onCleanup(up);
  return (
    <div
      class="ag-dock-grip"
      role="separator"
      aria-orientation="horizontal"
      aria-label={t("Resize the agents dock")}
      aria-valuenow={dockH()}
      aria-valuemin={DOCK_H.min}
      tabindex="0"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        if (!dockOpen()) setDockOpen(true);
        from = e.clientY;
        start = dockH();
        document.body.style.cursor = "ns-resize";
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
      }}
      onDblClick={() => saveDockH(DOCK_H.def)}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          saveDockH(dockH() + (e.key === "ArrowUp" ? 24 : -24));
        }
      }}
    />
  );
}

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
        <DockGrip />
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
