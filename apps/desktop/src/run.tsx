// One run: what the agent is doing, live, beside the figures and its handoff.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup } from "solid-js";
import { api, isLive, onRun, type Card, type RunMeta, type RunState, type Yard } from "./api";
import { holdTimeline, timelineOf } from "./store/timelines";
import { kindLabel } from "./agents";
import { AgentConsole } from "./console";
import { LaneTrack } from "./board";
import { noteBy } from "./cardsheet";
import { Counts, DiffViewer, statusLetter, statusWord, type ChangedFile } from "./diffview";
import { Prose } from "./prose";
import { parsePatch } from "./review";
import { Icon, type IconName } from "./icons";
import type { ChipState } from "./lane";
import { locale, plural, t } from "./i18n";
import { ErrorLine, toast } from "./ui";
import { Grip, RUN_SIDE_W } from "./layout";
import { age, clock, elapsed, kTokens, money, msg } from "./format";

export const runStateView: Record<RunState, { chip: ChipState; icon: IconName; label: string }> = {
  starting: { chip: "running", icon: "run", label: "Starting" },
  running: { chip: "running", icon: "run", label: "Running" },
  verifying: { chip: "running", icon: "verify", label: "Checking" },
  done: { chip: "done", icon: "done", label: "Done" },
  failed: { chip: "failed", icon: "fail", label: "Failed" },
  stopped: { chip: "queued", icon: "queued", label: "Stopped" },
  interrupted: { chip: "needs", icon: "needs", label: "Interrupted" },
};

