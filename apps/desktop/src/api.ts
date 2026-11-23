// Mirrors the serde shapes of pit-core and the Tauri commands in src-tauri/src/lib.rs.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Tauri in the app; in a plain browser during development, the preview fixture. */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) return tauriInvoke<T>(cmd, args);
  if (import.meta.env.DEV) return (await import("./dev/preview")).preview<T>(cmd, args);
  throw "Pitlane runs inside its desktop app.";
}

export type Landed = "ancestor" | "patch-id" | "merge-tree" | "pull-request";
export type Status =
  | "contaminated"
  | "can-delete"
  | "shipped-dirty"
  | "shipped"
  | "needs-production"
  | "in-progress"
  | "new"
  | "stale";

export interface LaneWorktree {
  path: string;
  dirty: number;
  is_main: boolean;
  is_current: boolean;
}

export type Sync =
  | { kind: "local-only" }
  | { kind: "gone" }
  | { kind: "synced" }
  | { kind: "ahead"; ahead: number }
  | { kind: "behind"; behind: number }
  | { kind: "diverged"; ahead: number; behind: number }
  | { kind: "remote-only" };

export interface Lane {
  number: number;
  branch: string;
  sha: string;
  subject: string;
  local: boolean;
  remote_sha: string | null;
  sync: Sync;
  last_push: number | null;
  upstream: string | null;
  upstream_gone: boolean;
  tracks_other: string | null;
  unpushed: number;
  ahead: number;
  worktree: LaneWorktree | null;
  in_integration: Landed | null;
  in_production: Landed | null;
  integration_commits: string[];
  idle_days: number;
  status: Status;
}

export interface Commit {
  sha: string;
  short: string;
  subject: string;
  author: string;
  days_ago: number;
}

export interface Yard {
  repo: string;
  production: string;
  integration: string | null;
  fetched: boolean;
  github: boolean;
  lanes: Lane[];
  /** The branch the repository's own checkout has out. */
  head?: string | null;
  only_in_integration: Commit[];
}

export interface ShipPlan {
  branch: string;
  sha: string;
  target: string;
  target_sha: string;
  already: Landed | null;
  tree: string | null;
  conflicts: string[];
  check: [string, string] | null;
  dirty: number;
  remote_behind: number;
  remote_ahead: number;
}

export type LocalUpdate =
  | { kind: "updated" }
  | { kind: "left-behind"; detail: string }
  | { kind: "diverged" }
  | { kind: "no-local-branch" };

export interface ShipReport {
  merge_sha: string;
  target: string;
  local: LocalUpdate;
}

export interface CleanItem {
  branch: string;
  sha: string;
  local: boolean;
  how: Landed | null;
  worktree: string | null;
  remote_sha: string | null;
  delete_remote: boolean;
}

export interface CleanPlan {
  items: CleanItem[];
  skipped: { branch: string; reason: string }[];
}

export interface CleanOutcome {
  branch: string;
  error: string | null;
}

export interface UndoReport {
  branch: string;
  sha: string;
  local_restored: boolean;
  remote_restored: boolean;
  worktree: string | null;
}

export interface NewReport {
  branch: string;
  path: string;
  base: string;
  base_sha: string;
  copied: string[];
  missing: string[];
  cloned: string[];
  setup_ok: boolean | null;
}

export type CardState = "inbox" | "ready" | "running" | "verifying" | "review" | "in-dev" | "done" | "blocked";
export const cardStates: CardState[] = ["inbox", "ready", "running", "verifying", "review", "in-dev", "done", "blocked"];

export interface Question {
  id: number;
  text: string;
  options?: string[];
  recommended?: number;
  /** What it is about and what each option changes, so it can be answered without the card. */
  context?: string;
  answer?: string;
  answered_at?: string;
  decision?: string;
  /** The run that asked it while working (Presence: Here). */
  run?: string;
  /** When the run asked it. */
  asked_at?: string;
  /** Answered with the agent's recommendation because nobody was there (Presence: Away, or no answer in time). */
  auto?: boolean;
  /** Talking it over with Claude; Claude's turns carry the options it offers from then on. */
  talk?: Turn[];
}

