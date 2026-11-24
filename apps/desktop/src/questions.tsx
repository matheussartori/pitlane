// A question a card asks, wherever it shows (the Inbox, the card's sheet): the question and its
// choices, talking it over with Claude, and the decision it became.
import { For, Show, createSignal, onCleanup } from "solid-js";
import { api, type Card, type Turn } from "./api";
import { Dictate } from "./dictate";
import { Prose, inline } from "./prose";
import type { OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { plural, t } from "./i18n";
import { Spinner, toast } from "./ui";
import { msg } from "./format";
import { repo as currentRepo } from "./store/repo";

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
  // By repository too: card numbers start at 1 in each one.
  const draft = `${currentRepo()}|${props.q.card.id}-${props.q.id}`;
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
