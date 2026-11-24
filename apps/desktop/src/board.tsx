// Board: cards grouped by who acts next. A card linked to a lane carries that lane's track.
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";
import { api, isLive, type Card, type Plan, type RunMeta, type Yard } from "./api";
import { columnEmpty, columnIcon, columnTitle, columns, holdNote, latestRun, movesFor, onItsWay, place, planNote, reviewSummary, unblockActions, type Column, type Move } from "./cards";
import { Icon } from "./icons";
import { Dictate } from "./dictate";
import { attachImages, imagesIn } from "./images";
import { Linked } from "./links";
import { refreshAgents } from "./store/agents";
import { MiniTrack } from "./track";
import { ForgetSheet, deleteCard, hasMemory, unblockCard } from "./cardactions";
import { ErrorLine, Sheet, Spinner, toast } from "./ui";
import { plural, t } from "./i18n";
import { clock, elapsed, msg } from "./format";


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
        placeholder={t("Ask Claude to write cards: paste a list or say what you need")}
        value={text()}
        onInput={(e) => (setText(e.currentTarget.value), fit())}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && !e.shiftKey && !e.isComposing) (e.preventDefault(), void send());
        }}
        disabled={busy()}
      />
      {/* The keys show only while you write, so the empty bar reads as one short line. */}
      <span class="board-ask-keys" aria-hidden="true">
        <span class="kbd">↵</span> {t("sends")} <span class="kbd">⇧↵</span> {t("new line")}
      </span>
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
  /** Brings Run all's questions back up: the way on when their sheet was closed some other way. */
  onQuestions: () => void;
  /** While Run all works: cards moved to Queued that Claude never read are analysed, then the batch takes them. */
  onJoin: (ids: number[]) => void;
  /** Cards being analysed to join the running batch. */
  joining: number[];
  /** Choose where the work lands, then these cards move to Queued and the agents start. */
  onStartDev: (ids: number[]) => void;
  onStopAll: () => void;
  /** The queue is empty but cards wait for review: close the batch now. */
  onFinish: () => void;
  /** Where Run all is. */
  phase: "idle" | "analyzing" | "asking" | "working" | "finishing";
  /** Cards Run all still has to start. */
  queued: number[];
  /** This repository's part of the agents while Run all works in more than one. */
  share?: { mine: number; total: number; repos: number } | null;
  /** The whole board's new order, after a card was dragged. */
  onReorder: (ids: number[]) => void;
  /** The planner's view of open cards, only while Run all works. */
  plan?: Plan | null;
  /** Every card the working batch took (started ones too). */
  batch?: number[];
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
  // Each card with where it sits and what it says. A recompute (a poll, the yard, Run all moving
  // on) hands back the same entry for a card that reads the same, so its tile keeps its DOM, its
  // focus and its hover instead of being rebuilt every few seconds while an agent works.
  type Placed = { card: Card; p: ReturnType<typeof place> };
  const placed = createMemo<Placed[]>((prev) => {
    const before = new Map(prev.map((x) => [x.card.id, x]));
    const busy = props.runs.filter((r) => isLive(r.state)).map((r) => r.card);
    const moving = onItsWay(props.cards ?? [], busy, props.queued, props.batch ?? props.queued);
    const same = (next: Placed) => {
      const o = before.get(next.card.id);
      return o && o.card === next.card && JSON.stringify(o.p) === JSON.stringify(next.p) ? o : next;
    };
    return sorted().map((c) => {
      const asking = props.runs.find((r) => r.card === c.id && isLive(r.state) && r.waiting.length > 0);
      if (c.state === "review" && !asking) {
        const r = reviewSummary(latestRun(props.runs, c.id));
        return same({ card: c, p: { ...place(c, props.yard), chip: r.chip, icon: r.icon, label: r.label, why: r.text } });
      }
      const p = asking
        ? { ...place(c, props.yard), column: "needs" as const, chip: "needs" as const, icon: "needs" as const, label: asking.waiting[0]!.tool === "Bash" ? t("Wants to run") : t("Wants {tool}", { tool: asking.waiting[0]!.tool }), why: t("Asks to run {detail}", { detail: asking.waiting[0]!.detail }) }
        : place(c, props.yard);
      const note = props.phase === "working" && props.queued.includes(c.id) ? (holdNote(c.id, props.cards ?? [], busy) ?? planNote(props.plan, c.id, props.cards ?? [], moving)) : null;
      return same({ card: c, p: note && p.column === "queued" ? { ...p, why: note } : p });
    });
  }, []);
  const byColumn = (col: string) => placed().filter((x) => x.p.column === col);
  const needs = () => byColumn("needs").length;
  const backlog = () => byColumn("backlog").filter(({ card }) => !liveRun(card.id) && card.state !== "done");
  /** Backlog cards a run may take: not held. */
  const takeable = () => backlog().filter(({ card }) => card.held == null);
  const fila = () => byColumn("queued").filter(({ card }) => !liveRun(card.id));
  /** In progress's runs an agent still works: what Pause stops. */
  const working = () => byColumn("progress").map(({ card }) => liveRun(card.id)).filter((r): r is RunMeta => !!r);
  const [pausing, setPausing] = createSignal(false);
  /** Pause: what closing the app does to this board. Run all stops first, so it starts nothing
   *  again; then each run ends its step and its card goes back (to Queued under Run all, else the
   *  Backlog) keeping its session, so the next Run picks up where it was. */
  async function pauseAll() {
    const runs = working();
    if (runs.length === 0 || pausing()) return;
    setPausing(true);
    if (props.phase === "working" || props.phase === "analyzing" || props.phase === "asking") props.onStopAll();
    const failed = (await Promise.allSettled(runs.map((r) => api.runStop(r.id)))).filter((x) => x.status === "rejected").length;
    setPausing(false);
    void refreshAgents();
    props.onChanged();
    if (failed > 0) return toast({ tone: "fail", title: plural(failed, "{n} card did not pause", "{n} cards did not pause") });
    toast({
      tone: "info",
      title: plural(runs.length, "Pausing {n} card", "Pausing {n} cards"),
      body: t("Each ends its current step and goes back to wait; its Run picks up the same session."),
    });
  }

  // ---- dragging a card: up or down its column, or to another column the rules allow
  // x0/y0: the pointer at the start; px/py: the pointer now; over: the column under it. While it
  // drags, a copy of the card (the ghost) follows the pointer and the card keeps its place, faded.
  type Drag = { id: number; col: Column; x0: number; y0: number; px: number; py: number; live: boolean; over: Column | null };
  const [drag, setDrag] = createSignal<Drag | null>(null);
  let dragged = false;
  let ghost: HTMLElement | null = null;
  let ghostAt = { dx: 0, dy: 0 };
  let boardEl: HTMLElement | undefined;
  const columnEls: Record<string, HTMLElement | undefined> = {};
  const bodyEls: Record<string, HTMLElement | undefined> = {};
  /** The column under the pointer. */
  const columnAt = (x: number): Column | null => columns.find((c) => {
    const r = columnEls[c]?.getBoundingClientRect();
    return !!r && x >= r.left && x <= r.right;
  }) ?? null;
  /** The move that dropping this card on that column makes, if the rules allow one (holding is not a drop). */
  const dropMove = (id: number, col: Column | null): Move | undefined => {
    const card = (props.cards ?? []).find((c) => c.id === id);
    if (!card || !col) return undefined;
    return movesFor(card, !!liveRun(id)).find((m) => m.column === col && m.to !== "hold");
  };
  /** How a column takes the card being dragged: its own column reorders, "ok" moves it, "no" refuses. */
  const dropMark = (col: Column): "ok" | "no" | undefined => {
    const d = drag();
    if (!d?.live || d.col === col) return undefined;
    return dropMove(d.id, col) ? "ok" : "no";
  };
  /** Near the top or bottom edge of its column, a dragged card scrolls the column, and near the board's left or right edge
   *  the board, faster the closer it gets. */
  const EDGE = 64;
  const edgeSpeed = (near: number, far: number) =>
    near < EDGE ? -Math.ceil(((EDGE - Math.max(near, 0)) / EDGE) * 16) : far < EDGE ? Math.ceil(((EDGE - Math.max(far, 0)) / EDGE) * 16) : 0;
  let scrolling = 0;
  function autoscroll() {
    scrolling = 0;
    let d = drag();
    if (!d?.live) return;
    if (boardEl && boardEl.scrollWidth > boardEl.clientWidth) {
      const b = boardEl.getBoundingClientRect();
      const before = boardEl.scrollLeft;
      boardEl.scrollLeft += edgeSpeed(d.px - b.left, b.right - d.px);
      if (boardEl.scrollLeft !== before) {
        d = { ...d, over: columnAt(d.px) };
        setDrag(d);
      }
    }
    const body = d.over ? bodyEls[d.over] : undefined;
    if (!body) {
      scrolling = requestAnimationFrame(autoscroll);
      return;
    }
    const r = body.getBoundingClientRect();
    const speed = edgeSpeed(d.py - r.top, r.bottom - d.py);
    if (speed !== 0) {
      const before = body.scrollTop;
      body.scrollTop += speed;
      // The gap line follows the cards under a still pointer.
      if (body.scrollTop !== before) setDrag({ ...d });
    }
    scrolling = requestAnimationFrame(autoscroll);
  }

  /** The card leaves its column for another: the daemon's rules, then its place there. */
  async function moveAcross(id: number, col: Column, index: number) {
    const m = dropMove(id, col);
    const card = (props.cards ?? []).find((c) => c.id === id);
    if (!m || !card || !props.repo) return;
    if (m.stops && !window.confirm(t("#{id} is running. Stop it and move it to {column}? Its session is kept: the next run picks up where it stopped.", { id, column: t(columnTitle[col]) }))) return;
    // Where it lands among the column's cards, in the whole board's order.
    const target = byColumn(col).map((x) => x.card.id).filter((x) => x !== id);
    const all = sorted().map((c) => c.id).filter((x) => x !== id);
    const at = index < target.length ? all.indexOf(target[index]!) : target.length > 0 ? all.indexOf(target[target.length - 1]!) + 1 : all.length;
    all.splice(at < 0 ? all.length : at, 0, id);
    try {
      await api.cardMove(props.repo, id, m.to);
      setOrder(all);
      props.onReorder(all);
      // Run all works the queue: a card it never read is read first.
      if (m.to === "queue" && props.phase === "working" && !card.analyzed) props.onJoin([id]);
    } catch (e) {
      toast({ tone: "fail", title: t("#{id} did not move", { id }), body: msg(e) });
      props.onChanged();
    }
  }

  /** Alt+← / Alt+→: to the column beside it, when the rules allow. */
  function stepAcross(id: number, col: Column, dir: -1 | 1) {
    const next = columns[columns.indexOf(col) + dir];
    if (next && dropMove(id, next)) void moveAcross(id, next, byColumn(next).length);
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

  function lift(el: HTMLElement, x: number, y: number) {
    const r = el.getBoundingClientRect();
    ghost = el.cloneNode(true) as HTMLElement;
    ghost.removeAttribute("data-placeholder");
    ghost.removeAttribute("data-gap");
    ghost.removeAttribute("tabindex");
    ghost.setAttribute("aria-hidden", "true");
    ghost.setAttribute("data-lifted", "");
    ghost.classList.add("card-ghost");
    Object.assign(ghost.style, { width: `${r.width}px`, left: `${r.left}px`, top: `${r.top}px` });
    ghostAt = { dx: x - r.left, dy: y - r.top };
    (boardEl ?? document.body).appendChild(ghost);
  }
  function drop() {
    ghost?.remove();
    ghost = null;
  }

  function onPointerDown(e: PointerEvent, col: Column, id: number) {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, a, input")) return;
    const el = e.currentTarget as HTMLElement;
    setDrag({ id, col, x0: e.clientX, y0: e.clientY, px: e.clientX, py: e.clientY, live: false, over: col });
    const move = (ev: PointerEvent) => {
      const d = drag();
      if (!d) return;
      if (!d.live && Math.hypot(ev.clientX - d.x0, ev.clientY - d.y0) < 5) return;
      if (!d.live) lift(el, d.x0, d.y0);
      if (ghost) Object.assign(ghost.style, { left: `${ev.clientX - ghostAt.dx}px`, top: `${ev.clientY - ghostAt.dy}px` });
      setDrag({ ...d, px: ev.clientX, py: ev.clientY, live: true, over: columnAt(ev.clientX) });
      if (!scrolling) scrolling = requestAnimationFrame(autoscroll);
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      cancelAnimationFrame(scrolling);
      scrolling = 0;
      drop();
      endDrag = null;
    };
    const up = (ev: PointerEvent) => {
      end();
      const d = drag();
      setDrag(null);
      if (!d?.live) return;
      dragged = true;
      setTimeout(() => (dragged = false), 0);
      if (ev.type === "pointercancel" || !d.over) return;
      // Where it lands: how many of the other cards' middles sit above the pointer.
      const others = [...(columnEls[d.over]?.querySelectorAll<HTMLElement>("[data-card-id]") ?? [])].filter((el) => Number(el.dataset.cardId) !== d.id);
      const index = others.filter((el) => {
        const r = el.getBoundingClientRect();
        return r.top + r.height / 2 < ev.clientY;
      }).length;
      if (d.over === d.col) moveTo(d.col, d.id, index);
      else void moveAcross(d.id, d.over, index);
    };
    endDrag?.();
    endDrag = end;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }
  // The Board closing mid-drag (another tab, another repository) takes the drag with it.
  let endDrag: (() => void) | null = null;
  onCleanup(() => endDrag?.());

  /** The line that shows where the dragged card would land: before or after which card. */
  const gapMark = (col: Column, id: number): "before" | "after" | undefined => {
    const d = drag();
    if (!d?.live || d.over !== col || d.id === id || dropMark(col) === "no") return undefined;
    const others = byColumn(col).map((x) => x.card.id).filter((x) => x !== d.id);
    const y = d.py;
    const els = others.map((x) => columnEls[col]?.querySelector<HTMLElement>(`[data-card-id="${x}"]`));
    const g = els.filter((el) => {
      const r = el?.getBoundingClientRect();
      return r && r.top + r.height / 2 < y;
    }).length;
    const from = byColumn(col).findIndex((x) => x.card.id === d.id);
    if (col === d.col && g === from) return undefined; // where it already is
    if (g < others.length) return others[g] === id ? "before" : undefined;
    return others[others.length - 1] === id ? "after" : undefined;
  };

  /** Backlog cards Claude never read (held ones wait). */
  const unanalysed = () => takeable().filter(({ card }) => !card.analyzed);
  /** Queued cards Claude never read, not being read now. */
  const filaUnread = () => fila().filter(({ card }) => !card.analyzed && !props.joining.includes(card.id));
  /** What Start takes when Queued is empty: the analysed Backlog, held cards aside. */
  const startable = () => takeable().filter(({ card }) => card.analyzed);
  /** Backlog cards that carry something of Claude's: the analysis, its notes, its open questions. */
  const remembered = () => backlog().map(({ card }) => card).filter(hasMemory);
  const [forgetting, setForgetting] = createSignal<number[] | null>(null);

  // Backlog's head: Analyse (Claude reads the ideas, asks once). Queued's head: the run itself.
  const analyseLabel = () =>
    props.phase === "analyzing" ? t("Stop analysing") : props.phase === "asking" ? t("Answer to go on") : t("Analyse {n}", { n: unanalysed().length });
  const analyseAction = () => {
    if (props.phase === "analyzing") return props.onStopAll();
    if (props.phase === "asking") return props.onQuestions();
    if (props.phase === "idle") props.onAnalyse(unanalysed().map(({ card }) => card.id));
  };
  const showAnalyse = () => props.phase === "analyzing" || props.phase === "asking" || (props.phase === "idle" && unanalysed().length > 0);

  const runLabel = () =>
    props.phase === "finishing"
      ? t("Finishing")
      : props.phase === "working"
        ? props.queued.length > 0
          ? t("Stop ({n} queued)", { n: props.queued.length })
          : t("Finish now")
        : fila().length > 0
          ? filaUnread().length > 0
            ? t("Analyse {n}", { n: filaUnread().length })
            : t("Continue")
          : t("Start ({n})", { n: startable().length });
  const runAction = () => {
    if (props.phase === "working") return props.queued.length > 0 ? props.onStopAll() : props.onFinish();
    if (props.phase !== "idle") return;
    if (fila().length > 0) {
      if (filaUnread().length > 0) return props.onAnalyse(filaUnread().map(({ card }) => card.id));
      return props.onStartDev(fila().map(({ card }) => card.id));
    }
    props.onStartDev(startable().map(({ card }) => card.id));
  };
  const showRun = () => props.phase === "working" || props.phase === "finishing" || (props.phase === "idle" && (fila().length > 0 || startable().length > 0));
  const runTitle = () =>
    props.phase === "working"
      ? props.queued.length > 0
        ? t("Stops starting queued cards; runs already going keep going, and Queued keeps its cards for the next Start")
        : t("Close the batch now: the cards still in review stay there")
      : fila().length > 0
        ? filaUnread().length > 0
          ? t("Claude reads the queued cards it never read: what it needs to know, notes, and which cards must land first")
          : t("Choose where the work lands and what happens at the end, then the agents work Queued")
        : t("Moves the analysed Backlog to Queued, then the agents work it. Cards not analysed or on hold stay in the Backlog")

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
        <button type="button" class="btn btn-primary" data-tip={`${t("New card")} · C`} onClick={props.onNew}>
          <Icon name="plus" small />
          {t("New card")}
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
          <div class="board" data-dragging={drag()?.live ? "" : undefined} ref={(el) => (boardEl = el)}>
            <For each={columns}>
              {(col) => (
                <section class="col" aria-label={t(columnTitle[col])} ref={(el) => (columnEls[col] = el)} data-drop={dropMark(col)}>
                  <h2 class="col-head" data-hot={col === "needs" && byColumn(col).length > 0 ? "" : undefined}>
                    <Icon name={columnIcon[col]} small />
                    {t(columnTitle[col])}
                    <span class={col === "needs" && byColumn(col).length > 0 ? "count count-hot" : "count"}>
                      {byColumn(col).length}
                    </span>
                    <Show when={col === "backlog" && showAnalyse()}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        aria-busy={props.phase === "analyzing"}
                        title={props.phase === "analyzing" ? t("Stops the analysis; nothing starts") : t("Claude reads each card not analysed yet: what it needs to know (asked one at a time), notes, and which cards must land first or share an engine. Nothing starts")}
                        onClick={analyseAction}
                      >
                        <Show when={props.phase === "analyzing"} fallback={<Icon name={props.phase === "asking" ? "needs" : "search"} small />}>
                          <Spinner />
                        </Show>
                        <span class="col-action-label">{analyseLabel()}</span>
                      </button>
                    </Show>
                    <Show when={col === "queued" && props.joining.length > 0}>
                      <span class="col-action col-note" title={t("Claude reads the cards moved to Queued during the run; once their questions are answered the batch takes them")}>
                        <Spinner />
                        <span class="col-action-label">{t("Analysing {n}", { n: props.joining.length })}</span>
                      </span>
                    </Show>
                    <Show when={col === "queued" && props.phase === "working" && props.share}>
                      {(sh) => (
                        <span
                          class="col-action col-note"
                          title={t("Run all works in {n} repositories: they share the {total} agents this Mac takes now. One with nothing ready lends its share to the others.", { n: sh().repos, total: sh().total })}
                        >
                          <Icon name="gauge" small />
                          <span class="col-action-label">{t("{mine} of {total} agents", { mine: sh().mine, total: sh().total })}</span>
                        </span>
                      )}
                    </Show>
                    <Show when={col === "queued" && showRun()}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        aria-busy={props.phase === "finishing"}
                        disabled={props.phase === "finishing"}
                        title={runTitle()}
                        onClick={runAction}
                      >
                        <Show
                          when={props.phase === "finishing"}
                          fallback={<Icon name={props.phase === "working" ? (props.queued.length > 0 ? "stop" : "done") : fila().length > 0 && filaUnread().length > 0 ? "search" : "play"} small />}
                        >
                          <Spinner />
                        </Show>
                        <span class="col-action-label">{runLabel()}</span>
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
                    <Show when={col === "progress" && working().length > 0}>
                      <button
                        type="button"
                        class="btn btn-ghost col-action"
                        aria-busy={pausing()}
                        disabled={pausing()}
                        title={t("Pause every card working here, as closing the app does: each ends its step and keeps its session, and its Run picks it up again")}
                        onClick={() => void pauseAll()}
                      >
                        <Show when={pausing()} fallback={<Icon name="pause" small />}>
                          <Spinner />
                        </Show>
                        <span class="col-action-label">{t("Pause")}</span>
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
                          data-placeholder={drag()?.live && drag()!.id === card.id ? "" : undefined}
                          data-held={card.held != null ? "" : undefined}
                          data-gap={gapMark(col, card.id)}
                          tabindex="0"
                          role="button"
                          aria-label={t("Card {id}: {title}. Alt and the arrow keys move it, left and right to another column; Command Backspace deletes it.", { id: card.id, title: card.title })}
                          onPointerDown={(e) => onPointerDown(e, col, card.id)}
                          onClick={() => !dragged && props.onOpen(card.id)}
                          onKeyDown={(e) => {
                            // ⌘⌫ deletes the focused card, with an undo.
                            if (e.metaKey && (e.key === "Backspace" || e.key === "Delete") && props.repo && !liveRun(card.id)) {
                              e.preventDefault();
                              void deleteCard(props.repo, card, props.onChanged);
                              return;
                            }
                            if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
                              e.preventDefault();
                              stepAcross(card.id, col, e.key === "ArrowLeft" ? -1 : 1);
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
                            <Show when={p.column === "backlog" || p.column === "queued"}>
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
                            <Show when={!liveRun(card.id) && props.repo}>
                              <For each={unblockActions(card).filter((a) => a.action !== "review")}>
                                {(a) => (
                                  <button
                                    type="button"
                                    class="btn btn-ghost btn-icon card-back"
                                    aria-label={t(a.label)}
                                    title={t(a.title)}
                                    onPointerDown={(e) => e.stopPropagation()}
                                    onKeyDown={(e) => e.stopPropagation()}
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      void unblockCard(props.repo!, card, a.action, props.onChanged);
                                    }}
                                  >
                                    <Icon name={a.icon} small />
                                  </button>
                                )}
                              </For>
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
