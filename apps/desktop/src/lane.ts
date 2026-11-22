// How a lane reads on screen. One place, so the table and the Tower never disagree.
import { landedLabel, landedProof, type Lane, type Yard } from "./api";
import type { IconName } from "./icons";
import { plural, t, tx } from "./i18n";

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
        label: plural(lane.integration_commits.length, "Carries {n} {integ} commit", "Carries {n} {integ} commits", { integ }),
        needsYou: true,
      };
    case "needs-production":
      // A release decision, not an alarm: calm until you choose to ship.
      return { chip: "queued", icon: "ship", label: t("Ready for {production}", { production: yard.production }), needsYou: false };
    case "shipped-dirty":
      return { chip: "needs", icon: "needs", label: t("Shipped, uncommitted"), needsYou: true };
    case "can-delete":
      return { chip: "done", icon: "done", label: t("Can delete"), needsYou: false };
    case "shipped":
      return { chip: "done", icon: "done", label: t("Shipped"), needsYou: false };
    case "in-progress":
      return { chip: "running", icon: "run", label: t("In progress"), needsYou: false };
    case "new":
      return { chip: "queued", icon: "queued", label: tx("New", "lane"), needsYou: false };
    case "stale":
      return { chip: "queued", icon: "queued", label: t("Idle {n}d", { n: lane.idle_days }), needsYou: false };
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
  const pushed = lane.last_push ? " " + t("You pushed {when}.", { when: ago(now - lane.last_push * 1000) }) : "";
  const at = lane.remote_sha ? " " + t("{remote} is at {sha}.", { remote, sha: lane.remote_sha.slice(0, 8) }) : "";
  const s = lane.sync;
  let v: SyncView;
  switch (s.kind) {
    case "synced":
      v = { label: t("Synced"), tone: "done", icon: "done", detail: t("Local and {remote} are the same commit.", { remote }) + pushed };
      break;
    case "ahead":
      v = { label: t("{n}↑ to push", { n: s.ahead }), tone: "plain", icon: null, detail: plural(s.ahead, "{n} local commit not on {remote} yet.", "{n} local commits not on {remote} yet.", { remote }) + at + pushed };
      break;
    case "behind":
      v = { label: t("{n}↓ to pull", { n: s.behind }), tone: "needs", icon: null, detail: plural(s.behind, "{remote} has {n} commit you don’t have locally.", "{remote} has {n} commits you don’t have locally.", { remote }) + at };
      break;
    case "diverged":
      v = { label: t("{ahead}↑ {behind}↓ diverged", { ahead: s.ahead, behind: s.behind }), tone: "failed", icon: null, detail: plural(s.ahead, "Both sides moved: {ahead} local commit and {behind} on {remote}.", "Both sides moved: {ahead} local commits and {behind} on {remote}.", { ahead: s.ahead, behind: s.behind, remote }) + at };
      break;
    case "local-only":
      v = atRisk
        ? { label: t("Local only, {n}↑", { n: lane.unpushed }), tone: "needs", icon: null, detail: plural(lane.unpushed, "Never pushed. {n} commit exists only on this machine.", "Never pushed. {n} commits exist only on this machine.") }
        : { label: t("Local only"), tone: "muted", icon: null, detail: t("Never pushed, but every commit is already on some remote branch.") };
      break;
    case "gone":
      v = atRisk
        ? { label: t("Deleted on remote, {n}↑", { n: lane.unpushed }), tone: "needs", icon: null, detail: plural(lane.unpushed, "{remote} was deleted. {n} commit exists only here.", "{remote} was deleted. {n} commits exist only here.", { remote }) }
        : { label: t("Deleted on remote"), tone: "muted", icon: null, detail: t("{remote} was deleted; its commits are safe elsewhere.", { remote }) };
      break;
    case "remote-only":
      v = { label: t("Remote only"), tone: "muted", icon: null, detail: t("Only {remote} exists; no local branch. Check it out to work on it.", { remote }) };
      break;
  }
  if (lane.tracks_other) {
    v = { ...v, tone: v.tone === "failed" ? "failed" : "needs", detail: `${v.detail} ${t("Its upstream is {other}, another branch: git pull and git push go there.", { other: lane.tracks_other })}` };
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
  if (w.is_main) return t("main checkout");
  return w.path.split("/").filter(Boolean).pop() ?? w.path;
}

