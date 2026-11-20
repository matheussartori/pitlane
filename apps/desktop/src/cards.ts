// Where a card sits on the board and why. The card file says what someone set; the lane it is
// linked to says where the work really is. The board groups by who acts next.
import type { Card, CardState, Lane, RunMeta, Yard } from "./api";
import type { IconName } from "./icons";
import { group, next, type ChipState } from "./lane";

export type Column = "needs" | "backlog" | "progress" | "landed";
export const columns: Column[] = ["needs", "backlog", "progress", "landed"];
export const columnTitle: Record<Column, string> = {
  needs: "Needs you",
  backlog: "Backlog",
  progress: "In progress",
  landed: "Landed",
};
export const columnEmpty: Record<Column, string> = {
  needs: "Cards with open questions or a lane waiting on you show here.",
  backlog: "New and ready cards wait here.",
  progress: "Cards being worked on, in a lane or by an agent.",
  landed: "Cards whose lane reached dev or main.",
};

export const stateLabel: Record<CardState, string> = {
  inbox: "New",
  ready: "Ready",
  running: "Running",
  verifying: "Checking",
  review: "In review",
  "in-dev": "In dev",
  done: "Done",
  blocked: "Blocked",
};

export interface Placement {
  column: Column;
  chip: ChipState;
  icon: IconName;
  label: string;
  /** One line under the title: why it waits, or where it is. */
  why?: string;
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

export function laneOf(card: Card, yard: Yard | null): Lane | undefined {
  return card.lane ? yard?.lanes.find((l) => l.branch === card.lane) : undefined;
}

export function place(card: Card, yard: Yard | null): Placement {
  const lane = laneOf(card, yard);
  const open = card.questions?.filter((q) => q.answer == null) ?? [];

  if (open.length > 0)
    return { column: "needs", chip: "needs", icon: "needs", label: plural(open.length, "question"), why: open[0]!.text };
  if (card.state === "blocked") return { column: "needs", chip: "needs", icon: "blocked", label: "Blocked", why: firstLine(card.body) };
  if (lane && yard && group(lane) === "waiting") {
    const n = next(lane, yard);
    const label = lane.status === "contaminated" ? `Mixed with ${yard.integration ?? "integration"}` : "Lane needs you";
    return {
      column: "needs",
      chip: lane.status === "contaminated" ? "failed" : "needs",
      icon: lane.status === "contaminated" ? "fail" : "needs",
      label,
      why: n?.text ?? `Tested in ${yard.integration ?? "integration"}, waiting to ship.`,
    };
  }
  if (card.state === "review")
    return { column: "needs", chip: "needs", icon: "review", label: "Ready to review" };
  if (card.state === "verifying") return { column: "progress", chip: "running", icon: "verify", label: "Checking" };
  if (card.state === "done" || lane?.in_production)
    return { column: "landed", chip: "done", icon: "done", label: `In ${yard?.production ?? "main"}` };
  if (card.state === "in-dev" || lane?.in_integration)
    return { column: "landed", chip: "done", icon: "done", label: `In ${yard?.integration ?? "dev"}` };
  if (card.state === "running" || lane)
    return { column: "progress", chip: "running", icon: "run", label: card.state === "running" ? "Running" : "In a lane" };
  return { column: "backlog", chip: "queued", icon: card.state === "ready" ? "done" : "queued", label: stateLabel[card.state] };
}

function firstLine(body: string): string | undefined {
  const line = body.split("\n").map((l) => l.replace(/^#+\s*/, "").trim()).find(Boolean);
  return line || undefined;
}

/** What the checks found on a card's latest run, in one sentence, and how loud to say it. */
export function reviewSummary(run: RunMeta | undefined): { text: string; chip: ChipState; icon: IconName; label: string } {
  const parts = [
    run?.proofs_ok === true ? "checks pass" : run?.proofs_ok === false ? "a check fails" : null,
    run?.verdict === "approve" ? "the judge approves" : run?.verdict === "changes" ? "the judge asks for changes" : null,
  ].filter(Boolean) as string[];
  const text = parts.length ? parts.join(", ") + "." : "Not checked; read the diff.";
  const sentence = text.charAt(0).toUpperCase() + text.slice(1);
  if (run?.proofs_ok === false) return { text: sentence, chip: "failed", icon: "fail", label: "Check fails" };
  if (run?.verdict === "changes") return { text: sentence, chip: "needs", icon: "review", label: "Judge asks changes" };
  return { text: sentence, chip: "needs", icon: "review", label: "Ready to review" };
}

/** A card's latest run: the highest review round, then the newest. */
export function latestRun(runs: RunMeta[], card: number): RunMeta | undefined {
  return runs.filter((r) => r.card === card).sort((a, b) => (b.round ?? 0) - (a.round ?? 0) || b.started_at - a.started_at)[0];
}

/** Every open question across the cards, oldest card first. */
export interface OpenQuestion {
  card: Card;
  id: number;
  text: string;
  options: string[];
  recommended?: number;
  /** 1-based position among the card's open questions, and how many are open. */
  index: number;
  of: number;
}

export function openQuestions(cards: Card[]): OpenQuestion[] {
  return cards.flatMap((card) => {
    const open = (card.questions ?? []).filter((q) => q.answer == null);
    return open.map((q, i) => ({
      card,
      id: q.id,
      text: q.text,
      options: q.options ?? [],
      recommended: q.recommended,
      index: i + 1,
      of: open.length,
    }));
  });
}
