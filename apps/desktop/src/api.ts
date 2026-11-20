// Mirrors the serde shapes of pit-core and the Tauri commands in src-tauri/src/lib.rs.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Tauri in the app; in a plain browser during development, the preview fixture. */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) return tauriInvoke<T>(cmd, args);
  if (import.meta.env.DEV) return (await import("./dev/preview")).preview<T>(cmd);
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
  answer?: string;
  answered_at?: string;
  decision?: string;
}

export interface Card {
  id: number;
  title: string;
  state: CardState;
  lane?: string;
  tags?: string[];
  created: string;
  updated: string;
  questions?: Question[];
  body: string;
  open_questions: number;
  file: string;
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
  next_steps?: string[];
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
  /** The account the route chose (`default` is the normal Claude Code login). */
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
}

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
  | { kind: "text"; text: string }
  | { kind: "tool"; id: string; name: string; detail: string }
  | { kind: "tool-done"; id: string; ok: boolean; detail: string }
  | { kind: "said"; text: string }
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
}

export const isOpenJob = (j: ShipJob) => j.state === "queued" || j.state === "shipping";

export interface RepoEntry {
  path: string;
  name: string;
}

export interface Settings {
  repos: RepoEntry[];
  last: string | null;
}

export const api = {
  settings: () => invoke<Settings>("settings"),
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
  runStart: (path: string, card: number, allowApiKey = false) => invoke<RunMeta>("run_start", { path, card, allowApiKey }),
  runContinue: (run: string, allowApiKey: boolean) => invoke<RunMeta>("run_continue", { run, allowApiKey }),
  accounts: (path: string) => invoke<Accounts>("accounts_list", { path }),
  runVerification: (run: string) => invoke<Verification | null>("run_verification", { run }),
  laneDiff: (path: string, branch: string, from?: string) => invoke<LaneDiff>("lane_diff", { path, branch, from: from ?? null }),
  queue: (path: string) => invoke<ShipJob[]>("queue_list", { path }),
  enqueue: (path: string, branch: string, to?: string) => invoke<ShipJob>("queue_add", { path, branch, to: to ?? null }),
  dequeue: (id: string) => invoke<void>("queue_remove", { id }),
  cardReview: (path: string, card: number, action: "approve" | "changes" | "discard", message?: string) =>
    invoke<{ run?: RunMeta; shipped_to?: string; state?: string }>("card_review", { path, card, action, message: message ?? null }),
  accountUse: (path: string, id: string) => invoke<void>("account_use", { path, id }),
  runResume: (run: string) => invoke<RunMeta>("run_resume", { run }),
  runSay: (run: string, text: string) => invoke<void>("run_say", { run, text }),
  runStop: (run: string) => invoke<void>("run_stop", { run }),
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
  cardAnswer: (path: string, id: number, question: number, answer: string) =>
    invoke<{ decision: string; card: Card }>("card_answer", { path, id, question, answer }),
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
