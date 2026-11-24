// The one-card view (its notes, its questions, its runs) and Run all's questions, asked one at a time.
import { For, Show, createEffect, createSignal, on } from "solid-js";
import { api, isLive, type Card, type RunMeta, type Yard } from "./api";
import { columnIcon, columnTitle, movesFor, noteBy, openQuestions, place, runStateView, unblockActions, type Move } from "./cards";
import { Icon } from "./icons";
import { Dictate } from "./dictate";
import { CardImages, imagesIn } from "./images";
import { Prose, inline } from "./prose";
import { DecisionItem, LiveMark, QuestionAbout, QuestionBlock, QuestionTalk, talking } from "./questions";
import { ErrorLine, Sheet, Spinner, dismissable, toast } from "./ui";
import { CARD_SIDE_W, Grip } from "./layout";
import { plural, t } from "./i18n";
import { age, clock, elapsed, money, msg } from "./format";
import { MiniTrack } from "./track";
import { ForgetSheet, deleteCard, hasMemory, unblockCard } from "./cardactions";
import { analyseCards, analysing, stopAnalysing } from "./store/analysis";

/**
 * Run all's questions, one at a time, each with its card as context and Claude to talk it over
 * with. Once all are answered nothing starts by itself: every decision of the batch shows, each
 * can change or be talked over again, and Start runs them.
 */
/** `onStop` is Run all's; without it the questions come from cards analysed on their own, and
 *  nothing starts once they are answered. */
