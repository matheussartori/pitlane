// Run all, as a session: analyse the backlog cards (questions and notes from Claude), ask the
// open questions one at a time, then move them to Queued and work that column, starting what can
// start and keeping going as work lands, until it is empty. Ideas added to the Backlog meanwhile
// stay out of the run unless someone moves them to Queued. The planner speaks only here.
//
// Each repository has its own session, and they go at once: switching to another repository
// leaves the one working where it was, still starting its cards. The Mac's agents are one pool:
// the repositories working share "Agents at once" (lowered while the Mac is loaded), and a
// repository with nothing ready lends its share to the others.
import { createEffect, createMemo, createSignal, on, type Accessor, type Setter } from "solid-js";
import { NEEDS_API_KEY, api, isLive, type Card, type Plan, type Finish, type RunMeta, type ShipJob, isOpenJob } from "./api";
import { holdNote, onItsWay, openQuestions, planNote } from "./cards";
import { setAgentsWatch } from "./store/agents";
import { toast, type Toast } from "./ui";
import { plural, t } from "./i18n";
import { money, msg } from "./format";
import { cards, jobs, loadCards, loadQueue, loadReports, loadRuns, repo, runs } from "./store/repo";
import type { SheetState, Tab } from "./App";

/** Where Run all lands the work, as chosen in its sheet. */
export interface RunAllChoice {
  /** null: the repository's flow (a lane per card from the base, shipped through each stage). */
  into: string | null;
  direct: boolean;
  /** What happens around and after the work, in the person's words; every agent reads it, and a
   * closing session does the "when it is all done" part. */
  instructions: string;
}

/** The shell's part in a session: the questions sheet, the Reports screen, and going to a repository. */
interface Shell {
  sheet: () => SheetState;
  setSheet: (s: SheetState) => void;
  setTab: (t: Tab) => void;
  /** Opens another repository in the window (its session, if any, keeps going either way). */
  switchTo: (path: string) => void | Promise<void>;
  /** The repository's name as the menu shows it. */
  nameOf: (path: string) => string | undefined;
}

type Phase = "idle" | "analyzing" | "asking" | "working" | "finishing";

/** One repository's Run all. */
interface Batch {
  path: string;
  name: string;
  phase: Accessor<Phase>;
  setPhase: Setter<Phase>;
  plan: Accessor<Plan | null>;
  setPlan: Setter<Plan | null>;
  /** The cards the analysis reads, while it does. */
  analysing: Accessor<number[]>;
  setAnalysing: Setter<number[]>;
  /** Cards moved to Queued while it works, being read before the batch takes them. */
  joining: Accessor<number[]>;
  setJoining: Setter<number[]>;
  choice: RunAllChoice;
  /** Every card the working batch took. */
  ids: Accessor<number[]>;
  setIds: Setter<number[]>;
  /** When development started (seconds), for this batch's runs and the report. */
  start: number;
  /** When it started working (ms): the older batch gets the odd agent of a split. */
  since: number;
  /** How many times each card came back to the queue and was taken again in this batch. */
  again: Map<number, number>;
  /** Bumped by Stop, the end and a new session: every await checks it is still the same one. */
  token: number;
  advancing: boolean;
  /** Asked to advance while it was: one more pass when this one ends (with the cards the API key was allowed for). */
  rerun: number[] | null;
  /** It has cards ready and fewer agents than its share: the others leave it the free ones. */
  hungry: boolean;
  tick: number | null;
  // The repository as this batch last read it: the screen's own data when it is the one open.
  cards: Card[];
  runs: RunMeta[];
  jobs: ShipJob[];
}

/** A card comes back at most this often: a run that never finishes it does not spend the night on it. */
const AGAIN = 5;
/** After a run ends, its next step (a fix round, landing) is the daemon's and takes a moment to show. */
const SETTLE = 45;

const [batches, setBatches] = createSignal<Batch[]>([]);
/** "Agents at once" as the last pass read it (the setting, lowered while the Mac is loaded). */
const [room, setRoom] = createSignal(0);

