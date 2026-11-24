// Inbox: what waits on you (a question, a tool an agent wants), one at a time, answered with one
// key. Each answer becomes a decision. With nothing waiting: what works, and what was just decided.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, isLive, type Approval, type Card, type RunMeta } from "./api";
import { ApprovalBlock } from "./approvals";
import { inline } from "./prose";
import { DecisionItem, LiveMark, QuestionAbout, QuestionBlock, QuestionTalk, talking } from "./questions";
import { openQuestions, type OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { Spinner, toast, ownsKey } from "./ui";
import { age, clock, elapsed, msg } from "./format";
import { kindLabel } from "./store/agents";


const MCP_INSTALL = "claude mcp add pitlane -- pit mcp";

/** One thing that waits on you: a tool an agent wants to use, or a question. One shows at a time. */
type Ask =
  | { kind: "approval"; key: string; w: { run: RunMeta; approval: Approval } }
  | { kind: "question"; key: string; q: OpenQuestion };

export function InboxView(props: {
  repo: string;
  cards: Card[] | null;
  active: boolean;
  onChanged: () => void;
  onOpenCard: (id: number) => void;
  approvals: { run: RunMeta; approval: Approval }[];
  onOpenRun: (id: string) => void;
  /** Every run of the repository, for what works while nothing waits on you. */
  runs: RunMeta[];
  now: number;
}) {
  const questions = () => openQuestions(props.cards ?? []);
  const [busy, setBusy] = createSignal(false);
  const key = (q: OpenQuestion) => `${q.card.id}-${q.id}`;

  // The same thing keeps the same object while it is unchanged, so the view of it (a draft, the
  // focus, an open talk) is not redrawn when the cards are read again.
  let kept = new Map<string, Ask>();
  const asks = createMemo<Ask[]>(() => {
    const next = new Map<string, Ask>();
    const list: Ask[] = [];
    for (const w of props.approvals) {
      const k = `a-${w.approval.id}`;
      const old = kept.get(k);
      const a: Ask = old && old.kind === "approval" && old.w.approval.id === w.approval.id ? old : { kind: "approval", key: k, w };
      next.set(k, a);
      list.push(a);
    }
    for (const q of questions()) {
      const k = key(q);
      const old = kept.get(k);
      const a: Ask = old && old.kind === "question" && old.q === q ? old : { kind: "question", key: k, q };
      next.set(k, a);
      list.push(a);
    }
    kept = next;
    return list;
  });

  // The selection follows the item, not its position: something arriving before it never moves
  // it; answering moves on to what was next.
  const [picked, setPicked] = createSignal<string | null>(null);
  let lastAt = 0;
  // Until you move, the first thing shows (what loads a moment later, a permission, goes first).
  let moved = false;
  // The browser preview can open on the n-th item (`?ask=2`), to look at it without keys.
  const askParam = import.meta.env.DEV ? Number(new URLSearchParams(location.search).get("ask") ?? 0) : 0;
  const current = createMemo(() => {
    const list = asks();
    const want = picked(); // read first, so a pick always reaches this
    if (list.length === 0) return undefined;
    if (!moved) return list[(lastAt = Math.min(Math.max(askParam - 1, 0), list.length - 1))];
    const i = list.findIndex((a) => a.key === want);
    if (i >= 0) return list[(lastAt = i)];
    return list[Math.min(lastAt, list.length - 1)];
  });
  const at = () => {
    const c = current();
    return c ? asks().indexOf(c) : 0;
  };
  const pick = (i: number) => {
    const a = asks()[i];
    if (!a) return;
    moved = true;
    setPicked(a.key);
  };
  createEffect(() => {
    const c = current();
    if (c && picked() !== c.key) setPicked(c.key);
  });
  let root!: HTMLDivElement;
  // A new item in view starts at its top.
  createEffect(
    on(
      () => current()?.key,
      () => root?.closest<HTMLElement>(".main")?.scrollTo({ top: 0 }),
      { defer: true },
    ),
  );

  async function answer(q: OpenQuestion, text: string) {
    if (busy()) return;
    setBusy(true);
    try {
      const r = await api.cardAnswer(props.repo, q.card.id, q.id, text);
      toast({ tone: "done", title: t("Recorded as {decision}", { decision: r.decision }), body: `#${q.card.id} ${q.card.title}: ${text}` });
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: t("Not recorded"), body: msg(e) });
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!props.active || e.metaKey || e.ctrlKey || e.altKey) return;
      if (ownsKey(e)) return;
      if (document.querySelector("dialog[open]")) return;
      const a = current();
      if (!a) return;
      if (e.key === "ArrowDown" || e.key === "ArrowRight" || e.key === "j") {
        e.preventDefault();
        pick(Math.min(at() + 1, asks().length - 1));
        return;
      }
      if (e.key === "ArrowUp" || e.key === "ArrowLeft" || e.key === "k") {
        e.preventDefault();
        pick(Math.max(at() - 1, 0));
        return;
      }
      // A permission takes its own keys (1 to 4) in its block.
      if (a.kind !== "question") return;
      const q = a.q;
      if (/^[1-9]$|^Enter$/.test(e.key) && talking(props.repo, q.card.id, q.id)) return;
      if (/^[1-9]$/.test(e.key)) {
        const o = q.options[Number(e.key) - 1];
        if (o) {
          e.preventDefault();
          void answer(q, o);
        }
      } else if (e.key === "Enter" && q.recommended != null && q.options[q.recommended]) {
        e.preventDefault();
        void answer(q, q.options[q.recommended]!);
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  /** Cards whose questions are all answered and that have not started: still time to go back. */
  const settled = () =>
    (props.cards ?? []).filter(
      (c) =>
        (c.state === "inbox" || c.state === "ready") &&
        !c.lane &&
        (c.questions ?? []).length > 0 &&
        (c.questions ?? []).every((q) => q.answer != null),
    );
  const cardsAsking = () => new Set(questions().map((q) => q.card.id)).size;
  /** Agents stopped on you now: a tool to allow, or a question asked mid-run. */
  const agentsWaiting = () => new Set([...props.approvals.map((w) => w.run.id), ...questions().filter((q) => q.live).map((q) => q.run!)]).size;
  const nextAsk = () => asks()[at() + 1];

  return (
    <div
      class="inbox"
      ref={root}
      // Typing in an item keeps it: a permission arriving meanwhile waits its turn instead of
      // taking the field (and your next keys) away.
      onInput={() => (moved = true)}
    >
      <header class="screen-head">
        <h1 class="title">{t("Inbox")}</h1>
        <Show when={asks().length > 0}>
          <p class="summary">
            <Show when={agentsWaiting() > 0}>
              <b>{plural(agentsWaiting(), "{n} agent waits for you.", "{n} agents wait for you.")}</b>{" "}
            </Show>
            <Show when={questions().length > 0}>
              {t("{questions} on {cards}.", {
                questions: plural(questions().length, "{n} question", "{n} questions"),
                cards: plural(cardsAsking(), "{n} card", "{n} cards"),
              })}
            </Show>
          </p>
        </Show>
        <span class="spacer" />
        <Show when={asks().length > 1}>
          <div class="ask-stepper" role="group" aria-label={t("Move between them")}>
            <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Previous")} data-tip={t("Previous · K")} disabled={at() === 0} onClick={() => pick(at() - 1)}>
              <Icon name="chev" small class="chev-prev" />
            </button>
            <span class="ask-count">{t("{n} of {total}", { n: at() + 1, total: asks().length })}</span>
            <button type="button" class="btn btn-ghost btn-icon" aria-label={t("Next")} data-tip={t("Next · J")} disabled={at() >= asks().length - 1} onClick={() => pick(at() + 1)}>
              <Icon name="chev" small class="chev-next" />
            </button>
          </div>
        </Show>
      </header>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> {t("Reading the cards")}</div>}>
        <Show
          when={current()}
          keyed
          fallback={
            <InboxClear
              repo={props.repo}
              cards={props.cards ?? []}
              runs={props.runs}
              now={props.now}
              settled={settled()}
              onChanged={props.onChanged}
              onOpenCard={props.onOpenCard}
              onOpenRun={props.onOpenRun}
            />
          }
        >
          {(a) => (
            <>
              <Show
                when={a.kind === "question" ? a.q : null}
                keyed
                fallback={
                  <Show when={a.kind === "approval" ? a.w : null} keyed>
                    {(w) => (
                      <article class="ask" aria-label={t("Card {id}: {title}", { id: w.run.card, title: w.run.card_title })}>
                        <div class="ask-from">
                          <button type="button" class="card-link" onClick={() => props.onOpenRun(w.run.id)} title={t("Open its run")}>
                            <span class="card-id">#{w.run.card}</span>
                            <span class="card-link-title">{w.run.card_title}</span>
                          </button>
                          <span class="caption">{t("Its agent waits, mid-run")}</span>
                        </div>
                        <ApprovalBlock run={w.run.id} approval={w.approval} keys={props.active} />
                      </article>
                    )}
                  </Show>
                }
              >
                {(q) => {
                  const decided = () => (q.card.questions ?? []).filter((x) => x.answer != null);
                  return (
                    <article class="ask" aria-label={t("Card {id}: {title}", { id: q.card.id, title: q.card.title })}>
                      <div class="ask-from">
                        <button type="button" class="card-link" onClick={() => props.onOpenCard(q.card.id)} title={t("Open card #{id}", { id: q.card.id })}>
                          <span class="card-id">#{q.card.id}</span>
                          <span class="card-link-title">{q.card.title}</span>
                        </button>
                        <Show when={q.of > 1}>
                          <span class="caption">{t("question {n} of {total} on this card", { n: q.index, total: q.of })}</span>
                        </Show>
                      </div>
                      <LiveMark q={q} />
                      <h2 class="ask-q">{inline(q.text)}</h2>
                      <QuestionAbout q={q} />
                      <QuestionBlock q={q} selected busy={busy() || talking(props.repo, q.card.id, q.id)} onAnswer={(x) => void answer(q, x)} />
                      <QuestionTalk repo={props.repo} card={q.card} q={q.id} talk={q.talk} onChanged={props.onChanged} />
                      <Show when={decided().length > 0}>
                        <details class="ask-decided">
                          <summary>{plural(decided().length, "{n} decision on this card so far", "{n} decisions on this card so far")}</summary>
                          <dl class="decided">
                            <For each={decided()}>{(d) => <DecisionItem repo={props.repo} card={q.card} q={d} onChanged={props.onChanged} />}</For>
                          </dl>
                        </details>
                      </Show>
                    </article>
                  );
                }}
              </Show>
              <div class="ask-foot">
                <Show when={nextAsk()}>
                  {(n) => (
                    <button type="button" class="ask-next" onClick={() => pick(at() + 1)}>
                      <span class="ask-next-label">{t("Up next")}</span>
                      <span class="ask-next-what">
                        {n().kind === "question"
                          ? `#${(n() as Extract<Ask, { kind: "question" }>).q.card.id} · ${(n() as Extract<Ask, { kind: "question" }>).q.text.replace(/`/g, "")}`
                          : `#${(n() as Extract<Ask, { kind: "approval" }>).w.run.card} · ${t("Its agent wants {tool}", { tool: (n() as Extract<Ask, { kind: "approval" }>).w.approval.tool })}`}
                      </span>
                      <span class="kbd">J</span>
                    </button>
                  )}
                </Show>
                <span class="spacer" />
                <p class="keys">
                  <Show when={a.kind === "question"} fallback={<><span class="kbd">1</span> {t("to")} <span class="kbd">4</span> {t("answers")}</>}>
                    <span class="kbd">1</span> {t("to")} <span class="kbd">9</span> {t("answers,")} <span class="kbd">↵</span> {t("takes the recommended")}
                  </Show>
                </p>
              </div>
            </>
          )}
        </Show>
      </Show>
    </div>
  );
}

