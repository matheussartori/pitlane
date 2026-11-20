// How a lane reads on screen. One place, so the table and the Tower never disagree.
import { landedLabel, landedProof, type Lane, type Yard } from "./api";
import type { IconName } from "./icons";

/** `data-state` of .chip and .tower-row in design/components.css. */
export type ChipState = "needs" | "running" | "done" | "failed" | "queued";

export interface LaneView {
  chip: ChipState;
  icon: IconName;
  label: string;
  /** Waiting on the user: the only rows that light up. */
  needsYou: boolean;
}

export function laneView(lane: Lane, yard: Yard): LaneView {
  const integ = yard.integration ?? "integration";
  switch (lane.status) {
    case "contaminated":
      return {
        chip: "failed",
        icon: "fail",
        label: `Carries ${lane.integration_commits.length} ${integ} commit${lane.integration_commits.length === 1 ? "" : "s"}`,
        needsYou: true,
      };
    case "needs-production":
      return { chip: "needs", icon: "needs", label: `Ready for ${yard.production}`, needsYou: true };
    case "shipped-dirty":
      return { chip: "needs", icon: "needs", label: "Shipped, uncommitted", needsYou: true };
    case "can-delete":
      return { chip: "done", icon: "done", label: "Can delete", needsYou: false };
    case "shipped":
      return { chip: "done", icon: "done", label: "Shipped", needsYou: false };
    case "in-progress":
      return { chip: "running", icon: "run", label: "In progress", needsYou: false };
    case "new":
      return { chip: "queued", icon: "queued", label: "New", needsYou: false };
    case "stale":
      return { chip: "queued", icon: "queued", label: `Idle ${lane.idle_days}d`, needsYou: false };
  }
}

export interface SyncView {
  label: string;
  /** Colour role: done = all on the remote, needs = your move, failed = diverged. */
  tone: "done" | "needs" | "failed" | "muted" | "plain";
  icon: IconName | null;
  /** Long form for the tooltip. */
  detail: string;
}

/** Where the lane stands against its branch of the same name on the remote. */
export function syncView(lane: Lane, now: number): SyncView {
  const remote = `origin/${lane.branch}`;
  const atRisk = lane.unpushed > 0 && !lane.in_production;
  const pushed = lane.last_push ? ` You pushed ${ago(now - lane.last_push * 1000)}.` : "";
  const at = lane.remote_sha ? ` ${remote} is at ${lane.remote_sha.slice(0, 8)}.` : "";
  const s = lane.sync;
  let v: SyncView;
  switch (s.kind) {
    case "synced":
      v = { label: "Synced", tone: "done", icon: "done", detail: `Local and ${remote} are the same commit.${pushed}` };
      break;
    case "ahead":
      v = { label: `${s.ahead}↑ to push`, tone: "plain", icon: null, detail: `${s.ahead} local commit${s.ahead === 1 ? "" : "s"} not on ${remote} yet.${at}${pushed}` };
      break;
    case "behind":
      v = { label: `${s.behind}↓ to pull`, tone: "needs", icon: null, detail: `${remote} has ${s.behind} commit${s.behind === 1 ? "" : "s"} you don't have locally.${at}` };
      break;
    case "diverged":
      v = { label: `${s.ahead}↑ ${s.behind}↓ diverged`, tone: "failed", icon: null, detail: `Both sides moved: ${s.ahead} local commit${s.ahead === 1 ? "" : "s"} and ${s.behind} on ${remote}.${at}` };
      break;
    case "local-only":
      v = atRisk
        ? { label: `Local only, ${lane.unpushed}↑`, tone: "needs", icon: null, detail: `Never pushed. ${lane.unpushed} commit${lane.unpushed === 1 ? "" : "s"} exist only on this machine.` }
        : { label: "Local only", tone: "muted", icon: null, detail: "Never pushed, but every commit is already on some remote branch." };
      break;
    case "gone":
      v = atRisk
        ? { label: `Deleted on remote, ${lane.unpushed}↑`, tone: "needs", icon: null, detail: `${remote} was deleted. ${lane.unpushed} commit${lane.unpushed === 1 ? "" : "s"} exist only here.` }
        : { label: "Deleted on remote", tone: "muted", icon: null, detail: `${remote} was deleted; its commits are safe elsewhere.` };
      break;
    case "remote-only":
      v = { label: "Remote only", tone: "muted", icon: null, detail: `Only ${remote} exists; no local branch. Check it out to work on it.` };
      break;
  }
  if (lane.tracks_other) {
    v = { ...v, tone: v.tone === "failed" ? "failed" : "needs", detail: `${v.detail} Its upstream is ${lane.tracks_other}, another branch: git pull and git push go there.` };
  }
  return v;
}

/** The stage `Ship` would move this lane to, or null when shipping makes no sense now. */
export function shipTarget(lane: Lane, yard: Yard): string | null {
  if (lane.status === "needs-production") return yard.production;
  if (!lane.local || lane.status !== "in-progress" || lane.ahead === 0) return null;
  // Behind or diverged: pull first, shipping now would leave the remote's commits out.
  if (lane.sync.kind === "behind" || lane.sync.kind === "diverged") return null;
  return yard.integration ?? yard.production;
}

