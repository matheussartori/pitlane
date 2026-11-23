// Board: cards grouped by who acts next. A card linked to a lane carries that lane's track.
import { For, Show, createEffect, createMemo, createSignal, on, type JSX } from "solid-js";
import { api, cardStates, isLive, type Card, type CardState, type Plan, type RunMeta, type Yard } from "./api";
import { clock, elapsed, money, runStateView } from "./run";
import { columnEmpty, columnTitle, columns, latestRun, openQuestions, place, reviewSummary, stateLabel } from "./cards";
import { Icon } from "./icons";
import { Dictate } from "./dictate";
import { CardImages, attachImages, imagesIn } from "./images";
import { Linked } from "./links";
import { Prose, inline } from "./prose";
import { refreshAgents } from "./agents";
import { DecisionItem, QuestionAbout, QuestionBlock, QuestionTalk, talking } from "./inbox";
import { track } from "./lane";
import { ErrorLine, Sheet, Spinner, toast } from "./ui";
import { CARD_SIDE_W, Grip } from "./layout";
import { locale, plural, t } from "./i18n";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** The one stop worth naming at card size: your move, a count to settle, or what is wrong. */
function stopNote(nodes: { stage: string; state: string; figure?: string }[]): string | null {
  const n = nodes.find((x) => x.state === "fail") ?? nodes.find((x) => x.state === "next") ?? nodes.find((x) => x.state === "count");
  if (!n) return null;
  if (n.state === "next") return t("next: {stage}", { stage: n.stage });
  if (n.state === "fail") return t("mixed in {stage}", { stage: n.stage });
  return n.figure ?? n.stage;
}

/** The lane's track at card size: four stops and the one that matters, in words. */
function MiniTrack(props: { card: Card; yard: Yard | null; now: number }) {
  return <LaneTrack branch={props.card.lane} yard={props.yard} now={props.now} />;
}

/** The track of the lane on `branch`, small, with the one stop that matters in words. */
export function LaneTrack(props: { branch: string | undefined; yard: Yard | null; now: number }) {
  const lane = () => (props.branch ? props.yard?.lanes.find((l) => l.branch === props.branch) : undefined);
  return (
    <Show when={lane() && props.yard ? lane() : null}>
      {(l) => (
        <span class="mini-track">
          <span
            class="rail rail-mini"
            role="img"
            aria-label={t("Lane {branch}: {stops}", { branch: l().branch, stops: track(l(), props.yard!, props.now).map((n) => `${n.stage} ${n.state === "on" ? t("reached") : n.state === "off" ? t("not reached") : n.state}`).join(", ") })}
          >
            <For each={track(l(), props.yard!, props.now)}>
              {(n) => (
                <span class="node" data-state={n.state} title={`${n.stage}: ${n.title}`}>
                  <span class="dot" aria-hidden="true" />
                </span>
              )}
            </For>
          </span>
          <Show when={stopNote(track(l(), props.yard!, props.now))}>
            {(note) => <span class="stop-note" aria-hidden="true">{note()}</span>}
          </Show>
        </span>
      )}
    </Show>
  );
}

/** One quiet line for a card the planner would hold back; nothing for one that can start now. */
export function planNote(plan: Plan | null | undefined, id: number, cards: Card[] = []): string | null {
  if (!plan) return null;
  const name = (f: string) => {
    const [n, dir] = f.split("/").reverse();
    return dir && /^(index|mod|__init__)\./.test(n!) ? `${dir}/${n}` : n!;
  };
  const called = (other: number) => {
    const c = cards.find((x) => x.id === other);
    return c ? `#${other} ${c.title}` : `#${other}`;
  };
  const wait = plan.waits.find((w) => w.card === id);
  if (wait) return t("Waits for {other} to land: both touch {file}", { other: wait.with_lane ?? called(wait.with_card!), file: name(wait.files[0]!) });
  const batch = plan.batches.findIndex((b) => b.includes(id));
  if (batch <= 0) return null;
  const earlier = new Set(plan.batches.slice(0, batch).flat());
  const c = plan.clashes.find((x) => (x.card === id && earlier.has(x.with_card ?? -1)) || (x.with_card === id && earlier.has(x.card)));
  if (!c) return null;
  const other = c.card === id ? c.with_card! : c.card;
  return t("After {other}: both touch {file}", { other: called(other), file: name(c.files[0]!) });
}

/**
 * Ask Claude about the board, typed or dictated: "make cards from this list", "split #4". A
 * session in the repository writes the cards; it shows in the Agents dock while it works.
 */
function BoardAsk(props: { repo: string }) {
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let field: HTMLTextAreaElement | undefined;
  const fit = () => {
    if (!field) return;
    field.style.height = "auto";
    field.style.height = `${Math.min(field.scrollHeight, 220)}px`;
  };
  const send = async () => {
    const msgText = text().trim();
    if (!msgText || busy()) return;
    setBusy(true);
    try {
      await api.boardAssist(props.repo, msgText);
      setText("");
      requestAnimationFrame(fit);
      // The session appears in the dock as soon as the daemon starts it.
      for (const ms of [800, 2500, 6000]) setTimeout(() => void refreshAgents(), ms);
      toast({ tone: "info", title: t("Claude is on it"), body: t("New cards show up here as it writes them. Follow it in the Agents dock.") });
    } catch (e) {
      toast({ tone: "fail", title: t("Claude did not get it"), body: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="board-ask">
      <span class="ag-prompt" aria-hidden="true">›</span>
      <textarea
        ref={field}
        class="board-ask-field"
        rows="1"
        aria-label={t("Ask Claude to write or change cards")}
        placeholder={t("Ask Claude: paste a list or say what you need, and it writes the cards (Enter sends, Shift+Enter for a new line)")}
        value={text()}
        onInput={(e) => (setText(e.currentTarget.value), fit())}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && !e.shiftKey && !e.isComposing) (e.preventDefault(), void send());
        }}
        disabled={busy()}
      />
      <Dictate field={() => field} value={text} onChange={(v) => (setText(v), requestAnimationFrame(fit))} />
      <button type="button" class="btn btn-ghost" disabled={busy() || !text().trim()} aria-busy={busy()} onClick={() => void send()}>
        <Show when={busy()} fallback={<Icon name="spark" small />}>
          <Spinner />
        </Show>
        {t("Ask")}
      </button>
    </div>
  );
}

/** The card carries something of Claude's: the analysis mark, its notes or the judge's, or open questions. */
export const hasMemory = (c: Card) =>
  !!c.analyzed || (c.notes ?? []).some((n) => n.by !== "you") || (c.questions ?? []).some((q) => q.answer == null) || (c.after?.length ?? 0) > 0;

/**
 * Forget what Claude learned, to analyse again from scratch: its notes and the judge's, the open
 * questions, the analysis mark and the order. Says exactly what goes and what stays, and needs a
 * deliberate confirm (no Enter-to-confirm, the safe button has the focus).
 */