/** Nothing waits on you: what is happening instead, the decisions just taken (still yours to
 *  change until their card starts), and how to connect Claude Code when it is not yet. */
function InboxClear(props: {
  repo: string;
  cards: Card[];
  runs: RunMeta[];
  now: number;
  settled: Card[];
  onChanged: () => void;
  onOpenCard: (id: number) => void;
  onOpenRun: (id: string) => void;
}) {
  const [mcp] = createResource(() => props.repo, (r) => api.mcpConnected(r).catch(() => true));
  const working = () => props.runs.filter((r) => isLive(r.state)).sort((a, b) => a.started_at - b.started_at);
  const queued = () => props.cards.filter((c) => c.state === "queued").length;
  const settledIds = () => new Set(props.settled.map((c) => c.id));
  /** The last decisions taken, newest first, outside the cards still open to change below. */
  const recent = () =>
    props.cards
      .filter((c) => !settledIds().has(c.id))
      .flatMap((c) => (c.questions ?? []).filter((q) => q.answer != null && q.answered_at).map((q) => ({ c, q })))
      .sort((a, b) => (b.q.answered_at ?? "").localeCompare(a.q.answered_at ?? ""))
      .slice(0, 5);
  return (
    <div class="inbox-clear">
      <div class="clear-head">
        <span class="clear-mark" aria-hidden="true">
          <Icon name="done" />
        </span>
        <div>
          <h2 class="clear-title">{t("Nothing waits on you")}</h2>
          <p class="clear-body">
            {working().length || queued()
              ? t("{working}, {queued}. When an agent needs a decision it shows here, one at a time.", {
                  working: plural(working().length, "{n} agent working", "{n} agents working"),
                  queued: plural(queued(), "{n} card queued", "{n} cards queued"),
                })
              : t("No agent is working. When one needs a decision it shows here, one at a time.")}
          </p>
        </div>
      </div>

      <Show when={mcp() === false}>
        <section class="clear-section">
          <h3 class="inbox-section-title">{t("Connect Claude Code")}</h3>
          <p class="caption">{t("So Claude Code can make cards and ask here from any terminal:")}</p>
          <div class="install">
            <code class="selectable">{MCP_INSTALL}</code>
            <button
              type="button"
              class="btn"
              onClick={() => {
                void navigator.clipboard.writeText(MCP_INSTALL);
                toast({ tone: "done", title: t("Copied"), body: t("Run it in a terminal, then ask Claude Code to plan a card.") });
              }}
            >
              <Icon name="copy" small />
              {t("Copy")}
            </button>
          </div>
        </section>
      </Show>

      <Show when={working().length > 0}>
        <section class="clear-section">
          <h3 class="inbox-section-title">{t("Working now")}</h3>
          <ul class="clear-list">
            <For each={working()}>
              {(r) => (
                <li>
                  <button type="button" class="clear-row" onClick={() => props.onOpenRun(r.id)}>
                    <span class="card-id">#{r.card}</span>
                    <span class="clear-row-title">{r.card_title}</span>
                    <span class="clear-row-meta">{kindLabel(r)}</span>
                    <span class="clear-row-clock">{clock(elapsed(r, props.now))}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>

      <Show when={props.settled.length > 0}>
        <section class="clear-section" aria-label={t("Decided, not started")}>
          <h3 class="inbox-section-title">{t("Decided, not started")}</h3>
          <p class="caption">{t("Until a card starts, any decision can change: choose again, or talk it over with Claude.")}</p>
          <For each={props.settled}>
            {(c) => (
              <details class="clear-settled">
                <summary>
                  <span class="card-id">#{c.id}</span>
                  <span class="clear-row-title">{c.title}</span>
                  <span class="clear-row-meta">{plural((c.questions ?? []).length, "{n} decision", "{n} decisions")}</span>
                </summary>
                <dl class="decided">
                  <For each={c.questions ?? []}>{(d) => <DecisionItem repo={props.repo} card={c} q={d} onChanged={props.onChanged} />}</For>
                </dl>
              </details>
            )}
          </For>
        </section>
      </Show>

      <Show when={recent().length > 0}>
        <section class="clear-section">
          <h3 class="inbox-section-title">{t("Decided lately")}</h3>
          <ul class="clear-list">
            <For each={recent()}>
              {(x) => (
                <li>
                  <button type="button" class="clear-row clear-decision" onClick={() => props.onOpenCard(x.c.id)}>
                    <span class="card-id">#{x.c.id}</span>
                    <span class="clear-decision-text">
                      <span class="clear-decision-q">{x.q.text}</span>
                      <span class="clear-decision-a">
                        <Icon name="done" small />
                        {x.q.answer}
                      </span>
                    </span>
                    <span class="clear-row-meta">{x.q.answered_at ? age(x.q.answered_at, props.now) : ""}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
    </div>
  );
}
