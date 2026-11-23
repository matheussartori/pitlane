// Inbox: every open question, answered with one key. Each answer becomes a decision.
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, type Approval, type Card, type RunMeta, type Turn } from "./api";
import { ApprovalBlock } from "./approvals";
import { Dictate } from "./dictate";
import { Prose, inline } from "./prose";
import { openQuestions, type OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { Spinner, toast, ownsKey } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

export const MCP_INSTALL = "claude mcp add pitlane -- pit mcp";

// What is typed into an answer, by card and question, so nothing redraws it away.
const drafts = new Map<string, string>();

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

type Group = { card: Card; qs: OpenQuestion[]; total: number; decided: NonNullable<Card["questions"]> };
// Same card object, same group: an unchanged re-read keeps the Inbox's DOM.
const grouped = new WeakMap<Card, Group>();

export function InboxView(props: {
  repo: string;
  cards: Card[] | null;
  active: boolean;
  onChanged: () => void;
  onOpenCard: (id: number) => void;
  approvals: { run: RunMeta; approval: Approval }[];
  onOpenRun: (id: string) => void;
}) {
  const questions = () => openQuestions(props.cards ?? []);
  const [busy, setBusy] = createSignal(false);
  const items = new Map<string, HTMLElement>();
  const key = (q: OpenQuestion) => `${q.card.id}-${q.id}`;
  // The selection follows the question, not its position: a question arriving above never moves it onto another one.
  const [picked, setPicked] = createSignal<string | null>(null);
  let lastAt = 0;
  const current = createMemo(() => {
    const qs = questions();
    if (qs.length === 0) return undefined;
    const i = qs.findIndex((q) => key(q) === picked());
    if (i >= 0) return qs[(lastAt = i)];
    return qs[Math.min(lastAt, qs.length - 1)];
  });
  const at = () => {
    const c = current();
    return c ? questions().indexOf(c) : 0;
  };
  const pick = (i: number) => {
    const q = questions()[i];
    if (q) setPicked(key(q));
  };
  const isCurrent = (q: OpenQuestion) => {
    const c = current();
    return !!c && key(c) === key(q);
  };
  createEffect(() => {
    const c = current();
    if (c && picked() !== key(c)) setPicked(key(c));
  });

  let root!: HTMLDivElement;
  let scroller: HTMLElement | null = null;
  /** Scrolls the Inbox's own scroller, never the window, just enough to show the element. */
  function reveal(el: HTMLElement | undefined) {
    if (!el || !scroller) return;
    const box = scroller.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    if (r.top < box.top) scroller.scrollTop += r.top - box.top - 8;
    else if (r.bottom > box.bottom) scroller.scrollTop += Math.min(r.bottom - box.bottom + 8, r.top - box.top - 8);
  }
  // Moving with J/K, or answering, brings the next question into view; questions arriving never scroll.
  createEffect(
    on(
      () => picked(),
      (k, prev) => prev && k && reveal(items.get(k)),
      { defer: true },
    ),
  );

  // Scroll anchoring by hand (WebKit has no overflow-anchor): the block at the top of the view stays exactly where it
  // was when questions arrive or leave above it, so the option under the pointer is the one the person was reading.
  const ANCHORS = ".screen-head, .inbox-card-head, .inbox-item, .inbox-card > .decided, .inbox-settled-head";
  let anchor: { el: Element; top: number } | null = null;
  function remember() {
    anchor = null;
    if (!scroller) return;
    const top = scroller.getBoundingClientRect().top;
    for (const el of root.querySelectorAll(ANCHORS)) {
      const r = el.getBoundingClientRect();
      if (r.bottom > top) {
        anchor = { el, top: r.top };
        break;
      }
    }
  }
  function hold() {
    if (scroller && anchor?.el.isConnected) {
      const moved = anchor.el.getBoundingClientRect().top - anchor.top;
      if (Math.abs(moved) >= 1) scroller.scrollTop += moved;
    }
    remember();
    countAbove();
  }

  // Questions that arrived above the view: a pill says so instead of pulling the person up to them.
  const [above, setAbove] = createSignal<string[]>([]);
  let seen: Set<string> | null = null;
  let fresh: string[] = [];
  createEffect(() => {
    const ks = questions().map(key);
    if (props.cards == null) return;
    if (seen) fresh = [...fresh.filter((k) => ks.includes(k)), ...ks.filter((k) => !seen!.has(k))];
    seen = new Set(ks);
  });
  function countAbove() {
    if (!scroller) return;
    const top = scroller.getBoundingClientRect().top;
    fresh = fresh.filter((k) => {
      const el = items.get(k);
      return !!el?.isConnected && el.getBoundingClientRect().bottom <= top;
    });
    setAbove(fresh);
  }
  function showAbove() {
    const first = fresh[0];
    const i = questions().findIndex((q) => key(q) === first);
    if (i >= 0) pick(i);
    const el = first ? items.get(first) : undefined;
    if (el && scroller) scroller.scrollTo({ top: scroller.scrollTop + el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 8, behavior: "smooth" });
  }

  onMount(() => {
    scroller = root.parentElement?.closest<HTMLElement>(".main") ?? null;
    if (!scroller) return;
    const s = scroller;
    const onScroll = () => {
      remember();
      countAbove();
    };
    s.addEventListener("scroll", onScroll, { passive: true });
    const mo = new MutationObserver(hold);
    mo.observe(root, { childList: true, subtree: true, characterData: true });
    const ro = new ResizeObserver(hold);
    ro.observe(root);
    remember();
    onCleanup(() => {
      s.removeEventListener("scroll", onScroll);
      mo.disconnect();
      ro.disconnect();
    });
  });

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
      const q = current();
      if (!q) return;
      if (/^[1-9]$|^Enter$/.test(e.key) && talking(props.repo, q.card.id, q.id)) return;
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        pick(Math.min(at() + 1, questions().length - 1));
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        pick(Math.max(at() - 1, 0));
      } else if (/^[1-9]$/.test(e.key)) {
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
  /** One head per card, its answered questions as context, then its open ones. */
  const byCard = () => {
    const groups: Group[] = [];
    for (const q of questions()) {
      if (groups.some((x) => x.card === q.card)) continue;
      let g = grouped.get(q.card);
      if (!g) {
        const all = q.card.questions ?? [];
        g = { card: q.card, qs: questions().filter((x) => x.card === q.card), total: all.length, decided: all.filter((x) => x.answer != null) };
        grouped.set(q.card, g);
      }
      groups.push(g);
    }
    return groups;
  };

  return (
    <div class="inbox" ref={root}>
      <div class="inbox-above-slot">
        <Show when={above().length > 0}>
          <button type="button" class="inbox-above" onClick={showAbove}>
            <Icon name="up" />
            {plural(above().length, "{n} new question above", "{n} new questions above")}
          </button>
        </Show>
      </div>
      <header class="screen-head">
        <h1 class="title">{t("Inbox")}</h1>
        <Show when={questions().length > 0 || props.approvals.length > 0}>
          <p class="summary">
            <Show when={props.approvals.length > 0}>
              <b>
                {plural(props.approvals.length, "{n} agent waits for you.", "{n} agents wait for you.")}
              </b>{" "}
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
        <Show when={questions().length > 0}>
          <p class="keys">
            <span class="kbd">1</span> {t("to")} <span class="kbd">9</span> {t("answers,")} <span class="kbd">↵</span> {t("takes the recommended,")}{" "}
            <span class="kbd">J</span> <span class="kbd">K</span> {t("move")}
          </p>
        </Show>
      </header>

      <For each={props.approvals}>
        {(w) => (
          <section class="inbox-card" aria-label={t("Card {id}: {title}", { id: w.run.card, title: w.run.card_title })}>
            <header class="inbox-card-head">
              <button type="button" class="card-link" onClick={() => props.onOpenRun(w.run.id)}>
                <span class="card-id">#{w.run.card}</span>
                {w.run.card_title}
              </button>
              <span class="caption">{t("Its agent waits, mid-run")}</span>
            </header>
            <ApprovalBlock run={w.run.id} approval={w.approval} />
          </section>
        )}
      </For>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> {t("Reading the cards")}</div>}>
        <Show
          when={questions().length > 0}
          fallback={
            <Show when={props.approvals.length === 0 && settled().length === 0}>
              <InboxEmpty />
            </Show>
          }
        >
          <div class="inbox-list">
            <For each={byCard()}>
              {(g) => (
                <section class="inbox-card" aria-label={t("Card {id}: {title}", { id: g.card.id, title: g.card.title })}>
                  <header class="inbox-card-head">
                    <button type="button" class="card-link" onClick={() => props.onOpenCard(g.card.id)} title={t("Open card #{id}", { id: g.card.id })}>
                      <span class="card-id">#{g.card.id}</span>
                      <span class="card-link-title">{g.card.title}</span>
                    </button>
                    <span class="inbox-count" data-open="">
                      <Icon name="needs" small />
                      {t("{open} open of {total}", { open: g.qs.length, total: g.total })}
                    </span>
                  </header>
                  <Show when={g.decided.length > 0}>
                    <p class="inbox-sub">{t("Decided so far")}</p>
                    <dl class="decided">
                      <For each={g.decided}>
                        {(d) => <DecisionItem repo={props.repo} card={g.card} q={d} onChanged={props.onChanged} />}
                      </For>
                    </dl>
                  </Show>
                  <ol class="inbox-questions">
                    <For each={g.qs}>
                      {(q) => (
                        <li
                          ref={(el) => items.set(key(q), el)}
                          class="inbox-item"
                          aria-selected={isCurrent(q)}
                          onFocusIn={() => setPicked(key(q))}
                          onClick={() => setPicked(key(q))}
                        >
                          <p class="inbox-q">{inline(q.text)}</p>
                          <QuestionAbout q={q} />
                          <QuestionBlock
                            q={q}
                            selected={isCurrent(q)}
                            busy={busy() || talking(props.repo, q.card.id, q.id)}
                            onAnswer={(a) => void answer(q, a)}
                          />
                          <QuestionTalk repo={props.repo} card={q.card} q={q.id} talk={q.talk} onChanged={props.onChanged} />
                        </li>
                      )}
                    </For>
                  </ol>
                </section>
              )}
            </For>
          </div>
        </Show>
        <Show when={settled().length > 0}>
          <section class="inbox-settled" aria-label={t("Decided, not started")}>
            <header class="inbox-settled-head">
              <h2 class="inbox-section-title">{t("Decided, not started")}</h2>
              <p class="caption">{t("Until a card starts, any decision can change: choose again, or talk it over with Claude.")}</p>
            </header>
            <For each={settled()}>
              {(c) => (
                <section class="inbox-card" aria-label={t("Card {id}: {title}", { id: c.id, title: c.title })}>
                  <header class="inbox-card-head">
                    <button type="button" class="card-link" onClick={() => props.onOpenCard(c.id)} title={t("Open card #{id}", { id: c.id })}>
                      <span class="card-id">#{c.id}</span>
                      <span class="card-link-title">{c.title}</span>
                    </button>
                    <span class="inbox-count">
                      <Icon name="done" small />
                      {plural((c.questions ?? []).length, "{n} decision", "{n} decisions")}
                    </span>
                  </header>
                  <dl class="decided">
                    <For each={c.questions ?? []}>
                      {(d) => <DecisionItem repo={props.repo} card={c} q={d} onChanged={props.onChanged} />}
                    </For>
                  </dl>
                </section>
              )}
            </For>
          </section>
        </Show>
      </Show>
    </div>
  );
}

function InboxEmpty() {
  return (
    <div class="empty">
      <div class="empty-title">{t("Nothing to answer")}</div>
      <p class="empty-body">
        {t("Agents ask here before they start, all their questions at once. Connect Claude Code to this repository’s cards:")}
      </p>
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
    </div>
  );
}
