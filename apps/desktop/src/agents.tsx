// Every Claude Code session Pitlane starts (runs, Run all's analyses, judges), live, grouped by
// card, with the subagents each one opens. Shown in the Terminal screen and in a dock on the
// Board. You can write to a session while it works and stop it with Ctrl+C. A session closes
// itself a moment after it ends, unless you talked to it.
import { For, Show, createEffect, createMemo, createSignal, on, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api, isLive, onRun, type RunEvent, type RunMeta } from "./api";
import { Icon, type IconName } from "./icons";
import { toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** How long an ended session stays in view (so you see how it ended) before it closes. */
const LINGER_MS = 20_000;
const KEPT_KEY = "pitlane.agents.kept";

const [repo, setRepo] = createSignal<string | null>(null);
// A store reconciled by id: rows keep their DOM while the list refreshes.
const [store, setStore] = createStore<{ list: RunMeta[] }>({ list: [] });
const sessions = () => store.list;
const setSessions = (all: RunMeta[]) => setStore("list", reconcile(all, { key: "id" }));
const [timelines, setTimelines] = createStore<Record<string, RunEvent[]>>({});
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
const endedMs = (m: RunMeta) => (m.ended_at ?? m.started_at) * 1000;

/** What the Terminal screen and the dock show: live sessions, ones you talked to, the one you
 *  are looking at, and ones that ended a moment ago. Oldest first, so rows do not jump. */
export const agentSessions = createMemo(() => {
  const sel = selected()?.id;
  const k = kept();
  return sessions()
    .filter((m) => live(m) || (!closed().includes(m.id) && (k.includes(m.id) || m.id === sel || now() - endedMs(m) < LINGER_MS)))
    .sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));
});

export interface AgentGroup {
  card: number;
  title: string;
  sessions: RunMeta[];
}

export const agentGroups = createMemo<AgentGroup[]>(() => {
  const out: AgentGroup[] = [];
  for (const m of agentSessions()) {
    let g = out.find((x) => x.card === m.card);
    if (!g) out.push((g = { card: m.card, title: m.card_title, sessions: [] }));
    g.sessions.push(m);
  }
  return out;
});

export const agentsWorking = () => agentSessions().filter(live).length;
export const agentsWanting = () => agentSessions().filter((m) => live(m) && m.waiting.length > 0).length;
export const agentSelected = () => (selected() && agentSessions().some((m) => m.id === selected()!.id) ? selected() : null);
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
    for (const m of all) {
      if (live(m)) {
        void ensureTimeline(m.id);
        void api.runFollow(m.id, 0).catch(() => {});
      }
    }
  } catch {
    /* no daemon yet, or the browser preview */
  }
}

const loading = new Set<string>();
async function ensureTimeline(id: string, force = false) {
  if ((!force && timelines[id]) || loading.has(id)) return;
  loading.add(id);
  try {
    const t = await api.runTimeline(id);
    const evs = t.map(([, e]) => e);
    // Live events may have arrived meanwhile: keep the longer one.
    if (!timelines[id] || evs.length >= timelines[id]!.length) setTimelines(id, reconcile(evs));
  } catch {
    if (!timelines[id]) setTimelines(id, []);
  } finally {
    loading.delete(id);
  }
}

export function setAgentsRepo(path: string | null) {
  if (path === repo()) return;
  setRepo(path);
  setSessions([]);
  setSelected(null);
  setTimelines(reconcile({}));
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
  await onRun(
    (run, i, e) => {
      const t = timelines[run];
      if (!t) return;
      if (i === t.length) setTimelines(run, t.length, e);
      else if (i > t.length) void ensureTimeline(run, true);
      if (e.kind === "approval" || e.kind === "decided" || e.kind === "started") void refreshAgents();
    },
    () => void refreshAgents(),
  );
  let last = 0;
  setInterval(() => {
    setNow(Date.now());
    const busy = watching() || sessions().some(live);
    const every = busy ? 1500 : 15_000;
    if (document.visibilityState === "visible" && Date.now() - last >= every) {
      last = Date.now();
      void refreshAgents();
    }
  }, 500);
}

// ---------------------------------------------------------------- reading a session

const KIND: Record<string, string> = { run: "Run", analysis: "Analysis", judge: "Judge" };
export function kindLabel(m: RunMeta): string {
  const k = KIND[m.kind ?? "run"] ?? "Run";
  return m.kind === "run" || !m.kind ? ((m.round ?? 0) > 0 ? `${k} · round ${(m.round ?? 0) + 1}` : k) : k;
}