const here = () => batches().find((b) => b.path === repo());
/** The cards the open repository's working batch took, for the Board to follow. */
export const batchIds = () => here()?.ids() ?? [];
/** Where Run all is in a repository, for the repository menu. */
export const phaseIn = (path: string): Phase => batches().find((b) => b.path === path)?.phase() ?? "idle";

const workingBatches = () =>
  batches()
    .filter((b) => b.phase() === "working")
    .sort((x, y) => x.since - y.since);

/** A working batch's part of the agents: an even split, the odd ones to the batches that started first. */
function shareOf(b: Batch, total: number): number {
  const list = workingBatches();
  const i = list.indexOf(b);
  if (list.length <= 1 || i < 0) return total;
  return Math.floor(total / list.length) + (i < total % list.length ? 1 : 0);
}

/** The open repository's part of the agents while more than one repository works, for the Board. */
export function agentShare(): { mine: number; total: number; repos: number } | null {
  const b = here();
  const list = workingBatches();
  if (!b || b.phase() !== "working" || list.length < 2 || room() < 1) return null;
  return { mine: shareOf(b, room()), total: room(), repos: list.length };
}

const baseName = (path: string) => path.replace(/\/+$/, "").split("/").pop() || path;

/** Called once in the shell: the sessions' state, their steps, and the effects that keep them going. */
export function createRunAll(shell: Shell) {
  const { sheet, setSheet, setTab, switchTo, nameOf } = shell;

  // While a Run all is on anywhere, the agents' list looks for new sessions every moment.
  createEffect(() => setAgentsWatch(batches().some((b) => b.phase() !== "idle")));

  // The open repository's queue: the Queued column, kept by the daemon (it outlives a restart),
  // in board order, without the ones running now.
  const queue = createMemo(() => queueOf(cards() ?? [], runs() ?? []));
  function queueOf(all: Card[], rs: RunMeta[]) {
    const live = new Set(rs.filter((r) => isLive(r.state)).map((r) => r.card));
    return all.filter((c) => c.state === "queued" && !live.has(c.id)).map((c) => c.id);
  }

  const isHere = (b: Batch) => b.path === repo();
  const alive = (b: Batch, token: number) => token === b.token && batches().includes(b);
  /** A toast about a batch: when it is not the repository on screen, the title names it. */
  function say(b: Batch, x: Omit<Toast, "id">, ms?: number) {
    toast(isHere(b) ? x : { ...x, title: `${b.name} · ${x.title}` }, ms);
  }

  function create(path: string): Batch {
    const [phase, setPhase] = createSignal<Phase>("idle");
    const [plan, setPlan] = createSignal<Plan | null>(null);
    const [analysing, setAnalysing] = createSignal<number[]>([]);
    const [joining, setJoining] = createSignal<number[]>([]);
    const [ids, setIds] = createSignal<number[]>([]);
    const b: Batch = {
      path,
      name: nameOf(path) ?? baseName(path),
      phase,
      setPhase,
      plan,
      setPlan,
      analysing,
      setAnalysing,
      joining,
      setJoining,
      choice: { into: null, direct: false, instructions: "" },
      ids,
      setIds,
      start: 0,
      since: 0,
      again: new Map(),
      token: 0,
      advancing: false,
      rerun: null,
      hungry: false,
      tick: null,
      cards: [],
      runs: [],
      jobs: [],
    };
    setBatches((l) => [...l.filter((x) => x.path !== path), b]);
    return b;
  }

  /** The batch is over (stopped, analysed or finished): nothing of it runs any more. */
  function end(b: Batch) {
    b.token++;
    if (b.tick != null) clearInterval(b.tick);
    b.tick = null;
    b.setPhase("idle");
    b.setPlan(null);
    b.setAnalysing([]);
    b.hungry = false;
    setBatches((l) => l.filter((x) => x !== b));
    // Its agents go to the others now, without waiting for their next look.
    for (const x of workingBatches()) void advance(x);
  }

  /** What the batch knows of its repository: the screen's data when it is open, the daemon's otherwise. */
  async function read(b: Batch, fresh: boolean) {
    if (isHere(b)) {
      if (fresh) await Promise.allSettled([loadCards(), loadRuns(), loadQueue()]);
      if (!isHere(b)) return read(b, true);
      b.cards = cards() ?? b.cards;
      b.runs = runs() ?? b.runs;
      b.jobs = jobs();
      return;
    }
    const [c, r, q] = await Promise.allSettled([api.cards(b.path), api.runs(b.path), api.queue(b.path)]);
    if (c.status === "fulfilled") b.cards = c.value.cards;
    if (r.status === "fulfilled") {
      b.runs = r.value;
      for (const m of r.value) if (isLive(m.state)) void api.runFollow(m.id, 0).catch(() => {});
    }
    if (q.status === "fulfilled") b.jobs = q.value;
  }

  /** The batch works with nobody looking: it reads its repository itself, every 15 s. */
  function working(b: Batch) {
    b.since = Date.now();
    b.setPhase("working");
    if (b.tick != null) clearInterval(b.tick);
    // What the screen polls stops while the window is hidden, and a card that landed, or came
    // back, would only be seen when some other run happened to end. The Mac's room changes on
    // its own too (builds end, tests calm down).
    b.tick = window.setInterval(() => {
      if (b.phase() !== "working") return;
      void read(b, true).then(async () => {
        // The open repository re-reads the planner as its cards change; another one does it here.
        if (!isHere(b)) await api.plan(b.path).then((p) => b.setPlan(p), () => {});
        void advance(b);
      });
    }, 15_000);
  }

  /** Step one: Claude reads each card not analysed yet; its questions are asked one at a time. */
  async function analyse(ids: number[]) {
    const path = repo();
    if (!path || here() || ids.length === 0) return;
    const b = create(path);
    const token = ++b.token;
    b.setAnalysing(ids);
    b.setPhase("analyzing");
    try {
      const r = await api.cardsAnalyze(path, ids);
      if (!alive(b, token)) return;
      const failed = r.analyzed.filter((a) => a.error && a.error !== "stopped before it started");
      if (failed.length > 0)
        say(b, { tone: "fail", title: t("Could not analyse {cards}", { cards: failed.map((f) => `#${f.card}`).join(", ") }), body: failed[0]!.error }, 0);
    } catch (e) {
      if (!alive(b, token)) return;
      say(b, { tone: "fail", title: t("The analysis did not run"), body: msg(e) }, 0);
    }
    await read(b, true);
    if (!alive(b, token) || b.phase() !== "analyzing") return; // stopped meanwhile
    const asking = openQuestions(b.cards.filter((c) => b.analysing().includes(c.id)));
    if (asking.length === 0) return analysedIn(b);
    b.setPhase("asking");
    if (isHere(b)) setSheet({ kind: "questions" });
    else
      say(
        b,
        { tone: "info", title: t("Claude has questions"), body: t("The analysis is done. Answer them and the batch can start."), action: { label: t("Answer"), run: () => void switchTo(b.path) } },
        0,
      );
  }

  // Back in a repository whose analysis waits for answers: its questions come up.
  createEffect(
    on(repo, () => {
      if (here()?.phase() === "asking" && sheet()?.kind !== "questions") setSheet({ kind: "questions" });
    }),
  );

  /**
   * A card moved to Queued while Run all works: Claude reads it like the first batch (when it was
   * never read), its questions are asked, and the batch takes it once they are answered.
   */
  async function analyseInto(ids: number[]) {
    const b = here();
    if (!b || b.phase() !== "working" || ids.length === 0 || b.joining().length > 0) return;
    const token = b.token;
    const fresh = ids.filter((id) => !(cards() ?? []).find((c) => c.id === id)?.analyzed);
    b.setJoining(ids);
    try {
      if (fresh.length > 0) {
        const r = await api.cardsAnalyze(b.path, fresh);
        const failed = r.analyzed.filter((a) => a.error && a.error !== "stopped before it started");
        if (failed.length > 0)
          say(b, { tone: "fail", title: t("Could not analyse {cards}", { cards: failed.map((f) => `#${f.card}`).join(", ") }), body: failed[0]!.error }, 0);
      }
    } catch (e) {
      say(b, { tone: "fail", title: t("The analysis did not run"), body: msg(e) }, 0);
    } finally {
      b.setJoining([]);
    }
    await read(b, true);
    if (!alive(b, token) || b.phase() !== "working") return;
    const ok = ids.filter((id) => b.cards.find((c) => c.id === id)?.analyzed);
    if (ok.length === 0) return;
    const asking = openQuestions(b.cards.filter((c) => ok.includes(c.id)));
    say(b, {
      tone: "done",
      title: t("{cards} joined the batch", { cards: ok.map((x) => `#${x}`).join(", ") }),
      body: asking.length > 0 ? t("Answer their questions and they start.") : t("They start as soon as there is room."),
    });
    if (asking.length > 0 && isHere(b)) setSheet({ kind: "questions" });
    void advance(b);
  }

  /** The analysis is over (and its questions answered or left for later): development is next. */
  function analysedIn(b: Batch) {
    end(b);
    say(b, { tone: "done", title: t("Analysis done"), body: t("Every card has its notes and decisions. Start development on the Board when you are ready.") });
  }
  function analysed() {
    const b = here();
    if (b) analysedIn(b);
  }

  /** Step two: the cards move to Queued, and the agents work that column into the chosen branch, in board order. */
  async function startDev(ids: number[], c: RunAllChoice) {
    const path = repo();
    if (!path || here() || ids.length === 0) return;
    const b = create(path);
    const token = ++b.token;
    // Into the Queued column first: what the batch works is on the board, not in this window.
    const moving = ids.filter((id) => (cards() ?? []).find((x) => x.id === id)?.state !== "queued");
    const moved = await Promise.allSettled(moving.map((id) => api.cardMove(path, id, "queue")));
    const failed = moving.filter((_, i) => moved[i]!.status === "rejected");
    if (failed.length > 0) say(b, { tone: "fail", title: t("{cards} stayed in the Backlog", { cards: failed.map((x) => `#${x}`).join(", ") }), body: msg((moved.find((m) => m.status === "rejected") as PromiseRejectedResult).reason) }, 0);
    if (!alive(b, token)) return;
    await read(b, true);
    b.setIds(ids.filter((id) => !failed.includes(id)));
    b.start = Math.floor(Date.now() / 1000) - 5;
    b.again.clear();
    b.choice = c;
    working(b);
    void announceSplit(b);
    try {
      const p = await api.plan(path);
      if (!alive(b, token)) return;
      b.setPlan(p);
    } catch {
      b.setPlan(null);
    }
    void advance(b);
  }

  /** A second repository starts working: say how the agents are split now. */
  async function announceSplit(b: Batch) {
    const list = workingBatches();
    if (list.length < 2) return;
    const total = await api.capacity().then((c) => c.agents, () => room() || 3);
    setRoom(total);
    const parts = list.map((x) => `${x.name} ${shareOf(x, total)}`).join(" · ");
    const over = list.filter((x) => x !== b && x.runs.filter((r) => isLive(r.state) && r.kind !== "analysis" && r.kind !== "judge" && r.kind !== "aside").length > shareOf(x, total));
    toast(
      {
        tone: "info",
        title: t("{n} repositories share the agents", { n: list.length }),
        body:
          t("{parts}, of {total} at once.", { parts, total }) +
          " " +
          (over.length > 0
            ? t("{repos} starts no new card until it is down to its share; its agents at work finish what they do.", { repos: over.map((x) => x.name).join(", ") })
            : t("A repository with nothing ready lends its share to the others.")),
      },
      0,
    );
  }

  /** Step three: what the person asked for at the end, then the report. */
  async function finishIn(b: Batch) {
    if (b.phase() !== "working") return;
    const token = b.token;
    const c = b.choice;
    b.setPhase("finishing");
    if (b.tick != null) clearInterval(b.tick);
    b.tick = null;
    // The agents it leaves go to the others.
    for (const x of workingBatches()) void advance(x);
    let finish: Finish | null = null;
    if (c.instructions) {
      say(b, { tone: "info", title: t("The closing step started"), body: t("Claude does what you asked for the end. Follow it in Terminal › Agents.") });
      try {
        finish = await api.runAllFinish(b.path, b.ids(), c.instructions, c.into);
      } catch (e) {
        finish = { run: null, ok: false, summary: msg(e), cost_usd: null };
      }
    }
    if (!alive(b, token)) return;
    try {
      const r = await api.reportCreate(b.path, b.ids(), b.start, c.into, c.direct, c.instructions, finish);
      say(
        b,
        {
          tone: finish && !finish.ok ? "fail" : "done",
          title: t("Run all is done"),
          body: plural(r.cards.length, "{n} card, {cost}. The report is ready.", "{n} cards, {cost}. The report is ready.", { cost: money(r.cost_usd) }),
          action: {
            label: t("Open the report"),
            run: () => {
              if (isHere(b)) return setTab("reports");
              void Promise.resolve(switchTo(b.path)).then(() => setTab("reports"));
            },
          },
        },
        0,
      );
    } catch (e) {
      say(b, { tone: "fail", title: t("The report was not written"), body: msg(e) }, 0);
    }
    end(b);
    if (isHere(b)) void loadReports();
  }
  async function finishBatch() {
    const b = here();
    if (b) await finishIn(b);
  }

  function stopRunAll() {
    const b = here();
    if (!b) return;
    const wasAnalysing = b.phase() === "analyzing";
    end(b);
    if (sheet()?.kind === "questions") setSheet(null);
    // Stopping while Claude reads the cards stops those readings too (runs already going keep going).
    if (!wasAnalysing) return;
    // Another repository reading its cards keeps reading: only this one's readings stop then.
    if (!batches().some((x) => x.phase() === "analyzing" || x.joining().length > 0)) void api.runsStopAnalyses().catch(() => {});
    else
      void api.sessions(b.path).then(
        (ss) => {
          for (const s of ss) if (s.kind === "analysis" && isLive(s.state)) void api.runStop(s.id).catch(() => {});
        },
        () => {},
      );
  }

  /** Agent sessions working cards on the whole Mac now, every repository counted. */
  async function onTheMac(): Promise<number> {
    try {
      const p = await api.perf();
      if (p.agents) return p.agents.filter((a) => a.kind === "run").length;
    } catch {
      /* no governor to ask: count the batches' own */
    }
    return batches().reduce((n, x) => n + x.runs.filter((r) => isLive(r.state) && r.kind !== "analysis" && r.kind !== "judge" && r.kind !== "aside").length, 0);
  }

  /** Advance passes, every repository's, one at a time: each counts the agents on the Mac
   *  before it starts more, so two repositories passing together never both take the last room.
   *  Resolves with the function that lets the next one go. */
  let passing: Promise<void> = Promise.resolve();
  function nextPass(): Promise<() => void> {
    let release!: () => void;
    const done = new Promise<void>((r) => (release = r));
    const turn = passing;
    passing = turn.then(() => done);
    return turn.then(() => release);
  }

  /** Starts every Queued card that can start now: no open question, nothing that must land first, nothing it would collide with, and room for it. */
  async function advance(b: Batch, allowApiKeyFor: number[] = []) {
    if (b.phase() !== "working" || !batches().includes(b)) return;
    // A trigger that comes mid-pass (a card landed, agents freed, the API key allowed) is not
    // dropped until the next tick: the pass runs once more when this one ends.
    if (b.advancing) {
      b.rerun = [...(b.rerun ?? []), ...allowApiKeyFor];
      return;
    }
    const token = b.token;
    const { into, direct } = b.choice;
    b.advancing = true;
    let release = () => {};
    try {
      release = await nextPass();
      await read(b, false);
      if (!alive(b, token)) return;
      const all = b.cards;
      const liveCards = b.runs.filter((r) => isLive(r.state)).map((r) => r.card);
      const live = (id: number) => liveCards.includes(id);
      const lastHere = (id: number) => b.runs.filter((r) => r.card === id && r.started_at >= b.start).sort((x, y) => y.started_at - x.started_at)[0];
      /** You stopped its run in this batch: it waits in Queued for the next Start. */
      const stoppedHere = (id: number) => lastHere(id)?.state === "stopped";
      const queueNow = () => queueOf(b.cards, b.runs);
      // The Queued column, minus what cannot go yet: a card Claude is still reading, one never
      // read, one whose run the person stopped in this batch (it waits for the next Start), and
      // one that came back too often (a run that never finishes it does not spend the night on it).
      const q = queueNow().filter((id) => {
        const c = all.find((x) => x.id === id);
        return !!c && !live(id) && !!c.analyzed && !b.joining().includes(id) && !stoppedHere(id) && (b.again.get(id) ?? 0) < AGAIN;
      });
      const taken = b.ids();
      const more = q.filter((id) => !taken.includes(id));
      if (more.length > 0) b.setIds([...taken, ...more]);
      // Directly in one branch: one card at a time, in board order.
      const busy = [...liveCards];
      // The planner's file clashes hold a card only for cards on their way to landing.
      const moving = onItsWay(all, liveCards, q, b.ids());
      const ready = q.filter((id) => {
        const c = all.find((x) => x.id === id)!;
        return (c.questions ?? []).every((x) => x.answer != null) && !planNote(b.plan(), id, all, moving);
      });
      const started: number[] = [];
      // How many agents this Mac takes now (the setting, lowered while it is loaded), shared by
      // every repository working; and how many work now, in all of them.
      const total = await api.capacity().then((c) => c.agents, () => 3);
      setRoom(total);
      const before = ready.length > 0 ? await onTheMac() : 0;
      if (!alive(b, token)) return;
      const share = shareOf(b, total);
      const others = () => workingBatches().filter((x) => x !== b && x.hungry);
      const mine = () => busy.filter((id) => b.runs.some((r) => r.card === id && isLive(r.state) && r.kind !== "analysis" && r.kind !== "judge" && r.kind !== "aside")).length + started.length;
      let wanting = false;
      for (const [i, id] of ready.entries()) {
        if (!alive(b, token)) return;
        // The Mac is full, or this repository has its share and another one waits for room.
        if (before + started.length >= total || mine() >= total) {
          wanting = mine() < share;
          break;
        }
        if (mine() >= share && others().length > 0) {
          for (const x of others()) void advance(x);
          break;
        }
        if (direct && busy.some((x) => queueNow().includes(x) || all.find((c) => c.id === x)?.target === into)) break;
        if (holdNote(id, all, busy)) continue;
        try {
          const m = await api.runStart(b.path, id, allowApiKeyFor.includes(id), into, direct, b.choice.instructions || null, true);
          void api.runFollow(m.id, 0).catch(() => {});
          // Taken again in this batch: its run closed one stage of a larger card, or it was freed.
          if (lastHere(id)) b.again.set(id, (b.again.get(id) ?? 0) + 1);
          started.push(id);
          busy.push(id);
        } catch (e) {
          const text = msg(e);
          if (text.startsWith(NEEDS_API_KEY)) {
            const rest = ready.slice(i).filter((x) => !started.includes(x));
            say(
              b,
              {
                tone: "info",
                title: t("Only the API key can take {cards} now", { cards: rest.map((x) => `#${x}`).join(", ") }),
                body: t("{why}. It is paid per token.", { why: text.slice(NEEDS_API_KEY.length) }),
                // Only these cards, and only while this Run all is still the one going.
                action: { label: t("Use the API key"), run: () => void (alive(b, token) && advance(b, rest)) },
              },
              0,
            );
            break;
          }
          say(b, { tone: "fail", title: t("#{id} did not start", { id }), body: text.split("\n")[0] }, 0);
          q.splice(q.indexOf(id), 1);
        }
      }
      b.hungry = wanting;
      if (!alive(b, token)) return;
      const left = q.filter((id) => !started.includes(id));
      if (started.length > 0) {
        say(b, { tone: "done", title: t("Started {cards}", { cards: started.map((x) => `#${x}`).join(", ") }), body: left.length > 0 ? t("{n} more in the queue.", { n: left.length }) : t("The queue is empty.") });
        await read(b, true);
      }
      // Nothing working and nothing landing: whatever is still waiting waits on something that will
      // not move by itself. Run all never stalls in silence: each such card goes to Blocked with why,
      // and the batch closes. A blocked card the person frees later still starts while it is open.
      const idle = () => !b.runs.some((r) => isLive(r.state)) && !b.jobs.some(isOpenJob);
      if (started.length === 0 && idle() && total > 0) {
        // Never on old news, and never in the moment after a run ends: what comes next for its
        // card (a fix round, landing) is the daemon's step and takes a moment to show. Judged
        // then, a card that was landing went to Blocked as "did not land".
        await read(b, true);
        if (!alive(b, token)) return;
        const ended = Math.max(0, ...b.runs.filter((r) => b.ids().includes(r.card)).map((r) => r.ended_at ?? 0));
        if (!idle() || Date.now() / 1000 - ended < SETTLE) return; // the next pass looks again
        const now = b.cards;
        const why = (id: number): string | null => {
          const c = now.find((x) => x.id === id);
          // Out of the batch (back in the Backlog), or waiting for the next Start (you stopped it).
          if (!c || c.state === "blocked" || c.state === "done" || c.state === "in-dev" || c.state === "inbox" || c.state === "ready") return null;
          if (c.state === "queued" && stoppedHere(id)) return null;
          if (c.state === "queued" && (b.again.get(id) ?? 0) >= AGAIN) return t("It came back {n} times without being finished.", { n: AGAIN });
          if (c.state === "queued" && !c.analyzed && !b.joining().includes(id)) return t("Claude could not analyse it.");
          if (c.state === "review") return t("Its work did not land in {into}. See the run and the notes above.", { into: into ?? t("its branch") });
          if (c.state === "running" || c.state === "verifying") return t("Its run ended without handing the card back.");
          // Open questions are yours already (Needs you): the batch waits for the answers.
          if ((c.questions ?? []).some((x) => x.answer == null)) return null;
          // Nothing is at work or landing by now: only what can still start counts as on its way.
          const held = holdNote(id, now, []) ?? planNote(b.plan(), id, now, onItsWay(now, [], queueNow(), []));
          return held ? `${held}.` : null;
        };
        const stuck = [...new Set([...queueNow(), ...b.ids()])].map((id) => [id, why(id)] as const).filter((x): x is readonly [number, string] => x[1] != null);
        for (const [id, reason] of stuck) {
          if (!alive(b, token)) return;
          try {
            await api.cardNote(b.path, id, t("Run all could not take this card further: {why} It waits for you in Blocked; the rest of the batch went on.", { why: reason }));
            await api.cardUpdate(b.path, id, { state: "blocked" });
          } catch {
            /* the card changed meanwhile: the next pass looks again */
          }
        }
        if (stuck.length > 0) {
          say(b, { tone: "info", title: t("{cards} went to Blocked", { cards: stuck.map(([id]) => `#${id}`).join(", ") }), body: t("Each says why. The rest of the batch went on.") }, 0);
          await read(b, true);
        }
        if (!alive(b, token)) return;
        const waiting = b.cards.filter((c) => b.ids().includes(c.id) && (c.state === "running" || c.state === "verifying" || c.state === "review"));
        const startable = queueNow().filter((id) => !stoppedHere(id));
        if (waiting.length === 0 && startable.length === 0) void finishIn(b);
      }
    } finally {
      release();
      b.advancing = false;
      const again = b.rerun;
      b.rerun = null;
      if (again && alive(b, token)) void advance(b, again);
    }
  }

  // While the open repository's batch works, every change to its cards re-reads the planner and starts what can.
  createEffect(
    on(cards, () => {
      const b = here();
      if (!b || b.phase() !== "working") return;
      void api.plan(b.path).then(
        (p) => (b.setPlan(p), void advance(b)),
        () => void advance(b),
      );
    }),
  );

  return {
    plan: () => here()?.plan() ?? null,
    phase: (): Phase => here()?.phase() ?? "idle",
    queue,
    joining: () => here()?.joining() ?? [],
    analyse,
    analyseInto,
    analysed,
    startDev,
    finishBatch,
    stopRunAll,
  };
}
