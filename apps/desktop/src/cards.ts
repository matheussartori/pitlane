// Where a card sits on the board and why. The card file says what someone set; the lane it is
// linked to says where the work really is. The board groups by who acts next.
import type { BlockReason, Card, CardState, Lane, MoveTo, RunMeta, Turn, Yard } from "./api";
import type { IconName } from "./icons";
import { group, next, type ChipState } from "./lane";
import { locale, plural, t } from "./i18n";

export type Column = "needs" | "backlog" | "queued" | "progress" | "landed";
export const columns: Column[] = ["needs", "backlog", "queued", "progress", "landed"];
export const columnTitle: Record<Column, string> = {
  needs: "Needs you",
  backlog: "Backlog",
  queued: "Queued",
  progress: "In progress",
  landed: "Landed",
};
export const columnIcon: Record<Column, IconName> = { needs: "needs", backlog: "inbox", queued: "queued", progress: "run", landed: "done" };
export const columnEmpty: Record<Column, string> = {
  needs: "Cards with open questions or a lane waiting on you show here.",
  backlog: "Ideas wait here. No run takes them until you move them to Queued.",
  queued: "Run all works these, in order. Start moves the analysed Backlog here; drag a card in to add it to the run.",
  progress: "Cards being worked on, in a lane or by an agent.",
  landed: "Cards finished and committed in the branch they were asked for.",
};

export const stateLabel: Record<CardState, string> = {
  inbox: "New",
  ready: "Ready",
  queued: "Queued",
  running: "Running",
  verifying: "Checking",
  review: "In review",
  "in-dev": "In dev",
  done: "Done",
  blocked: "Blocked",
};

/** What blocked a card, on its tile. */
export const blockLabel: Record<BlockReason, string> = {
  questions: "Blocked on questions",
  stuck: "Agent stuck",
  failed: "Run failed",
  "no-route": "No account free",
  "api-key": "Waits for the API key",
  conflict: "Conflict",
  checks: "Checks fail",
  other: "Blocked",
};

export type UnblockAction = "retry" | "review" | "backlog";

/** What you can do about a blocked card, the first being what usually frees it. None while it
 *  is being read again, or while a question is open (answering is the way out then). */
export function unblockActions(card: Card): { action: UnblockAction; label: string; title: string; icon: IconName }[] {
  const b = card.blocked;
  if (card.state !== "blocked" || b?.checking || (card.questions ?? []).some((q) => q.answer == null)) return [];
  const retry: Record<BlockReason, [string, string]> = {
    questions: ["Read again", "Read the card again with your answers; it goes back to the Backlog when nothing else is open"],
    stuck: ["Try again", "Run it again; the card's notes say where the last run stopped"],
    failed: ["Try again", "Run it again; a paused session picks up where it was"],
    "no-route": ["Try now", "Run it now, if an account is free"],
    "api-key": ["Use the API key", "Run it on the API key, paid per token"],
    conflict: ["Resolve the conflict", "A run brings the branch it lands in into the lane and resolves the conflict"],
    checks: ["Another round", "Another fix round on what the checks and the judge said"],
    other: ["Run again", "Run it again"],
  };
  const reason = b?.reason ?? "other";
  const [label, title] = retry[reason];
  const out: { action: UnblockAction; label: string; title: string; icon: IconName }[] = [{ action: "retry", label, title, icon: "refresh" }];
  if (card.lane && (reason === "checks" || reason === "conflict" || reason === "stuck"))
    out.push({ action: "review", label: "Review as it is", title: "Leave it for your review, as the lane holds it now", icon: "review" });
  out.push({ action: "backlog", label: "Back to Backlog", title: "Back to Backlog: its next run picks up from your notes and its lane", icon: "inbox" });
  return out;
}

/** A move a person can make, as the daemon's rules allow it (crates/pit-core/src/cards.rs `move_rule`). */
export interface Move {
  to: MoveTo;
  /** The column the card lands in. */
  column: Column;
  label: string;
  /** A running card stops first: ask before. */
  stops?: boolean;
}