export interface Turn {
  at: string;
  /** "you" or "claude". */
  by: string;
  text: string;
  options?: string[];
  recommended?: number;
}

export interface CardNote {
  at: string;
  /** "claude", "judge" or "you". */
  by: string;
  text: string;
}

/** Why a card is blocked: what would free it. */
export type BlockReason = "questions" | "stuck" | "failed" | "no-route" | "api-key" | "conflict" | "checks" | "other";
export interface Block {
  reason: BlockReason;
  detail?: string;
  since: string;
  run?: string;
  /** Pitlane is reading the card again (your answers, or what stopped the agent). */
  checking?: string;
  /** When Pitlane tries it again by itself (Unix seconds). */
  retry_at?: number;
}

export interface Card {
  id: number;
  title: string;
  state: CardState;
  lane?: string;
  tags?: string[];
  rank?: number;
  analyzed?: string;
  /** Where its work lands (Run all's Into); equal to lane when worked directly there. */
  target?: string;
  /** Cards that must land first (from the analysis). */
  after?: number[];
  /** Shared parts of the code it changes; cards sharing one never run together. */
  areas?: string[];
  notes?: CardNote[];
  /** Attached screenshots, by file name; Claude gets them with the spec. */
  images?: string[];
  /** A few lines on what it is about, written by Claude (Sonnet) when it was made or its spec changed. */
  summary?: string;
  /** The run stopped mid-work: the next Run resumes its Claude Code session instead of starting over. */
  paused?: string;
  /** Why it is blocked (only while it is). */
  blocked?: Block;
  created: string;
  updated: string;
  questions?: Question[];
  body: string;
  open_questions: number;
  file: string;
}

export interface Finish {
  run: string | null;
  ok: boolean;
  summary: string | null;
  cost_usd: number | null;
}

export interface Report {
  id: string;
  repo: string;
  repo_name: string;
  started_at: number;
  ended_at: number;
  into: string | null;
  direct: boolean;
  instructions: string;
  cards: {
    id: number;
    title: string;
    state: string;
    lane: string | null;
    target: string | null;
    summary: string | null;
    verdict: string | null;
    runs: number;
    cost_usd: number;
    duration_ms: number;
    files: { path: string; added: number; removed: number }[];
    /** The last screenshots its runs took. */
    shots?: { run: string; file: string }[];
  }[];
  sessions: number;
  cost_usd: number;
  finish: Finish | null;
  /** What the runs noticed beyond their cards; `made` is the card made from it. */
  suggestions?: { card: number; title: string; body: string; made: number | null }[];
  /** Claude's short reading (Sonnet): a headline, a few points, one line per suggestion. */
  digest?: { headline: string; points: string[]; suggestions: string[] };
}

export interface CardList {
  cards: Card[];
  problems: string[];
}

export type RunState = "starting" | "running" | "verifying" | "done" | "failed" | "stopped" | "interrupted";

export type Verdict = "approve" | "changes";

export interface Proof {
  command: string;
  ok: boolean;
  code: number | null;
  duration_ms: number;
  tail: string;
}

export interface Finding {
  file?: string | null;
  line?: number | null;
  severity: "blocker" | "major" | "minor" | "nit" | string;
  text: string;
}

export interface Verification {
  run: string;
  proofs: Proof[];
  no_commands: boolean;
  max_rounds: number;
  judge: {
    model: string | null;
    judgement: { verdict: Verdict; summary: string; findings: Finding[] } | null;
    error: string | null;
    cost_usd: number | null;
    duration_ms: number | null;
  } | null;
  started_at: number;
  ended_at: number | null;
}

export interface LaneDiff {
  base: string;
  head: string;
  files: { path: string; added: number; removed: number }[];
  patch: string;
  truncated: boolean;
}

