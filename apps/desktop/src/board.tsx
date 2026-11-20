// Board: cards grouped by who acts next. A card linked to a lane carries that lane's track.
import { For, Show, createMemo, createSignal } from "solid-js";
import { api, cardStates, isLive, type Card, type CardState, type RunMeta, type Yard } from "./api";
import { clock, elapsed, money, runStateView } from "./run";
import { columnEmpty, columnTitle, columns, openQuestions, place, stateLabel } from "./cards";
import { Icon } from "./icons";
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

export function BoardView(props: {
  cards: Card[] | null;
  problems: string[];
  yard: Yard | null;
  now: number;
  runs: RunMeta[];
  onOpen: (id: number) => void;
  onNew: () => void;
}) {
  const liveRun = (id: number) => props.runs.find((r) => r.card === id && isLive(r.state));
  const placed = createMemo(() =>
    (props.cards ?? []).map((c) => {
      const asking = props.runs.find((r) => r.card === c.id && isLive(r.state) && r.waiting.length > 0);
      const p = asking
        ? { ...place(c, props.yard), column: "needs" as const, chip: "needs" as const, icon: "needs" as const, label: asking.waiting[0]!.tool === "Bash" ? "Wants to run" : `Wants ${asking.waiting[0]!.tool}`, why: `Asks to run ${asking.waiting[0]!.detail}` }
        : place(c, props.yard);
      return { card: c, p };
    }),
  );
  const byColumn = (col: string) => placed().filter((x) => x.p.column === col);
  const needs = () => byColumn("needs").length;

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

      <For each={props.problems}>{(p) => <ErrorLine>{p}</ErrorLine>}</For>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> Reading the cards</div>}>
        <Show
          when={props.cards!.length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">No cards yet</div>
              <p class="empty-body">
                Each card is a markdown file in .pitlane/cards, versioned with the code. Add one with New card, or let
                Claude Code add them through MCP.
              </p>
            </div>
          }
        >
          <div class="board">
            <For each={columns}>
              {(col) => (
                <section class="col" aria-label={columnTitle[col]}>
                  <h2 class="col-head">
                    {columnTitle[col]}
                    <span class={col === "needs" && byColumn(col).length > 0 ? "count count-hot" : "count"}>
                      {byColumn(col).length}
                    </span>
                  </h2>
                  <Show when={byColumn(col).length > 0} fallback={<p class="col-empty">{columnEmpty[col]}</p>}>
                    <For each={byColumn(col)}>
                      {({ card, p }) => (
                        <article
                          class="card"
                          data-state={p.column === "needs" ? "needs" : undefined}
                          tabindex="0"
                          role="button"
                          aria-label={`Card ${card.id}: ${card.title}`}
                          onClick={() => props.onOpen(card.id)}
                          onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), props.onOpen(card.id))}
                        >
                          <div class="card-top">
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
                            <span class="card-id">#{card.id}</span>
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

// ---------------------------------------------------------------- one card

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
  const [state, setState] = createSignal<CardState>(props.card.state);
  const [lane, setLane] = createSignal(props.card.lane ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const dirty = () =>
    title().trim() !== props.card.title ||
    body() !== props.card.body ||
    state() !== props.card.state ||
    lane().trim() !== (props.card.lane ?? "");
  const open = () => openQuestions([props.card]);
  const live = () => props.runs.find((r) => isLive(r.state));
  const answered = () => (props.card.questions ?? []).filter((q) => q.answer != null);

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

  return (
    <Sheet
      title={`Card #${props.card.id}`}
      wide
      onClose={props.onClose}
      foot={
        <>
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
      <div class="field">
        <label class="field-label" for="card-title">
          Title
        </label>
        <input id="card-title" class="input" value={title()} onInput={(e) => setTitle(e.currentTarget.value)} />
      </div>
      <div class="card-meta">
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
        </div>
      </div>
      <p class="card-where">
        On the board in <b>{columnTitle[place(props.card, props.yard).column]}</b>
        <Show when={place(props.card, props.yard).column === "needs"}>, {place(props.card, props.yard).label.toLowerCase()}</Show>.
      </p>
      <Show when={props.card.lane}>
        <div class="card-lane-track">
          <MiniTrack card={props.card} yard={props.yard} now={props.now} />
        </div>
      </Show>
      <div class="field">
        <label class="field-label" for="card-body">
          Spec
        </label>
        <textarea
          id="card-body"
          class="input textarea input-mono"
          rows="8"
          placeholder="Goal, acceptance criteria, out of scope, how to verify"
          value={body()}
          onInput={(e) => setBody(e.currentTarget.value)}
        />
      </div>
      <Show when={open().length > 0}>
        <h3 class="sheet-sub">Open questions</h3>
        <For each={open()}>
          {(q) => (
            <div class="sheet-question">
              <p class="inbox-q">{q.text}</p>
              <QuestionBlock q={q} selected={false} busy={busy()} onAnswer={(a) => void answer(q.id, a)} />
            </div>
          )}
        </For>
      </Show>
      <Show when={props.runs.length > 0}>
        <h3 class="sheet-sub">Runs</h3>
        <ul class="card-runs">
          <For each={props.runs}>
            {(r) => (
              <li>
                <button type="button" class="card-run" onClick={() => props.onOpenRun(r.id)}>
                  <span class="chip" data-state={runStateView[r.state].chip}>
                    <Icon name={runStateView[r.state].icon} small />
                    {runStateView[r.state].label}
                  </span>
                  <span class="card-run-summary">{r.handoff?.summary ?? r.error?.split("\n")[0] ?? r.branch}</span>
                  <span class="figure">{clock(elapsed(r, props.now))}</span>
                  <span class="figure">{money(r.cost_usd)}</span>
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={answered().length > 0}>
        <h3 class="sheet-sub">Decisions</h3>
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
      </Show>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}

export function NewCardSheet(props: { repo: string; onDone: (id: number) => void; onClose: () => void }) {
  const [title, setTitle] = createSignal("");
  const [body, setBody] = createSignal("");
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
          <label class="field-label" for="new-card-body">
            Spec
          </label>
          <textarea
            id="new-card-body"
            class="input textarea input-mono"
            rows="6"
            placeholder="Optional: goal, acceptance criteria, how to verify"
            value={body()}
            onInput={(e) => setBody(e.currentTarget.value)}
          />
          <span class="field-help">Saved as a markdown file in .pitlane/cards, versioned with the code.</span>
        </div>
      </form>
      <Show when={error()}>
        <ErrorLine>{error()}</ErrorLine>
      </Show>
    </Sheet>
  );
}