function ForgetSheet(props: { repo: string; cards: Card[]; onDone: () => void; onClose: () => void }) {
  const [busy, setBusy] = createSignal(false);
  const notes = () => props.cards.reduce((n, c) => n + (c.notes ?? []).filter((x) => x.by !== "you").length, 0);
  const open = () => props.cards.reduce((n, c) => n + (c.questions ?? []).filter((q) => q.answer == null).length, 0);
  const one = () => props.cards.length === 1;
  const go = async () => {
    setBusy(true);
    try {
      const r = await api.cardsForget(props.repo, props.cards.map((c) => c.id));
      props.onDone();
      toast({ tone: "done", title: one() ? t("#{id} is back to not analysed", { id: props.cards[0]!.id }) : t("{n} cards are back to not analysed", { n: props.cards.length }), body: t("Forgot {notes} and {questions}.", { notes: plural(r.notes, "{n} note", "{n} notes"), questions: plural(r.questions, "{n} open question", "{n} open questions") }) });
      props.onClose();
    } catch (e) {
      toast({ tone: "fail", title: t("Nothing was forgotten"), body: e instanceof Error ? e.message : String(e) });
      setBusy(false);
    }
  };
  return (
    <Sheet
      title={one() ? t("Forget the analysis of #{id}", { id: props.cards[0]!.id }) : t("Forget the analysis of {n} cards", { n: props.cards.length })}
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" autofocus onClick={props.onClose} disabled={busy()}>
            {t("Keep it")}
          </button>
          <button type="button" class="btn btn-danger" aria-busy={busy()} disabled={busy()} onClick={() => void go()}>
            <Show when={busy()} fallback={<Icon name="undo" small />}>
              <Spinner />
            </Show>
            {one() ? t("Forget it") : t("Forget all")}
          </button>
        </>
      }
    >
      <p>
        {one() ? t("Claude starts over on this card: the next Analyse reads it from scratch.") : t("Claude starts over on these cards: the next Analyse reads them from scratch.")}
      </p>
      <ul class="forget-list">
        <li>
          <b>{t("Goes:")}</b>{" "}
          {t("{notes}, {questions}, the analysis mark and the order it worked out.", {
            notes: plural(notes(), "{n} note by Claude and the judge", "{n} notes by Claude and the judge"),
            questions: plural(open(), "{n} unanswered question", "{n} unanswered questions"),
          })}
        </li>
        <li>
          <b>{t("Stays:")}</b> {t("the spec, your own notes, your answers to its questions, and any lane or run.")}
        </li>
      </ul>
      <p class="field-help">{t("This cannot be undone.")}</p>
    </Sheet>
  );
}

