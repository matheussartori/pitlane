// Inbox: every open question, answered with one key. Each answer becomes a decision.
import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { api, type Card } from "./api";
import { openQuestions, type OpenQuestion } from "./cards";
import { Icon } from "./icons";
import { Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

export const MCP_INSTALL = "claude mcp add pitlane -- pit mcp";

/** One question with its options; the recommended one is marked. Used by the Inbox and the card. */
export function QuestionBlock(props: {
  q: OpenQuestion;
  selected: boolean;
  busy: boolean;
  onAnswer: (answer: string) => void;
}) {
  const [other, setOther] = createSignal("");
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
          if (other().trim()) props.onAnswer(other().trim());
        }}
      >
        <label class="visually-hidden" for={`other-${props.q.card.id}-${props.q.id}`}>
          Another answer
        </label>
        <input
          id={`other-${props.q.card.id}-${props.q.id}`}
          class="input"
          placeholder={props.q.options.length ? "Or answer in your own words" : "Your answer"}
          autocomplete="off"
          value={other()}
          disabled={props.busy}
          onInput={(e) => setOther(e.currentTarget.value)}
        />
        <button type="submit" class="btn" disabled={props.busy || !other().trim()}>
          Answer
        </button>
      </form>
    </div>
  );
}

export function InboxView(props: {
  repo: string;
  cards: Card[] | null;
  active: boolean;
  onChanged: () => void;
  onOpenCard: (id: number) => void;
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
    const groups: { card: Card; qs: OpenQuestion[]; total: number; decided: NonNullable<Card["questions"]> }[] = [];
    for (const q of questions()) {
      let g = groups.find((x) => x.card.id === q.card.id);
      if (!g) {
        const all = q.card.questions ?? [];
        g = { card: q.card, qs: [], total: all.length, decided: all.filter((x) => x.answer != null) };
        groups.push(g);
      }
      g.qs.push(q);
    }
    return groups;
  };

  return (
    <div class="inbox">
      <header class="screen-head">
        <h1 class="title">Inbox</h1>
        <Show when={questions().length > 0}>
          <p class="summary">
            {questions().length} {questions().length === 1 ? "question" : "questions"} on {cardsAsking()}{" "}
            {cardsAsking() === 1 ? "card" : "cards"}.
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

      <Show when={props.cards} fallback={<div class="loading-line"><Spinner /> Reading the cards</div>}>
        <Show when={questions().length > 0} fallback={<InboxEmpty />}>
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
                        {(d) => (
                          <div>
                            <dt>{d.text}</dt>
                            <dd>
                              {d.answer}
                              <Show when={d.decision}>
                                <span class="tag">{d.decision}</span>
                              </Show>
                            </dd>
                          </div>
                        )}
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
