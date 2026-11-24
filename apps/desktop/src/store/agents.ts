// The sessions Pitlane started (runs, Run all's analyses, judges), as every screen reads them: the
// list kept fresh from the daemon, which one is selected, and what each one is in words.
import { createMemo, createRoot, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api, isLive, type RunEvent, type RunMeta } from "../api";
import { dropTimelines, ensureTimeline, resetTimelines, startTimelines, timelineOf } from "./timelines";
import type { IconName } from "../icons";
import { plural, t, tx } from "../i18n";
import { toast } from "../ui";
import { msg } from "../format";

/** How long an ended session stays in view (so you see how it ended) before it closes. */
const LINGER_MS = 20_000;
const KEPT_KEY = "pitlane.agents.kept";

const [repo, setRepo] = createSignal<string | null>(null);
// A store reconciled by id: rows keep their DOM while the list refreshes.
const [store, setStore] = createStore<{ list: RunMeta[] }>({ list: [] });
export const sessions = () => store.list;
const setSessions = (all: RunMeta[]) => setStore("list", reconcile(all, { key: "id" }));
const [kept, setKept] = createSignal<string[]>(readKept());
const [selected, setSelected] = createSignal<{ id: string; sub: string | null } | null>(null);
export { selected };
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
export function keep(id: string, on: boolean) {
  const next = on ? [...new Set([...kept(), id])] : kept().filter((x) => x !== id);
  setKept(next.slice(-50));
  try {
    localStorage.setItem(KEPT_KEY, JSON.stringify(kept()));
  } catch {
    /* private window: kept for this launch only */
  }
}

export const live = (m: RunMeta) => isLive(m.state);
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

interface AgentGroup {
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

export type Look = { icon: IconName; word: string; s: "work" | "needs" | "done" | "fail" | "stopped" };
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

export function since(m: RunMeta): string {
  const end = live(m) ? now() : endedMs(m);
  const s = Math.max(0, Math.round(end / 1000 - m.started_at));
  return s >= 3600 ? `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Takes ended sessions out of view (live ones only stop, never hide: nothing runs unseen). */
export function closeAll(ids: string[]) {
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
