// Inbox: every open question, answered with one key. Each answer becomes a decision.
import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { api, type Approval, type Card, type RunMeta } from "./api";
import { ApprovalBlock } from "./approvals";
import { Dictate } from "./dictate";
import { openQuestions, type OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { Spinner, toast } from "./ui";

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
              <small>Recommended</small>
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
          Another answer
        </label>
        <input
          ref={field}
          id={`other-${props.q.card.id}-${props.q.id}`}
          class="input"
          placeholder={props.q.options.length ? "Or answer in your own words" : "Your answer"}
          autocomplete="off"
          value={other()}
          disabled={props.busy}
          onInput={(e) => write(e.currentTarget.value)}
        />
        <Dictate field={() => field} value={other} onChange={write} />
        <button type="submit" class="btn" disabled={props.busy || !other().trim()}>
          Answer
        </button>
      </form>
    </div>
  );
}

/** An answered question, with Change to choose again: the same decision takes the new answer. */
export function DecisionItem(props: { repo: string; card: Card; q: NonNullable<Card["questions"]>[number]; onChanged: () => void }) {
  const [editing, setEditing] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const asked = (): OpenQuestion => ({
    card: props.card,
    id: props.q.id,
    text: props.q.text,
    options: props.q.options ?? [],
    recommended: props.q.recommended,
    index: 0,
    of: 0,
  });
  async function choose(text: string) {
    setBusy(true);
    try {
      const r = await api.cardReanswer(props.repo, props.card.id, props.q.id, text);
      toast({ tone: "done", title: `${r.decision} changed`, body: text });
      setEditing(false);
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: "The decision did not change", body: msg(e) }, 0);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div class="decision" data-editing={editing() ? "" : undefined}>
      <dt>{props.q.text}</dt>
      <dd>
        {props.q.answer}
        <Show when={props.q.decision}>
          <span class="tag">{props.q.decision}</span>
        </Show>
        <button
          type="button"
          class="btn btn-ghost decision-change"
          aria-expanded={editing()}
          disabled={busy()}
          onClick={() => setEditing(!editing())}
        >
          {editing() ? "Keep" : "Change"}
        </button>
      </dd>
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
  const [index, setIndex] = createSignal(0);
  const [busy, setBusy] = createSignal(false);
  const items = new Map<string, HTMLElement>();
  const key = (q: OpenQuestion) => `${q.card.id}-${q.id}`;
  const current = () => questions()[Math.min(index(), Math.max(0, questions().length - 1))];
  const isCurrent = (q: OpenQuestion) => {
    const c = current();
    return !!c && key(c) === key(q);
  };

  createEffect(() => {
    const q = current();
    if (q) items.get(key(q))?.scrollIntoView({ block: "nearest" });
  });

  async function answer(q: OpenQuestion, text: string) {
    if (busy()) return;
    setBusy(true);
    try {
      const r = await api.cardAnswer(props.repo, q.card.id, q.id, text);
      toast({ tone: "done", title: `Recorded as ${r.decision}`, body: `#${q.card.id} ${q.card.title}: ${text}` });
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: "Not recorded", body: msg(e) });
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!props.active || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (document.querySelector("dialog[open]")) return;
      const q = current();
      if (!q) return;
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setIndex(Math.min(index() + 1, questions().length - 1));
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setIndex(Math.max(index() - 1, 0));
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
    <div class="inbox">
      <header class="screen-head">
        <h1 class="title">Inbox</h1>
        <Show when={questions().length > 0 || props.approvals.length > 0}>
          <p class="summary">
            <Show when={props.approvals.length > 0}>
              <b>
                {props.approvals.length} {props.approvals.length === 1 ? "agent waits" : "agents wait"} for you.
              </b>{" "}
            </Show>
            <Show when={questions().length > 0}>
              {questions().length} {questions().length === 1 ? "question" : "questions"} on {cardsAsking()}{" "}
              {cardsAsking() === 1 ? "card" : "cards"}.
            </Show>
          </p>
        </Show>
        <span class="spacer" />
        <Show when={questions().length > 0}>
          <p class="keys">
            <span class="kbd">1</span> to <span class="kbd">9</span> answers, <span class="kbd">↵</span> takes the
            recommended, <span class="kbd">J</span> <span class="kbd">K</span> move
          </p>
        </Show>
      </header>

      <For each={props.approvals}>
        {(w) => (
          <section class="inbox-card" aria-label={`Card ${w.run.card}: ${w.run.card_title}`}>
            <header class="inbox-card-head">
              <button type="button" class="card-link" onClick={() => props.onOpenRun(w.run.id)}>
                <span class="card-id">#{w.run.card}</span>
                {w.run.card_title}
              </button>
              <span class="caption">Its agent waits, mid-run</span>
            </header>
            <ApprovalBlock run={w.run.id} approval={w.approval} />
          </section>
        )}
      </For>

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> Reading the cards</div>}>
        <Show when={questions().length > 0} fallback={<Show when={props.approvals.length === 0}><InboxEmpty /></Show>}>
          <div class="inbox-list">
            <For each={byCard()}>
              {(g) => (
                <section class="inbox-card" aria-label={`Card ${g.card.id}: ${g.card.title}`}>
                  <header class="inbox-card-head">
                    <button type="button" class="card-link" onClick={() => props.onOpenCard(g.card.id)}>
                      <span class="card-id">#{g.card.id}</span>
                      {g.card.title}
                    </button>
                    <span class="caption">
                      {g.qs.length} open of {g.total}
                    </span>
                  </header>
                  <Show when={g.decided.length > 0}>
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
                          onFocusIn={() => setIndex(questions().findIndex((x) => key(x) === key(q)))}
                          onClick={() => setIndex(questions().findIndex((x) => key(x) === key(q)))}
                        >
                          <p class="inbox-q">{q.text}</p>
                          <QuestionBlock q={q} selected={isCurrent(q)} busy={busy()} onAnswer={(a) => void answer(q, a)} />
                        </li>
                      )}
                    </For>
                  </ol>
                </section>
              )}
            </For>
          </div>
        </Show>
      </Show>
    </div>
  );
}

function InboxEmpty() {
  return (
    <div class="empty">
      <div class="empty-title">Nothing to answer</div>
      <p class="empty-body">
        Agents ask here before they start, all their questions at once. Connect Claude Code to this repository’s cards:
      </p>
      <div class="install">
        <code class="selectable">{MCP_INSTALL}</code>
        <button
          type="button"
          class="btn"
          onClick={() => {
            void navigator.clipboard.writeText(MCP_INSTALL);
            toast({ tone: "done", title: "Copied", body: "Run it in a terminal, then ask Claude Code to plan a card." });
          }}
        >
          <Icon name="copy" small />
          Copy
        </button>
      </div>
    </div>
  );
}
