// Mirrors the serde shapes of pit-core and the Tauri commands in src-tauri/src/lib.rs.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

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

export type CardState = "inbox" | "ready" | "running" | "review" | "in-dev" | "done" | "blocked";
export const cardStates: CardState[] = ["inbox", "ready", "running", "review", "in-dev", "done", "blocked"];

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
