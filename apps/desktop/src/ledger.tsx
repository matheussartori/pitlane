// Ledger: what each run cost, in time, tokens and money, as the agents reported it. An overview on
// top (today, the last 7 days, all, what works now, where the money went), then the runs by day.
import { For, Show, createMemo } from "solid-js";
import { isLive, type RunMeta } from "./api";
import { Icon } from "./icons";
import { runStateView } from "./cards";
import { kindLabel } from "./store/agents";
import { modelLabel } from "./console";
import { clock, elapsed, kTokens, money, timeOfDay } from "./format";
import { locale, plural, t } from "./i18n";
import { Spinner } from "./ui";

const tokensOf = (r: RunMeta) => r.tokens.input + r.tokens.output + r.tokens.cache_read + r.tokens.cache_write;
const DAY = 86_400_000;
const dayStart = (ms: number) => {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

function sum(rs: RunMeta[]) {
  return { cost: rs.reduce((n, r) => n + (r.cost_usd ?? 0), 0), runs: rs.length };
}

export function LedgerView(props: { runs: RunMeta[] | null; now: number; onOpen: (id: string) => void }) {
  const runs = () => props.runs ?? [];
  const today = () => dayStart(props.now);
  const overview = createMemo(() => {
    const rs = runs();
    return {
      today: sum(rs.filter((r) => r.started_at * 1000 >= today())),
      week: sum(rs.filter((r) => r.started_at * 1000 >= today() - 6 * DAY)),
      all: sum(rs),
      live: rs.filter((r) => isLive(r.state)).length,
      seconds: rs.reduce((n, r) => n + elapsed(r, props.now), 0),
      tokens: rs.reduce((n, r) => n + tokensOf(r), 0),
    };
  });
  /** The cards that took the most, all runs of each together. */
  const topCards = createMemo(() => {
    const by = new Map<number, { card: number; title: string; cost: number; runs: number }>();
    for (const r of runs()) {
      const c = by.get(r.card) ?? { card: r.card, title: r.card_title, cost: 0, runs: 0 };
      c.cost += r.cost_usd ?? 0;
      c.runs += 1;
      by.set(r.card, c);
    }
    return [...by.values()].sort((a, b) => b.cost - a.cost).slice(0, 5);
  });
  /** The runs by the day they started, newest first, each day with its total. */
  const days = createMemo(() => {
    const out: { start: number; runs: RunMeta[]; cost: number }[] = [];
    for (const r of runs()) {
      const s = dayStart(r.started_at * 1000);
      let d = out.find((x) => x.start === s);
      if (!d) out.push((d = { start: s, runs: [], cost: 0 }));
      d.runs.push(r);
      d.cost += r.cost_usd ?? 0;
    }
    return out.sort((a, b) => b.start - a.start);
  });
  const dayLabel = (s: number) =>
    s === today()
      ? t("Today")
      : s === today() - DAY
        ? t("Yesterday")
        : new Date(s).toLocaleDateString(locale(), { weekday: "long", day: "numeric", month: "long" });

  return (
    <div class="ledger">
      <header class="screen-head">
        <h1 class="title">{t("Ledger")}</h1>
        <Show when={runs().length > 0}>
          <p class="summary">{t("{runs}, {cost} in all.", { runs: plural(runs().length, "{n} run", "{n} runs"), cost: money(overview().all.cost) })}</p>
        </Show>
      </header>
      <Show when={props.runs} fallback={<div class="loading-line"><Spinner /> {t("Reading the runs")}</div>}>
        <Show
          when={runs().length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">{t("No runs yet")}</div>
              <p class="empty-body">
                {t("Each run of an agent on a card lands here with its time, turns, tokens and cost, as the agent reported them. Start one from a ready card on the Board.")}
              </p>
            </div>
          }
        >
          <section class="ledger-overview" aria-label={t("Overview")}>
            <div class="stat">
              <span class="stat-label">{t("Today")}</span>
              <span class="stat-value">{money(overview().today.cost)}</span>
              <span class="stat-sub">{plural(overview().today.runs, "{n} run", "{n} runs")}</span>
            </div>
            <div class="stat">
              <span class="stat-label">{t("Last 7 days")}</span>
              <span class="stat-value">{money(overview().week.cost)}</span>
              <span class="stat-sub">{plural(overview().week.runs, "{n} run", "{n} runs")}</span>
            </div>
            <div class="stat">
              <span class="stat-label">{t("All runs")}</span>
              <span class="stat-value">{money(overview().all.cost)}</span>
              <span class="stat-sub">
                {t("{time} of agent time, {tokens} tokens", { time: clock(overview().seconds), tokens: kTokens(overview().tokens) })}
              </span>
            </div>
            <div class="stat">
              <span class="stat-label">{t("Working now")}</span>
              <span class="stat-value" data-live={overview().live > 0 ? "" : undefined}>{overview().live}</span>
              <span class="stat-sub">{overview().live > 0 ? plural(overview().live, "{n} agent", "{n} agents") : t("nothing running")}</span>
            </div>
          </section>

          <section class="ledger-top" aria-label={t("Where it went")}>
            <h2 class="inbox-section-title">{t("Where it went")}</h2>
            <ol class="top-cards">
              <For each={topCards()}>
                {(c) => (
                  <li class="top-card">
                    <span class="card-id">#{c.card}</span>
                    <span class="top-card-title">{c.title}</span>
                    <span class="top-card-bar" aria-hidden="true">
                      <span style={{ "--v": String(topCards()[0]!.cost ? c.cost / topCards()[0]!.cost : 0) }} />
                    </span>
                    <span class="top-card-runs">{plural(c.runs, "{n} run", "{n} runs")}</span>
                    <span class="top-card-cost">{money(c.cost)}</span>
                  </li>
                )}
              </For>
            </ol>
          </section>

          <table class="table ledger-table">
            <colgroup>
              <col />
              <col class="l-result" />
              <col class="l-at" />
              <col class="l-num" />
              <col class="l-num" />
              <col class="l-num" />
            </colgroup>
            <thead>
              <tr>
                <th scope="col">{t("Card")}</th>
                <th scope="col">{t("Result")}</th>
                <th scope="col">{t("Started")}</th>
                <th scope="col" class="end">{t("Time")}</th>
                <th scope="col" class="end">{t("Tokens")}</th>
                <th scope="col" class="end">{t("Cost")}</th>
              </tr>
            </thead>
            <For each={days()}>
              {(d) => (
                <tbody>
                  <tr class="ledger-day">
                    <th scope="rowgroup" colspan="5">
                      {dayLabel(d.start)} <span class="ledger-day-count">{plural(d.runs.length, "{n} run", "{n} runs")}</span>
                    </th>
                    <td class="num">{money(d.cost)}</td>
                  </tr>
                  <For each={d.runs}>
                    {(r) => (
                      <tr tabindex="0" onClick={() => props.onOpen(r.id)} onKeyDown={(e) => e.key === "Enter" && props.onOpen(r.id)}>
                        <td>
                          <span class="ledger-run">
                            <span class="ledger-card">
                              <span class="card-id">#{r.card}</span>
                              <span class="ledger-card-title">{r.card_title}</span>
                            </span>
                            <span class="ledger-sub">
                              {[kindLabel(r), modelLabel(r.model), r.account && r.account !== "default" ? r.account : null].filter(Boolean).join(" · ")}
                            </span>
                          </span>
                        </td>
                        <td>
                          <span class="chip" data-state={runStateView[r.state].chip}>
                            <Icon name={runStateView[r.state].icon} small />
                            {t(runStateView[r.state].label)}
                          </span>
                        </td>
                        <td class="muted">{timeOfDay(r.started_at)}</td>
                        <td class="num">{clock(elapsed(r, props.now))}</td>
                        <td class="num">{kTokens(tokensOf(r))}</td>
                        <td class="num cost">{money(r.cost_usd ?? 0)}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              )}
            </For>
          </table>
        </Show>
      </Show>
    </div>
  );
}
