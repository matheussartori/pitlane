// Reports: what each Run all came to, card by card, and what its closing step did. Exported as a
// PDF through the system's print dialog (Save as PDF).
import { For, Show, createMemo, createSignal } from "solid-js";
import { api, type Report } from "./api";
import { locale, plural, t } from "./i18n";
import { Icon } from "./icons";
import { ago } from "./lane";
import { clock, money } from "./run";
import { Spinner, toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
const when = (secs: number) => new Date(secs * 1000).toLocaleString(locale(), { dateStyle: "medium", timeStyle: "short" });

export function ReportsView(props: { reports: Report[] | null; now: number }) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const report = createMemo(() => {
    const all = props.reports ?? [];
    return all.find((r) => r.id === picked()) ?? all[0] ?? null;
  });
  const exportPdf = async () => {
    try {
      await api.printPage();
    } catch (e) {
      toast({ tone: "fail", title: t("Could not open the print dialog"), body: msg(e) });
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
                  <div class="report-actions">
                    <button type="button" class="btn" onClick={() => void exportPdf()}>
                      <Icon name="ship" small /> {t("Export PDF")}
                    </button>
                  </div>
                  <h2 class="report-title">
                    {t("Run all on {repo}", { repo: r.repo_name })}
                    {r.into ? ` ${t("into {branch}", { branch: r.into })}` : ""}
                  </h2>
                  <p class="report-meta">
                    {t("{from} to {to}", { from: when(r.started_at), to: when(r.ended_at) })} · {plural(r.cards.length, "{n} card", "{n} cards")} · {plural(r.sessions, "{n} agent session", "{n} agent sessions")} ·{" "}
                    {money(r.cost_usd)}
                    {r.direct ? ` · ${t("worked directly in the branch")}` : ""}
                  </p>
                  <Show when={r.instructions}>
                    <section class="report-sec">
                      <h3>{t("What you asked for")}</h3>
                      <p class="report-quote">{r.instructions}</p>
                    </section>
                  </Show>
                  <section class="report-sec">
                    <h3>{t("Cards")}</h3>
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
                </article>
              )}
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}
