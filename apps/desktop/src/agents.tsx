// Every Claude Code session Pitlane starts (runs, Run all's analyses, judges), live, grouped by
// card, with the subagents each one opens. Shown in the Terminal screen and in a dock on the
// Board. You can write to a session while it works and stop it with Ctrl+C. A session closes
// itself a moment after it ends, unless you talked to it.
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api, isLive, onRun, type RunEvent, type RunMeta } from "./api";
import { Icon, type IconName } from "./icons";
import { toast } from "./ui";
import { ApprovalBlock } from "./approvals";
import { Prose } from "./prose";

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
/** Stop all: every session the daemon runs, and analyses still waiting for a slot. */
export async function stopAllAgents(): Promise<void> {
  try {
    const n = await api.runsStopAll();
    toast({ tone: "info", title: n > 0 ? `Stopping ${n} agent${n > 1 ? "s" : ""}` : "Nothing was running", body: n > 0 ? "Each ends its current step; anything still there after a few seconds is killed." : undefined });
    void refreshAgents();
  } catch (e) {
    toast({ tone: "fail", title: "Could not stop the agents", body: msg(e) });
  }
}

function StopAll() {
  return (
    <Show when={agentsWorking() > 0}>
      <button type="button" class="btn btn-ghost ag-stop-all" title="Stop every agent Pitlane started" onClick={() => void stopAllAgents()}>
        <Icon name="stop" small /> Stop all
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

const KIND: Record<string, string> = { run: "Run", analysis: "Analysis", judge: "Judge", finish: "Closing step", assist: "Assistant" };
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
            <span class="ag-bar-fill" />
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
    // Results show under their tool call, as in Claude Code; a result with no call in view
    // (a failure, say) still shows on its own.
    const shown = new Set((timelines[mm.id] ?? []).filter((e) => e.kind === "tool").map((e) => (e as { id: string }).id));
    return (timelines[mm.id] ?? []).filter((e) => {
      const p = "parent" in e ? e.parent : undefined;
      if (e.kind === "quota") return false;
      if (e.kind === "tool-done" && shown.has(e.id)) return false;
      if (s) return p === s;
      return !p;
    });
  });
  const results = createMemo(() => {
    const mm = m();
    const out = new Map<string, Extract<RunEvent, { kind: "tool-done" }>>();
    for (const e of mm ? (timelines[mm.id] ?? []) : []) if (e.kind === "tool-done") out.set(e.id, e);
    return out;
  });
  const [text, setText] = createSignal("");
  let log: HTMLDivElement | undefined;
  let input: HTMLInputElement | undefined;
  let pane: HTMLDivElement | undefined;
  let stick = true;

  createEffect(
    on([() => events().length, () => selected()?.id, () => selected()?.sub], () => {
      if (log && stick) requestAnimationFrame(() => log && (log.scrollTop = log.scrollHeight));
    }),
  );
  createEffect(on(() => selected()?.id, () => ((stick = true), requestAnimationFrame(() => input?.focus()))));
  // A new question takes the keys, like a terminal prompt: 1-4 answer it, Esc goes back to typing.
  createEffect(
    on(
      () => m()?.waiting.length ?? 0,
      (n, prev) => {
        if (n > (prev ?? 0) && document.activeElement === input) pane?.focus();
        if (n > 0 && log) requestAnimationFrame(() => log && (log.scrollTop = log.scrollHeight));
      },
    ),
  );

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
        <div class="ag-pane" ref={pane} tabindex="-1" onKeyDown={onKey}>
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
            <For each={events()} fallback={<div class="ag-line ag-muted">{live(mm()) ? "Starting…" : "Nothing recorded."}</div>}>
              {(e) => <Line e={e} m={mm()} done={e.kind === "tool" ? results().get(e.id) : undefined} />}
            </For>
            <Show when={live(mm())}>
              <div class="ag-asks">
                <For each={mm().waiting}>{(a, i) => <ApprovalBlock run={mm().id} approval={a} keys={i() === 0} />}</For>
              </div>
            </Show>
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

/** Claude Code's names for its tools, as its own terminal shows them. */
const TOOL_LABEL: Record<string, string> = { Edit: "Update", MultiEdit: "Update", Grep: "Search", WebFetch: "Fetch", WebSearch: "Web search", NotebookEdit: "Update" };