export function QuestionsSheet(props: { repo: string; cards: Card[]; onChanged: () => void; onDone: () => void; onStop?: () => void }) {
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
      title={review() && open().length === 0 ? t("All decided") : props.onStop ? t("Before the run") : t("Claude's questions")}
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
          <Show when={props.onStop}>
            {(stop) => (
              <button type="button" class="btn btn-ghost" onClick={stop()} disabled={busy()}>
                {t("Stop Run all")}
              </button>
            )}
          </Show>
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
              <button
                type="button"
                class="btn"
                onClick={props.onDone}
                disabled={busy()}
                title={props.onStop ? t("Cards with open questions wait; the rest start now") : t("They stay on the cards, and in the Inbox")}
              >
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
              <Show when={props.onStop} fallback={t("Done")}>
                <Icon name="play" small />
                {t("Start")}
              </Show>
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
              {props.onStop
                ? t("Go back to any of them before the cards start: choose again, or talk it over with Claude (\u201cand if we did it like this?\u201d).")
                : t("Go back to any of them before a run: choose again, or talk it over with Claude (\u201cand if we did it like this?\u201d).")}
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
              <LiveMark q={q} />
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

export function CardSheet(props: {
  repo: string;
  card: Card;
  yard: Yard | null;
  now: number;
  runs: RunMeta[];
  /** `fresh`: start over instead of resuming the stopped session. */
  onRun: (fresh?: boolean) => void;
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
  // Move to…: the same rules as the Board's drag (the daemon's), never a free choice of state.
  const [moveMenu, setMoveMenu] = createSignal(false);
  const [holding, setHolding] = createSignal<string | null>(null);
  let moveBox: HTMLDivElement | undefined;
  dismissable(() => moveBox, moveMenu, () => setMoveMenu(false));
  const [lane, setLane] = createSignal(props.card.lane ?? "");
  // The card can change under an open sheet (its agent renames it, an analysis rewrites the
  // spec). A field you have not touched follows the card; one you are editing keeps your text,
  // and the sheet shows as edited until you save or discard.
  const follow = (theirs: () => string, mine: () => string, set: (v: string) => void) =>
    createEffect(on(theirs, (next, prev) => mine() === prev && set(next), { defer: true }));
  follow(() => props.card.title, title, setTitle);
  follow(() => props.card.body, body, setBody);
  follow(() => props.card.lane ?? "", lane, setLane);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal("");
  let noteRef: HTMLTextAreaElement | undefined;
  const [noting, setNoting] = createSignal(false);
  const [forgetOne, setForgetOne] = createSignal(false);
  const dirty = () =>
    title().trim() !== props.card.title ||
    body() !== props.card.body ||
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
        lane: lane().trim() !== (props.card.lane ?? "") ? lane().trim() : undefined,
      });
      toast({ tone: "done", title: t("#{id} saved", { id: props.card.id }) });
      props.onChanged();
      props.onClose();
    } catch (e) {
      setError(msg(e));
    } finally {
      setBusy(false);
    }
  }

  const moves = () => movesFor(props.card, !!live());
  /** Moves the card under the Board's rules; a running card stops first, after you say so. */
  async function move(m: Move, reason?: string) {
    setMoveMenu(false);
    if (m.stops && !window.confirm(t("#{id} is running. Stop it and move it to {column}? Its session is kept: the next run picks up where it stopped.", { id: props.card.id, column: t(columnTitle[m.column]) }))) return;
    setBusy(true);
    setError(null);
    try {
      await api.cardMove(props.repo, props.card.id, m.to, reason);
      toast({
        tone: "done",
        title: m.to === "hold" ? t("#{id} on hold", { id: props.card.id }) : t("#{id} in {column}", { id: props.card.id, column: t(columnTitle[m.column]) }),
        body: m.to === "queue" ? t("Run all takes it in board order.") : m.to === "hold" ? t("Analyse and Start leave it out until you release it.") : undefined,
      });
      setHolding(null);
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
          <Show when={!live()}>
            <For each={unblockActions(props.card).filter((a) => a.action !== "backlog")}>
              {(a) => (
                <button
                  type="button"
                  class="btn btn-ghost"
                  disabled={busy() || dirty()}
                  title={dirty() ? t("Save your changes first") : t(a.title)}
                  onClick={() => void unblockCard(props.repo, props.card, a.action, props.onChanged).then(props.onClose)}
                >
                  <Icon name={a.icon} small /> {t(a.label)}
                </button>
              )}
            </For>
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
                <>
                  <Show when={props.card.paused}>
                    <button
                      type="button"
                      class="btn"
                      disabled={busy() || open().length > 0 || props.card.state === "done"}
                      title={t("A new session from the card; the stopped one stays under Runs")}
                      onClick={() => props.onRun(true)}
                    >
                      {t("Start over")}
                    </button>
                  </Show>
                  <button
                    type="button"
                    class="btn btn-primary"
                    disabled={busy() || open().length > 0 || props.card.state === "done"}
                    title={
                      open().length > 0
                        ? t("Answer the open questions first")
                        : props.card.paused
                          ? t("Picks up the stopped session: Claude keeps all it did and said, and hears what the card got since")
                          : t("Claude Code works the card in its own worktree")
                    }
                    onClick={() => props.onRun()}
                  >
                    <Icon name="run" small />
                    {props.card.paused ? t("Resume") : t("Run")}
                  </button>
                </>
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
                    <LiveMark q={q} />
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
              <Show when={!props.card.analyzed && props.card.state !== "done" && !live()}>
                <Show
                  when={analysing(props.repo).includes(props.card.id)}
                  fallback={
                    <button
                      type="button"
                      class="btn btn-ghost cs-forget"
                      onClick={() => void analyseCards(props.repo, [props.card.id], props.onChanged)}
                      title={t("Claude reads this card: what it needs to know (asked one at a time), notes, and which cards must land first. Nothing starts")}
                    >
                      <Icon name="search" small /> {t("Analyse")}
                    </button>
                  }
                >
                  <button type="button" class="btn btn-ghost cs-forget" aria-busy="true" onClick={() => void stopAnalysing(props.repo, [props.card.id])} title={t("Claude is reading this card. Click to stop")}>
                    <Spinner /> {t("Analysing")}
                  </button>
                </Show>
              </Show>
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
                          {age(n.at, props.now)}
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
            <span class="field-label">{t("State")}</span>
            <div class="cs-move">
              <span class="cs-move-where">
                <Icon name={columnIcon[where().column]} small />
                {t(columnTitle[where().column])}
                <Show when={where().label !== t(columnTitle[where().column])}>
                  <span class="cs-move-label">· {where().label}</span>
                </Show>
              </span>
              <Show when={moves().length > 0}>
                <div class="anchor" ref={moveBox}>
                  <button
                    type="button"
                    class="btn btn-ghost"
                    aria-expanded={moveMenu()}
                    disabled={busy() || dirty()}
                    title={dirty() ? t("Save your changes first") : t("Where you can move it: running, checking, review and landing are Pitlane's to set")}
                    onClick={() => setMoveMenu(!moveMenu())}
                  >
                    {t("Move to…")}
                  </button>
                  <Show when={moveMenu()}>
                    <div class="pop cs-move-pop" role="menu">
                      <For each={moves()}>
                        {(m) => (
                          <button type="button" class="menu-item" role="menuitem" onClick={() => (m.to === "hold" ? (setMoveMenu(false), setHolding("")) : void move(m))}>
                            <Icon name={m.to === "hold" ? "pause" : columnIcon[m.column]} small />
                            <span>{t(m.label)}</span>
                            <Show when={m.stops}>
                              <span class="cs-move-label">{t("stops the run")}</span>
                            </Show>
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>
            </div>
            <Show when={holding() != null}>
              <form
                class="cs-hold"
                onSubmit={(e) => {
                  e.preventDefault();
                  const m = moves().find((x) => x.to === "hold");
                  if (m) void move(m, holding() ?? "");
                }}
              >
                <input class="input" autofocus placeholder={t("Why, in a few words (optional)")} value={holding() ?? ""} onInput={(e) => setHolding(e.currentTarget.value)} />
                <button type="submit" class="btn btn-ghost" disabled={busy()}>{t("Hold")}</button>
                <button type="button" class="btn btn-ghost" onClick={() => setHolding(null)}>{t("Cancel")}</button>
              </form>
            </Show>
            <Show when={props.card.held != null}>
              <p class="field-help">{props.card.held ? t("On hold: {why}", { why: props.card.held }) : t("On hold: Analyse and Start leave it out.")}</p>
            </Show>
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
                      <button
                        type="button"
                        class="card-run cs-run"
                        onClick={() => props.onOpenRun(r.id)}
                        title={r.id === props.card.paused ? t("Stopped mid-work: Resume picks up this session") : (r.handoff?.summary ?? r.error ?? r.branch)}
                      >
                        <span class="chip" data-state={runStateView[r.state].chip}>
                          <Icon name={r.id === props.card.paused ? "pause" : runStateView[r.state].icon} small />
                          {r.id === props.card.paused ? t("Paused") : t(runStateView[r.state].label)}
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
            {t("Created {when}", { when: age(props.card.created, props.now) })}
            <Show when={props.card.analyzed}>, {t("analysed {when}", { when: age(props.card.analyzed!, props.now) })}</Show>
          </p>
        </aside>
      </div>
    </Sheet>
  );
}