export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 45) return t("just now");
  const m = Math.round(s / 60);
  if (m < 60) return t("{n}m ago", { n: m });
  const h = Math.round(m / 60);
  if (h < 24) return t("{n}h ago", { n: h });
  return t("{n}d ago", { n: Math.round(h / 24) });
}

export const pad2 = (n: number) => String(n).padStart(2, "0");

// ---------------------------------------------------------------- groups: who acts next

export type Group = "waiting" | "progress" | "release" | "shipped" | "parked";
export const groupOrder: Group[] = ["waiting", "progress", "release", "shipped", "parked"];
export const groupTitle: Record<Group, string> = {
  waiting: "Waiting on you",
  progress: "In progress",
  release: "Ready to release",
  shipped: "Shipped",
  parked: "Parked",
};

/** Waiting on you is only what is stuck without you; a lane ready for production waits for a release, calmly. */
export function group(l: Lane): Group {
  if (l.status === "contaminated" || l.status === "shipped-dirty") return "waiting";
  if (l.sync.kind === "behind" || l.sync.kind === "diverged" || l.tracks_other) return "waiting";
  if (l.status === "can-delete" || l.status === "shipped") return "shipped";
  if (l.status === "stale") return "parked";
  if (l.status === "needs-production") return "release";
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
  const remote: TrackNode = { stage: t("Remote"), state: "off", title: s.detail };
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
      remote.figure = t("deleted");
      break;
  }
  if (l.tracks_other && remote.state === "off") remote.state = "count";

  const nodes: TrackNode[] = [
    {
      stage: t("Local"),
      state: l.local ? "on" : "off",
      title: l.local ? t("Local branch at {sha}.", { sha: l.sha.slice(0, 8) }) : t("No local branch."),
    },
    remote,
  ];
  if (y.integration) {
    const contaminated = l.integration_commits.length > 0;
    nodes.push({
      stage: y.integration,
      state: contaminated ? "fail" : l.in_integration ? "on" : "off",
      title: contaminated
        ? t("Carries commits that exist only in {integ}: {commits}.", { integ: y.integration, commits: l.integration_commits.join(", ") })
        : l.in_integration
          ? t("In {stage}, {how}.", { stage: y.integration, how: t(landedLabel[l.in_integration]) }) + " " + t(landedProof[l.in_integration])
          : t("Not in {stage}.", { stage: y.integration }),
    });
  }
  nodes.push({
    stage: y.production,
    state: l.in_production ? "on" : l.status === "needs-production" ? "next" : "off",
    title: l.in_production
      ? t("In {stage}, {how}.", { stage: y.production, how: t(landedLabel[l.in_production]) }) + " " + t(landedProof[l.in_production])
      : l.status === "needs-production"
        ? t("Tested in {integ}; ready for {production}.", { integ: y.integration ?? "", production: y.production })
        : t("Not in {stage}.", { stage: y.production }),
  });
  return nodes;
}

// ---------------------------------------------------------------- the one line that says what is next

export interface Next {
  text: string;
  tone: "needs" | "failed" | "muted" | "plain";
}

export function next(l: Lane, y: Yard): Next | null {
  const integ = y.integration ?? "integration";
  if (l.integration_commits.length > 0)
    return { text: plural(l.integration_commits.length, "Carries {n} {integ} commit; rebuild from {production}", "Carries {n} {integ} commits; rebuild from {production}", { integ, production: y.production }), tone: "failed" };
  if (l.sync.kind === "diverged") return { text: t("Diverged from the remote"), tone: "failed" };
  if (l.sync.kind === "behind") return { text: plural(l.sync.behind, "Pull {n} commit first", "Pull {n} commits first"), tone: "needs" };
  if (l.status === "shipped-dirty") return { text: t("Uncommitted changes after shipping"), tone: "needs" };
  if (l.status === "needs-production") return null; // the Ship button says it
  if (l.in_production) {
    const how = { ancestor: t("Merged into {production}", { production: y.production }), "patch-id": t("Rebased into {production}", { production: y.production }), "merge-tree": t("Squashed into {production}", { production: y.production }), "pull-request": t("PR merged") }[l.in_production];
    return { text: how, tone: "muted" };
  }
  if (l.idle_days >= 2) return { text: plural(l.idle_days, "Idle {n} day", "Idle {n} days"), tone: "muted" };
  return null;
}

/** Idle time for the table: blank while the lane moved today, so only quiet lanes draw the eye. */
export function idle(days: number): string {
  return days === 0 ? "" : `${days}d`;
}
