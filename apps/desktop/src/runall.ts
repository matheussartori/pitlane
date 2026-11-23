// Run all, as a session: analyse the backlog cards (questions and notes from Claude), ask the
// open questions one at a time, then move them to Queued and work that column, starting what can
// start and keeping going as work lands, until it is empty. Ideas added to the Backlog meanwhile
// stay out of the run unless someone moves them to Queued. The planner speaks only here.
import { createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";
import { NEEDS_API_KEY, api, isLive, type Plan, type Finish, isOpenJob } from "./api";
import { holdNote, onItsWay, planNote } from "./board";
import { openQuestions } from "./cards";
import { setAgentsWatch } from "./agents";
import type { RunAllChoice } from "./sheets";
import { toast } from "./ui";
import { plural, t } from "./i18n";
import { msg } from "./format";
import { cards, jobs, loadCards, loadQueue, loadReports, loadRuns, repo, runs } from "./store/repo";
import type { SheetState, Tab } from "./App";

/** The shell's part in a session: the questions sheet, and the Reports screen at the end. */
interface Shell {
  sheet: () => SheetState;
  setSheet: (s: SheetState) => void;
  setTab: (t: Tab) => void;
}

/**
 * Every card the working batch took. A plain variable, not a signal, at module level so the shell
 * hands it to the Board as the same plain value it always did.
 */
export let sessionIds: number[] = [];
// The same cards, for the Board to follow while it is open (a batch can start meanwhile).
const [batchSig, setBatchSig] = createSignal<number[]>([]);
export const batchIds = batchSig;

/** Called once in the shell: the session's state, its steps, and the effects that keep it going. */
export function createRunAll(shell: Shell) {
  const { sheet, setSheet, setTab } = shell;

  // The planner only speaks during Run all: read when it starts and again as cards change.
  const [plan, setPlan] = createSignal<Plan | null>(null);

  const [phase, setPhase] = createSignal<"idle" | "analyzing" | "asking" | "working" | "finishing">("idle");
  // The cards the analysis reads, while it does.
  const [analysing, setAnalysing] = createSignal<number[]>([]);
  // The queue is the Queued column, kept by the daemon: it outlives a restart, and the batch
  // works nothing else. In board order, without the ones running now.
  const queue = createMemo(() => {
    const live = new Set((runs() ?? []).filter((r) => isLive(r.state)).map((r) => r.card));
    return (cards() ?? []).filter((c) => c.state === "queued" && !live.has(c.id)).map((c) => c.id);
  });
  let advancing = false;
  // While Run all works, the agents' list looks for new sessions every moment.
  createEffect(() => setAgentsWatch(phase() !== "idle"));

  // The session's target (Run all's Into) and a token: every await checks it is still the same
  // session, so Stop, a new Run all or a repository switch end the old one for good.
  let session = 0;
  let sessionPath: string | null = null;
  const [choice, setChoice] = createSignal<RunAllChoice>({ into: null, direct: false, instructions: "" });
  const current_ = (token: number) => token === session && repo() === sessionPath;

  /** Step one: Claude reads each card not analysed yet; its questions are asked one at a time. */
  async function analyse(ids: number[]) {
    const path = repo();
    if (!path || phase() !== "idle" || ids.length === 0) return;
    const token = ++session;
    sessionPath = path;
    setAnalysing(ids);
    setPhase("analyzing");
    try {
      const r = await api.cardsAnalyze(path, ids);
      if (!current_(token)) return;
      const failed = r.analyzed.filter((a) => a.error && a.error !== "stopped before it started");
      if (failed.length > 0)
        toast({ tone: "fail", title: t("Could not analyse {cards}", { cards: failed.map((f) => `#${f.card}`).join(", ") }), body: failed[0]!.error }, 0);
    } catch (e) {
      if (!current_(token)) return;
      toast({ tone: "fail", title: t("The analysis did not run"), body: msg(e) }, 0);
    }
    await loadCards();
    if (!current_(token) || phase() !== "analyzing") return; // stopped meanwhile
    const asking = openQuestions((cards() ?? []).filter((c) => analysing().includes(c.id)));
    if (asking.length > 0) {
      setPhase("asking");
      setSheet({ kind: "questions" });
    } else analysed();
  }

  /**
   * A card moved to Queued while Run all works: Claude reads it like the first batch (when it was
   * never read), its questions are asked, and the batch takes it once they are answered.
   */
  const [joining, setJoining] = createSignal<number[]>([]);
  async function analyseInto(ids: number[]) {
    const path = repo();
    if (!path || phase() !== "working" || ids.length === 0 || joining().length > 0) return;
    const token = session;
    const fresh = ids.filter((id) => !(cards() ?? []).find((c) => c.id === id)?.analyzed);
    setJoining(ids);
    try {
      if (fresh.length > 0) {
        const r = await api.cardsAnalyze(path, fresh);
        const failed = r.analyzed.filter((a) => a.error && a.error !== "stopped before it started");
        if (failed.length > 0)
          toast({ tone: "fail", title: t("Could not analyse {cards}", { cards: failed.map((f) => `#${f.card}`).join(", ") }), body: failed[0]!.error }, 0);
      }
    } catch (e) {
      toast({ tone: "fail", title: t("The analysis did not run"), body: msg(e) }, 0);
    } finally {
      setJoining([]);
    }
    await loadCards();
    if (!current_(token) || phase() !== "working") return;
    const all = cards() ?? [];
    const ok = ids.filter((id) => all.find((c) => c.id === id)?.analyzed);
    if (ok.length === 0) return;
    const asking = openQuestions(all.filter((c) => ok.includes(c.id)));
    toast({
      tone: "done",
      title: t("{cards} joined the batch", { cards: ok.map((x) => `#${x}`).join(", ") }),
      body: asking.length > 0 ? t("Answer their questions and they start.") : t("They start as soon as there is room."),
    });
    if (asking.length > 0) setSheet({ kind: "questions" });
    void advance();
  }

  /** The analysis is over (and its questions answered or left for later): development is next. */
  function analysed() {
    setPhase("idle");
    setAnalysing([]);
    session++;
    toast({ tone: "done", title: t("Analysis done"), body: t("Every card has its notes and decisions. Start development on the Board when you are ready.") });
  }

  /** Step two: the cards move to Queued, and the agents work that column into the chosen branch, in board order. */
  let sessionStart = 0;
  /** How many times each card came back to the queue and was taken again in this batch. */
  const again = new Map<number, number>();
  /** A card comes back at most this often: a run that never finishes it does not spend the night on it. */
  const AGAIN = 5;
  /** After a run ends, its next step (a fix round, landing) is the daemon's and takes a moment to show. */
  const SETTLE = 45;
  async function startDev(ids: number[], c: RunAllChoice) {
    const path = repo();
    if (!path || phase() !== "idle" || ids.length === 0) return;
    const token = ++session;
    sessionPath = path;
    // Into the Queued column first: what the batch works is on the board, not in this window.
    const moving = ids.filter((id) => (cards() ?? []).find((x) => x.id === id)?.state !== "queued");
    const moved = await Promise.allSettled(moving.map((id) => api.cardMove(path, id, "queue")));
    const failed = moving.filter((_, i) => moved[i]!.status === "rejected");
    if (failed.length > 0) toast({ tone: "fail", title: t("{cards} stayed in the Backlog", { cards: failed.map((x) => `#${x}`).join(", ") }), body: msg((moved.find((m) => m.status === "rejected") as PromiseRejectedResult).reason) }, 0);
    if (!current_(token)) return;
    await loadCards();
    sessionIds = ids.filter((id) => !failed.includes(id));
    setBatchSig(sessionIds);
    sessionStart = Math.floor(Date.now() / 1000) - 5;
    again.clear();
    setChoice(c);
    setPhase("working");
    try {
      const p = await api.plan(path);
      if (!current_(token)) return;
      setPlan(p);
    } catch {
      setPlan(null);
    }
    void advance();
  }

  /** Step three: what the person asked for at the end, then the report. */
  async function finishBatch() {
    const path = repo();
    if (!path || phase() !== "working" || path !== sessionPath) return;
    const token = session;
    const c = choice();
    setPhase("finishing");
    let finish: Finish | null = null;
    if (c.instructions) {
      toast({ tone: "info", title: t("The closing step started"), body: t("Claude does what you asked for the end. Follow it in Terminal › Agents.") });
      try {
        finish = await api.runAllFinish(path, sessionIds, c.instructions, c.into);
      } catch (e) {
        finish = { run: null, ok: false, summary: msg(e), cost_usd: null };
      }
    }
    if (token !== session) return;
    try {
      const r = await api.reportCreate(path, sessionIds, sessionStart, c.into, c.direct, c.instructions, finish);
      toast(
        { tone: finish && !finish.ok ? "fail" : "done", title: t("Run all is done"), body: plural(r.cards.length, "{n} card, {cost}. The report is ready.", "{n} cards, {cost}. The report is ready.", { cost: `$${r.cost_usd.toFixed(2)}` }), action: { label: t("Open the report"), run: () => setTab("reports") } },
        0,
      );
    } catch (e) {
      toast({ tone: "fail", title: t("The report was not written"), body: msg(e) }, 0);
    }
    session++;
    sessionPath = null;
    setPhase("idle");
    setPlan(null);
    void loadReports();
  }

  function stopRunAll() {
    const analysing = phase() === "analyzing";
    session++;
    sessionPath = null;
    setAnalysing([]);
    setPhase("idle");
    setPlan(null);
    if (sheet()?.kind === "questions") setSheet(null);
    // Stopping while Claude reads the cards stops those readings too (runs already going keep going).
    if (analysing) void api.runsStopAnalyses().catch(() => {});
  }

  /** Starts every Queued card that can start now: no open question, nothing that must land first, nothing it would collide with. */
  async function advance(allowApiKeyFor: number[] = []) {
    const path = repo();
    if (!path || phase() !== "working" || advancing || path !== sessionPath) return;
    const token = session;
    const { into, direct } = choice();
    advancing = true;
    try {
      const all = cards() ?? [];
      const liveCards = (runs() ?? []).filter((r) => isLive(r.state)).map((r) => r.card);
      const live = (id: number) => liveCards.includes(id);
      // The Queued column, minus what cannot go yet: a card Claude is still reading, one never
      // read, one whose run the person stopped in this batch (it waits for the next Start), and
      // one that came back too often (a run that never finishes it does not spend the night on it).
      const q = queue().filter((id) => {
        const c = all.find((x) => x.id === id);
        return !!c && !live(id) && !!c.analyzed && !joining().includes(id) && !stoppedHere(id) && (again.get(id) ?? 0) < AGAIN;
      });
      for (const id of q) if (!sessionIds.includes(id)) sessionIds = [...sessionIds, id];
      if (sessionIds.length !== batchSig().length) setBatchSig(sessionIds);
      // Directly in one branch: one card at a time, in board order.
      const busy = [...liveCards];
      // The planner's file clashes hold a card only for cards on their way to landing.
      const moving = onItsWay(all, liveCards, q, sessionIds);
      const ready = q.filter((id) => {
        const c = all.find((x) => x.id === id)!;
        return (c.questions ?? []).every((x) => x.answer != null) && !planNote(plan(), id, all, moving);
      });
      const started: number[] = [];
      // How many agents this Mac takes now (the setting, lowered while it is loaded).
      const room = await api.capacity().then((c) => c.agents, () => 3);
      const working = () => busy.filter((b) => (runs() ?? []).some((r) => r.card === b && isLive(r.state) && r.kind !== "analysis" && r.kind !== "judge")).length + started.length;
      for (const [i, id] of ready.entries()) {
        if (!current_(token)) return;
        if (working() >= room) break;
        if (direct && busy.some((b) => queue().includes(b) || all.find((x) => x.id === b)?.target === into)) break;
        if (holdNote(id, all, busy)) continue;
        try {
          const m = await api.runStart(path, id, allowApiKeyFor.includes(id), into, direct, choice().instructions || null, true);
          void api.runFollow(m.id, 0).catch(() => {});
          // Taken again in this batch: its run closed one stage of a larger card, or it was freed.
          if (ranHere(id)) again.set(id, (again.get(id) ?? 0) + 1);
          started.push(id);
          busy.push(id);
        } catch (e) {
          const text = msg(e);
          if (text.startsWith(NEEDS_API_KEY)) {
            const rest = ready.slice(i).filter((x) => !started.includes(x));
            toast(
              {
                tone: "info",
                title: t("Only the API key can take {cards} now", { cards: rest.map((x) => `#${x}`).join(", ") }),
                body: t("{why}. It is paid per token.", { why: text.slice(NEEDS_API_KEY.length) }),
                // Only these cards, and only while this Run all is still the one going.
                action: { label: t("Use the API key"), run: () => void (current_(token) && advance(rest)) },
              },
              0,
            );
            break;
          }
          toast({ tone: "fail", title: t("#{id} did not start", { id }), body: text.split("\n")[0] }, 0);
          q.splice(q.indexOf(id), 1);
        }
      }
      if (!current_(token)) return;
      const left = q.filter((id) => !started.includes(id));
      if (started.length > 0) {
        toast({ tone: "done", title: t("Started {cards}", { cards: started.map((x) => `#${x}`).join(", ") }), body: left.length > 0 ? t("{n} more in the queue.", { n: left.length }) : t("The queue is empty.") });
        await loadRuns();
        void loadCards();
      }
      // Nothing working and nothing landing: whatever is still waiting waits on something that will
      // not move by itself. Run all never stalls in silence: each such card goes to Blocked with why,
      // and the batch closes. A blocked card the person frees later still starts while it is open.
      const idle = () => !(runs() ?? []).some((r) => isLive(r.state)) && !jobs().some(isOpenJob);
      if (started.length === 0 && idle() && room > 0) {
        // Never on old news, and never in the moment after a run ends: what comes next for its
        // card (a fix round, landing) is the daemon's step and takes a moment to show. Judged
        // then, a card that was landing went to Blocked as "did not land".
        await Promise.allSettled([loadRuns(), loadQueue(), loadCards()]);
        if (!current_(token)) return;
        const ended = Math.max(0, ...(runs() ?? []).filter((r) => sessionIds.includes(r.card)).map((r) => r.ended_at ?? 0));
        if (!idle() || Date.now() / 1000 - ended < SETTLE) return; // the next pass looks again
        const now = cards() ?? [];
        const why = (id: number): string | null => {
          const c = now.find((x) => x.id === id);
          // Out of the batch (back in the Backlog), or waiting for the next Start (you stopped it).
          if (!c || c.state === "blocked" || c.state === "done" || c.state === "in-dev" || c.state === "inbox" || c.state === "ready") return null;
          if (c.state === "queued" && stoppedHere(id)) return null;
          if (c.state === "queued" && (again.get(id) ?? 0) >= AGAIN) return t("It came back {n} times without being finished.", { n: AGAIN });
          if (c.state === "queued" && !c.analyzed && !joining().includes(id)) return t("Claude could not analyse it.");
          if (c.state === "review") return t("Its work did not land in {into}. See the run and the notes above.", { into: into ?? t("its branch") });
          if (c.state === "running" || c.state === "verifying") return t("Its run ended without handing the card back.");
          // Open questions are yours already (Needs you): the batch waits for the answers.
          if ((c.questions ?? []).some((x) => x.answer == null)) return null;
          // Nothing is at work or landing by now: only what can still start counts as on its way.
          const held = holdNote(id, now, []) ?? planNote(plan(), id, now, onItsWay(now, [], queue(), []));
          return held ? `${held}.` : null;
        };
        const stuck = [...new Set([...queue(), ...sessionIds])].map((id) => [id, why(id)] as const).filter((x): x is readonly [number, string] => x[1] != null);
        for (const [id, reason] of stuck) {
          if (!current_(token)) return;
          try {
            await api.cardNote(path, id, t("Run all could not take this card further: {why} It waits for you in Blocked; the rest of the batch went on.", { why: reason }));
            await api.cardUpdate(path, id, { state: "blocked" });
          } catch {
            /* the card changed meanwhile: the next pass looks again */
          }
        }
        if (stuck.length > 0) {
          toast({ tone: "info", title: t("{cards} went to Blocked", { cards: stuck.map(([id]) => `#${id}`).join(", ") }), body: t("Each says why. The rest of the batch went on.") }, 0);
          await loadCards();
        }
        if (!current_(token)) return;
        const waiting = (cards() ?? []).filter((c) => sessionIds.includes(c.id) && (c.state === "running" || c.state === "verifying" || c.state === "review"));
        const startable = queue().filter((id) => !stoppedHere(id));
        if (waiting.length === 0 && startable.length === 0) void finishBatch();
      }
    } finally {
      advancing = false;
    }
  }

  /** The card's latest run in this batch, if it had one. */
  const lastHere = (id: number) => (runs() ?? []).filter((r) => r.card === id && r.started_at >= sessionStart).sort((x, y) => y.started_at - x.started_at)[0];
  const ranHere = (id: number) => !!lastHere(id);
  /** You stopped its run in this batch: it waits in Queued for the next Start. */
  const stoppedHere = (id: number) => lastHere(id)?.state === "stopped";

  // While the session works, every change to the cards re-reads the planner and starts what can.
  createEffect(
    on(cards, () => {
      if (phase() !== "working") return;
      const path = repo();
      if (!path) return;
      void api.plan(path).then(
        (p) => (setPlan(p), void advance()),
        () => void advance(),
      );
    }),
  );

  // The batch works with nobody looking, so it reads the board, the runs and the merge queue
  // itself: what the screen polls stops while the window is hidden, and a card that landed, or
  // came back, would only be seen when some other run happened to end. The Mac's room changes
  // on its own too (builds end, tests calm down).
  createEffect(() => {
    if (phase() !== "working") return;
    const tick = window.setInterval(() => void Promise.allSettled([loadCards(), loadRuns(), loadQueue()]).then(() => advance()), 15_000);
    onCleanup(() => clearInterval(tick));
  });

  return {
    plan,
    phase,
    queue,
    joining,
    analyse,
    analyseInto,
    analysed,
    startDev,
    finishBatch,
    stopRunAll,
  };
}