/** What a finished tool came to, in one line, the way Claude Code sums it up. */
function resultLine(name: string, d: Extract<RunEvent, { kind: "tool-done" }>): string {
  const n = d.lines ?? (d.detail ? 1 : 0);
  if (!d.ok) return d.detail || "Failed";
  if (name === "Read") return `Read ${n} line${n === 1 ? "" : "s"}`;
  if (name === "Edit" || name === "MultiEdit" || name === "Write" || name === "NotebookEdit") return d.detail || "Done";
  if (name === "Grep" || name === "Glob") return n > 1 ? `Found ${n} results` : d.detail || "No matches";
  const more = n > 1 ? `  … +${n - 1} lines` : "";
  return `${d.detail || "Done"}${more}`;
}

/** An edit as Claude Code shows it: removed lines, then added ones. */
function Diff(props: { change: { old: string; new: string; cut: number } }) {
  const old = () => (props.change.old ? props.change.old.split("\n") : []);
  const add = () => (props.change.new ? props.change.new.split("\n") : []);
  return (
    <div class="cc-diff">
      <For each={old()}>{(l) => <div class="cc-del"><span class="cc-sign">-</span>{l || " "}</div>}</For>
      <For each={add()}>{(l) => <div class="cc-add"><span class="cc-sign">+</span>{l || " "}</div>}</For>
      <Show when={props.change.cut > 0}>
        <div class="cc-more">… {props.change.cut} more line{props.change.cut === 1 ? "" : "s"}</div>
      </Show>
    </div>
  );
}

function Line(props: { e: RunEvent; m: RunMeta; done?: Extract<RunEvent, { kind: "tool-done" }> }) {
  const e = props.e;
  switch (e.kind) {
    case "text":
      return (
        <div class="cc-msg">
          <span class="cc-dot">●</span>
          <div class="cc-text">
            <Prose text={e.text.trim()} />
          </div>
        </div>
      );
    case "tool": {
      if (e.name === "Task" || e.name === "Agent")
        return (
          <button type="button" class="ag-line ag-spawn cc-call" onClick={() => selectAgent(props.m.id, e.id)}>
            <span class="cc-dot" data-s={props.done ? (props.done.ok ? "ok" : "fail") : live(props.m) ? "work" : "ended"}>●</span>
            <span>
              <b class="cc-name">Task</b>
              <span class="cc-arg">({e.detail || e.name})</span> <span class="ag-muted">› open</span>
            </span>
          </button>
        );
      const d = props.done;
      return (
        <div class="cc-tool">
          <div class="cc-call">
            <span class="cc-dot" data-s={d ? (d.ok ? "ok" : "fail") : live(props.m) ? "work" : "ended"}>●</span>
            <span>
              <b class="cc-name">{TOOL_LABEL[e.name] ?? e.name}</b>
              <Show when={e.detail}>
                <span class="cc-arg">({e.detail})</span>
              </Show>
            </span>
          </div>
          <Show when={e.change}>{(c) => <Diff change={c()} />}</Show>
          <Show when={d} fallback={<Show when={live(props.m)}><div class="cc-result ag-muted">⎿  Running…</div></Show>}>
            {(d) => <div class={`cc-result ${d().ok ? "ag-muted" : "ag-fail"}`}>⎿  {resultLine(e.name, d())}</div>}
          </Show>
        </div>
      );
    }
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
          <span class="ag-glyph">?</span> Asks to use {e.approval.tool}: {e.approval.detail}
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

const DOCK_H_KEY = "pitlane.agents.dock.h";
const DOCK_H = { min: 64, def: 132 };
const dockMax = () => Math.round(window.innerHeight * 0.6);
const [dockH, setDockH] = createSignal(readDockH());
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
      aria-label="Resize the agents dock"
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
      <section class="ag-dock" aria-label="Agents" data-open={dockOpen() ? "" : undefined}>
        <DockGrip />
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
          <StopAll />
          <button type="button" class="btn btn-ghost" onClick={props.onOpenAll}>
            <Icon name="term" small /> Open in Terminal
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
