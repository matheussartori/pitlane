// Reports: what each Run all came to, card by card, and what its closing step did. Exported as a
// PDF through the system's print dialog (Save as PDF).
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { api, type Report } from "./api";
import { locale, plural, t } from "./i18n";
import { Icon } from "./icons";
import { Prose } from "./prose";
import { ago } from "./lane";
import { money } from "./run";
import { Spinner, toast } from "./ui";
import { clock, msg } from "./format";

/** The first sentence of a suggestion's spec, for its tile when Claude's line is not there. */
const firstLine = (body: string) => {
  const line = body.replace(/[#*_`>]/g, "").split(/\n+/).map((l) => l.trim()).find(Boolean) ?? "";
  const cut = line.search(/[.!?](\s|$)/);
  return cut > 0 ? line.slice(0, cut + 1) : line;
};

/** A run's screenshot in the report: a thumbnail under its card's name that opens full size. */
function ReportShot(props: { run: string; file: string; label: string }) {
  const [src] = createResource(() => [props.run, props.file] as const, ([r, f]) => api.runShot(r, f).catch(() => ""));
  const [big, setBig] = createSignal(false);
  const key = (ev: KeyboardEvent) => ev.key === "Escape" && setBig(false);
  createEffect(() => (big() ? window.addEventListener("keydown", key) : window.removeEventListener("keydown", key)));
  onCleanup(() => window.removeEventListener("keydown", key));
  return (
    <Show when={src()}>
      {(u) => (
        <figure class="report-shot">
          <button type="button" class="report-shot-thumb" title={t("Open full size")} onClick={() => setBig(true)}>
            <img src={u()} alt={props.label} loading="lazy" />
          </button>
          <figcaption>{props.label}</figcaption>
          <Show when={big()}>
            <Portal>
              <div class="shot-view" role="dialog" aria-label={t("Screenshot")} onClick={() => setBig(false)}>
                <img src={u()} alt={props.label} />
              </div>
            </Portal>
          </Show>
        </figure>
      )}
    </Show>
  );
}

const when = (secs: number) => new Date(secs * 1000).toLocaleString(locale(), { dateStyle: "medium", timeStyle: "short" });

export function ReportsView(props: { reports: Report[] | null; now: number; onChanged: () => void; onCardMade?: () => void }) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const report = createMemo(() => {
    const all = props.reports ?? [];
    return all.find((r) => r.id === picked()) ?? all[0] ?? null;
  });
  // Claude's digest, asked for once per report the first time it is shown without one.
  const [digesting, setDigesting] = createSignal<Record<string, "busy" | "failed">>({});
  const digest = (r: Report) => {
    setDigesting((d) => ({ ...d, [r.id]: "busy" }));
    api.reportDigest(r.repo, r.id).then(
      () => {
        setDigesting((d) => {
          const { [r.id]: _, ...rest } = d;
          return rest;
        });
        props.onChanged();
      },
      () => setDigesting((d) => ({ ...d, [r.id]: "failed" })),
    );
  };
  createEffect(() => {
    const r = report();
    if (r && !r.digest && !digesting()[r.id]) digest(r);
  });
  const exportPdf = async () => {
    try {
      await api.printPage();
    } catch (e) {
      toast({ tone: "fail", title: t("Could not open the print dialog"), body: msg(e) });
    }
  };
  const [making, setMaking] = createSignal<string | null>(null);
  const makeCard = async (r: Report, index: number) => {
    const key = `${r.id}:${index}`;
    if (making()) return;
    setMaking(key);
    try {
      const c = await api.reportSuggestCard(r.repo, r.id, index);
      props.onChanged();
      props.onCardMade?.();
      toast({ tone: "done", title: t("#{id} is in the backlog", { id: c.id }), body: t("Analyse it on the Board; once its questions are answered it joins the next batch, or the one running.") });
    } catch (e) {
      toast({ tone: "fail", title: t("The card was not made"), body: msg(e) });
    } finally {
      setMaking(null);
    }
  };
  const remove = async (r: Report) => {
    const name = t("Run all on {repo}", { repo: r.repo_name });
    try {
      await api.reportDelete(r.id);
      if (picked() === r.id) setPicked(null);
      props.onChanged();
      toast(
        {
          tone: "done",
          title: t("Report deleted"),
          body: `${name} · ${when(r.ended_at)}`,
          action: {
            label: t("Undo"),
            run: () => void api.reportRestore(r.id).then(props.onChanged, (e) => toast({ tone: "fail", title: t("Could not restore the report"), body: msg(e) })),
          },
        },
        12000,
      );
    } catch (e) {
      toast({ tone: "fail", title: t("Could not delete the report"), body: msg(e) });
    }
  };
  return (
    <div class="reports">
      <header class="screen-head">
        <h1 class="title">{t("Reports")}</h1>
        <Show when={(props.reports?.length ?? 0) > 0}>
          <p class="summary">{t("One per Run all, newest first.")}</p>
        </Show>
      </header>
      <Show when={props.reports} fallback={<div class="loading-line"><Spinner /> {t("Reading the reports")}</div>}>
        <Show
          when={props.reports!.length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">{t("No reports yet")}</div>
              <p class="empty-body">{t("When a Run all ends, its report lands here: what each card came to, what changed, what it cost, and what the closing step did.")}</p>
            </div>
          }
        >
          <div class="reports-body">
            <nav class="reports-list" aria-label={t("Reports")}>
              <For each={props.reports!}>
                {(r) => (
                  <button type="button" class="reports-item" aria-current={report()?.id === r.id ? "true" : undefined} onClick={() => setPicked(r.id)}>
                    <span class="reports-item-title">{plural(r.cards.length, "{n} card", "{n} cards")}{r.into ? ` ${t("into {branch}", { branch: r.into })}` : ""}</span>
                    <span class="reports-item-meta">
                      {ago(props.now - r.ended_at * 1000)} · {money(r.cost_usd)}
                    </span>
                  </button>
                )}
              </For>
            </nav>
            <Show when={report()} keyed>
              {(r) => (
                <article class="report-doc">
                  <header class="report-head">
                  <div class="report-head-text">
                  <h2 class="report-title">
                    {t("Run all on {repo}", { repo: r.repo_name })}
                    {r.into ? ` ${t("into {branch}", { branch: r.into })}` : ""}
                  </h2>
                  <p class="report-meta">
                    {t("{from} to {to}", { from: when(r.started_at), to: when(r.ended_at) })} · {plural(r.cards.length, "{n} card", "{n} cards")} · {plural(r.sessions, "{n} agent session", "{n} agent sessions")} ·{" "}
                    {money(r.cost_usd)}
                    {r.direct ? ` · ${t("worked directly in the branch")}` : ""}
                  </p>
                  </div>
                  <div class="report-actions">
                    <button type="button" class="btn" onClick={() => void exportPdf()}>
                      <Icon name="ship" small /> {t("Export PDF")}
                    </button>
                    <button type="button" class="btn" title={t("Delete this report (you can undo)")} onClick={() => void remove(r)}>
                      <Icon name="trash" small /> {t("Delete")}
                    </button>
                  </div>
                  </header>
                  <section class="report-sec report-digest" aria-busy={digesting()[r.id] === "busy"}>
                    <h3>{t("Summary")}</h3>
                    <Show
                      when={r.digest}
                      fallback={
                        <Show
                          when={digesting()[r.id] === "failed"}
                          fallback={<div class="loading-line"><Spinner /> {t("Claude is summarizing this run")}</div>}
                        >
                          <p class="report-meta">
                            {t("The summary could not be written.")}{" "}
                            <button type="button" class="link-btn" onClick={() => digest(r)}>{t("Try again")}</button>
                          </p>
                        </Show>
                      }
                    >
                      {(d) => (
                        <>
                          <p class="report-headline">{d().headline}</p>
                          <Show when={d().points.length > 0}>
                            <ul class="report-points">
                              <For each={d().points}>{(p) => <li>{p}</li>}</For>
                            </ul>
                          </Show>
                        </>
                      )}
                    </Show>
                  </section>
                  <Show when={r.cards.some((c) => (c.shots?.length ?? 0) > 0)}>
                    <section class="report-sec">
                      <h3>{t("Screens")}</h3>
                      <div class="report-shots">
                        <For each={r.cards.flatMap((c) => (c.shots ?? []).map((s) => ({ ...s, label: `#${c.id} ${c.title}` })))}>
                          {(s) => <ReportShot run={s.run} file={s.file} label={s.label} />}
                        </For>
                      </div>
                    </section>
                  </Show>
                  <Show when={(r.suggestions?.length ?? 0) > 0}>
                    <section class="report-sec">
                      <h3>{t("Suggestions")}</h3>
                      <div class="report-ideas">
                        <For each={r.suggestions}>
                          {(s, i) => (
                            <button
                              type="button"
                              class="report-idea"
                              data-made={s.made != null ? "" : undefined}
                              disabled={s.made != null || making() != null}
                              aria-busy={making() === `${r.id}:${i()}`}
                              title={s.made != null ? t("Card #{id}", { id: s.made }) : t("Make a card from this suggestion")}
                              onClick={() => void makeCard(r, i())}
                            >
                              <span class="report-idea-title">{s.title}</span>
                              <span class="report-idea-line">{r.digest?.suggestions[i()] || firstLine(s.body)}</span>
                              <span class="report-idea-foot">
                                <Show
                                  when={s.made == null}
                                  fallback={<><Icon name="done" small /> {t("Card #{id}", { id: s.made! })}</>}
                                >
                                  <Show when={making() === `${r.id}:${i()}`} fallback={<Icon name="plus" small />}>
                                    <Spinner />
                                  </Show>
                                  {t("Make a card")}
                                </Show>
                              </span>
                            </button>
                          )}
                        </For>
                      </div>
                    </section>
                  </Show>
                  <details class="report-more">
                    <summary>
                      {t("Details")}
                      <span class="report-meta">
                        {" · "}
                        {plural(r.cards.length, "{n} card", "{n} cards")} · {plural(r.cards.reduce((n, c) => n + c.files.length, 0), "{n} file", "{n} files")}
                      </span>
                    </summary>
                    <div class="report-more-body">
                  <Show when={r.instructions}>
                    <section class="report-sec">
                      <h3>{t("What you asked for")}</h3>
                      <p class="report-quote">{r.instructions}</p>
                    </section>
                  </Show>
                  <section class="report-sec">
                    <h3>{t("Cards")}</h3>
                    <Show when={r.cards.length === 0}>
                      <p class="report-meta">{t("No card was recorded in this run.")}</p>
                    </Show>
                    <For each={r.cards}>
                      {(c) => (
                        <div class="report-card">
                          <div class="report-card-head">
                            <span class="card-id">#{c.id}</span>
                            <span class="report-card-title">{c.title}</span>
                            <span class="report-state" data-state={c.state}>
                              {c.state}
                            </span>
                          </div>
                          <p class="report-card-meta">
                            {[
                              c.lane ? (c.target && c.target !== c.lane ? `${c.lane} → ${c.target}` : c.lane) : null,
                              c.verdict ? t("judge: {verdict}", { verdict: c.verdict }) : null,
                              plural(c.runs, "{n} run", "{n} runs"),
                              c.duration_ms ? clock(Math.round(c.duration_ms / 1000)) : null,
                              money(c.cost_usd),
                            ]
                              .filter(Boolean)
                              .join(" · ")}
                          </p>
                          <Show when={c.summary}>
                            <p class="report-summary">{c.summary}</p>
                          </Show>
                          <Show when={c.files.length > 0}>
                            <ul class="report-files">
                              <For each={c.files}>
                                {(f) => (
                                  <li>
                                    <span class="report-path">{f.path}</span>
                                    <span class="report-add">+{f.added}</span>
                                    <span class="report-del">−{f.removed}</span>
                                  </li>
                                )}
                              </For>
                            </ul>
                          </Show>
                        </div>
                      )}
                    </For>
                  </section>
                  <Show when={r.finish}>
                    {(f) => (
                      <section class="report-sec">
                        <h3>{t("When it was all done")}</h3>
                        <p class="report-summary" data-fail={f().ok ? undefined : ""}>
                          {f().summary ?? (f().ok ? t("Done.") : t("It did not finish."))}
                        </p>
                      </section>
                    )}
                  </Show>
                    </div>
                  </details>
                </article>
              )}
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}
