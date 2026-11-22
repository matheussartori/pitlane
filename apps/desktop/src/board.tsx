// Board: cards grouped by who acts next. A card linked to a lane carries that lane's track.
import { For, Show, createEffect, createMemo, createSignal, on, type JSX } from "solid-js";
import { api, cardStates, isLive, type Card, type CardState, type Plan, type RunMeta, type Yard } from "./api";
import { clock, elapsed, money, runStateView } from "./run";
import { columnEmpty, columnTitle, columns, latestRun, openQuestions, place, reviewSummary, stateLabel } from "./cards";
import { Icon } from "./icons";
import { Dictate } from "./dictate";
import { refreshAgents } from "./agents";
import { QuestionBlock } from "./inbox";
import { track } from "./lane";
import { ErrorLine, Sheet, Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** The one stop worth naming at card size: your move, a count to settle, or what is wrong. */
function stopNote(nodes: { stage: string; state: string; figure?: string }[]): string | null {
  const n = nodes.find((x) => x.state === "fail") ?? nodes.find((x) => x.state === "next") ?? nodes.find((x) => x.state === "count");
  if (!n) return null;
  if (n.state === "next") return `next: ${n.stage}`;
  if (n.state === "fail") return `mixed in ${n.stage}`;
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
            aria-label={`Lane ${l().branch}: ${track(l(), props.yard!, props.now).map((n) => `${n.stage} ${n.state === "on" ? "reached" : n.state === "off" ? "not reached" : n.state}`).join(", ")}`}
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
            {(t) => <span class="stop-note" aria-hidden="true">{t()}</span>}
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
  if (wait) return `Waits for ${wait.with_lane ?? called(wait.with_card!)} to land: both touch ${name(wait.files[0]!)}`;
  const batch = plan.batches.findIndex((b) => b.includes(id));
  if (batch <= 0) return null;
  const earlier = new Set(plan.batches.slice(0, batch).flat());
  const c = plan.clashes.find((x) => (x.card === id && earlier.has(x.with_card ?? -1)) || (x.with_card === id && earlier.has(x.card)));
  if (!c) return null;
  const other = c.card === id ? c.with_card! : c.card;
  return `After ${called(other)}: both touch ${name(c.files[0]!)}`;
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
    const t = text().trim();
    if (!t || busy()) return;
    setBusy(true);
    try {
      await api.boardAssist(props.repo, t);
      setText("");
      requestAnimationFrame(fit);
      // The session appears in the dock as soon as the daemon starts it.
      for (const ms of [800, 2500, 6000]) setTimeout(() => void refreshAgents(), ms);
      toast({ tone: "info", title: "Claude is on it", body: "New cards show up here as it writes them. Follow it in the Agents dock." });
    } catch (e) {
      toast({ tone: "fail", title: "Claude did not get it", body: e instanceof Error ? e.message : String(e) });
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
        aria-label="Ask Claude to write or change cards"
        placeholder="Ask Claude: paste a list or say what you need, and it writes the cards (Enter sends, Shift+Enter for a new line)"
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
        Ask
      </button>
    </div>
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
        title: `Deleted #${card.id} ${card.title}`,
        body: card.lane ? `Its lane ${card.lane} stays; clean it from Lanes if you do not need it.` : undefined,
        action: {
          label: "Undo",
          run: () =>
            void api.cardRestore(repo, card.id).then(onChanged, (e) => toast({ tone: "fail", title: `Could not restore #${card.id}`, body: e instanceof Error ? e.message : String(e) })),
        },
      },
      12000,
    );
    return true;
  } catch (e) {
    toast({ tone: "fail", title: `Could not delete #${card.id}`, body: e instanceof Error ? e.message : String(e) });
    return false;
  }
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
    return `After ${called(dep)} lands: it builds on it`;
  }
  const mine = new Set(c.areas ?? []);
  if (mine.size > 0)
    for (const b of busy) {
      const o = cards.find((x) => x.id === b);
      const shared = o && o.id !== id ? (o.areas ?? []).find((a) => mine.has(a)) : undefined;
      if (o && shared) return `Waits for ${called(o)}: both change ${shared}`;
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
        ? { ...place(c, props.yard), column: "needs" as const, chip: "needs" as const, icon: "needs" as const, label: asking.waiting[0]!.tool === "Bash" ? "Wants to run" : `Wants ${asking.waiting[0]!.tool}`, why: `Asks to run ${asking.waiting[0]!.detail}` }
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
  const [drag, setDrag] = createSignal<{ id: number; col: string; y0: number; dy: number; live: boolean } | null>(null);
  let dragged = false;
  const columnEls: Record<string, HTMLElement | undefined> = {};

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
    setDrag({ id, col, y0: e.clientY, dy: 0, live: false });
    const move = (ev: PointerEvent) => {
      const d = drag();
      if (!d) return;
      const dy = ev.clientY - d.y0;
      if (!d.live && Math.abs(dy) < 5) return;
      setDrag({ ...d, dy, live: true });
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
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
  }

  /** The line that shows where the dragged card would land: before or after which card. */
  const gapMark = (col: string, id: number): "before" | "after" | undefined => {
    const d = drag();
    if (!d?.live || d.col !== col || d.id === id) return undefined;
    const others = byColumn(col).map((x) => x.card.id).filter((x) => x !== d.id);
    const y = d.y0 + d.dy;
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
  const runLabel = () =>
    props.phase === "analyzing"
      ? "Stop analysing"
      : props.phase === "asking"
        ? "Answer to go on"
        : props.phase === "finishing"
          ? "Finishing"
          : props.phase === "working"
            ? props.queued.length > 0
              ? `Stop (${props.queued.length} queued)`
              : "Finish now"
            : unanalysed().length > 0
              ? `Analyse ${unanalysed().length}`
              : "Start development";
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
        ? "Stops starting queued cards; runs already going keep going"
        : "Close the batch now: the cards still in review stay there"
      : props.phase === "analyzing"
        ? "Stops the analysis; nothing starts"
        : unanalysed().length > 0
          ? "Claude reads each card not analysed yet: what it needs to know (asked one at a time), notes, and which cards must land first or share an engine"
          : "Choose where the work lands and what happens at the end, then the agents start";

  return (
    <div class="board-screen">
      <header class="screen-head">
        <h1 class="title">Board</h1>
        <Show when={(props.cards?.length ?? 0) > 0}>
          <p class="summary">
            {props.cards!.length} {props.cards!.length === 1 ? "card" : "cards"},{" "}
            <Show when={needs() > 0} fallback="nothing needs you.">
              <b>{needs()} need you.</b>
            </Show>
          </p>
        </Show>
        <span class="spacer" />
        <button type="button" class="btn btn-primary" onClick={props.onNew}>
          <Icon name="plus" small />
          New card <span class="kbd">C</span>
        </button>
      </header>

      <Show when={props.repo}>{(r) => <BoardAsk repo={r()} />}</Show>

      <For each={props.problems}>{(p) => <ErrorLine>{p}</ErrorLine>}</For>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> Reading the cards</div>}>
        <Show
          when={props.cards!.length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">No cards yet</div>
              <p class="empty-body">
                Cards live outside the repository, so every branch and worktree sees the same board. Add one with New card, or let
                Claude Code add them through MCP.
              </p>
            </div>
          }
        >
          <div class="board" data-dragging={drag()?.live ? "" : undefined}>
            <For each={columns}>
              {(col) => (
                <section class="col" aria-label={columnTitle[col]} ref={(el) => (columnEls[col] = el)}>
                  <h2 class="col-head" data-hot={col === "needs" && byColumn(col).length > 0 ? "" : undefined}>
                    <Icon name={colIcon[col]} small />
                    {columnTitle[col]}
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
                        {runLabel()}
                      </button>
                    </Show>
                  </h2>
                  <Show when={byColumn(col).length > 0} fallback={<p class="col-empty">{columnEmpty[col]}</p>}>
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
                          aria-label={`Card ${card.id}: ${card.title}. Alt and the arrow keys move it; Command Backspace deletes it.`}
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
                                  Running <span class="figure">{clock(elapsed(r(), props.now))}</span>
                                </span>
                              )}
                            </Show>
                            <Show when={p.column === "backlog"}>
                              <span class="card-analysis" data-on={card.analyzed ? "" : undefined} title={card.analyzed ? `Analysed ${card.analyzed}` : "Not analysed: Run all reads it first"}>
                                {card.analyzed ? "Analysed" : "Not analysed"}
                              </span>
                            </Show>
                            <Show when={(card.notes?.length ?? 0) > 0}>
                              <span class="card-notes" title={`${card.notes!.length} ${card.notes!.length === 1 ? "note" : "notes"}`}>
                                <Icon name="note" small />
                                {card.notes!.length}
                              </span>
                            </Show>
                          </div>
                          <div class="card-title">{card.title}</div>
                          <Show when={p.why}>
                            <div class="card-why">{p.why}</div>
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
                </section>
              )}
            </For>
          </div>
        </Show>
      </Show>
    </div>
  );
}

