// Inbox: what waits on you (a question, a tool an agent wants), one at a time, answered with one
// key. Each answer becomes a decision. With nothing waiting: what works, and what was just decided.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, isLive, type Approval, type Card, type RunMeta, type Turn } from "./api";
import { ApprovalBlock } from "./approvals";
import { Dictate } from "./dictate";
import { Prose, inline } from "./prose";
import { openQuestions, type OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { Spinner, toast, ownsKey } from "./ui";
import { age, clock, elapsed, msg } from "./format";
import { kindLabel } from "./agents";


export const MCP_INSTALL = "claude mcp add pitlane -- pit mcp";

// What is typed into an answer, by card and question, so nothing redraws it away.
const drafts = new Map<string, string>();

/** Minutes and seconds since `iso`, ticking: how long an agent has waited on you. */
function Waited(props: { since: string }) {
  const [now, setNow] = createSignal(Date.now());
  const tick = window.setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(tick));
  const text = () => {
    const s = Math.max(0, Math.floor((now() - Date.parse(props.since)) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  return <span class="mono" title={t("How long its agent has waited")}>{text()}</span>;
}

/** Over a question its agent asked mid-run and waits on now (Presence: Here). */
export function LiveMark(props: { q: OpenQuestion }) {
  return (
    <Show when={props.q.live}>
      <p class="q-live">
        <Icon name="needs" small />
        <span class="q-live-mark">{t("Needs you")}</span>
        <span>{t("its agent waits mid-run")}</span>
        <Show when={props.q.askedAt}>{(at) => <Waited since={at()} />}</Show>
      </p>
    </Show>
  );
}

/** One question with its options; the recommended one is marked. Used by the Inbox and the card. */
export function QuestionBlock(props: {
  q: OpenQuestion;
  selected: boolean;
  busy: boolean;
  onAnswer: (answer: string) => void;
}) {
  const draft = `${props.q.card.id}-${props.q.id}`;
  const [other, setOther] = createSignal(drafts.get(draft) ?? "");
  const write = (v: string) => {
    setOther(v);
    if (v) drafts.set(draft, v);
    else drafts.delete(draft);
  };
  let field: HTMLInputElement | undefined;
  return (
    <div class="options" role="group" aria-label={props.q.text}>
      <For each={props.q.options}>
        {(o, i) => (
          <button
            type="button"
            class="option"
            data-recommended={props.q.recommended === i() ? "" : undefined}
            disabled={props.busy}
            onClick={() => props.onAnswer(o)}
          >
            <span class="kbd">{i() + 1}</span>
            <span>{o}</span>
            <Show when={props.q.recommended === i()}>
              <small>{t("Recommended")}</small>
            </Show>
          </button>
        )}
      </For>
      <form
        class="other"
        onSubmit={(e) => {
          e.preventDefault();
          if (other().trim()) {
            props.onAnswer(other().trim());
            drafts.delete(draft);
          }
        }}
      >
        <label class="visually-hidden" for={`other-${props.q.card.id}-${props.q.id}`}>
          {t("Another answer")}
        </label>
        <input
          ref={field}
          id={`other-${props.q.card.id}-${props.q.id}`}
          class="input"
          placeholder={props.q.options.length ? t("Or answer in your own words") : t("Your answer")}
          autocomplete="off"
          value={other()}
          disabled={props.busy}
          onInput={(e) => write(e.currentTarget.value)}
        />
        <Dictate field={() => field} value={other} onChange={write} />
        <button type="submit" class="btn" disabled={props.busy || !other().trim()}>
          {t("Answer")}
        </button>
      </form>
    </div>
  );
}

/**
 * What a question is about, under it: the asker's context, and the card itself (spec and what
 * the analysis found) a click away, so answering never means leaving for the card.
 */
export function QuestionAbout(props: { q: OpenQuestion; card?: boolean }) {
  const notes = () => (props.q.card.notes ?? []).filter((n) => n.by === "claude");
  return (
    <>
      <Show when={props.q.context}>
        <p class="q-context">{inline(props.q.context!)}</p>
      </Show>
      <Show when={props.card !== false && (props.q.card.body.trim() || notes().length)}>
        <details class="q-card">
          <summary>
            {t("Read card #{id}", { id: props.q.card.id })}
            <Show when={(props.q.card.images ?? []).length}> · {plural((props.q.card.images ?? []).length, "{n} image", "{n} images")}</Show>
          </summary>
          <div class="q-card-body">
            <Show when={props.q.card.body.trim()}>
              <Prose text={props.q.card.body} />
            </Show>
            <Show when={notes().length}>
              <p class="cs-label">{t("What Claude found")}</p>
              <For each={notes()}>{(n) => <Prose text={n.text} />}</For>
            </Show>
          </div>
        </details>
      </Show>
    </>
  );
}

// Conversations waiting on Claude, by repository, card and question, with what was just sent
// (shown until the card is read again with it). Module-level: a redraw or another view of the
// same question sees the same wait.
const [waiting, setWaiting] = createSignal<ReadonlyMap<string, string | null>>(new Map());
const stopped = new Set<string>();
// Decisions whose conversation is open: a re-read of the cards redraws them, open as they were.
const [opened, setOpened] = createSignal<ReadonlySet<string>>(new Set());
const talkKey = (repo: string, card: number, q: number) => `${repo}|${card}-${q}`;

/** Whether Claude is answering on this question, or on any question of the card. */
export function talking(repo: string, card: number, q?: number): boolean {
  const w = waiting();
  if (q != null) return w.has(talkKey(repo, card, q));
  const prefix = `${repo}|${card}-`;
  for (const k of w.keys()) if (k.startsWith(prefix)) return true;
  return false;
}

async function talk(repo: string, card: Card, q: number, message: string | null, onChanged: () => void) {
  const k = talkKey(repo, card.id, q);
  if (waiting().has(k)) return;
  setWaiting((w) => new Map(w).set(k, message));
  stopped.delete(k);
  // What you said is on the card before Claude answers: read it back so it shows as sent.
  const early = setTimeout(onChanged, 400);
  try {
    await api.cardTalk(repo, card.id, q, message);
  } catch (e) {
    if (!stopped.has(k)) toast({ tone: "fail", title: t("Claude did not answer"), body: msg(e) }, 0);
  } finally {
    clearTimeout(early);
    stopped.delete(k);
    setWaiting((w) => {
      const n = new Map(w);
      n.delete(k);
      return n;
    });
    onChanged();
  }
}

const turnBy: Record<string, { name: string; icon: "spark" | "person" }> = {
  claude: { name: "Claude", icon: "spark" },
  you: { name: "You", icon: "person" },
};

/**
 * Talking a question over with Claude: ask why, propose another way ("and if we did it like
 * this?"), and Claude answers with the code in view and the options as they stand after it.
 * Decided questions too: Claude's new recommendation can replace the decision with one click.
 */
export function QuestionTalk(props: {
  repo: string;
  card: Card;
  q: number;
  talk: Turn[];
  /** The decision, when the question is answered: a different recommendation offers to replace it. */
  answer?: string;
  onUse?: (option: string) => void;
  onChanged: () => void;
  /** Open from the start (the composer shows even before the first message). */
  open?: boolean;
}) {
  const draft = `talk-${props.repo}-${props.card.id}-${props.q}`;
  const [text, setText] = createSignal(drafts.get(draft) ?? "");
  const [shown, setShown] = createSignal(!!props.open);
  const write = (v: string) => {
    setText(v);
    if (v) drafts.set(draft, v);
    else drafts.delete(draft);
  };
  let field: HTMLInputElement | undefined;
  const k = () => talkKey(props.repo, props.card.id, props.q);
  const busy = () => waiting().has(k());
  // What was sent and is not on the card yet.
  const sent = () => {
    const m = waiting().get(k());
    if (!m) return null;
    const last = props.talk[props.talk.length - 1];
    return last?.by === "you" && last.text === m ? null : m;
  };
  const unanswered = () => !busy() && props.talk[props.talk.length - 1]?.by === "you";
  const lastClaude = () => [...props.talk].reverse().find((x) => x.by === "claude");
  /** On a decided question: what Claude recommends now, when it is not the decision. */
  const suggestion = () => {
    const c = lastClaude();
    if (props.answer == null || !c?.options?.length || c.recommended == null) return null;
    const o = c.options[c.recommended];
    return o && o !== props.answer ? o : null;
  };
  const open = () => shown() || props.talk.length > 0 || busy();

  function send(e?: Event) {
    e?.preventDefault();
    const m = text().trim();
    if (!m || busy()) return;
    write("");
    void talk(props.repo, props.card, props.q, m, props.onChanged);
  }

  return (
    <Show
      when={open()}
      fallback={
        <button
          type="button"
          class="btn btn-ghost talk-start"
          onClick={() => {
            setShown(true);
            queueMicrotask(() => field?.focus());
          }}
        >
          <Icon name="spark" small />
          {t("Talk it over with Claude")}
        </button>
      }
    >
      <div class="talk">
        <Show when={props.talk.length > 0 || sent()}>
          <ol class="talk-turns">
            <For each={props.talk}>
              {(x) => (
                <li class="talk-turn" data-by={x.by}>
                  <span class="talk-by">
                    <Icon name={turnBy[x.by]?.icon ?? "person"} small />
                    {turnBy[x.by] ? t(turnBy[x.by]!.name) : x.by}
                  </span>
                  <Prose text={x.text} />
                </li>
              )}
            </For>
            <Show when={sent()}>
              {(m) => (
                <li class="talk-turn" data-by="you">
                  <span class="talk-by">
                    <Icon name="person" small />
                    {t("You")}
                  </span>
                  <Prose text={m()} />
                </li>
              )}
            </Show>
          </ol>
        </Show>
        <Show when={busy()}>
          <div class="talk-wait" role="status">
            <Spinner />
            <span>{t("Claude is reading the code to answer")}</span>
            <button
              type="button"
              class="btn btn-ghost"
              onClick={() => {
                stopped.add(k());
                void api.cardTalkStop(props.repo, props.card.id);
              }}
            >
              <Icon name="stop" small />
              {t("Stop")}
            </button>
          </div>
        </Show>
        <Show when={unanswered()}>
          <div class="talk-wait">
            <span>{t("Claude has not answered this yet.")}</span>
            <button type="button" class="btn btn-ghost" onClick={() => void talk(props.repo, props.card, props.q, null, props.onChanged)}>
              <Icon name="refresh" small />
              {t("Ask again")}
            </button>
          </div>
        </Show>
        <Show when={suggestion()}>
          {(o) => (
            <div class="talk-suggest">
              <span>
                {t("Claude now recommends")} <b>{o()}</b>
              </span>
              <button type="button" class="btn" disabled={busy()} onClick={() => props.onUse?.(o())}>
                {t("Decide this instead")}
              </button>
            </div>
          )}
        </Show>
        <form class="other talk-compose" onSubmit={send}>
          <label class="visually-hidden" for={`talk-${props.card.id}-${props.q}`}>
            {t("Say something to Claude about this question")}
          </label>
          <input
            ref={field}
            id={`talk-${props.card.id}-${props.q}`}
            class="input"
            placeholder={props.talk.length ? t("Reply, or propose another way") : t("And if we did it another way? Ask or propose")}
            autocomplete="off"
            value={text()}
            disabled={busy()}
            onInput={(e) => write(e.currentTarget.value)}
          />
          <Dictate field={() => field} value={text} onChange={write} />
          <button type="submit" class="btn" disabled={busy() || !text().trim()}>
            {t("Send")}
          </button>
        </form>
      </div>
    </Show>
  );
}

/**
 * An answered question. Change chooses again (the same decision takes the new answer); Talk
 * reopens it with Claude ("and if we did it like this?"), whose new recommendation can replace it.
 */
export function DecisionItem(props: { repo: string; card: Card; q: NonNullable<Card["questions"]>[number]; onChanged: () => void }) {
  const [editing, setEditing] = createSignal(false);
  const tk = () => talkKey(props.repo, props.card.id, props.q.id);
  const talkOpen = () => opened().has(tk());
  const setTalkOpen = (v: boolean) =>
    setOpened((o) => {
      const n = new Set(o);
      if (v) n.add(tk());
      else n.delete(tk());
      return n;
    });
  const [busy, setBusy] = createSignal(false);
  const asked = (): OpenQuestion => ({
    card: props.card,
    id: props.q.id,
    text: props.q.text,
    options: props.q.options ?? [],
    recommended: props.q.recommended,
    context: props.q.context,
    talk: props.q.talk ?? [],
    live: false,
    index: 0,
    of: 0,
  });
  const turns = () => (props.q.talk ?? []).length;
  const showTalk = () => talkOpen() || talking(props.repo, props.card.id, props.q.id);
  async function choose(text: string) {
    setBusy(true);
    try {
      const r = await api.cardReanswer(props.repo, props.card.id, props.q.id, text);
      toast({ tone: "done", title: t("{decision} changed", { decision: r.decision }), body: text });
      setEditing(false);
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: t("The decision did not change"), body: msg(e) }, 0);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div class="decision" data-editing={editing() || showTalk() ? "" : undefined}>
      <dt class="decision-q">{inline(props.q.text)}</dt>
      <dd class="decision-a">
        <Icon name="done" small />
        <span>{props.q.answer}</span>
      </dd>
      <Show when={props.q.auto}>
        <dd class="decision-auto">{t("Taken for you while you were away")}</dd>
      </Show>
      <dd class="decision-foot">
        <Show when={props.q.decision}>
          <span class="tag">{props.q.decision}</span>
        </Show>
        <span class="decision-actions">
          <button
            type="button"
            class="btn decision-btn"
            aria-expanded={showTalk()}
            disabled={busy()}
            title={t("Ask Claude about it, or propose another way")}
            onClick={() => setTalkOpen(!showTalk())}
          >
            <Icon name="spark" small />
            {turns() ? t("Talk ({n})", { n: turns() }) : t("Talk")}
          </button>
          <button
            type="button"
            class="btn decision-btn"
            aria-expanded={editing()}
            disabled={busy()}
            title={editing() ? undefined : t("Choose another answer")}
            onClick={() => setEditing(!editing())}
          >
            <Icon name={editing() ? "close" : "edit"} small />
            {editing() ? t("Keep") : t("Change")}
          </button>
        </span>
      </dd>
      <Show when={showTalk()}>
        <div class="decision-more">
          <Show when={props.q.context}>
            <p class="q-context">{inline(props.q.context!)}</p>
          </Show>
          <QuestionTalk
            repo={props.repo}
            card={props.card}
            q={props.q.id}
            talk={props.q.talk ?? []}
            answer={props.q.answer}
            onUse={(o) => void choose(o)}
            onChanged={props.onChanged}
            open
          />
        </div>
      </Show>
      <Show when={editing()}>
        <QuestionBlock q={asked()} selected busy={busy()} onAnswer={(a) => void choose(a)} />
      </Show>
    </div>
  );
}

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
    <div class="inbox" ref={root}>
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