/** Where a person can move this card, the first being the likeliest. `live`: a run of it is going. */
export function movesFor(card: Card, live: boolean): Move[] {
  const backlog: Move = { to: "backlog", column: "backlog", label: "Backlog" };
  const queue: Move = { to: "queue", column: "queued", label: "Queued" };
  const hold: Move = { to: "hold", column: "backlog", label: "Hold in the Backlog" };
  switch (card.state) {
    case "verifying":
      return [];
    case "running":
      return [{ ...queue, stops: live }, { ...backlog, stops: live }];
    case "inbox":
    case "ready":
      return card.held != null ? [queue, { ...backlog, label: "Release the hold" }] : [queue, hold];
    case "queued":
      return [backlog, hold];
    case "blocked": {
      const stuck = !!card.blocked?.checking || (card.questions ?? []).some((q) => q.answer == null);
      return stuck ? [backlog] : [queue, backlog];
    }
    case "review":
      return [{ ...backlog, label: "Backlog (discard the review)" }];
    case "done":
    case "in-dev":
      return [{ ...backlog, label: "Backlog (open again)" }];
  }
}

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
  if (card.state === "blocked") {
    const b = card.blocked;
    // Answered: Pitlane reads it again, and it goes back to the Backlog on its own when nothing else is open.
    if (b?.checking) return { column: "needs", chip: "running", icon: "verify", label: t(b.reason === "stuck" ? "Working out what it needs" : "Rereading with your answers"), why: b.detail || about(card) };
    const why = b?.detail || about(card);
    // Pitlane tries it again by itself: say when.
    if (b?.retry_at) {
      const at = new Date(b.retry_at * 1000).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" });
      return { column: "needs", chip: "queued", icon: "refresh", label: t("Tries again at {time}", { time: at }), why };
    }
    return { column: "needs", chip: "needs", icon: "blocked", label: t(blockLabel[b?.reason ?? "other"]), why };
  }
  // Landed in the branch it was asked for (Run all's Into): done, whatever its own lane waits on.
  if (card.target && (card.state === "done" || card.state === "in-dev"))
    return { column: "landed", chip: "done", icon: "done", label: t("In {name}", { name: card.target }) };
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
    return { column: "landed", chip: "done", icon: "done", label: t("In {name}", { name: card.target ?? yard?.production ?? "main" }) };
  if (card.state === "in-dev" || lane?.in_integration)
    return { column: "landed", chip: "done", icon: "done", label: t("In {name}", { name: card.target ?? yard?.integration ?? "dev" }) };
  // In Run all's queue: the batch takes it in board order.
  if (card.state === "queued")
    return card.paused
      ? { column: "queued", chip: "queued", icon: "pause", label: t("Paused"), why: about(card) }
      : { column: "queued", chip: "queued", icon: "queued", label: t("Queued"), why: about(card) };
  // Ready or inbox with a lane: sent back to the backlog (from Blocked, say); its next run goes on in that lane.
  if (card.state === "running" || (lane && card.state !== "ready" && card.state !== "inbox"))
    return { column: "progress", chip: "running", icon: "run", label: card.state === "running" ? t("Running") : t("In a lane") };
  // Held by you: Analyse and Start leave it out.
  if (card.held != null) return { column: "backlog", chip: "queued", icon: "pause", label: t("On hold"), why: card.held || about(card) };
  // Stopped mid-work: its next run resumes the session, with all the agent did and said.
  if (card.paused) return { column: "backlog", chip: "queued", icon: "pause", label: t("Paused"), why: about(card) };
  return { column: "backlog", chip: "queued", icon: card.state === "ready" ? "done" : "queued", label: t(stateLabel[card.state]), why: about(card) };
}

/** What the card is about, for its tile: Claude's summary, else the spec's first line. */
export function about(card: Card): string | undefined {
  return card.summary?.trim() || firstLine(card.body);
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

/** Every open question across the cards: the ones an agent waits on now first, then oldest card first. */
export interface OpenQuestion {
  card: Card;
  id: number;
  /** Its agent asked it mid-run and waits on the answer now (Presence: Here). */
  live: boolean;
  /** The run that asked it, and when, for a live one. */
  run?: string;
  askedAt?: string;
  text: string;
  options: string[];
  recommended?: number;
  context?: string;
  talk: Turn[];
  /** 1-based position among the card's open questions, and how many are open. */
  index: number;
  of: number;
}

export function openQuestions(cards: Card[]): OpenQuestion[] {
  const all = cards.flatMap((card) => {
    const cached = asked.get(card);
    if (cached) return cached;
    const open = (card.questions ?? []).filter((q) => q.answer == null);
    const qs = open.map((q, i) => ({
      card,
      id: q.id,
      live: !!q.run && card.state === "running",
      run: q.run,
      askedAt: q.asked_at,
      text: q.text,
      options: q.options ?? [],
      recommended: q.recommended,
      context: q.context,
      talk: q.talk ?? [],
      index: i + 1,
      of: open.length,
    }));
    asked.set(card, qs);
    return qs;
  });
  // A card whose agent waits mid-run goes first; the order stays otherwise.
  return all.some((q) => q.live) ? [...all.filter((q) => q.live), ...all.filter((q) => !q.live)] : all;
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