/** Run all's questions, one at a time, each with its card as context. */
export function QuestionsSheet(props: { repo: string; cards: Card[]; onChanged: () => void; onDone: () => void; onStop: () => void }) {
  const open = () => openQuestions(props.cards);
  // The count at the start, so "2 of 5" holds while answers land.
  const total = open().length;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const current = () => open()[0];
  createEffect(on(() => open().length, (n) => n === 0 && props.onDone(), { defer: true }));

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
      title="Before the run"
      onClose={props.onDone}
      foot={
        <>
          <span class="sheet-note">
            Question {Math.min(total, total - open().length + 1)} of {total}
          </span>
          <button type="button" class="btn btn-ghost" onClick={props.onStop} disabled={busy()}>
            Stop Run all
          </button>
          <button type="button" class="btn" onClick={props.onDone} disabled={busy()} title="Cards with open questions wait; the rest start now">
            Answer later
          </button>
        </>
      }
    >
      <Show when={current()} keyed>
        {(q) => (
          <div class="ask-one">
            <p class="ask-card">
              <span class="card-id">#{q.card.id}</span> {q.card.title}
            </p>
            <p class="inbox-q">{q.text}</p>
            <QuestionBlock q={q} selected busy={busy()} onAnswer={(a) => void answer(a)} />
          </div>
        )}
      </Show>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

// ---------------------------------------------------------------- one card

/** A note's age in words: "now", "12 min", "3 h", "2 d", then the date. */
function ago(at: string, now: number): string {
  const t = Date.parse(at);
  if (Number.isNaN(t)) return at;
  const m = Math.floor((now - t) / 60000);
  if (m < 1) return "now";
  if (m < 60) return `${m} min ago`;
  if (m < 60 * 24) return `${Math.floor(m / 60)} h ago`;
  if (m < 60 * 24 * 7) return `${Math.floor(m / 1440)} d ago`;
  return new Date(t).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

/** Inline markdown that notes use: `code` and **bold**. Never HTML. */
function inline(text: string): JSX.Element {
  return (
    <For each={text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g).filter(Boolean)}>
      {(part) =>
        part.startsWith("`") && part.endsWith("`") && part.length > 2 ? (
          <code>{part.slice(1, -1)}</code>
        ) : part.startsWith("**") && part.endsWith("**") && part.length > 4 ? (
          <b>{part.slice(2, -2)}</b>
        ) : (
          part
        )
      }
    </For>
  );
}

/** A note's markdown, small: paragraphs, bullet lists, headings as bold lines, fenced code. */
export function Prose(props: { text: string }) {
  const blocks = () => {
    const out: ({ kind: "p" | "h"; text: string } | { kind: "ul"; items: string[] } | { kind: "pre"; text: string })[] = [];
    const lines = props.text.replace(/\r/g, "").split("\n");
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]!;
      if (line.startsWith("```")) {
        const code: string[] = [];
        while (++i < lines.length && !lines[i]!.startsWith("```")) code.push(lines[i]!);
        out.push({ kind: "pre", text: code.join("\n") });
      } else if (/^\s*[-*] /.test(line)) {
        const last = out[out.length - 1];
        const item = line.replace(/^\s*[-*] /, "");
        if (last?.kind === "ul") last.items.push(item);
        else out.push({ kind: "ul", items: [item] });
      } else if (/^#{1,6} /.test(line)) {
        out.push({ kind: "h", text: line.replace(/^#+ /, "") });
      } else if (line.trim() === "") {
        out.push({ kind: "p", text: "" });
      } else {
        const last = out[out.length - 1];
        if (last?.kind === "p" && last.text) last.text += ` ${line.trim()}`;
        else out.push({ kind: "p", text: line.trim() });
      }
    }
    return out.filter((b) => !(b.kind === "p" && !b.text));
  };
  return (
    <div class="prose">
      <For each={blocks()}>
        {(b) =>
          b.kind === "ul" ? (
            <ul>
              <For each={b.items}>{(x) => <li>{inline(x)}</li>}</For>
            </ul>
          ) : b.kind === "pre" ? (
            <pre>{b.text}</pre>
          ) : b.kind === "h" ? (
            <p class="prose-h">{inline(b.text)}</p>
          ) : (
            <p>{inline(b.text)}</p>
          )
        }
      </For>
    </div>
  );
}

const noteBy: Record<string, { name: string; icon: "spark" | "person" | "verify" }> = {
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
  const [state, setState] = createSignal<CardState>(props.card.state);
  const [lane, setLane] = createSignal(props.card.lane ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal("");
  const [noting, setNoting] = createSignal(false);
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

  async function save() {
    setBusy(true);
    setError(null);
    try {
      await api.cardUpdate(props.repo, props.card.id, {
        title: title().trim() !== props.card.title ? title().trim() : undefined,
        body: body() !== props.card.body ? body() : undefined,
        state: state() !== props.card.state ? state() : undefined,
        lane: lane().trim() !== (props.card.lane ?? "") ? lane().trim() : undefined,
      });
      toast({ tone: "done", title: `#${props.card.id} saved` });
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
      toast({ tone: "done", title: `Recorded as ${r.decision}`, body: text });
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
      props.onChanged();
    } catch (e) {
      setError(msg(e));
    } finally {
      setNoting(false);
    }
  }

  return (
    <Sheet
      title={`Card #${props.card.id}`}
      class="card-sheet"
      onClose={props.onClose}
      head={
        <div class="cs-head">
          <div class="cs-head-line">
            <span class="cs-id">#{props.card.id}</span>
            <span class="chip" data-state={where().chip}>
              <Icon name={where().icon} small />
              {where().label}
            </span>
            <span class="cs-where">in {columnTitle[where().column]}</span>
            <button type="button" class="btn btn-ghost btn-icon cs-close" aria-label="Close" onClick={props.onClose}>
              <Icon name="close" small />
            </button>
          </div>
          <input
            class="cs-title"
            aria-label="Title"
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
            title={live() ? "Stop its run first" : "Delete this card (you can undo)"}
            onClick={() => void deleteCard(props.repo, props.card, props.onChanged).then((ok) => ok && props.onClose())}
          >
            <Icon name="trash" small /> Delete
          </button>
          <span class="sheet-note">{props.card.file}</span>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            {dirty() ? "Cancel" : "Close"}
          </button>
          <Show
            when={!dirty()}
            fallback={
              <button type="button" class="btn btn-primary" disabled={busy() || !title().trim()} aria-busy={busy()} onClick={save}>
                <Show when={busy()}>
                  <Spinner />
                </Show>
                Save
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
                  title={open().length > 0 ? "Answer the open questions first" : "Claude Code works the card in its own worktree"}
                  onClick={props.onRun}
                >
                  <Icon name="run" small />
                  Run
                </button>
              }
            >
              {(r) => (
                <button type="button" class="btn btn-primary" onClick={() => props.onOpenRun(r().id)}>
                  <Icon name="run" small />
                  Open the run <span class="figure">{clock(elapsed(r(), props.now))}</span>
                </button>
              )}
            </Show>
          </Show>
        </>
      }
    >
      <div class="cs-grid">
        <div class="cs-main">
          <section class="cs-section">
            <div class="field-head">
              <label class="cs-label" for="card-body">
                Spec
              </label>
              <Dictate field={() => specRef} value={body} onChange={(v) => (setBody(v), specRef && fit(specRef))} />
            </div>
            <textarea
              ref={(el) => {
                specRef = el;
                requestAnimationFrame(() => fit(el));
              }}
              id="card-body"
              class="input textarea cs-spec"
              placeholder="Goal, acceptance criteria, out of scope, how to verify"
              value={body()}
              onInput={(e) => {
                setBody(e.currentTarget.value);
                fit(e.currentTarget);
              }}
            />
          </section>

          <Show when={open().length > 0}>
            <section class="cs-section">
              <h3 class="cs-label">
                Open questions <span class="count count-hot">{open().length}</span>
              </h3>
              <For each={open()}>
                {(q) => (
                  <div class="sheet-question">
                    <p class="inbox-q">{q.text}</p>
                    <QuestionBlock q={q} selected={false} busy={busy()} onAnswer={(a) => void answer(q.id, a)} />
                  </div>
                )}
              </For>
            </section>
          </Show>

          <section class="cs-section">
            <h3 class="cs-label">
              Notes <span class="count">{notes().length}</span>
              <span class="cs-hint">What Claude learned working this card, and what you add. Every run reads them.</span>
            </h3>
            <Show when={notes().length > 0} fallback={<p class="cs-empty">No notes yet. Run all's analysis and each run add theirs here.</p>}>
              <ol class="notes">
                <For each={notes()}>
                  {(n) => (
                    <li class="note" data-by={n.by}>
                      <div class="note-head">
                        <Icon name={noteBy[n.by]?.icon ?? "person"} small />
                        <span class="note-by">{noteBy[n.by]?.name ?? n.by}</span>
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
            <div class="note-compose">
              <textarea
                class="input textarea"
                rows="2"
                aria-label="Add a note"
                placeholder="Add context for the next run (⌘↩ adds it)"
                value={note()}
                onInput={(e) => setNote(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void addNote();
                  }
                }}
              />
              <button type="button" class="btn" disabled={noting() || !note().trim()} aria-busy={noting()} onClick={() => void addNote()}>
                <Show when={noting()} fallback={<Icon name="plus" small />}>
                  <Spinner />
                </Show>
                Add note
              </button>
            </div>
          </section>
          <Show when={error()}>
            <ErrorLine>{error()}</ErrorLine>
          </Show>
        </div>

        <aside class="cs-side">
          <div class="field">
            <label class="field-label" for="card-state">
              State
            </label>
            <select id="card-state" class="select" value={state()} onChange={(e) => setState(e.currentTarget.value as CardState)}>
              <For each={cardStates}>{(s) => <option value={s}>{stateLabel[s]}</option>}</For>
            </select>
          </div>
          <div class="field">
            <label class="field-label" for="card-lane">
              Lane
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
              <h3 class="field-label">Runs</h3>
              <ul class="card-runs">
                <For each={props.runs}>
                  {(r) => (
                    <li>
                      <button type="button" class="card-run cs-run" onClick={() => props.onOpenRun(r.id)} title={r.handoff?.summary ?? r.error ?? r.branch}>
                        <span class="chip" data-state={runStateView[r.state].chip}>
                          <Icon name={runStateView[r.state].icon} small />
                          {runStateView[r.state].label}
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
              <h3 class="field-label">Decisions</h3>
              <dl class="decisions">
                <For each={answered()}>
                  {(q) => (
                    <div>
                      <dt>{q.text}</dt>
                      <dd>
                        {q.answer}
                        <Show when={q.decision}>
                          <span class="tag">{q.decision}</span>
                        </Show>
                      </dd>
                    </div>
                  )}
                </For>
              </dl>
            </div>
          </Show>

          <p class="cs-meta">
            Created {ago(props.card.created, props.now)}
            <Show when={props.card.analyzed}>, analysed {ago(props.card.analyzed!, props.now)}</Show>
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
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function create(e: Event) {
    e.preventDefault();
    if (!title().trim()) return;
    setBusy(true);
    setError(null);
    try {
      const c = await api.cardAdd(props.repo, title().trim(), body());
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
      title="New card"
      onClose={props.onClose}
      foot={
        <>
          <button type="button" class="btn btn-ghost" onClick={props.onClose} disabled={busy()}>
            Cancel
          </button>
          <button type="submit" form="new-card" class="btn btn-primary" disabled={busy() || !title().trim()} aria-busy={busy()}>
            <Show when={busy()} fallback={<Icon name="plus" small />}>
              <Spinner />
            </Show>
            Create card
          </button>
        </>
      }
    >
      <form id="new-card" class="sheet-form" onSubmit={create}>
        <div class="field">
          <label class="field-label" for="new-card-title">
            Title
          </label>
          <input
            id="new-card-title"
            class="input"
            placeholder="One line, in your words"
            autocomplete="off"
            autofocus
            value={title()}
            onInput={(e) => setTitle(e.currentTarget.value)}
          />
        </div>
        <div class="field">
          <div class="field-head">
            <label class="field-label" for="new-card-body">
              Spec
            </label>
            <Dictate field={() => newSpecRef} value={body} onChange={setBody} />
          </div>
          <textarea
            ref={newSpecRef}
            id="new-card-body"
            class="input textarea input-mono"
            rows="6"
            placeholder="Optional: goal, acceptance criteria, how to verify"
            value={body()}
            onInput={(e) => setBody(e.currentTarget.value)}
          />
          <span class="field-help">Saved to this repository's board in ~/.pitlane, the same in every branch and worktree.</span>
        </div>
      </form>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}