export function worktreeName(lane: Lane): string | null {
  const w = lane.worktree;
  if (!w) return null;
  if (w.is_main) return "main checkout";
  return w.path.split("/").filter(Boolean).pop() ?? w.path;
}

export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}

export const pad2 = (n: number) => String(n).padStart(2, "0");

// ---------------------------------------------------------------- groups: who acts next

export type Group = "waiting" | "progress" | "shipped" | "parked";
export const groupOrder: Group[] = ["waiting", "progress", "shipped", "parked"];
export const groupTitle: Record<Group, string> = {
  waiting: "Waiting on you",
  progress: "In progress",
  shipped: "Shipped",
  parked: "Parked",
};

export function group(l: Lane): Group {
  if (l.status === "contaminated" || l.status === "needs-production" || l.status === "shipped-dirty") return "waiting";
  if (l.sync.kind === "behind" || l.sync.kind === "diverged" || l.tracks_other) return "waiting";
  if (l.status === "can-delete" || l.status === "shipped") return "shipped";
  if (l.status === "stale") return "parked";
  return "progress";
}

// ---------------------------------------------------------------- the track

/** off: not there · on: there · next: your move takes it here (the only orange) · ahead/behind: a count to settle · gone: deleted on the remote · fail: wrong. */
export type NodeState = "off" | "on" | "next" | "count" | "gone" | "fail";
export interface TrackNode {
  stage: string;
  state: NodeState;
  /** A short figure under the node, e.g. 2↑. */
  figure?: string;
  title: string;
}

/** The lane's place on local → remote → integration → production. */
export function track(l: Lane, y: Yard, now: number): TrackNode[] {
  const s = syncView(l, now);
  const atRisk = l.unpushed > 0 && !l.in_production;
  const remote: TrackNode = { stage: "Remote", state: "off", title: s.detail };
  switch (l.sync.kind) {
    case "synced":
    case "remote-only":
      remote.state = "on";
      break;
    case "ahead":
      remote.state = "on";
      remote.figure = `${l.sync.ahead}↑`;
      break;
    case "behind":
      remote.state = "count";
      remote.figure = `${l.sync.behind}↓`;
      break;
    case "diverged":
      remote.state = "fail";
      remote.figure = `${l.sync.ahead}↑${l.sync.behind}↓`;
      break;
    case "local-only":
      if (atRisk) {
        remote.state = "count";
        remote.figure = `${l.unpushed}↑`;
      }
      break;
    case "gone":
      remote.state = "gone";
      remote.figure = "deleted";
      break;
  }
  if (l.tracks_other && remote.state === "off") remote.state = "count";

  const nodes: TrackNode[] = [
    {
      stage: "Local",
      state: l.local ? "on" : "off",
      title: l.local ? `Local branch at ${l.sha.slice(0, 8)}.` : "No local branch.",
    },
    remote,
  ];
  if (y.integration) {
    const contaminated = l.integration_commits.length > 0;
    nodes.push({
      stage: y.integration,
      state: contaminated ? "fail" : l.in_integration ? "on" : "off",
      title: contaminated
        ? `Carries commits that exist only in ${y.integration}: ${l.integration_commits.join(", ")}.`
        : l.in_integration
          ? `In ${y.integration}, ${landedLabel[l.in_integration]}. ${landedProof[l.in_integration]}`
          : `Not in ${y.integration}.`,
    });
  }
  nodes.push({
    stage: y.production,
    state: l.in_production ? "on" : l.status === "needs-production" ? "next" : "off",
    title: l.in_production
      ? `In ${y.production}, ${landedLabel[l.in_production]}. ${landedProof[l.in_production]}`
      : l.status === "needs-production"
        ? `Tested in ${y.integration}; ready for ${y.production}.`
        : `Not in ${y.production}.`,
  });
  return nodes;
}

// ---------------------------------------------------------------- the one line that says what is next

export interface Next {
  text: string;
  tone: "needs" | "failed" | "muted" | "plain";
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

export function next(l: Lane, y: Yard): Next | null {
  const integ = y.integration ?? "integration";
  if (l.integration_commits.length > 0)
    return { text: `Carries ${plural(l.integration_commits.length, `${integ} commit`)}; rebuild from ${y.production}`, tone: "failed" };
  if (l.sync.kind === "diverged") return { text: "Diverged from the remote", tone: "failed" };
  if (l.sync.kind === "behind") return { text: `Pull ${plural(l.sync.behind, "commit")} first`, tone: "needs" };
  if (l.tracks_other) return { text: `Tracks ${l.tracks_other}`, tone: "needs" };
  if (l.status === "shipped-dirty") return { text: "Uncommitted changes after shipping", tone: "needs" };
  if (l.status === "needs-production") return null; // the Ship button says it
  if (l.in_production) {
    const how = { ancestor: "Merged", "patch-id": "Rebased", "merge-tree": "Squashed", "pull-request": "PR merged" }[l.in_production];
    return { text: l.in_production === "pull-request" ? how : `${how} into ${y.production}`, tone: "muted" };
  }
  if (l.idle_days >= 2) return { text: `Idle ${plural(l.idle_days, "day")}`, tone: "muted" };
  return null;
}

/** Idle time for the table: blank while the lane moved today, so only quiet lanes draw the eye. */
export function idle(days: number): string {
  return days === 0 ? "" : `${days}d`;
}
