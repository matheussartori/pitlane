// One run: what the agent is doing, live, beside the figures and its handoff.
import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";
import { api, isLive, onRun, type RunEvent, type RunMeta, type RunState, type Yard } from "./api";
import { ApprovalBlock } from "./approvals";
import { LaneTrack } from "./board";
import { Icon, type IconName } from "./icons";
import type { ChipState } from "./lane";
import { locale, plural, t, tx } from "./i18n";
import { ErrorLine, Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

export const runStateView: Record<RunState, { chip: ChipState; icon: IconName; label: string }> = {
  starting: { chip: "running", icon: "run", label: "Starting" },
  running: { chip: "running", icon: "run", label: "Running" },
  verifying: { chip: "running", icon: "verify", label: "Checking" },
  done: { chip: "done", icon: "done", label: "Done" },
  failed: { chip: "failed", icon: "fail", label: "Failed" },
  stopped: { chip: "queued", icon: "queued", label: "Stopped" },
  interrupted: { chip: "needs", icon: "needs", label: "Interrupted" },
};

/** 95 → 1:35, 4000 → 1:06:40. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Wall time of a run: ticking while live, fixed once it ends. */
export function elapsed(m: RunMeta, nowMs: number): number {
  const end = m.ended_at ?? (isLive(m.state) ? nowMs / 1000 : m.started_at + (m.duration_ms ?? 0) / 1000);
  return end - m.started_at;
}

export const money = (n: number | null | undefined) => (n == null ? "" : `$${n.toFixed(2)}`);
export const kTokens = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M` : n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);

export function RunScreen(props: {
  meta: RunMeta | undefined;
  yard: Yard | null;
  now: number;
  onBack: () => void;
  onChanged: () => void;
  onOpenFile: (branch: string | null | undefined, path: string) => void;
}) {
  const [events, setEvents] = createSignal<Map<number, RunEvent>>(new Map());
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [more, setMore] = createSignal(false);
  let list: HTMLDivElement | undefined;
  let stick = true;

  const add = (pairs: [number, RunEvent][]) =>
    setEvents((prev) => {
      const next = new Map(prev);
      for (const [i, e] of pairs) next.set(i, e);
      return next;
    });

  // Once the run is known (the list may land after the screen opens): listen first, then read
  // the history, so an event can arrive twice but is never missed.
  createEffect(
    on(
      () => props.meta?.id,
      (id) => {
        setEvents(new Map());
        if (!id) return;
        let off: (() => void) | undefined;
        let gone = false;
        onCleanup(() => {
          gone = true;
          off?.();
        });
        void (async () => {
          const unlisten = await onRun(
            (run, i, e) => run === id && add([[i, e]]),
            (run) => run === id && props.onChanged(),
          );
          if (gone) return unlisten();
          off = unlisten;
          try {
            add(await api.runTimeline(id));
            if (props.meta && isLive(props.meta.state)) await api.runFollow(id, 0);
          } catch (e) {
            setError(msg(e));
          }
        })();
      },
    ),
  );

  const ordered = createMemo(() => [...events().entries()].sort((a, b) => a[0] - b[0]).map(([, e]) => e));
  const doneById = createMemo(() => {
    const m = new Map<string, Extract<RunEvent, { kind: "tool-done" }>>();
    for (const e of ordered()) if (e.kind === "tool-done") m.set(e.id, e);
    return m;
  });

  createEffect(() => {
    ordered();
    if (list && stick) queueMicrotask(() => list && (list.scrollTop = list.scrollHeight));
  });

  // Paths inside the worktree read relative to it; anything under a Pitlane worktree root too.
  const rel = (detail: string) => {
    const wt = props.meta?.worktree;
    let d = wt ? detail.replaceAll(`${wt}/`, "").replaceAll(wt, ".") : detail;
    d = d.replace(/\/[^\s"']*?\/wt\/[^/\s]+\/[^/\s]+\//g, "");
    return d;
  };
  // Setup notes matter until the agent starts; after that one line says it.
  const startedAt = createMemo(() => ordered().findIndex((e) => e.kind === "started"));
  const openTool = createMemo(() => ordered().some((e) => e.kind === "tool" && !doneById().has(e.id)));
  const steps = (cmd: string) => cmd.split(/\s*(?:;|&&)\s*/).filter(Boolean);

  async function say(e: Event) {
    e.preventDefault();
    const m = props.meta;
    if (!m || !text().trim()) return;
    setBusy(true);
    try {
      await api.runSay(m.id, text().trim());
      setText("");
    } catch (err) {
      toast({ tone: "fail", title: t("Not sent"), body: msg(err) });
    } finally {
      setBusy(false);
    }
  }

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
            <button type="button" class="btn btn-ghost btn-icon back" aria-label={t("Back to the board")} onClick={props.onBack}>
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
            <span title={m().reason ?? undefined}>
              {m().model ?? t("default model")}
              {m().account ? ` ${t("on {account}", { account: m().account! })}` : ""}
            </span>
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

          <div class="run-body" data-live={isLive(m().state) ? "" : undefined}>
            <div
              class="timeline"
              ref={list}
              aria-live="polite"
              onScroll={(e) => {
                const el = e.currentTarget;
                stick = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
              }}
            >
              <For each={ordered()}>
                {(e) => (
                  <Switch>
                    <Match when={e.kind === "started" && e}>
                      {(x) => <p class="tl-note">{t("Claude Code started with {model} in the lane’s worktree.", { model: x().model })}</p>}
                    </Match>
                    <Match when={e.kind === "text" && e}>{(x) => <p class="tl-text selectable">{x().text}</p>}</Match>
                    <Match when={e.kind === "tool" && e}>
                      {(x) => {
                        const done = () => doneById().get(x().id);
                        return (
                          <div class="tl-tool" data-state={done() ? (done()!.ok ? "ok" : "fail") : "open"}>
                            <span class="tl-tool-name">{x().name}</span>
                            <code class="tl-tool-detail selectable" title={x().detail}>
                              {rel(x().detail)}
                            </code>
                            <span class="tl-tool-mark">
                              <Show when={done()} fallback={<Spinner />}>
                                <Icon name={done()!.ok ? "done" : "fail"} small />
                              </Show>
                            </span>
                            <Show when={done() && !done()!.ok}>
                              <span class="tl-tool-fail selectable">{done()!.detail}</span>
                            </Show>
                          </div>
                        );
                      }}
                    </Match>
                    <Match when={e.kind === "said" && e}>
                      {(x) => (
                        <p class="tl-said selectable">
                          <b>{t("You")}</b>
                          {x().text}
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "approval" && e}>
                      {(x) => (
                        <p class="tl-note tl-asked" data-pending={m().waiting.some((w) => w.id === x().approval.id) ? "" : undefined}>
                          <Icon name="needs" small /> {x().approval.tool === "Bash" ? t("Asked to run") : t("Asked to use")}{" "}
                          <code>{rel(x().approval.detail || x().approval.tool)}</code>
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "proof" && e}>
                      {(x) => (
                        <div class="tl-tool" data-state={x().ok ? "ok" : "fail"}>
                          <span class="tl-tool-name">{t("Check")}</span>
                          <code class="tl-tool-detail selectable">{x().command}</code>
                          <span class="tl-tool-mark">
                            <Icon name={x().ok ? "done" : "fail"} small />
                          </span>
                        </div>
                      )}
                    </Match>
                    <Match when={e.kind === "judged" && e}>
                      {(x) => (
                        <p class="tl-finished" data-ok={x().verdict === "approve" ? "" : undefined}>
                          <Icon name={x().verdict === "approve" ? "done" : "needs"} small />
                          {x().verdict === "approve" ? t("The judge approves: {summary}", { summary: x().summary }) : t("The judge asks for changes ({findings}): {summary}", { findings: plural(x().findings, "{n} finding", "{n} findings"), summary: x().summary })}
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "limited" && e}>
                      {(x) => (
                        <p class="tl-note tl-limit">
                          <Icon name="needs" small /> {t("Hit the {window} usage limit on {account}.", { window: x().window.replace("_", "-"), account: m().account ?? t("default") })}
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "decided" && e}>
                      {(x) => (
                        <p class="tl-note">
                          {t({ once: "You allowed it once.", run: "You allowed it for this run.", always: "You allowed it in this repository.", deny: "You denied it." }[x().decision])}
                          {x().message ? ` ${x().message}` : ""}
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "note" && (startedAt() < 0 || ordered().indexOf(e) > startedAt()) && e}>
                      {(x) => <p class="tl-note">{x().text}</p>}
                    </Match>
                    <Match when={e.kind === "finished" && e}>
                      {(x) => (
                        <p class="tl-finished" data-ok={x().ok ? "" : undefined}>
                          <Icon name={x().ok ? "done" : "fail"} small />
                          {x().ok ? t("Finished. What it did is beside the log.") : t("Finished with an error.")}
                        </p>
                      )}
                    </Match>
                    <Match when={e.kind === "exited" && e.state !== "done" && e}>
                      {(x) => <p class="tl-note">{t("The agent exited: {state}.", { state: t(runStateView[x().state].label).toLowerCase() })}</p>}
                    </Match>
                  </Switch>
                )}
              </For>
              <Show when={isLive(m().state) && !openTool() && m().waiting.length === 0}>
                <p class="tl-note tl-working">
                  <Spinner /> {t("Working")}
                </p>
              </Show>
            </div>

            <aside class="run-side" aria-label={t("What this run did")}>
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
                        <Show when={files().length > 0}>
                          <span class="rs-outcome-part">{plural(files().length, "{n} file changed", "{n} files changed")}</span>
                        </Show>
                      </div>
                      <Show when={files().length > 0}>
                        <section class="rs-sec">
                          <h2 class="side-title">{t("What changed")}</h2>
                          <ul class="rs-files">
                            <For each={files()}>
                              {(f) => {
                                const cut = f.lastIndexOf("/");
                                return (
                                  <li>
                                    <button type="button" class="rs-file" title={t("Open in Code")} onClick={() => props.onOpenFile(m().branch, f)}>
                                      <span class="rs-file-name">{f.slice(cut + 1)}</span>
                                      <span class="rs-file-dir">{cut > 0 ? f.slice(0, cut) : ""}</span>
                                    </button>
                                  </li>
                                );
                              }}
                            </For>
                          </ul>
                        </section>
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
                      <Show when={(h().next_steps?.length ?? 0) > 0}>
                        <section class="rs-sec">
                          <h2 class="side-title">{t("Next steps")}</h2>
                          <ul class="side-list">
                            <For each={h().next_steps}>{(s) => <li>{s}</li>}</For>
                          </ul>
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
            </aside>
          </div>

          <For each={m().waiting}>{(a, i) => <ApprovalBlock run={m().id} approval={a} keys={i() === 0} />}</For>

          <Show when={isLive(m().state)}>
            <form class="composer" onSubmit={say}>
              <label class="visually-hidden" for="say">
                {t("Tell the agent")}
              </label>
              <input
                id="say"
                class="input"
                placeholder={t("Tell the agent something without stopping it")}
                autocomplete="off"
                value={text()}
                disabled={busy()}
                onInput={(e) => setText(e.currentTarget.value)}
              />
              <button type="submit" class="btn" disabled={busy() || !text().trim()}>
                {t("Send")}
              </button>
            </form>
          </Show>
        </div>
      )}
    </Show>
  );
}