type Look = { icon: IconName; word: string; s: "work" | "needs" | "done" | "fail" | "stopped" };
export function look(m: RunMeta): Look {
  if (live(m) && m.waiting.length > 0) return { icon: "needs", word: "Needs you", s: "needs" };
  if (m.state === "verifying") return { icon: "verify", word: "Checking", s: "work" };
  if (live(m)) return { icon: "run", word: "Working", s: "work" };
  if (m.state === "done") return { icon: "done", word: "Done", s: "done" };
  if (m.state === "stopped") return { icon: "stop", word: "Stopped", s: "stopped" };
  return { icon: "fail", word: m.state === "interrupted" ? "Interrupted" : "Failed", s: "fail" };
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
  const t = timelines[m.id] ?? [];
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
  const t = timelines[m.id] ?? [];
  for (let i = t.length - 1; i >= 0; i--) {
    const e = t[i]!;
    if (e.kind === "text") return e.text.trim().split("\n").pop() ?? "";
    if (e.kind === "tool") return `${e.name} ${e.detail}`.trim();
    if (e.kind === "note") return e.text;
    if (e.kind === "said") return `you: ${e.text}`;
  }
  return m.state === "starting" ? "Preparing the worktree" : "Starting";
}

function since(m: RunMeta): string {
  const end = live(m) ? now() : endedMs(m);
  const s = Math.max(0, Math.round(end / 1000 - m.started_at));
  return s >= 3600 ? `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

// ---------------------------------------------------------------- the list in the Terminal screen

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
                  <span class="ag-dot" data-s={look(m).s} />
                  <span class="tt-name">
                    #{m.card} {kindLabel(m)}
                  </span>
                </button>
              </div>
            )}
          </For>
        }
      >
        <div class="ag-list" role="tablist" aria-label="Agents" aria-orientation="vertical">
          <div class="ag-head">
            <span class="terms-title">Agents</span>
            <Show when={agentsWorking() > 0}>
              <span class="ag-working">{agentsWorking()} working</span>
            </Show>
          </div>
          <For each={agentGroups()}>
            {(g) => (
              <div class="ag-group">
                <div class="ag-card" title={g.title}>
                  <span class="ag-num">#{g.card}</span>
                  <span class="ag-title">{g.title}</span>
                </div>
                <For each={g.sessions}>
                  {(m) => (
                    <>
                      <SessionRow m={m} />
                      <For each={subagentsOf(m)}>{(s) => <SubRow m={m} s={s} />}</For>
                    </>
                  )}
                </For>
              </div>
            )}
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
      <button type="button" role="tab" class="tt-main" aria-selected={active()} title={lastLine(props.m)} onClick={() => selectAgent(props.m.id)}>
        <span class="ag-dot" data-s={l().s} />
        <span class="tt-text">
          <span class="tt-name">{kindLabel(props.m)}</span>
          <span class="tt-sub">{lastLine(props.m)}</span>
        </span>
        <span class="tt-status ag-status" data-s={l().s}>
          {l().word}
        </span>
      </button>
      <Show when={!live(props.m)}>
        <button type="button" class="tt-close" aria-label="Close this session" title="Close" onClick={() => close(props.m.id)}>
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
      <button type="button" role="tab" class="tt-main" aria-selected={active()} title={props.s.label} onClick={() => selectAgent(props.m.id, props.s.id)}>
        <span class="ag-dot" data-s={props.s.state} />
        <span class="tt-text">
          <span class="tt-name ag-sub-name">{props.s.label}</span>
        </span>
        <span class="tt-status ag-status" data-s={props.s.state}>
          {props.s.state === "work" ? "Working" : props.s.state === "done" ? "Done" : props.s.state === "fail" ? "Failed" : "Ended"}
        </span>
      </button>
    </div>
  );
}

function close(id: string) {
  keep(id, false);
  if (selected()?.id === id) setSelected(null);
  setClosed([...closed(), id]);
}

// ---------------------------------------------------------------- one session, as a terminal

export function AgentPane(props: { onOpenRun: (id: string) => void }) {
  const m = () => sessions().find((x) => x.id === selected()?.id) ?? null;
  const sub = () => selected()?.sub ?? null;
  const subLabel = () => {
    const s = sub();
    const mm = m();
    return s && mm ? (subagentsOf(mm).find((x) => x.id === s)?.label ?? "subagent") : null;
  };
  const events = createMemo(() => {
    const mm = m();
    if (!mm) return [];
    const s = sub();
    return (timelines[mm.id] ?? []).filter((e) => {
      const p = "parent" in e ? e.parent : undefined;
      if (s) return p === s;
      if (e.kind === "quota" || (e.kind === "tool-done" && e.ok)) return false;
      return !p;
    });
  });
  const [text, setText] = createSignal("");
  let log: HTMLDivElement | undefined;
  let input: HTMLInputElement | undefined;
  let stick = true;

  createEffect(
    on([() => events().length, () => selected()?.id, () => selected()?.sub], () => {
      if (log && stick) requestAnimationFrame(() => log && (log.scrollTop = log.scrollHeight));
    }),
  );
  createEffect(on(() => selected()?.id, () => ((stick = true), requestAnimationFrame(() => input?.focus()))));

  const send = async () => {
    const mm = m();
    const t = text().trim();
    if (!mm || !t) return;
    try {
      await api.runSay(mm.id, t);
      setText("");
      keep(mm.id, true);
    } catch (e) {
      toast({ tone: "fail", title: "The agent did not get it", body: msg(e) });
    }
  };
  const stop = async () => {
    const mm = m();
    if (!mm || !live(mm)) return;
    try {
      await api.runStop(mm.id);
      toast({ tone: "info", title: `Stopping #${mm.card} ${kindLabel(mm)}`, body: "It ends its current step, then stops." });
    } catch (e) {
      toast({ tone: "fail", title: "Could not stop it", body: msg(e) });
    }
  };
  const onKey = (e: KeyboardEvent) => {
    // Ctrl+C stops the agent, like in a terminal (unless you are copying a selection).
    if (e.ctrlKey && !e.metaKey && e.key.toLowerCase() === "c" && !window.getSelection()?.toString()) {
      e.preventDefault();
      void stop();
    }
  };

  return (
    <Show when={m()}>
      {(mm) => (
        <div class="ag-pane" tabindex="-1" onKeyDown={onKey}>
          <div class="ag-bar">
            <span class="ag-dot" data-s={look(mm()).s} />
            <span class="ag-bar-title">
              <span class="ag-num">#{mm().card}</span> {mm().card_title}
            </span>
            <span class="ag-bar-sep">·</span>
            <Show when={sub()} fallback={<span class="ag-bar-kind">{kindLabel(mm())}</span>}>
              <button type="button" class="ag-crumb" onClick={() => selectAgent(mm().id)}>
                {kindLabel(mm())}
              </button>
              <span class="ag-bar-sep">›</span>
              <span class="ag-bar-kind">{subLabel()}</span>
            </Show>
            <span class="ag-bar-meta">
              {[mm().model, since(mm()), mm().cost_usd != null ? `$${mm().cost_usd!.toFixed(2)}` : null].filter(Boolean).join("  ·  ")}
            </span>
            <span class="ag-bar-fill" />
            <Show when={(mm().kind ?? "run") === "run"}>
              <button type="button" class="btn btn-ghost" onClick={() => props.onOpenRun(mm().id)}>
                Open run
              </button>
            </Show>
            <Show
              when={live(mm())}
              fallback={
                <button type="button" class="btn btn-ghost" onClick={() => close(mm().id)}>
                  Close
                </button>
              }
            >
              <button type="button" class="btn btn-ghost" title="Stop this agent (Ctrl+C)" onClick={() => void stop()}>
                <Icon name="stop" small /> Stop <span class="kbd">⌃C</span>
              </button>
            </Show>
          </div>
          <div class="ag-log" ref={log} onScroll={(e) => (stick = e.currentTarget.scrollTop + e.currentTarget.clientHeight >= e.currentTarget.scrollHeight - 24)}>
            <Show when={live(mm()) && mm().waiting.length > 0}>
              <div class="ag-line ag-needs">
                <Icon name="needs" small /> Waiting for you to allow {mm().waiting.map((w) => w.detail || w.tool).join(", ")}.
                <Show when={(mm().kind ?? "run") === "run"}>
                  {" "}
                  <button type="button" class="ag-link" onClick={() => props.onOpenRun(mm().id)}>
                    Answer in the run
                  </button>
                </Show>
              </div>
            </Show>
            <For each={events()} fallback={<div class="ag-line ag-muted">{live(mm()) ? "Starting…" : "Nothing recorded."}</div>}>
              {(e) => <Line e={e} m={mm()} />}
            </For>
          </div>
          <div class="ag-input" data-off={live(mm()) ? undefined : ""}>
            <span class="ag-prompt">›</span>
            <input
              ref={input}
              class="ag-field"
              disabled={!live(mm())}
              placeholder={
                live(mm())
                  ? sub()
                    ? "Message the agent that opened this subagent (Enter). Ctrl+C stops it"
                    : "Tell the agent more: context, a check to make… (Enter). Ctrl+C stops it"
                  : `This session ended (${look(mm()).word.toLowerCase()})`
              }
              aria-label="Message the agent"
              value={text()}
              onInput={(e) => setText(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.isComposing) (e.preventDefault(), void send());
                e.stopPropagation();
                onKey(e);
              }}
            />
          </div>
        </div>
      )}
    </Show>
  );
}