/** Deletes a card, with an undo in the toast (the file waits in the board's deleted folder). */
export async function deleteCard(repo: string, card: Card, onChanged: () => void): Promise<boolean> {
  try {
    await api.cardDelete(repo, card.id);
    onChanged();
    toast(
      {
        tone: "done",
        title: t("Deleted #{id} {title}", { id: card.id, title: card.title }),
        body: card.lane ? t("Its lane {lane} stays; clean it from Lanes if you do not need it.", { lane: card.lane }) : undefined,
        action: {
          label: t("Undo"),
          run: () =>
            void api.cardRestore(repo, card.id).then(onChanged, (e) => toast({ tone: "fail", title: t("Could not restore #{id}", { id: card.id }), body: e instanceof Error ? e.message : String(e) })),
        },
      },
      12000,
    );
    return true;
  } catch (e) {
    toast({ tone: "fail", title: t("Could not delete #{id}", { id: card.id }), body: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

/** Deletes every card given (the Landed column's Clear all), with one undo that brings them all back. */
async function deleteCards(repo: string, cards: Card[], onChanged: () => void) {
  const gone: Card[] = [];
  let failed: unknown;
  for (const c of cards) {
    try {
      await api.cardDelete(repo, c.id);
      gone.push(c);
    } catch (e) {
      failed ??= e;
    }
  }
  onChanged();
  if (failed !== undefined) toast({ tone: "fail", title: t("Could not delete {n} cards", { n: cards.length - gone.length }), body: failed instanceof Error ? failed.message : String(failed) });
  if (gone.length === 0) return;
  toast(
    {
      tone: "done",
      title: plural(gone.length, "Cleared {n} landed card", "Cleared {n} landed cards"),
      action: {
        label: t("Undo"),
        run: () =>
          void Promise.allSettled(gone.map((c) => api.cardRestore(repo, c.id))).then((rs) => {
            onChanged();
            const bad = rs.filter((r) => r.status === "rejected").length;
            if (bad > 0) toast({ tone: "fail", title: t("Could not restore {n} cards", { n: bad }) });
          }),
      },
    },
    12000,
  );
}

/** Landed where it was going: dependencies on it are met. */
const landedState = (c: Card) => c.state === "done" || c.state === "in-dev";

/**
 * Why Run all holds a card back beyond the planner's file clashes: a card the analysis said must
 * land first, or a shared area (an engine, a schema) another card is changing right now.
 * `busy` is the cards running now (or started in this pass).
 */
export function holdNote(id: number, cards: Card[], busy: number[]): string | null {
  const c = cards.find((x) => x.id === id);
  if (!c) return null;
  const called = (o: Card) => `#${o.id} ${o.title}`;
  for (const a of c.after ?? []) {
    const dep = cards.find((x) => x.id === a);
    // A dependency both ways is the analysts disagreeing: the board order decides.
    if (!dep || landedState(dep) || (dep.after ?? []).includes(id)) continue;
    return t("After {other} lands: it builds on it", { other: called(dep) });
  }
  const mine = new Set(c.areas ?? []);
  if (mine.size > 0)
    for (const b of busy) {
      const o = cards.find((x) => x.id === b);
      const shared = o && o.id !== id ? (o.areas ?? []).find((a) => mine.has(a)) : undefined;
      if (o && shared) return t("Waits for {other}: both change {area}", { other: called(o), area: shared });
    }
  return null;
}

export function BoardView(props: {
  cards: Card[] | null;
  problems: string[];
  yard: Yard | null;
  now: number;
  runs: RunMeta[];
  onOpen: (id: number) => void;
  onNew: () => void;
  /** Starts Run all on these backlog cards, in board order: analyse, ask, then work them. */
  repo: string | null;
  /** Cards changed here (deleted, restored): read them again. */
  onChanged: () => void;
  /** Claude reads the backlog cards not analysed yet (questions, notes, order). */
  onAnalyse: (ids: number[]) => void;
  /** While Run all works: backlog cards outside its queue (new ones) are analysed and join it. */
  onJoin: (ids: number[]) => void;
  /** Cards being analysed to join the running batch. */
  joining: number[];
  /** Every backlog card analysed: choose where the work lands, then start. */
  onStartDev: (ids: number[]) => void;
  onStopAll: () => void;
  /** The queue is empty but cards wait for review: close the batch now. */
  onFinish: () => void;
  /** Where Run all is. */
  phase: "idle" | "analyzing" | "asking" | "working" | "finishing";
  /** Cards Run all still has to start. */
  queued: number[];
  /** The whole board's new order, after a card was dragged. */
  onReorder: (ids: number[]) => void;
  /** The planner's view of open cards, only while Run all works. */
  plan?: Plan | null;
}) {
  const liveRun = (id: number) => props.runs.find((r) => r.card === id && isLive(r.state));
  // The order you just dragged to, shown until the saved cards come back.
  const [order, setOrder] = createSignal<number[] | null>(null);
  createEffect(on(() => props.cards, () => setOrder(null), { defer: true }));
  const sorted = createMemo(() => {
    const all = props.cards ?? [];
    const o = order();
    return o ? [...all].sort((x, y) => o.indexOf(x.id) - o.indexOf(y.id)) : all;
  });
  const placed = createMemo(() =>
    sorted().map((c) => {
      const asking = props.runs.find((r) => r.card === c.id && isLive(r.state) && r.waiting.length > 0);
      if (c.state === "review" && !asking) {
        const r = reviewSummary(latestRun(props.runs, c.id));
        return { card: c, p: { ...place(c, props.yard), chip: r.chip, icon: r.icon, label: r.label, why: r.text } };
      }
      const p = asking
        ? { ...place(c, props.yard), column: "needs" as const, chip: "needs" as const, icon: "needs" as const, label: asking.waiting[0]!.tool === "Bash" ? t("Wants to run") : t("Wants {tool}", { tool: asking.waiting[0]!.tool }), why: t("Asks to run {detail}", { detail: asking.waiting[0]!.detail }) }
        : place(c, props.yard);
      const busy = props.runs.filter((r) => isLive(r.state)).map((r) => r.card);
      const note = props.queued.includes(c.id) ? (holdNote(c.id, props.cards ?? [], busy) ?? planNote(props.plan, c.id, props.cards ?? [])) : null;
      return { card: c, p: note && p.column === "backlog" ? { ...p, why: note } : p };
    }),
  );
  const byColumn = (col: string) => placed().filter((x) => x.p.column === col);
  const needs = () => byColumn("needs").length;
  const backlog = () => byColumn("backlog").filter(({ card }) => !liveRun(card.id) && card.state !== "done");
  const colIcon = { needs: "needs", backlog: "queued", progress: "run", landed: "done" } as const;

  // ---- dragging a card up or down its column
  // y0/s0: pointer and column scroll at the start; py: the pointer now. dy moves the lifted card
  // with the pointer and with the column's own scrolling.
  const [drag, setDrag] = createSignal<{ id: number; col: string; y0: number; s0: number; py: number; dy: number; live: boolean } | null>(null);
  let dragged = false;
  const columnEls: Record<string, HTMLElement | undefined> = {};
  const bodyEls: Record<string, HTMLElement | undefined> = {};
  /** Near the top or bottom edge of its column, a dragged card scrolls the column, faster the closer it gets. */
  const EDGE = 64;
  let scrolling = 0;
  function autoscroll() {
    scrolling = 0;
    const d = drag();
    const body = d?.live ? bodyEls[d.col] : undefined;
    if (!d || !body) return;
    const r = body.getBoundingClientRect();
    const top = d.py - r.top;
    const bottom = r.bottom - d.py;
    const speed = top < EDGE ? -Math.ceil(((EDGE - Math.max(top, 0)) / EDGE) * 16) : bottom < EDGE ? Math.ceil(((EDGE - Math.max(bottom, 0)) / EDGE) * 16) : 0;
    if (speed !== 0) {
      const before = body.scrollTop;
      body.scrollTop += speed;
      if (body.scrollTop !== before) setDrag({ ...d, dy: d.py - d.y0 + (body.scrollTop - d.s0) });
    }
    scrolling = requestAnimationFrame(autoscroll);
  }

  function moveTo(col: string, id: number, index: number) {
    const ids = byColumn(col).map((x) => x.card.id);
    const from = ids.indexOf(id);
    if (from < 0 || index === from) return;
    ids.splice(from, 1);
    ids.splice(Math.max(0, Math.min(index, ids.length)), 0, id);
    // The column's new sequence takes the places its cards held in the whole board's order.
    const all = sorted().map((c) => c.id);
    const slots = all.map((x, i) => (ids.includes(x) ? i : -1)).filter((i) => i >= 0);
    slots.forEach((slot, i) => (all[slot] = ids[i]!));
    setOrder(all);
    props.onReorder(all);
  }

  function onPointerDown(e: PointerEvent, col: string, id: number) {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, a, input")) return;
    setDrag({ id, col, y0: e.clientY, s0: bodyEls[col]?.scrollTop ?? 0, py: e.clientY, dy: 0, live: false });
    const move = (ev: PointerEvent) => {
      const d = drag();
      if (!d) return;
      if (!d.live && Math.abs(ev.clientY - d.y0) < 5) return;
      const scrolled = (bodyEls[d.col]?.scrollTop ?? 0) - d.s0;
      setDrag({ ...d, py: ev.clientY, dy: ev.clientY - d.y0 + scrolled, live: true });
      if (!scrolling) scrolling = requestAnimationFrame(autoscroll);
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      cancelAnimationFrame(scrolling);
      scrolling = 0;
      const d = drag();
      setDrag(null);
      if (!d?.live) return;
      dragged = true;
      setTimeout(() => (dragged = false), 0);
      // Where it lands: how many of the other cards' middles sit above the pointer.
      const others = [...(columnEls[d.col]?.querySelectorAll<HTMLElement>("[data-card-id]") ?? [])].filter((el) => Number(el.dataset.cardId) !== d.id);
      const index = others.filter((el) => {
        const r = el.getBoundingClientRect();
        return r.top + r.height / 2 < ev.clientY;
      }).length;
      moveTo(d.col, d.id, index);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }

  /** The line that shows where the dragged card would land: before or after which card. */
  const gapMark = (col: string, id: number): "before" | "after" | undefined => {
    const d = drag();
    if (!d?.live || d.col !== col || d.id === id) return undefined;
    const others = byColumn(col).map((x) => x.card.id).filter((x) => x !== d.id);
    const y = d.py;
    const els = others.map((x) => columnEls[col]?.querySelector<HTMLElement>(`[data-card-id="${x}"]`));
    const g = els.filter((el) => {
      const r = el?.getBoundingClientRect();
      return r && r.top + r.height / 2 < y;
    }).length;
    const from = byColumn(col).findIndex((x) => x.card.id === d.id);
    if (g === from) return undefined; // where it already is
    if (g < others.length) return others[g] === id ? "before" : undefined;
    return others[others.length - 1] === id ? "after" : undefined;
  };

  const unanalysed = () => backlog().filter(({ card }) => !card.analyzed);
  /** While Run all works: backlog cards it does not have yet (made after it started). */
  const outside = () => (props.phase === "working" ? backlog().filter(({ card }) => !props.queued.includes(card.id) && !props.joining.includes(card.id)).map(({ card }) => card) : []);
  /** Backlog cards that carry something of Claude's: the analysis, its notes, its open questions. */
  const remembered = () => backlog().map(({ card }) => card).filter(hasMemory);
  const [forgetting, setForgetting] = createSignal<number[] | null>(null);
  const runLabel = () =>
    props.phase === "analyzing"
      ? t("Stop analysing")
      : props.phase === "asking"
        ? t("Answer to go on")
        : props.phase === "finishing"
          ? t("Finishing")
          : props.phase === "working"
            ? props.queued.length > 0
              ? t("Stop ({n} queued)", { n: props.queued.length })
              : t("Finish now")
            : unanalysed().length > 0
              ? t("Analyse {n}", { n: unanalysed().length })
              : t("Start development");
  const runAction = () => {
    if (props.phase === "analyzing") return props.onStopAll();
    if (props.phase === "working") return props.queued.length > 0 ? props.onStopAll() : props.onFinish();
    if (props.phase !== "idle") return;
    if (unanalysed().length > 0) return props.onAnalyse(unanalysed().map(({ card }) => card.id));
    props.onStartDev(backlog().map(({ card }) => card.id));
  };
  const runTitle = () =>
    props.phase === "working"
      ? props.queued.length > 0
        ? t("Stops starting queued cards; runs already going keep going")
        : t("Close the batch now: the cards still in review stay there")
      : props.phase === "analyzing"
        ? t("Stops the analysis; nothing starts")
        : unanalysed().length > 0
          ? t("Claude reads each card not analysed yet: what it needs to know (asked one at a time), notes, and which cards must land first or share an engine")
          : t("Choose where the work lands and what happens at the end, then the agents start");

  return (
    <div class="board-screen">
      <header class="screen-head">
        <h1 class="title">{t("Board")}</h1>
        <Show when={(props.cards?.length ?? 0) > 0}>
          <p class="summary">
            {plural(props.cards!.length, "{n} card", "{n} cards")},{" "}
            <Show when={needs() > 0} fallback={t("nothing needs you.")}>
              <b>{plural(needs(), "{n} needs you.", "{n} need you.")}</b>
            </Show>
          </p>
        </Show>
        <span class="spacer" />
        <button type="button" class="btn btn-primary" onClick={props.onNew}>
          <Icon name="plus" small />
          {t("New card")} <span class="kbd">C</span>
        </button>
      </header>

      <Show when={props.repo}>{(r) => <BoardAsk repo={r()} />}</Show>

      <For each={props.problems}>{(p) => <ErrorLine>{p}</ErrorLine>}</For>
      <Show when={forgetting() && props.repo}>
        {(repo) => (
          <ForgetSheet
            repo={repo()}
            cards={(props.cards ?? []).filter((c) => forgetting()!.includes(c.id))}
            onDone={props.onChanged}
            onClose={() => setForgetting(null)}
          />
        )}
      </Show>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> {t("Reading the cards")}</div>}>
        <Show
          when={props.cards!.length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">{t("No cards yet")}</div>
              <p class="empty-body">
                {t("Cards live outside the repository, so every branch and worktree sees the same board. Add one with New card, or let Claude Code add them through MCP.")}
              </p>
            </div>
          }
        >
          <div class="board" data-dragging={drag()?.live ? "" : undefined}>
            <For each={columns}>
              {(col) => (
                <section class="col" aria-label={t(columnTitle[col])} ref={(el) => (columnEls[col] = el)}>
                  <h2 class="col-head" data-hot={col === "needs" && byColumn(col).length > 0 ? "" : undefined}>
                    <Icon name={colIcon[col]} small />
                    {t(columnTitle[col])}
                    <span class={col === "needs" && byColumn(col).length > 0 ? "count count-hot" : "count"}>
                      {byColumn(col).length}
                    </span>
                    <Show when={col === "backlog" && (backlog().length > 0 || props.phase !== "idle")}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        disabled={props.phase === "asking" || props.phase === "finishing"}
                        aria-busy={props.phase === "analyzing" || props.phase === "finishing"}
                        title={runTitle()}
                        onClick={runAction}
                      >
                        <Show
                          when={props.phase === "analyzing" || props.phase === "finishing"}
                          fallback={<Icon name={props.phase === "working" ? (props.queued.length > 0 ? "stop" : "done") : unanalysed().length > 0 ? "search" : "play"} small />}
                        >
                          <Spinner />
                        </Show>
                        <span class="col-action-label">{runLabel()}</span>
                      </button>
                    </Show>
                    <Show when={col === "backlog" && (outside().length > 0 || props.joining.length > 0)}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        disabled={props.joining.length > 0}
                        aria-busy={props.joining.length > 0}
                        title={t("Claude reads the cards made after Run all started; once their questions are answered they join the batch")}
                        onClick={() => props.onJoin(outside().map((c) => c.id))}
                      >
                        <Show when={props.joining.length > 0} fallback={<Icon name="search" small />}>
                          <Spinner />
                        </Show>
                        <span class="col-action-label">
                          {props.joining.length > 0
                            ? t("Analysing {n}", { n: props.joining.length })
                            : outside().some((c) => !c.analyzed)
                              ? t("Analyse {n} new", { n: outside().length })
                              : t("Add {n} to the batch", { n: outside().length })}
                        </span>
                      </button>
                    </Show>
                    <Show when={col === "backlog" && props.phase === "idle" && props.repo && remembered().length > 0}>
                      <button
                        type="button"
                        class="btn btn-ghost btn-icon col-action col-forget"
                        aria-label={t("Forget the analysis")}
                        title={plural(remembered().length, "Forget what Claude learned on {n} backlog card, to analyse again", "Forget what Claude learned on {n} backlog cards, to analyse again")}
                        onClick={() => setForgetting(remembered().map((c) => c.id))}
                      >
                        <Icon name="undo" small />
                      </button>
                    </Show>
                    <Show when={col === "landed" && props.repo && byColumn(col).some(({ card }) => !liveRun(card.id))}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        title={t("Delete every card in Landed (you can undo)")}
                        onClick={() => void deleteCards(props.repo!, byColumn(col).map((x) => x.card).filter((c) => !liveRun(c.id)), props.onChanged)}
                      >
                        <Icon name="trash" small />
                        <span class="col-action-label">{t("Clear all")}</span>
                      </button>
                    </Show>
                  </h2>
                  <div class="col-body" ref={(el) => (bodyEls[col] = el)} onScroll={(e) => e.currentTarget.toggleAttribute("data-scrolled", e.currentTarget.scrollTop > 0)}>
                  <Show when={byColumn(col).length > 0} fallback={<p class="col-empty">{t(columnEmpty[col])}</p>}>
                    <For each={byColumn(col)}>
                      {({ card, p }, i) => (
                        <article
                          class="card"
                          data-card-id={card.id}
                          data-state={p.column === "needs" ? "needs" : p.column === "landed" ? "landed" : undefined}
                          data-tone={p.chip === "failed" ? "failed" : undefined}
                          data-lifted={drag()?.live && drag()!.id === card.id ? "" : undefined}
                          data-gap={gapMark(col, card.id)}
                          style={drag()?.live && drag()!.id === card.id ? { transform: `translateY(${drag()!.dy}px)` } : undefined}
                          tabindex="0"
                          role="button"
                          aria-label={t("Card {id}: {title}. Alt and the arrow keys move it; Command Backspace deletes it.", { id: card.id, title: card.title })}
                          onPointerDown={(e) => onPointerDown(e, col, card.id)}
                          onClick={() => !dragged && props.onOpen(card.id)}
                          onKeyDown={(e) => {
                            // ⌘⌫ deletes the focused card, with an undo.
                            if (e.metaKey && (e.key === "Backspace" || e.key === "Delete") && props.repo && !liveRun(card.id)) {
                              e.preventDefault();
                              void deleteCard(props.repo, card, props.onChanged);
                              return;
                            }
                            if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
                              e.preventDefault();
                              moveTo(col, card.id, i() + (e.key === "ArrowUp" ? -1 : 1));
                              const el = e.currentTarget;
                              requestAnimationFrame(() => el.isConnected && el.focus());
                              return;
                            }
                            if (e.key === "Enter" || e.key === " ") (e.preventDefault(), props.onOpen(card.id));
                          }}
                        >
                          <div class="card-top">
                            <span class="card-id">#{card.id}</span>
                            <Show
                              when={p.column !== "needs" && liveRun(card.id)}
                              fallback={
                                <span class="chip" data-state={p.chip}>
                                  <Icon name={p.icon} small />
                                  {p.label}
                                </span>
                              }
                            >
                              {(r) => (
                                <span class="chip" data-state="running">
                                  <Icon name="run" small />
                                  {t("Running")} <span class="figure">{clock(elapsed(r(), props.now))}</span>
                                </span>
                              )}
                            </Show>
                            <Show when={p.column === "backlog"}>
                              <span class="card-analysis" data-on={card.analyzed ? "" : undefined} title={card.analyzed ? t("Analysed {at}", { at: card.analyzed }) : t("Not analysed: Run all reads it first")}>
                                {card.analyzed ? t("Analysed") : t("Not analysed")}
                              </span>
                            </Show>
                            <Show when={(card.notes?.length ?? 0) > 0}>
                              <span class="card-notes" title={plural(card.notes!.length, "{n} note", "{n} notes")}>
                                <Icon name="note" small />
                                {card.notes!.length}
                              </span>
                            </Show>
                            <Show when={card.state === "blocked" && (card.questions ?? []).every((q) => q.answer != null) && !liveRun(card.id) && props.repo}>
                              <button
                                type="button"
                                class="btn btn-ghost btn-icon card-back"
                                aria-label={t("Back to Backlog")}
                                title={t("Back to Backlog: its next run picks up from your notes and its lane")}
                                onPointerDown={(e) => e.stopPropagation()}
                                onKeyDown={(e) => e.stopPropagation()}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  void api
                                    .cardUpdate(props.repo!, card.id, { state: "ready" })
                                    .then(() => (toast({ tone: "done", title: t("#{id} back in Backlog", { id: card.id }) }), props.onChanged()))
                                    .catch((err) => toast({ tone: "fail", title: t("Could not move #{id}", { id: card.id }), body: msg(err) }));
                                }}
                              >
                                <Icon name="queued" small />
                              </button>
                            </Show>
                          </div>
                          <div class="card-title"><Linked text={card.title} /></div>
                          <Show when={p.why}>
                            <div class="card-why"><Linked text={p.why ?? ""} /></div>
                          </Show>
                          <Show when={card.lane}>
                            <div class="card-foot">
                              <span class="lane-tag">{card.lane}</span>
                              <span class="spacer">
                                <MiniTrack card={card} yard={props.yard} now={props.now} />
                              </span>
                            </div>
                          </Show>
                        </article>
                      )}
                    </For>
                  </Show>
                  </div>
                </section>
              )}
            </For>
          </div>
        </Show>
      </Show>
    </div>
  );
}

/**
 * Run all's questions, one at a time, each with its card as context and Claude to talk it over
 * with. Once all are answered nothing starts by itself: every decision of the batch shows, each
 * can change or be talked over again, and Start runs them.
 */
export function QuestionsSheet(props: { repo: string; cards: Card[]; onChanged: () => void; onDone: () => void; onStop: () => void }) {
  const open = () => openQuestions(props.cards);
  // The count at the start, so "2 of 5" holds while answers land.
  const total = open().length;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [review, setReview] = createSignal(total === 0);
  const current = () => open()[0];
  createEffect(on(() => open().length, (n) => n === 0 && setReview(true), { defer: true }));
  const decided = () => props.cards.filter((c) => (c.questions ?? []).some((q) => q.answer != null));
  const decisions = () => decided().reduce((n, c) => n + (c.questions ?? []).filter((q) => q.answer != null).length, 0);
  const thinking = () => props.cards.some((c) => talking(props.repo, c.id));

  async function answer(text: string) {
    const q = current();
    if (!q) return;
    setBusy(true);
    setError(null);
    try {
      await api.cardAnswer(props.repo, q.card.id, q.id, text);
      props.onChanged();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      title={review() && open().length === 0 ? t("All decided") : t("Before the run")}
      onClose={props.onDone}
      foot={
        <>
          <Show
            when={!review()}
            fallback={
              <span class="sheet-note">
                {open().length
                  ? plural(open().length, "{n} question still open", "{n} questions still open")
                  : plural(decisions(), "{n} decision, all can still change", "{n} decisions, all can still change")}
              </span>
            }
          >
            <span class="sheet-note">
              {t("Question {i} of {total}", { i: Math.min(total, total - open().length + 1), total })}
            </span>
          </Show>
          <button type="button" class="btn btn-ghost" onClick={props.onStop} disabled={busy()}>
            {t("Stop Run all")}
          </button>
          <Show when={!review() && decisions() > 0}>
            <button type="button" class="btn btn-ghost" onClick={() => setReview(true)} disabled={busy()}>
              {t("Review decisions")}
            </button>
          </Show>
          <Show when={review() && open().length > 0}>
            <button type="button" class="btn btn-ghost" onClick={() => setReview(false)}>
              {t("Keep answering")}
            </button>
          </Show>
          <Show
            when={review() && open().length === 0}
            fallback={
              <button type="button" class="btn" onClick={props.onDone} disabled={busy()} title={t("Cards with open questions wait; the rest start now")}>
                {t("Answer later")}
              </button>
            }
          >
            <button
              type="button"
              class="btn btn-primary"
              onClick={props.onDone}
              disabled={thinking()}
              title={thinking() ? t("Claude is still answering on one of these cards") : undefined}
            >
              <Icon name="play" small />
              {t("Start")}
            </button>
          </Show>
        </>
      }
    >
      <Show
        when={!review()}
        fallback={
          <div class="ask-review">
            <p class="caption">
              {t("Go back to any of them before the cards start: choose again, or talk it over with Claude (\u201cand if we did it like this?\u201d).")}
            </p>
            <For each={decided()}>
              {(c) => (
                <section class="inbox-card">
                  <p class="ask-card">
                    <span class="card-id">#{c.id}</span> {c.title}
                  </p>
                  <dl class="decided">
                    <For each={(c.questions ?? []).filter((q) => q.answer != null)}>
                      {(q) => <DecisionItem repo={props.repo} card={c} q={q} onChanged={props.onChanged} />}
                    </For>
                  </dl>
                </section>
              )}
            </For>
          </div>
        }
      >
        <Show when={current()} keyed>
          {(q) => (
            <div class="ask-one">
              <p class="ask-card">
                <span class="card-id">#{q.card.id}</span> {q.card.title}
              </p>
              <p class="inbox-q">{inline(q.text)}</p>
              <QuestionAbout q={q} />
              <QuestionBlock q={q} selected busy={busy() || talking(props.repo, q.card.id, q.id)} onAnswer={(a) => void answer(a)} />
              <QuestionTalk repo={props.repo} card={q.card} q={q.id} talk={q.talk} onChanged={props.onChanged} />
            </div>
          )}
        </Show>
      </Show>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- one card

/** A note's age in words: "now", "12 min", "3 h", "2 d", then the date. */
export function ago(at: string, now: number): string {
  const ts = Date.parse(at);
  if (Number.isNaN(ts)) return at;
  const m = Math.floor((now - ts) / 60000);
  if (m < 1) return t("now");
  if (m < 60) return t("{n} min ago", { n: m });
  if (m < 60 * 24) return t("{n} h ago", { n: Math.floor(m / 60) });
  if (m < 60 * 24 * 7) return t("{n} d ago", { n: Math.floor(m / 1440) });
  return new Date(ts).toLocaleDateString(locale(), { day: "numeric", month: "short" });
}

export const noteBy: Record<string, { name: string; icon: "spark" | "person" | "verify" }> = {
  claude: { name: "Claude", icon: "spark" },
  you: { name: "You", icon: "person" },
  judge: { name: "Judge", icon: "verify" },
};

export function CardSheet(props: {
  repo: string;
  card: Card;
  yard: Yard | null;
  now: number;
  runs: RunMeta[];
  onRun: () => void;
  onOpenRun: (id: string) => void;
  onChanged: () => void;
  onClose: () => void;
}) {
  const [title, setTitle] = createSignal(props.card.title);
  const [body, setBody] = createSignal(props.card.body);
  let specRef: HTMLTextAreaElement | undefined;
  // The spec reads as text (its links open) until Edit; an empty one opens for writing.
  const [editing, setEditing] = createSignal(!props.card.body.trim());
  let addImages: ((files: File[]) => void) | undefined;
  const [state, setState] = createSignal<CardState>(props.card.state);
  const [lane, setLane] = createSignal(props.card.lane ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal("");
  let noteRef: HTMLTextAreaElement | undefined;
  const [noting, setNoting] = createSignal(false);
  const [forgetOne, setForgetOne] = createSignal(false);
  const dirty = () =>
    title().trim() !== props.card.title ||
    body() !== props.card.body ||
    state() !== props.card.state ||
    lane().trim() !== (props.card.lane ?? "");
  const open = () => openQuestions([props.card]);
  const live = () => props.runs.find((r) => isLive(r.state));
  const answered = () => (props.card.questions ?? []).filter((q) => q.answer != null);
  const notes = () => props.card.notes ?? [];
  const where = () => place(props.card, props.yard);

  // The spec grows with its text, so a long spec reads without a second scroll.
  const fit = (el: HTMLTextAreaElement) => {
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + 2}px`;
  };

  async function save(to?: CardState) {
    setBusy(true);
    setError(null);
    try {
      const next = to ?? state();
      await api.cardUpdate(props.repo, props.card.id, {
        title: title().trim() !== props.card.title ? title().trim() : undefined,
        body: body() !== props.card.body ? body() : undefined,
        state: next !== props.card.state ? next : undefined,
        lane: lane().trim() !== (props.card.lane ?? "") ? lane().trim() : undefined,
      });
      toast(
        to
          ? { tone: "done", title: t("#{id} back in Backlog", { id: props.card.id }), body: t("Its next run picks up from your notes and its lane.") }
          : { tone: "done", title: t("#{id} saved", { id: props.card.id }) },
      );
      props.onChanged();
      props.onClose();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  async function answer(question: number, text: string) {
    setBusy(true);
    try {
      const r = await api.cardAnswer(props.repo, props.card.id, question, text);
      toast({ tone: "done", title: t("Recorded as {decision}", { decision: r.decision }), body: text });
      props.onChanged();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  async function addNote() {
    if (!note().trim()) return;
    setNoting(true);
    setError(null);
    try {
      await api.cardNote(props.repo, props.card.id, note().trim());
      setNote("");
      if (noteRef) noteRef.style.height = "";
      props.onChanged();
    } catch (e) {
      setError(msg(e));
    } finally {
      setNoting(false);
    }
  }

  return (
    <Sheet
      title={t("Card #{id}", { id: props.card.id })}
      class="card-sheet"
      onClose={props.onClose}
      dirty={() => dirty() || !!note().trim()}
      head={
        <div class="cs-head">
          <div class="cs-head-line">
            <span class="cs-id">#{props.card.id}</span>
            <span class="chip" data-state={where().chip}>
              <Icon name={where().icon} small />
              {where().label}
            </span>
            <span class="cs-where">{t("in {column}", { column: t(columnTitle[where().column]) })}</span>
            <button type="button" class="btn btn-ghost btn-icon cs-close" aria-label={t("Close")} onClick={props.onClose}>
              <Icon name="close" small />
            </button>
          </div>
          <input
            class="cs-title"
            aria-label={t("Title")}
            value={title()}
            onInput={(e) => setTitle(e.currentTarget.value)}
          />
        </div>
      }
      foot={
        <>
          <button
            type="button"
            class="btn btn-ghost cs-delete"
            disabled={busy() || !!live()}
            title={live() ? t("Stop its run first") : t("Delete this card (you can undo)")}
            onClick={() => void deleteCard(props.repo, props.card, props.onChanged).then((ok) => ok && props.onClose())}
          >
            <Icon name="trash" small /> {t("Delete")}
          </button>
          <span class="sheet-note">{props.card.file}</span>
          <Show when={props.card.state === "blocked" && !live()}>
            <button
              type="button"
              class="btn btn-ghost"
              disabled={busy() || !title().trim()}
              title={t("Save your changes and send it back to Backlog, to be worked again")}
              onClick={() => void save("ready")}
            >
              <Icon name="queued" small /> {t("Back to Backlog")}
            </button>
          </Show>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {dirty() ? t("Cancel") : t("Close")}
          </button>
          <Show
            when={!dirty()}
            fallback={
              <button type="button" class="btn btn-primary" disabled={busy() || !title().trim()} aria-busy={busy()} onClick={() => void save()}>
                <Show when={busy()}>
                  <Spinner />
                </Show>
                {t("Save")}
              </button>
            }
          >
            <Show
              when={live()}
              fallback={
                <button
                  type="button"
                  class="btn btn-primary"
                  disabled={busy() || open().length > 0 || props.card.state === "done"}
                  title={open().length > 0 ? t("Answer the open questions first") : t("Claude Code works the card in its own worktree")}
                  onClick={props.onRun}
                >
                  <Icon name="run" small />
                  {t("Run")}
                </button>
              }
            >
              {(r) => (
                <button type="button" class="btn btn-primary" onClick={() => props.onOpenRun(r().id)}>
                  <Icon name="run" small />
                  {t("Open the run")} <span class="figure">{clock(elapsed(r(), props.now))}</span>
                </button>
              )}
            </Show>
          </Show>
        </>
      }
    >
      <div
        class="cs-grid"
        style={{ "--side-w": `${CARD_SIDE_W.get()}px` }}
        onPaste={(e) => {
          const files = imagesIn(e.clipboardData);
          if (!files.length) return;
          e.preventDefault();
          addImages?.(files);
        }}
      >
        {/* The sheet opens with focus here, not on the close button (which drew its ring). */}
        <div class="cs-main" tabindex="-1" autofocus>
          <section class="cs-section">
            <div class="field-head">
              <label class="cs-label" for="card-body">
                {t("Spec")}
              </label>
              <Show
                when={editing()}
                fallback={
                  <button type="button" class="btn btn-ghost cs-act" onClick={() => setEditing(true)}>
                    <Icon name="edit" small /> {t("Edit")}
                  </button>
                }
              >
                <Dictate field={() => specRef} value={body} onChange={(v) => (setBody(v), specRef && fit(specRef))} />
              </Show>
            </div>
            <Show
              when={editing()}
              fallback={
                <div class="cs-spec-read" onDblClick={() => setEditing(true)} title={t("Double-click or Edit to change it")}>
                  <Prose text={body()} />
                </div>
              }
            >
              <textarea
                ref={(el) => {
                  specRef = el;
                  requestAnimationFrame(() => {
                    fit(el);
                    if (body()) el.focus();
                  });
                }}
                id="card-body"
                class="input textarea cs-spec"
                placeholder={t("Goal, acceptance criteria, out of scope, how to verify")}
                value={body()}
                onInput={(e) => {
                  setBody(e.currentTarget.value);
                  fit(e.currentTarget);
                }}
              />
            </Show>
          </section>

          <CardImages repo={props.repo} card={props.card} onChanged={props.onChanged} ref={(add) => (addImages = add)} />

          <Show when={open().length > 0}>
            <section class="cs-section">
              <h3 class="cs-label">
                {t("Open questions")} <span class="count count-hot">{open().length}</span>
              </h3>
              <For each={open()}>
                {(q) => (
                  <div class="sheet-question">
                    <p class="inbox-q">{inline(q.text)}</p>
                    <QuestionAbout q={q} card={false} />
                    <QuestionBlock q={q} selected={false} busy={busy() || talking(props.repo, q.card.id, q.id)} onAnswer={(a) => void answer(q.id, a)} />
                    <QuestionTalk repo={props.repo} card={q.card} q={q.id} talk={q.talk} onChanged={props.onChanged} />
                  </div>
                )}
              </For>
            </section>
          </Show>

          <section class="cs-section">
            <div class="field-head">
              <h3 class="cs-label">
                {t("Notes")} <span class="count">{notes().length}</span>
              </h3>
              <Show when={hasMemory(props.card)}>
                <button type="button" class="btn btn-ghost cs-forget" disabled={!!live()} onClick={() => setForgetOne(true)} title={t("Forget Claude's notes, its open questions and the analysis, to analyse this card again")}>
                  <Icon name="undo" small /> {t("Forget")}
                </button>
              </Show>
            </div>
            <p class="cs-sub">{t("What Claude learned working this card, and what you add. Every run reads them.")}</p>
            <Show when={forgetOne()}>
              <ForgetSheet repo={props.repo} cards={[props.card]} onDone={props.onChanged} onClose={() => setForgetOne(false)} />
            </Show>
            <div class="note-compose" data-filled={note().trim() ? "" : undefined}>
              <textarea
                ref={(el) => (noteRef = el)}
                class="note-input"
                rows="2"
                aria-label={t("Add a note")}
                placeholder={t("Add context for the next run")}
                value={note()}
                onInput={(e) => {
                  setNote(e.currentTarget.value);
                  fit(e.currentTarget);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void addNote();
                  }
                }}
              />
              <div class="note-compose-bar">
                <Dictate field={() => noteRef} value={note} onChange={(v) => (setNote(v), noteRef && fit(noteRef))} />
                <span class="note-keys">⌘↩</span>
                <button type="button" class="btn btn-primary btn-sm" disabled={noting() || !note().trim()} aria-busy={noting()} onClick={() => void addNote()}>
                  <Show when={noting()} fallback={<Icon name="plus" small />}>
                    <Spinner />
                  </Show>
                  {t("Add note")}
                </button>
              </div>
            </div>
            <Show when={notes().length > 0} fallback={<p class="cs-empty">{t("No notes yet. Run all's analysis and each run add theirs here.")}</p>}>
              <ol class="notes">
                <For each={[...notes()].reverse()}>
                  {(n) => (
                    <li class="note" data-by={n.by}>
                      <div class="note-head">
                        <Icon name={noteBy[n.by]?.icon ?? "person"} small />
                        <span class="note-by">{noteBy[n.by] ? t(noteBy[n.by]!.name) : n.by}</span>
                        <span class="note-at" title={n.at}>
                          {ago(n.at, props.now)}
                        </span>
                      </div>
                      <Prose text={n.text} />
                    </li>
                  )}
                </For>
              </ol>
            </Show>
          </section>
          <Show when={error()}>
            <ErrorLine>{error()}</ErrorLine>
          </Show>
        </div>

        <Grip size={CARD_SIDE_W} edge="left" label={t("Resize the side column")} style={{ left: "auto", right: "calc(var(--side-w) - 5px)" }} />
        <aside class="cs-side">
          <div class="field">
            <label class="field-label" for="card-state">
              {t("State")}
            </label>
            <select id="card-state" class="select" value={state()} onChange={(e) => setState(e.currentTarget.value as CardState)}>
              <For each={cardStates}>{(s) => <option value={s}>{t(stateLabel[s])}</option>}</For>
            </select>
          </div>
          <div class="field">
            <label class="field-label" for="card-lane">
              {t("Lane")}
            </label>
            <input
              id="card-lane"
              class="input input-mono"
              list="lane-branches"
              placeholder="feat/thing"
              autocomplete="off"
              spellcheck={false}
              value={lane()}
              onInput={(e) => setLane(e.currentTarget.value)}
            />
            <datalist id="lane-branches">
              <For each={props.yard?.lanes ?? []}>{(l) => <option value={l.branch} />}</For>
            </datalist>
            <Show when={props.card.lane}>
              <div class="card-lane-track">
                <MiniTrack card={props.card} yard={props.yard} now={props.now} />
              </div>
            </Show>
          </div>

          <Show when={props.runs.length > 0}>
            <div class="cs-side-block">
              <h3 class="field-label">{t("Runs")}</h3>
              <ul class="card-runs">
                <For each={props.runs}>
                  {(r) => (
                    <li>
                      <button type="button" class="card-run cs-run" onClick={() => props.onOpenRun(r.id)} title={r.handoff?.summary ?? r.error ?? r.branch}>
                        <span class="chip" data-state={runStateView[r.state].chip}>
                          <Icon name={runStateView[r.state].icon} small />
                          {t(runStateView[r.state].label)}
                        </span>
                        <span class="figure">{clock(elapsed(r, props.now))}</span>
                        <span class="figure">{money(r.cost_usd)}</span>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </div>
          </Show>

          <Show when={answered().length > 0}>
            <div class="cs-side-block">
              <h3 class="field-label">{t("Decisions")}</h3>
              <dl class="decisions">
                <For each={answered()}>
                  {(q) => <DecisionItem repo={props.repo} card={props.card} q={q} onChanged={props.onChanged} />}
                </For>
              </dl>
            </div>
          </Show>

          <p class="cs-meta">
            {t("Created {when}", { when: ago(props.card.created, props.now) })}
            <Show when={props.card.analyzed}>, {t("analysed {when}", { when: ago(props.card.analyzed!, props.now) })}</Show>
          </p>
        </aside>
      </div>
    </Sheet>
  );
}

export function NewCardSheet(props: { repo: string; onDone: (id: number) => void; onClose: () => void }) {
  const [title, setTitle] = createSignal("");
  const [body, setBody] = createSignal("");
  let newSpecRef: HTMLTextAreaElement | undefined;
  let newPicker: HTMLInputElement | undefined;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  // Images wait here until the card exists, then go on it.
  const [pending, setPending] = createSignal<File[]>([]);
  const take = (files: File[]) => files.length && setPending((p) => [...p, ...files]);

  async function create(e: Event) {
    e.preventDefault();
    if (!title().trim()) return;
    setBusy(true);
    setError(null);
    try {
      const c = await api.cardAdd(props.repo, title().trim(), body());
      if (pending().length) await attachImages(props.repo, c.id, pending());
      toast({ tone: "done", title: `#${c.id} ${c.title}`, body: c.file });
      props.onDone(c.id);
      props.onClose();
    } catch (err) {
      setError(msg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      title={t("New card")}
      onClose={props.onClose}
      dirty={() => !!title().trim() || !!body().trim()}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {t("Cancel")}
          </button>
          <button type="submit" form="new-card" class="btn btn-primary" disabled={busy() || !title().trim()} aria-busy={busy()}>
            <Show when={busy()} fallback={<Icon name="plus" small />}>
              <Spinner />
            </Show>
            {t("Create card")}
          </button>
        </>
      }
    >
      <form
        id="new-card"
        class="sheet-form"
        onSubmit={create}
        onPaste={(e) => {
          const files = imagesIn(e.clipboardData);
          if (files.length) (e.preventDefault(), take(files));
        }}
        onDragOver={(e) => e.dataTransfer?.types.includes("Files") && e.preventDefault()}
        onDrop={(e) => {
          const files = imagesIn(e.dataTransfer);
          if (files.length) (e.preventDefault(), take(files));
        }}
      >
        <div class="field">
          <label class="field-label" for="new-card-title">
            {t("Title")}
          </label>
          <input
            id="new-card-title"
            class="input"
            placeholder={t("One line, in your words")}
            autocomplete="off"
            autofocus
            value={title()}
            onInput={(e) => setTitle(e.currentTarget.value)}
          />
        </div>
        <div class="field">
          <div class="field-head">
            <label class="field-label" for="new-card-body">
              {t("Spec")}
            </label>
            <Dictate field={() => newSpecRef} value={body} onChange={setBody} />
          </div>
          <textarea
            ref={newSpecRef}
            id="new-card-body"
            class="input textarea input-mono"
            rows="6"
            placeholder={t("Optional: goal, acceptance criteria, how to verify")}
            value={body()}
            onInput={(e) => setBody(e.currentTarget.value)}
          />
          <span class="field-help">{t("Claude (Sonnet) turns your title and spec into the card's title and a short summary for the board, in a few seconds. Saved to this repository's board in ~/.pitlane.")}</span>
        </div>
        <div class="field">
          <div class="field-head">
            <span class="field-label">
              {t("Images")} <span class="count">{pending().length}</span>
            </span>
            <button type="button" class="btn btn-ghost" onClick={() => newPicker?.click()}>
              <Icon name="plus" small /> {t("Add")}
            </button>
            <input
              ref={newPicker}
              type="file"
              accept="image/png,image/jpeg,image/gif,image/webp"
              multiple
              hidden
              onChange={(e) => {
                take(Array.from(e.currentTarget.files ?? []));
                e.currentTarget.value = "";
              }}
            />
          </div>
          <Show when={pending().length > 0} fallback={<span class="field-help">{t("Drop screenshots here, pick them with Add, or paste with ⌘V.")}</span>}>
            <ul class="img-pending">
              <For each={pending()}>
                {(f, i) => (
                  <li>
                    <span>{f.name || t("Pasted image")}</span>
                    <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Remove")} onClick={() => setPending((p) => p.filter((_, j) => j !== i()))}>
                      <Icon name="close" small />
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
      </form>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}
