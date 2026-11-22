// Where a card sits on the board and why. The card file says what someone set; the lane it is
// linked to says where the work really is. The board groups by who acts next.
import type { Card, CardState, Lane, RunMeta, Yard } from "./api";
import type { IconName } from "./icons";
import { group, next, type ChipState } from "./lane";
import { plural, t } from "./i18n";

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

export function laneOf(card: Card, yard: Yard | null): Lane | undefined {
  // Worked directly in its target (Run all's "directly"), the branch is shared, not the card's:
  // what that branch holds or where it landed says nothing about this card, so its state decides.
  if (!card.lane || card.lane === card.target) return undefined;
  return yard?.lanes.find((l) => l.branch === card.lane);
}

export function place(card: Card, yard: Yard | null): Placement {
  const lane = laneOf(card, yard);
  const open = card.questions?.filter((q) => q.answer == null) ?? [];

  if (open.length > 0)
    return { column: "needs", chip: "needs", icon: "needs", label: plural(open.length, "{n} question", "{n} questions"), why: open[0]!.text };
  if (card.state === "blocked") return { column: "needs", chip: "needs", icon: "blocked", label: t("Blocked"), why: firstLine(card.body) };
  if (lane && yard && group(lane) === "waiting") {
    const n = next(lane, yard);
    const label = lane.status === "contaminated" ? t("Mixed with {name}", { name: yard.integration ?? "integration" }) : t("Lane needs you");
    return {
      column: "needs",
      chip: lane.status === "contaminated" ? "failed" : "needs",
      icon: lane.status === "contaminated" ? "fail" : "needs",
      label,
      why: n?.text ?? t("Tested in {name}, waiting to ship.", { name: yard.integration ?? "integration" }),
    };
  }
  if (card.state === "review")
    return { column: "needs", chip: "needs", icon: "review", label: t("Ready to review") };
  if (card.state === "verifying") return { column: "progress", chip: "running", icon: "verify", label: t("Checking") };
  if (card.state === "done" || lane?.in_production)
    return { column: "landed", chip: "done", icon: "done", label: t("In {name}", { name: yard?.production ?? "main" }) };
  if (card.state === "in-dev" || lane?.in_integration)
    return { column: "landed", chip: "done", icon: "done", label: t("In {name}", { name: yard?.integration ?? "dev" }) };
  if (card.state === "running" || lane)
    return { column: "progress", chip: "running", icon: "run", label: card.state === "running" ? t("Running") : t("In a lane") };
  return { column: "backlog", chip: "queued", icon: card.state === "ready" ? "done" : "queued", label: t(stateLabel[card.state]) };
}

function firstLine(body: string): string | undefined {
  const line = body.split("\n").map((l) => l.replace(/^#+\s*/, "").trim()).find(Boolean);
  return line || undefined;
}

/** What the checks found on a card's latest run, in one sentence, and how loud to say it. */
export function reviewSummary(run: RunMeta | undefined): { text: string; chip: ChipState; icon: IconName; label: string } {
  const parts = [
    run?.proofs_ok === true ? t("checks pass") : run?.proofs_ok === false ? t("a check fails") : null,
    run?.verdict === "approve" ? t("the judge approves") : run?.verdict === "changes" ? t("the judge asks for changes") : null,
  ].filter(Boolean) as string[];
  const text = parts.length ? parts.join(", ") + "." : t("Not checked; read the diff.");
  const sentence = text.charAt(0).toUpperCase() + text.slice(1);
  if (run?.proofs_ok === false) return { text: sentence, chip: "failed", icon: "fail", label: t("Check fails") };
  if (run?.verdict === "changes") return { text: sentence, chip: "needs", icon: "review", label: t("Judge asks changes") };
  return { text: sentence, chip: "needs", icon: "review", label: t("Ready to review") };
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
  context?: string;
  /** 1-based position among the card's open questions, and how many are open. */
  index: number;
  of: number;
}

export function openQuestions(cards: Card[]): OpenQuestion[] {
  return cards.flatMap((card) => {
    const cached = asked.get(card);
    if (cached) return cached;
    const open = (card.questions ?? []).filter((q) => q.answer == null);
    const qs = open.map((q, i) => ({
      card,
      id: q.id,
      text: q.text,
      options: q.options ?? [],
      recommended: q.recommended,
      context: q.context,
      index: i + 1,
      of: open.length,
    }));
    asked.set(card, qs);
    return qs;
  });
}

// One list per card object: with keepUnchanged, a re-read that changed nothing hands the lists
// the same questions, so they keep their DOM (and whatever is being typed into it).
const asked = new WeakMap<Card, OpenQuestion[]>();

/** The fresh cards, reusing each old object whose content did not change. */
export function keepUnchanged(old: Card[] | null, fresh: Card[]): Card[] {
  if (!old) return fresh;
  const before = new Map(old.map((c) => [c.id, c]));
  const next = fresh.map((c) => {
    const o = before.get(c.id);
    return o && JSON.stringify(o) === JSON.stringify(c) ? o : c;
  });
  return next.length === old.length && next.every((c, i) => c === old[i]) ? old : next;
}
