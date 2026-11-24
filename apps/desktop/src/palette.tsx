// ⌘K: one field that reaches everything. Cards, lanes, agents at work, screens and the app's
// actions, matched as you type; Enter does the obvious thing, ⌘Enter the second one (a card's
// last run, a terminal in a lane's worktree). Every action shows its own key, so the palette
// also teaches the keys.
import { createMemo, createSignal, For, onMount, Show, untrack } from "solid-js";
import { Icon, type IconName } from "./icons";
import { t } from "./i18n";

export interface Command {
  id: string;
  group: string;
  icon: IconName;
  title: string;
  /** A quiet second line part: a state, a branch, a folder. */
  sub?: string;
  /** The key that does the same from anywhere. */
  keys?: string;
  /** Extra words the match looks at (a branch, a number). */
  words?: string;
  run: () => void;
  /** ⌘Enter. */
  alt?: { label: string; run: () => void };
  /** Shown only once you type (lanes and older cards would crowd the empty list). */
  searchOnly?: boolean;
}

/** Subsequence match with bonuses for starts of words and runs of letters; -1 when it misses. */
export function score(text: string, q: string): number {
  const s = text.toLowerCase();
  if (!q) return 0;
  const at = s.indexOf(q);
  if (at >= 0) return 1000 - at * 2 - (s.length - q.length) * 0.1 + (at === 0 || /[\s/#\-_.(]/.test(s[at - 1]!) ? 200 : 0);
  let i = 0;
  let total = 0;
  let run = 0;
  for (let j = 0; j < s.length && i < q.length; j++) {
    if (s[j] === q[i]) {
      run++;
      total += 10 + run * 4 + (j === 0 || /[\s/#\-_.(]/.test(s[j - 1]!) ? 25 : 0);
      i++;
    } else run = 0;
  }
  return i === q.length ? total - s.length * 0.2 : -1;
}

export function Palette(props: { commands: () => Command[]; onClose: () => void }) {
  let ref!: HTMLDialogElement;
  let field!: HTMLInputElement;
  let list!: HTMLDivElement;
  const [q, setQ] = createSignal("");
  const [active, setActive] = createSignal(0);
  onMount(() => {
    ref.showModal();
    field.focus();
  });

  // What ⌘K reaches, read once as it opens: a poll landing while you choose must not rebuild the
  // list and move a row from under the cursor just before Enter.
  const all = untrack(() => props.commands());
  const shown = createMemo(() => {
    const query = q().trim().toLowerCase();
    if (!query) return all.filter((c) => !c.searchOnly);
    return all
      .map((c) => ({ c, s: Math.max(score(c.title, query), score(`${c.words ?? ""} ${c.sub ?? ""}`, query) - 150) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 60)
      .map((x) => x.c);
  });
  /** Groups in first-seen order, so the best match's group leads. */
  const groups = createMemo(() => {
    const out: { name: string; items: { c: Command; i: number }[] }[] = [];
    shown().forEach((c, i) => {
      let g = out.find((x) => x.name === c.group);
      if (!g) out.push((g = { name: c.group, items: [] }));
      g.items.push({ c, i });
    });
    return out;
  });
  const flat = createMemo(() => groups().flatMap((g) => g.items.map((x) => x.c)));
  const current = () => flat()[Math.min(active(), flat().length - 1)];

  const move = (d: number) => {
    const n = flat().length;
    if (!n) return;
    setActive((a) => (Math.min(a, n - 1) + d + n) % n);
    list.querySelector<HTMLElement>(`[data-i="${active()}"]`)?.scrollIntoView({ block: "nearest" });
  };
  const go = (c: Command | undefined, alt: boolean) => {
    if (!c) return;
    props.onClose();
    if (alt && c.alt) c.alt.run();
    else c.run();
  };

  return (
    <dialog
      ref={ref}
      class="palette"
      aria-label={t("Go to anything")}
      onCancel={(e) => {
        e.preventDefault();
        props.onClose();
      }}
      onPointerDown={(e) => e.target === ref && props.onClose()}
    >
      <div class="palette-field">
        <Icon name="search" />
        <input
          ref={field}
          value={q()}
          placeholder={t("Search cards, lanes, agents, screens and actions")}
          spellcheck={false}
          autocomplete="off"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          onInput={(e) => {
            setQ(e.currentTarget.value);
            setActive(0);
            list.scrollTop = 0;
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) (e.preventDefault(), move(1));
            else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) (e.preventDefault(), move(-1));
            else if (e.key === "Enter") (e.preventDefault(), go(current(), e.metaKey));
          }}
        />
        <span class="kbd">esc</span>
      </div>
      <div class="palette-list" id="palette-list" role="listbox" ref={list}>
        <For each={groups()}>
          {(g) => (
            <div class="palette-group" role="group" aria-label={g.name}>
              <div class="palette-head">{g.name}</div>
              <For each={g.items}>
                {({ c }) => {
                  const i = () => flat().indexOf(c);
                  return (
                    <div
                      class="palette-row"
                      role="option"
                      data-i={i()}
                      aria-selected={i() === Math.min(active(), flat().length - 1)}
                      onPointerMove={() => setActive(i())}
                      onClick={(e) => go(c, e.metaKey)}
                    >
                      <Icon name={c.icon} small />
                      <span class="palette-title">{c.title}</span>
                      <Show when={c.sub}>
                        <span class="palette-sub">{c.sub}</span>
                      </Show>
                      <Show when={c.keys}>
                        <span class="palette-keys">{c.keys}</span>
                      </Show>
                    </div>
                  );
                }}
              </For>
            </div>
          )}
        </For>
        <Show when={!flat().length}>
          <p class="palette-empty">{t("Nothing matches “{q}”.", { q: q().trim() })}</p>
        </Show>
      </div>
      <div class="palette-foot">
        <span>
          <span class="kbd">↑</span>
          <span class="kbd">↓</span> {t("choose")}
        </span>
        <span>
          <span class="kbd">↵</span> {t("open")}
        </span>
        <Show when={current()?.alt}>
          {(a) => (
            <span>
              <span class="kbd">⌘↵</span> {a().label}
            </span>
          )}
        </Show>
      </div>
    </dialog>
  );
}