export interface Tokens {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

export interface Handoff {
  status?: "done" | "partial" | "blocked";
  summary?: string;
  files_changed?: string[];
  commits?: string[];
  tests?: { command?: string; passed?: boolean; notes?: string };
  /** Only with status partial: what the card still needs. */
  left?: string[];
  /** Work noticed outside the card's spec; the report turns each into a card. */
  suggestions?: { title: string; body: string }[];
}

export interface RunMeta {
  id: string;
  repo: string;
  card: number;
  card_title: string;
  branch: string;
  worktree: string;
  agent: string;
  model: string | null;
  /** The effort level (`--effort`, `/effort`), when set. */
  effort?: string | null;
  session_id: string;
  pid: number | null;
  state: RunState;
  started_at: number;
  ended_at: number | null;
  cost_usd: number | null;
  duration_ms: number | null;
  turns: number | null;
  tokens: Tokens;
  handoff: Handoff | null;
  error: string | null;
  denied: string[];
  resumed_from: string | null;
  waiting: Approval[];
  /** Card questions the agent asked while working and waits on (Presence: Here). */
  asking?: number[];
  /** The account the route chose (`default` is the normal Claude Code login). */
  /** Failed, but Pitlane carries it on by itself (out of turns under Run all): not worth a notice. */
  goes_on?: boolean;
  account?: string;
  /** Why the route chose this account and model. */
  reason?: string | null;
  handoff_from?: string | null;
  exhausted?: string[];
  /** It hit a usage limit and the next option (an API key) waits for your yes. */
  next?: string | null;
  /** When the limit this run hit resets (unix seconds). */
  resets_at?: number | null;
  round?: number;
  verdict?: Verdict | null;
  proofs_ok?: boolean | null;
  /** The lane's commit when this run started: where this round's diff begins. */
  base_sha?: string | null;
  /** The map of where to look that the run's first message carried. */
  brief?: { files: number; chars: number; ms: number; indexed: number; paths: string[] } | null;
  /** A card's run, or a side session beside it (Run all's analysis, the judge). */
  kind?: SessionKind;
  /** The run a side session belongs to. */
  parent?: string | null;
}

export type SessionKind = "run" | "analysis" | "judge" | "finish";

export interface Quota {
  /** The account the meters are about: this repository's. */
  account: string;
  events: Extract<RunEvent, { kind: "quota" }>[];
  five_hour_at: number;
  seven_day_at: number;
}

export type AccountKind = "subscription" | "team" | "api-key";

export interface AccountView {
  id: string;
  kind: AccountKind;
  status: { logged_in: boolean; method: string | null; email: string | null; problem: string | null };
  usage: { five_hour: number | null; seven_day: number | null } | null;
  here: boolean;
}

export interface Accounts {
  accounts: AccountView[];
  route: string[];
  five_hour_at: number;
  seven_day_at: number;
  file: string;
}

/** A start the route refused because its next option is paid per token. */
export const NEEDS_API_KEY = "needs-api-key: ";

export interface Approval {
  id: string;
  tool: string;
  detail: string;
  input: unknown;
  /** What "the same command" means: `Bash(npm test:*)`, or the tool's name. */
  rule: string;
  asked_at: number;
}

export type Decision = "once" | "run" | "always" | "deny";

export type RunEvent =
  | { kind: "started"; model: string; session: string }
  | { kind: "text"; text: string; parent?: string }
  | { kind: "tool"; id: string; name: string; detail: string; parent?: string; change?: { old: string; new: string; cut: number } }
  | { kind: "tool-done"; id: string; ok: boolean; detail: string; parent?: string; lines?: number }
  | { kind: "shot"; tool: string; file: string; parent?: string }
  | { kind: "said"; text: string }
  | { kind: "tuned"; model?: string; effort?: string }
  | { kind: "quota"; window: string; used: number; resets_at: number }
  | { kind: "finished"; ok: boolean; cost_usd: number; duration_ms: number; turns: number; tokens: Tokens; handoff: Handoff | null; denied: string[] }
  | { kind: "note"; text: string }
  | { kind: "approval"; approval: Approval }
  | { kind: "decided"; id: string; decision: Decision; message: string | null }
  | { kind: "limited"; window: string; resets_at: number }
  | { kind: "proof"; command: string; ok: boolean; duration_ms: number }
  | { kind: "judged"; verdict: Verdict; summary: string; findings: number }
  | { kind: "exited"; code: number | null; state: RunState };

export const isLive = (s: RunState) => s === "starting" || s === "running" || s === "verifying";

// ---------------------------------------------------------------- the planner

export interface Footprint {
  card: number;
  title: string;
  source: "predicted" | "lane";
  lane: string | null;
  files: string[];
}
export interface Clash {
  card: number;
  with_card: number | null;
  with_lane: string | null;
  files: string[];
}
export interface Plan {
  footprints: Footprint[];
  clashes: Clash[];
  batches: number[][];
  waits: Clash[];
}

// ---------------------------------------------------------------- files (for paths in terminals)

export const files = {
  list: (root: string) => invoke<string[]>("files_list", { root }),
};

// ---------------------------------------------------------------- free terminals

export interface TermInfo {
  id: number;
  /** The Claude Code session this terminal resumed, if it is one. */
  session?: string;
  cwd: string;
  label: string;
  exited: number | null;
}

export const terms = {
  open: (cwd: string, label: string, cols: number, rows: number) => invoke<TermInfo>("term_open", { cwd, label, cols, rows }),
  /** Claude Code resumes an agent's ended session, in its folder and on its account. */
  resume: (run: string, cols: number, rows: number) => invoke<TermInfo>("term_resume", { run, cols, rows }),
  write: (id: number, data: string) => invoke<void>("term_write", { id, data }),
  resize: (id: number, cols: number, rows: number) => invoke<void>("term_resize", { id, cols, rows }),
  close: (id: number) => invoke<void>("term_close", { id }),
  list: () => invoke<TermInfo[]>("term_list"),
  /** The terminal's recent output, and the number of the last piece it holds (see onTerm). */
  buffer: (id: number) => invoke<{ text: string; seq: number }>("term_buffer", { id }),
};

/** A web link in the person's browser (http and https only). */
export function openExternal(url: string): Promise<void> {
  if (!inTauri) {
    window.open(url, "_blank", "noopener");
    return Promise.resolve();
  }
  return invoke<void>("open_external", { url });
}

// ---------------------------------------------------------------- VS Code (code-server)

export const vscode = {
  /** Pitlane's colours for VS Code's dark and light themes, written into its settings. */
  theme: (t: { dark: Record<string, string>; light: Record<string, string>; mode: string }) => invoke<void>("vscode_theme", t),
  /** Loads VS Code on `folder` out of sight, so the Code tab opens at once. */
  prewarm: (folder: string, w: number, h: number, bg: string) => invoke<void>("vscode_prewarm", { folder, w, h, bg }),
  /** Puts VS Code on the tab; true when it is already drawn, else it comes in by itself (onVscodeReady). */
  show: (folder: string, r: { x: number; y: number; w: number; h: number }, bg: string) => invoke<boolean>("vscode_show", { folder, ...r, bg }),
  hide: () => invoke<void>("vscode_hide"),
  /** What the page draws over VS Code (window rects: x, y, w, h, corner radius), shown through it. */
  holes: (holes: [number, number, number, number, number][]) => invoke<void>("vscode_holes", { holes }),
  /** The folders VS Code's own picker offers. */
  places: (places: { root: string; label: string; kind: "repo" | "lane" }[]) => invoke<void>("vscode_places", { places }),
  open: (path: string, line?: number, col?: number) => invoke<void>("vscode_open", { path, line: line ?? null, col: col ?? null }),
};

/** The folder picker inside VS Code chose a folder. Nothing in the browser preview. */
export async function onVscodePlace(cb: (root: string) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<string>("vscode-place", (e) => cb(e.payload));
}

/** VS Code finished drawing on a folder. Nothing in the browser preview. */
export async function onVscodeReady(cb: (folder: string) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<{ folder: string }>("vscode-ready", (e) => cb(e.payload.folder));
}


/** Terminal output and exits. Nothing in the browser preview. */
export async function onTerm(onData: (id: number, data: string, seq: number) => void, onExit: (id: number, code: number) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const a = await listen<{ id: number; data: string; seq: number }>("term-data", (e) => onData(e.payload.id, e.payload.data, e.payload.seq));
  const b = await listen<{ id: number; code: number }>("term-exit", (e) => onExit(e.payload.id, e.payload.code));
  return () => {
    a();
    b();
  };
}

/** Live events from the daemon, pushed by the Tauri backend. Nothing in the browser preview. */
export async function onRun(
  onEvent: (run: string, i: number, event: RunEvent) => void,
  onEnd: (run: string, meta: RunMeta | null) => void,
): Promise<() => void> {
  if (!inTauri) return () => {};
  const a = await listen<{ run: string; i: number; event: RunEvent }>("run-event", (e) => onEvent(e.payload.run, e.payload.i, e.payload.event));
  const b = await listen<{ run: string; meta: RunMeta | null }>("run-end", (e) => onEnd(e.payload.run, e.payload.meta));
  return () => {
    a();
    b();
  };
}

export type JobState = "queued" | "shipping" | "done" | "conflict" | "failed" | "removed";

export interface ShipJob {
  id: string;
  repo: string;
  branch: string;
  target: string;
  state: JobState;
  queued_at: number;
  finished_at: number | null;
  merge_sha: string | null;
  conflicts: string[];
  error: string | null;
  card: number | null;
  /** For a lane in a related repository: the repository whose board holds the card. */
  card_repo?: string | null;
}

export const isOpenJob = (j: ShipJob) => j.state === "queued" || j.state === "shipping";

export interface RepoEntry {
  path: string;
  name: string;
}

export interface Settings {
  repos: RepoEntry[];
  last: string | null;
  prefs?: Prefs;
}

/** The app's own preferences (a repository's flow lives in its .pitlane/flow.toml). */
export interface Prefs {
  theme: "system" | "light" | "dark";
  notify_approvals: boolean;
  notify_runs: boolean;
  notify_terminals: boolean;
  code_prewarm: boolean;
  /** Pitlane's own words: "system", "en" or "pt-BR". */
  ui_language: string;
  /** What agents write in for you: "system" or a language tag. */
  agent_language: string;
  /** Run all: at most this many agents at once. */
  max_agents: number;
  /** Run all: fewer agents while this Mac is loaded, more again when it has room. */
  adaptive_agents: boolean;
}

export const DEFAULT_PREFS: Prefs = {
  theme: "system",
  notify_approvals: true,
  notify_runs: true,
  notify_terminals: true,
  code_prewarm: true,
  ui_language: "system",
  agent_language: "system",
  max_agents: 3,
  adaptive_agents: true,
};

/** Presence: Here, runs ask you the calls that are yours; Away, they take their recommendation. */
export type PresenceMode = "here" | "away";
export interface PresenceConfig {
  mode: PresenceMode;
  /** Here while you use the Mac, Away after Performance's `away_minutes`. */
  auto: boolean;
  /** How long a run waits for your answer before it takes its recommendation. */
  wait_minutes: number;
  /** Questions one run may ask you. */
  per_run: number;
}
export interface PresenceSnapshot {
  config: PresenceConfig;
  /** The mode in force now (with auto, it follows whether you use the Mac). */
  mode: PresenceMode;
  away_minutes: number;
}

/** Performance: Full takes the Mac while you are away, Light leaves room while you use it. */
export type PerfMode = "full" | "light";
export interface PerfConfig {
  mode: PerfMode;
  /** Light while you use the Mac, Full after `away_minutes` away. */
  auto: boolean;
  away_minutes: number;
  light_agents: number;
  light_background: boolean;
}
export type PerfAgentState = "working" | "background" | "paused" | "waiting" | "starting";
export interface PerfAgent {
  id: string;
  card: number;
  title: string;
  kind: string;
  repo: string;
  branch: string;
  model: string | null;
  started_at: number;
  pid: number | null;
  state: PerfAgentState;
  paused_by: "you" | "mode" | "memory" | null;
  paused_since: number | null;
  /** Bytes, as Activity Monitor counts them, for the whole process tree. */
  mem: number;
  /** Percent of one core, summed over the tree (Activity Monitor's CPU column). */
  cpu: number;
  procs: number;
  top: { name: string; mem: number; cpu: number }[];
}
export interface PerfSample {
  t: number;
  cpu: number;
  mem_used: number;
  agents_mem: number;
  agents_cpu: number;
}
export interface PerfEvent {
  t: number;
  kind: "full" | "light" | "away" | "back" | "paused_mode" | "paused_memory" | "resumed" | "paused_you" | "resumed_you";
  card?: number;
  title?: string;
  n?: number;
}
export interface PerfSnapshot {
  governed: boolean;
  mode: PerfMode;
  config: PerfConfig;
  allowed: number;
  at?: number;
  cap?: number;
  max?: number;
  adaptive?: boolean;
  idle_secs?: number;
  tight?: boolean;
  critical?: boolean;
  background?: boolean;
  awake?: boolean;
  system?: {
    mem_total: number;
    mem_used: number;
    free_pct: number;
    pressure: "normal" | "warn" | "critical";
    swap_used: number;
    swap_total: number;
    cpu: number;
    cores: number;
    p_cores: number;
    load: number;
  };
  agents?: PerfAgent[];
  agents_mem?: number;
  agents_cpu?: number;
  others?: { name: string; mem: number; cpu: number }[];
  history: PerfSample[];
  events: PerfEvent[];
}

/** How many agents Run all may have working now, and why. */
export interface Capacity {
  agents: number;
  max: number;
  load: number;
  cores: number;
  free_memory: number | null;
}

export interface FlowFile {
  flow: { base: string; integration: string; production: string; remote: string; naming: string[]; stale_days: number };
  run: { allow: string[]; max_turns: number; model: string | null; branch_prefix: string; brief: boolean; browser: boolean; sandbox: boolean };
  verify: { commands: string[]; judge: boolean; judge_model: string | null; max_rounds: number };
  ship?: { check: string | null };
}

export const api = {
  settings: () => invoke<Settings>("settings"),
  prefsSet: (prefs: Prefs) => invoke<void>("prefs_set", { prefs }),
  removeRepo: (path: string) => invoke<Settings>("remove_repo", { path }),
  flowGet: (root: string) => invoke<{ exists: boolean; path: string; flow: FlowFile }>("flow_get", { root }),
  flowSet: (root: string, table: string, key: string, value: unknown) => invoke<void>("flow_set", { root, table, key, value }),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  pitlaneHome: () => invoke<string>("pitlane_home"),
  addRepo: (path: string) => invoke<RepoEntry>("add_repo", { path }),
  selectRepo: (path: string) => invoke<void>("select_repo", { path }),
  yard: (path: string, fetch: boolean) => invoke<Yard>("yard", { path, fetch }),
  newLane: (path: string, branch: string) => invoke<NewReport>("new_lane", { path, branch }),
  shipPlan: (path: string, branch: string, to: string | null) =>
    invoke<ShipPlan>("ship_plan", { path, branch, to }),
  ship: (path: string, plan: ShipPlan) =>
    invoke<ShipReport>("ship", {
      path,
      branch: plan.branch,
      to: plan.target,
      sha: plan.sha,
      targetSha: plan.target_sha,
    }),
  cleanPlan: (path: string) => invoke<CleanPlan>("clean_plan", { path }),
  clean: (path: string, branches: string[]) => invoke<CleanOutcome[]>("clean", { path, branches }),
  undoClean: (path: string, branch: string) => invoke<UndoReport>("undo_clean", { path, branch }),
  cards: (path: string) => invoke<CardList>("cards_list", { path }),
  runs: (path: string) => invoke<RunMeta[]>("runs_list", { path }),
  sessions: (path: string) => invoke<RunMeta[]>("sessions_list", { path }),
  capacity: () => invoke<Capacity>("capacity"),
  perf: () => invoke<PerfSnapshot>("perf"),
  perfSet: (config: Partial<PerfConfig>) => invoke<{ config: PerfConfig }>("perf_set", { config }),
  presence: () => invoke<PresenceSnapshot>("presence"),
  presenceSet: (config: Partial<PresenceConfig>) => invoke<PresenceSnapshot>("presence_set", { config }),
  perfHold: (run: string, hold: boolean) => invoke<{ changed: boolean }>("perf_hold", { run, hold }),
  /** Sibling repositories this project also works in (flow.toml's [project] repos). */
  projectRepos: (path: string) => invoke<string[]>("project_repos", { path }),
  projectRepoRemove: (path: string, rel: string) => invoke<string[]>("project_repo_remove", { path, rel }),
  projectContext: (path: string) => invoke<string>("project_context", { path }),
  projectContextSet: (path: string, text: string) => invoke<string>("project_context_set", { path, text }),
  /** `autopilot`: Run all's cards, which fix, continue and land on their own. */
  /** `fresh`: start over instead of resuming the card's stopped session. */
  runStart: (path: string, card: number, allowApiKey = false, into: string | null = null, direct = false, instructions: string | null = null, autopilot = false, fresh = false) =>
    invoke<RunMeta>("run_start", { path, card, allowApiKey, into, direct, instructions, autopilot, fresh }),
  runsStopAnalyses: () => invoke<number>("runs_stop_analyses"),
  runAllFinish: (path: string, cards: number[], instructions: string, into: string | null) =>
    invoke<Finish>("run_all_finish", { path, cards, instructions, into }),
  reportCreate: (path: string, cards: number[], since: number, into: string | null, direct: boolean, instructions: string, finish: Finish | null) =>
    invoke<Report>("report_create", { path, cards, since, into, direct, instructions, finish }),
  reports: (path: string) => invoke<Report[]>("reports_list", { path }),
  reportDigest: (path: string, id: string) => invoke<Report>("report_digest", { path, id }),
  reportDelete: (id: string) => invoke<void>("report_delete", { id }),
  reportSuggestCard: (path: string, id: string, index: number) => invoke<Card>("report_suggest_card", { path, id, index }),
  reportRestore: (id: string) => invoke<void>("report_restore", { id }),
  printPage: () => invoke<void>("print_page"),
  cardsForget: (path: string, ids: number[]) => invoke<{ notes: number; questions: number }>("cards_forget", { path, ids }),
  cardDelete: (path: string, id: number) => invoke<void>("card_delete", { path, id }),
  cardRestore: (path: string, id: number) => invoke<void>("card_restore", { path, id }),
  boardAssist: (path: string, text: string) => invoke<void>("board_assist", { path, text }),
  runContinue: (run: string, allowApiKey: boolean) => invoke<RunMeta>("run_continue", { run, allowApiKey }),
  accounts: (path: string) => invoke<Accounts>("accounts_list", { path }),
  runVerification: (run: string) => invoke<Verification | null>("run_verification", { run }),
  laneDiff: (path: string, branch: string, from?: string) => invoke<LaneDiff>("lane_diff", { path, branch, from: from ?? null }),
  plan: (path: string) => invoke<Plan>("plan", { path }),
  dropHunk: (path: string, branch: string, base: string, head: string, file: string, header: string) =>
    invoke<string>("lane_drop_hunk", { path, branch, base, head, file, header }),
  queue: (path: string) => invoke<ShipJob[]>("queue_list", { path }),
  queuePaused: () => invoke<number>("queue_paused"),
  queueResume: () => invoke<void>("queue_resume"),
  enqueue: (path: string, branch: string, to?: string) => invoke<ShipJob>("queue_add", { path, branch, to: to ?? null }),
  dequeue: (id: string) => invoke<void>("queue_remove", { id }),
  cardReview: (path: string, card: number, action: "approve" | "changes" | "discard", message?: string) =>
    invoke<{ run?: RunMeta; shipped_to?: string; state?: string }>("card_review", { path, card, action, message: message ?? null }),
  accountUse: (path: string, id: string) => invoke<void>("account_use", { path, id }),
  runResume: (run: string) => invoke<RunMeta>("run_resume", { run }),
  runSay: (run: string, text: string) => invoke<void>("run_say", { run, text }),
  /** `/model` and `/effort` while the session lives. */
  runTune: (run: string, tune: { model?: string; effort?: string }) => invoke<void>("run_tune", { run, model: tune.model ?? null, effort: tune.effort ?? null }),
  runStop: (run: string) => invoke<void>("run_stop", { run }),
  runsStopAll: () => invoke<number>("runs_stop_all"),
  runDecide: (run: string, approval: string, decision: Decision, message?: string) =>
    invoke<void>("run_decide", { run, approval, decision, message: message ?? null }),
  runTimeline: (run: string) => invoke<[number, RunEvent][]>("run_timeline", { run }),
  runFollow: (run: string, from: number) => invoke<void>("run_follow", { run, from }),
  quota: (path: string) => invoke<Quota>("quota", { path }),
  cardAdd: (path: string, title: string, body: string) => invoke<Card>("card_add", { path, title, body }),
  cardUpdate: (
    path: string,
    id: number,
    patch: { title?: string; body?: string; state?: CardState; lane?: string },
  ) => invoke<Card>("card_update", { path, id, ...patch }),
  /** What you chose for a blocked card; the run it started, if any. */
  unblockCard: (path: string, id: number, action: "retry" | "review" | "backlog") => invoke<RunMeta | null>("card_unblock", { path, id, action }),
  cardReorder: (path: string, ids: number[]) => invoke<void>("card_reorder", { path, ids }),
  cardNote: (path: string, id: number, text: string) => invoke<Card>("card_note", { path, id, text }),
  cardsAnalyze: (path: string, ids: number[]) =>
    invoke<{ analyzed: { card: number; questions?: number; notes?: number; cost_usd?: number; error?: string }[] }>("cards_analyze", { path, ids }),
  cardAnswer: (path: string, id: number, question: number, answer: string) =>
    invoke<{ decision: string; card: Card }>("card_answer", { path, id, question, answer }),
  cardAttach: (path: string, id: number, name: string, data: string) => invoke<Card>("card_attach", { path, id, name, data }),
  cardDetach: (path: string, id: number, file: string) => invoke<Card>("card_detach", { path, id, file }),
  /** A data URL of one of the card's images. */
  cardImage: (path: string, id: number, file: string) => invoke<string>("card_image", { path, id, file }),
  runShot: (run: string, file: string) => invoke<string>("run_shot", { run, file }),
  /** Choose again on an answered question; its decision is rewritten in place. */
  cardReanswer: (path: string, id: number, question: number, answer: string) =>
    invoke<{ decision: string; card: Card }>("card_reanswer", { path, id, question, answer }),
  /** Talks a question over with Claude; resolves with the card once Claude has answered. */
  cardTalk: (path: string, id: number, question: number, message: string | null) =>
    invoke<{ card: Card; cost_usd?: number }>("card_talk", { path, id, question, message }),
  cardTalkStop: (path: string, id: number) => invoke<number>("card_talk_stop", { path, id }),
};

export const landedLabel: Record<Landed, string> = {
  ancestor: "merged",
  "patch-id": "rebased",
  "merge-tree": "squashed",
  "pull-request": "PR",
};

export const landedProof: Record<Landed, string> = {
  ancestor: "Its tip is an ancestor of the target (normal merge).",
  "patch-id": "Every commit has a patch-equivalent in the target (rebase or cherry-pick).",
  "merge-tree": "Merging it would not change the target's tree (squash, or any other way).",
  "pull-request": "GitHub shows a merged PR from this exact tip.",
};

export const short = (sha: string) => sha.slice(0, 8);

/** Dictation: the microphone, transcribed on this Mac by the bundled Whisper model. */
export const dictation = {
  available: inTauri,
  start: () => invoke<void>("dictate_start"),
  stop: () => invoke<string>("dictate_stop"),
  cancel: () => invoke<void>("dictate_cancel"),
  /** The microphone's loudness while listening, 0 to about 0.3, every 80ms. */
  onLevel: async (f: (level: number) => void): Promise<() => void> =>
    inTauri ? listen<number>("dictate-level", (e) => f(e.payload)) : () => {},
};