export function RunScreen(props: {
  repo: string;
  card?: Card;
  onOpenCard: (id: number) => void;
  meta: RunMeta | undefined;
  yard: Yard | null;
  now: number;
  onBack: () => void;
  onChanged: () => void;
  onOpenFile: (branch: string | null | undefined, path: string) => void;
  /** The run's worktree in a terminal, or in Code. */
  onTerminal: (path: string, label: string) => void;
  onCode: (path: string) => void;
}) {
  const [sub, setSub] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [more, setMore] = createSignal(false);
  const [side, setSide] = createSignal<"run" | "card">("run");
  const [viewing, setViewing] = createSignal<string | null>(null);

  // Escape closes the diff viewer when one is open, else the screen (like Back). Not while a
  // dialog or menu takes it, nor while you type in the reply box.
  const onEscape = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented || e.isComposing) return;
    if (document.querySelector("dialog[open]")) return;
    const el = e.target as HTMLElement | null;
    // A terminal's Escape belongs to what runs in it (vim, Claude Code).
    if (el?.closest(".xterm")) return;
    if (el && (el.isContentEditable || ((el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) && el.value))) return;
    e.preventDefault();
    if (viewing()) setViewing(null);
    else props.onBack();
  };
  window.addEventListener("keydown", onEscape);
  onCleanup(() => window.removeEventListener("keydown", onEscape));

  // The run's events come from the one timeline store every screen reads (the Terminal's Agents
  // shows the same session): held while this page is open, grown by the daemon's events.
  createEffect(
    on(
      () => props.meta?.id,
      (id) => {
        setError(null); // the last run's failure is not this one's
        if (!id) return;
        onCleanup(holdTimeline(id));
        let off: (() => void) | undefined;
        let gone = false;
        onCleanup(() => {
          gone = true;
          off?.();
        });
        // Its end refreshes the page (the card moved on, the checks ran).
        void onRun(
          () => {},
          (run) => run === id && props.onChanged(),
        ).then((unlisten) => (gone ? unlisten() : (off = unlisten)));
        if (props.meta && isLive(props.meta.state)) void api.runFollow(id, 0).catch((e) => setError(msg(e)));
      },
    ),
  );

  const ordered = () => (props.meta ? timelineOf(props.meta.id) : undefined) ?? [];
  // A Task's steps show in the log when you open it; another run starts on the session itself.
  createEffect(on(() => props.meta?.id, () => setSub(null)));

  const steps = (cmd: string) => cmd.split(/\s*(?:;|&&)\s*/).filter(Boolean);

  // What the lane changed in this run (from the commit it started on), with counts per file.
  // Compared by what it says: the run's record is re-read every few seconds while it works, and
  // a key that is a new object each time would fetch and parse the same diff again each time.
  const diffKey = createMemo(
    () => {
      const m = props.meta;
      if (!m?.branch) return null;
      return { repo: props.repo, branch: m.branch, from: m.base_sha ?? undefined, files: (m.handoff?.files_changed ?? []).join("|"), state: m.state };
    },
    undefined,
    { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) },
  );
  const [diff] = createResource(diffKey, (k) => api.laneDiff(k.repo, k.branch, k.from).catch(() => null));
  const parsed = createMemo(() => (diff() ? parsePatch(diff()!.patch) : []));
  const changed = createMemo<ChangedFile[]>(() => {
    const d = diff();
    const out: ChangedFile[] = [];
    const seen = new Set<string>();
    for (const f of d?.files ?? []) {
      seen.add(f.path);
      out.push({ path: f.path, added: f.added, removed: f.removed, status: parsed().find((p) => p.path === f.path)?.status ?? null });
    }
    // Files the agent reports outside this diff (a related repository, not committed yet).
    for (const p of props.meta?.handoff?.files_changed ?? []) if (!seen.has(p)) out.push({ path: p, added: null, removed: null, status: null });
    return out;
  });
  const totals = createMemo(() => changed().reduce((a, f) => ({ add: a.add + (f.added ?? 0), del: a.del + (f.removed ?? 0) }), { add: 0, del: 0 }));

  async function stop() {
    const m = props.meta;
    if (!m) return;
    try {
      await api.runStop(m.id);
      toast({ tone: "info", title: t("Stopping"), body: t("The agent ends its current step, then the card goes back to Ready.") });
    } catch (e) {
      toast({ tone: "fail", title: t("Not stopped"), body: msg(e) });
    }
  }

  async function continueOnKey() {
    const m = props.meta;
    if (!m) return;
    try {
      const r = await api.runContinue(m.id, true);
      toast({ tone: "done", title: t("Run {id} continues the card", { id: r.id }), body: r.reason ?? undefined });
      props.onChanged();
      location.hash = `#run/${r.id}`;
    } catch (e) {
      toast({ tone: "fail", title: t("Not continued"), body: msg(e) });
    }
  }

  async function resume() {
    const m = props.meta;
    if (!m) return;
    try {
      const r = await api.runResume(m.id);
      toast({ tone: "done", title: t("Run {id} continues the session", { id: r.id }) });
      props.onChanged();
      location.hash = `#run/${r.id}`;
    } catch (e) {
      toast({ tone: "fail", title: t("Not resumed"), body: msg(e) });
    }
  }

  return (
    <Show
      when={props.meta}
      fallback={
        <div class="empty">
          <div class="empty-title">{t("Run not found")}</div>
          <p class="empty-body">{t("Its record may belong to another repository.")}</p>
          <button type="button" class="btn" onClick={props.onBack}>
            {t("Back to the board")}
          </button>
        </div>
      }
    >
      {(m) => (
        <div class="run-screen">
          <header class="screen-head">
            <button type="button" class="btn btn-icon back" aria-label={t("Back to the board")} title={t("Back to the board")} onClick={props.onBack}>
              <Icon name="chev" small />
            </button>
            <h1 class="title">
              #{m().card} {m().card_title}
            </h1>
            <Show
              when={m().waiting.length > 0}
              fallback={
                <span class="chip" data-state={runStateView[m().state].chip}>
                  <Icon name={runStateView[m().state].icon} small />
                  {t(runStateView[m().state].label)}
                </span>
              }
            >
              <span class="chip" data-state="needs">
                <Icon name="needs" small />
                {m().waiting[0]!.tool === "Bash" ? t("Wants to run a command") : t("Wants {tool}", { tool: m().waiting[0]!.tool })}
              </span>
            </Show>
            <span class="run-clock" aria-label={t("Elapsed")}>
              {clock(elapsed(m(), props.now))}
            </span>
            <span class="spacer" />
            <Show when={isLive(m().state)}>
              <button type="button" class="btn btn-danger" onClick={stop}>
                {t("Stop")}
              </button>
            </Show>
            <Show when={!isLive(m().state)}>
              <button type="button" class="btn" onClick={resume}>
                <Icon name="ship" small />
                {m().state === "done" ? t("Continue the session") : t("Resume")}
              </button>
            </Show>
          </header>
          <p class="run-where">
            <span class="branch selectable" title={m().worktree}>
              {m().branch || t("Preparing the lane")}
            </span>
            <LaneTrack branch={m().branch} yard={props.yard} now={props.now} />
            <Show when={m().worktree}>
              {(wt) => (
                <span class="run-where-go">
                  <button type="button" class="btn btn-ghost run-go" title={wt()} onClick={() => props.onTerminal(wt(), m().branch || t("Run"))}>
                    <Icon name="term" small />
                    {t("Terminal")}
                  </button>
                  <button type="button" class="btn btn-ghost run-go" title={wt()} onClick={() => props.onCode(wt())}>
                    <Icon name="code" small />
                    {t("Code")}
                  </button>
                </span>
              )}
            </Show>
            <Show when={m().resumed_from}>
              {(prev) => (
                <a href={`#run/${prev()}`} title={t("The same Claude Code session: the agent has all that run did and said")}>
                  {t("continues {run}", { run: prev() })}
                </a>
              )}
            </Show>
          </p>
          <Show when={m().next}>
            {(next) => (
              <section class="approval" aria-label={t("Continue on the API key")}>
                <p class="approval-ask">{t("Stopped at the usage limit of {account}", { account: m().account ?? t("its account") })}</p>
                <p class="approval-why">
                  {t("Continue as {next}, paid per token", { next: next() })}
                  <Show when={m().resets_at}>
                    {(at) => <>{t(", or wait: the window resets at {time}", { time: new Date(at() * 1000).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" }) })}</>}
                  </Show>
                  .
                </p>
                <div class="row-actions">
                  <button type="button" class="btn btn-primary" onClick={() => void continueOnKey()}>
                    {t("Continue as {next}", { next: next() })}
                  </button>
                </div>
              </section>
            )}
          </Show>

          <Show when={error()}>
            <ErrorLine>{error()}</ErrorLine>
          </Show>

          <div class="run-body" data-live={isLive(m().state) ? "" : undefined} style={{ "--side-w": `${RUN_SIDE_W.get()}px` }}>
            <Grip size={RUN_SIDE_W} edge="left" label={t("Resize the side column")} style={{ left: "auto", right: "calc(var(--side-w) + var(--space-lg) / 2 - 5px)" }} />
            <AgentConsole
              m={m()}
              events={ordered()}
              sub={sub()}
              onSub={setSub}
              kind={kindLabel(m())}
              ended={t(runStateView[m().state].label)}
              framed
            />

            <aside class="run-side" aria-label={t("What this run did")}>
              <div class="rs-tabs" role="tablist" aria-label={t("Beside the log")}>
                <button type="button" role="tab" aria-selected={side() === "run"} onClick={() => setSide("run")}>
                  {t("This run")}
                </button>
                <button type="button" role="tab" aria-selected={side() === "card"} onClick={() => setSide("card")}>
                  {t("Card #{id}", { id: m().card })}
                </button>
              </div>
              <Show when={side() === "card"}>
                <RunCard card={props.card} now={props.now} onOpen={() => props.onOpenCard(m().card)} />
              </Show>
              <Show when={side() === "run"}>
              <Show when={!m().handoff && changed().length > 0}>
                <ChangedList files={changed()} totals={totals()} onPick={setViewing} />
              </Show>
              <Show when={m().handoff}>
                {(h) => {
                  const files = () => h().files_changed ?? [];
                  const tested = () => h().tests?.command;
                  const view = () => runStateView[m().state];
                  return (
                    <>
                      {/* The outcome first: what happened, in one line. */}
                      <div class="rs-outcome" data-tone={view().chip}>
                        <Icon name={view().icon} small />
                        <span class="rs-outcome-state">{t(view().label)}</span>
                        <Show when={tested()}>
                          <span class="rs-outcome-part">{h().tests?.passed ? t("checks pass") : t("checks fail")}</span>
                        </Show>
                        <Show when={Math.max(files().length, changed().length) > 0}>
                          <span class="rs-outcome-part">{plural(Math.max(files().length, changed().length), "{n} file changed", "{n} files changed")}</span>
                        </Show>
                      </div>
                      <Show when={changed().length > 0}>
                        <ChangedList files={changed()} totals={totals()} onPick={setViewing} />
                      </Show>
                      <Show when={tested()}>
                        <section class="rs-sec">
                          <h2 class="side-title">{t("How it was checked")}</h2>
                          <ul class="rs-checks" data-ok={h().tests?.passed ? "" : undefined}>
                            <For each={steps(tested()!)}>
                              {(c) => (
                                <li>
                                  <Icon name={h().tests?.passed ? "done" : "fail"} small />
                                  <code class="selectable">{c}</code>
                                </li>
                              )}
                            </For>
                          </ul>
                        </section>
                      </Show>
                      <Show when={h().summary}>
                        <section class="rs-sec">
                          <h2 class="side-title">{t("Summary")}</h2>
                          <p class="rs-summary selectable" data-open={more() ? "" : undefined}>
                            {h().summary}
                          </p>
                          <Show when={(h().summary?.length ?? 0) > 220}>
                            <button type="button" class="link-btn rs-more" onClick={() => setMore(!more())}>
                              {more() ? t("Less") : t("More")}
                            </button>
                          </Show>
                        </section>
                      </Show>
                    </>
                  );
                }}
              </Show>
              <Show when={m().denied.length > 0}>
                <section class="rs-sec">
                  <h2 class="side-title">{t("Not allowed")}</h2>
                  <p class="side-hint">{t("Add these to [run] allow in .pitlane/flow.toml if the agent should run them.")}</p>
                  <ul class="side-list mono">
                    <For each={m().denied}>{(d) => <li class="selectable">{d}</li>}</For>
                  </ul>
                </section>
              </Show>
              <Show when={m().brief}>
                {(b) => {
                  const changed = () => new Set(m().handoff?.files_changed ?? []);
                  return (
                    <details class="rs-map">
                      <summary>
                        {t("Where Pitlane pointed it")} <span class="rs-count">{b().paths.length}</span>
                      </summary>
                      <p class="side-hint">{t("Files from the repository index sent with the first message, so the agent starts in the right place.")}</p>
                      <ul class="rs-files">
                        <For each={b().paths}>
                          {(p) => {
                            const cut = p.lastIndexOf("/");
                            return (
                              <li class="rs-file is-static" data-hit={changed().has(p) ? "" : undefined}>
                                <span class="rs-file-name">{p.slice(cut + 1)}</span>
                                <span class="rs-file-dir">{cut > 0 ? p.slice(0, cut) : ""}</span>
                                <Show when={changed().has(p)}>
                                  <span class="rs-hit">{t("changed")}</span>
                                </Show>
                              </li>
                            );
                          }}
                        </For>
                      </ul>
                    </details>
                  );
                }}
              </Show>
              {/* Figures last, on one quiet line: useful, rarely the question. */}
              <Show when={m().turns != null} fallback={<p class="side-hint">{t("Turns, cost and tokens arrive with the agent's first report.")}</p>}>
                <p class="rs-figures" title={t("{n} cached", { n: kTokens(m().tokens.cache_read) })}>
                  <span>{plural(m().turns ?? 0, "{n} turn", "{n} turns")}</span>
                  <span>{money(m().cost_usd ?? 0)}</span>
                  <span>{t("{n} in", { n: kTokens(m().tokens.input + m().tokens.cache_read + m().tokens.cache_write) })}</span>
                  <span>{t("{n} out", { n: kTokens(m().tokens.output) })}</span>
                </p>
              </Show>
              <Show when={m().error && !m().next}>
                <ErrorLine>{m().error}</ErrorLine>
              </Show>
              </Show>
            </aside>
          </div>

          <Show when={viewing()}>
            {(start) => (
              <DiffViewer
                title={t("What changed")}
                files={changed()}
                diffs={parsed()}
                truncated={!!diff()?.truncated}
                start={start()}
                onOpenInCode={(f) => {
                  setViewing(null);
                  props.onOpenFile(m().branch, f);
                }}
                onClose={() => setViewing(null)}
              />
            )}
          </Show>

        </div>
      )}
    </Show>
  );
}

/** What changed, file by file: its mark (added, modified, deleted), the lines in and out. */
function ChangedList(props: { files: ChangedFile[]; totals: { add: number; del: number }; onPick: (path: string) => void }) {
  return (
    <section class="rs-sec">
      <h2 class="side-title rs-title">
        {t("What changed")}
        <Counts added={props.totals.add} removed={props.totals.del} />
      </h2>
      <ul class="rs-files rs-changed">
        <For each={props.files}>
          {(f) => {
            const cut = f.path.lastIndexOf("/");
            const st = f.status ?? "modified";
            return (
              <li>
                <button type="button" class="rs-file rs-change" title={t("{status}: see the diff", { status: t(statusWord[st]) })} onClick={() => props.onPick(f.path)}>
                  <span class="dv-status" data-status={st} aria-label={t(statusWord[st])}>
                    {statusLetter[st]}
                  </span>
                  <span class="rs-file-name" data-status={st}>{f.path.slice(cut + 1)}</span>
                  <Counts added={f.added} removed={f.removed} />
                  <span class="rs-file-dir">{cut > 0 ? f.path.slice(0, cut) : ""}</span>
                </button>
              </li>
            );
          }}
        </For>
      </ul>
    </section>
  );
}

/** The card the run works on, as its sheet shows it: the spec, the decisions, the notes. */
function RunCard(props: { card?: Card; now: number; onOpen: () => void }) {
  const [full, setFull] = createSignal(false);
  const [clipped, setClipped] = createSignal(false);
  const measure = (el: HTMLDivElement) => requestAnimationFrame(() => setClipped(el.scrollHeight > el.clientHeight + 2));
  const decided = () => (props.card?.questions ?? []).filter((q) => q.answer != null);
  const notes = () => props.card?.notes ?? [];
  return (
    <Show when={props.card} fallback={<p class="side-hint">{t("The card is not on this board any more.")}</p>}>
      {(c) => (
        <>
          <section class="rs-sec">
            <h2 class="side-title rs-title">
              {t("Spec")}
              <button type="button" class="link-btn rs-more rs-open-card" onClick={props.onOpen}>
                {t("Open card")}
              </button>
            </h2>
            <Show when={c().body.trim()} fallback={<p class="side-hint">{t("This card has no spec, only its title.")}</p>}>
              <div class="rs-spec" ref={measure} data-open={full() ? "" : undefined} data-clipped={clipped() ? "" : undefined}>
                <Prose text={c().body} />
              </div>
              <Show when={clipped() || full()}>
                <button type="button" class="link-btn rs-more" onClick={() => setFull(!full())}>
                  {full() ? t("Less") : t("More")}
                </button>
              </Show>
            </Show>
          </section>
          <Show when={decided().length > 0}>
            <section class="rs-sec">
              <h2 class="side-title">{t("Decisions")}</h2>
              <ul class="rs-decisions">
                <For each={decided()}>
                  {(q) => (
                    <li class="rs-decision">
                      <p class="rs-decision-q">{q.text}</p>
                      <p class="rs-decision-a">
                        <Icon name="done" small />
                        <span>{q.answer}</span>
                      </p>
                      <Show when={q.decision}>
                        <span class="tag">{q.decision}</span>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </section>
          </Show>
          <section class="rs-sec">
            <h2 class="side-title rs-title">
              {t("Notes")} <span class="rs-count">{notes().length}</span>
            </h2>
            <Show when={notes().length > 0} fallback={<p class="side-hint">{t("No notes yet. Run all's analysis and each run add theirs here.")}</p>}>
              <ol class="notes rs-notes">
                <For each={notes()}>
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
        </>
      )}
    </Show>
  );
}