function Line(props: { e: RunEvent; m: RunMeta }) {
  const e = props.e;
  switch (e.kind) {
    case "text":
      return <div class="ag-line ag-text">{e.text.trim()}</div>;
    case "tool":
      return e.name === "Task" || e.name === "Agent" ? (
        <button type="button" class="ag-line ag-spawn" onClick={() => selectAgent(props.m.id, e.id)}>
          <span class="ag-glyph">⤷</span> Subagent: {e.detail || e.name} <span class="ag-muted">open</span>
        </button>
      ) : (
        <div class="ag-line ag-tool">
          <span class="ag-glyph">●</span> <span class="ag-tool-name">{e.name}</span> <span class="ag-muted">{e.detail}</span>
        </div>
      );
    case "tool-done":
      return e.ok ? (
        <div class="ag-line ag-muted">  ⎿ {e.detail}</div>
      ) : (
        <div class="ag-line ag-fail">  ✕ {e.detail}</div>
      );
    case "said":
      return (
        <div class="ag-line ag-said">
          <span class="ag-glyph">›</span> {e.text}
        </div>
      );
    case "note":
      return <div class="ag-line ag-muted">{e.text}</div>;
    case "started":
      return <div class="ag-line ag-muted">Session started{e.model ? ` on ${e.model}` : ""}</div>;
    case "approval":
      return (
        <div class="ag-line ag-needs">
          Asks to use {e.approval.tool}: {e.approval.detail}
        </div>
      );
    case "decided":
      return <div class="ag-line ag-muted">You answered: {e.decision}</div>;
    case "proof":
      return (
        <div class={`ag-line ${e.ok ? "ag-muted" : "ag-fail"}`}>
          {e.ok ? "✓" : "✕"} {e.command}
        </div>
      );
    case "judged":
      return (
        <div class="ag-line ag-text">
          Judge: {e.verdict === "approve" ? "approves" : "asks for changes"}. {e.summary}
        </div>
      );
    case "limited":
      return <div class="ag-line ag-fail">Hit the {e.window.replace("_", "-")} usage limit.</div>;
    case "finished":
      return (
        <div class="ag-line ag-muted">
          — {e.ok ? "finished" : "stopped with an error"} · {e.turns} turns · ${e.cost_usd.toFixed(2)}
        </div>
      );
    case "exited":
      return <div class="ag-line ag-muted">— session ended: {e.state}</div>;
    default:
      return null;
  }
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
      <section class="ag-dock" aria-label="Agents" data-open={dockOpen() ? "" : undefined}>
        <div class="ag-dock-head">
          <button type="button" class="ag-dock-toggle" aria-expanded={dockOpen()} onClick={toggle}>
            <Icon name="chev" small class={dockOpen() ? "ag-chev-open" : undefined} />
            <span class="terms-title">Agents</span>
            <span class="ag-working">
              {agentsWorking() > 0 ? `${agentsWorking()} working` : "all ended"}
              {agentsWanting() > 0 ? ` · ${agentsWanting()} need you` : ""}
            </span>
          </button>
          <span class="ag-bar-fill" />
          <button type="button" class="btn btn-ghost" onClick={props.onOpenAll}>
            <Icon name="term" small /> Open in Terminal
          </button>
        </div>
        <Show when={dockOpen()}>
          <div class="ag-dock-list">
            <For each={agentSessions()}>
              {(m) => {
                onMount(() => void ensureTimeline(m.id));
                const subs = () => subagentsOf(m).filter((s) => s.state === "work").length;
                return (
                  <button type="button" class="ag-cell" data-ended={live(m) ? undefined : ""} onClick={() => props.onOpen(m.id)}>
                    <span class="ag-cell-head">
                      <span class="ag-dot" data-s={look(m).s} />
                      <span class="ag-num">#{m.card}</span>
                      <span class="ag-cell-kind">{kindLabel(m)}</span>
                      <span class="ag-bar-fill" />
                      <span class="ag-status" data-s={look(m).s}>
                        {look(m).word}
                      </span>
                    </span>
                    <span class="ag-cell-title">{m.card_title}</span>
                    <span class="ag-cell-last">
                      {subs() > 0 ? `${subs()} subagent${subs() > 1 ? "s" : ""} · ` : ""}
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
