// Ledger: what each run cost, in time, turns, tokens and money, as the agents reported it.
import { For, Show } from "solid-js";
import type { RunMeta } from "./api";
import { Icon } from "./icons";
import { ago } from "./lane";
import { clock, elapsed, kTokens, money, runStateView } from "./run";
import { Spinner } from "./ui";

export function LedgerView(props: { runs: RunMeta[] | null; now: number; onOpen: (id: string) => void }) {
  const total = () => {
    const rs = props.runs ?? [];
    return {
      cost: rs.reduce((n, r) => n + (r.cost_usd ?? 0), 0),
      seconds: rs.reduce((n, r) => n + elapsed(r, props.now), 0),
      turns: rs.reduce((n, r) => n + (r.turns ?? 0), 0),
      tokens: rs.reduce((n, r) => n + r.tokens.input + r.tokens.output + r.tokens.cache_read + r.tokens.cache_write, 0),
    };
  };
  return (
    <div class="ledger">
      <header class="screen-head">
        <h1 class="title">Ledger</h1>
        <Show when={(props.runs?.length ?? 0) > 0}>
          <p class="summary">
            {props.runs!.length} {props.runs!.length === 1 ? "run" : "runs"}, {money(total().cost)} in all.
          </p>
        </Show>
      </header>
      <Show when={props.runs} fallback={<div class="loading-line"><Spinner /> Reading the runs</div>}>
        <Show
          when={props.runs!.length > 0}
          fallback={
            <div class="empty">
              <div class="empty-title">No runs yet</div>
              <p class="empty-body">
                Each run of an agent on a card lands here with its time, turns, tokens and cost, as the agent reported them.
                Start one from a ready card on the Board.
              </p>
            </div>
          }
        >
          <table class="table ledger-table">
            <thead>
              <tr>
                <th scope="col">Card</th>
                <th scope="col">Lane</th>
                <th scope="col">Result</th>
                <th scope="col">Started</th>
                <th scope="col" class="end">
                  Time
                </th>
                <th scope="col" class="end">
                  Turns
                </th>
                <th scope="col" class="end">
                  Tokens
                </th>
                <th scope="col" class="end">
                  Cost
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={props.runs}>
                {(r) => (
                  <tr tabindex="0" onClick={() => props.onOpen(r.id)} onKeyDown={(e) => e.key === "Enter" && props.onOpen(r.id)}>
                    <td>
                      <span class="lane-name">
                        <span class="ledger-card">
                          <span class="card-id">#{r.card}</span> {r.card_title}
                        </span>
                        <span class="subject">
                          {r.model ?? "default model"}
                          {r.account ? ` on ${r.account}` : ""}
                        </span>
                      </span>
                    </td>
                    <td class="mono">{r.branch}</td>
                    <td>
                      <span class="chip" data-state={runStateView[r.state].chip}>
                        <Icon name={runStateView[r.state].icon} small />
                        {runStateView[r.state].label}
                      </span>
                    </td>
                    <td class="muted">{ago(props.now - r.started_at * 1000)}</td>
                    <td class="num">{clock(elapsed(r, props.now))}</td>
                    <td class="num">{r.turns ?? 0}</td>
                    <td class="num">{kTokens(r.tokens.input + r.tokens.output + r.tokens.cache_read + r.tokens.cache_write)}</td>
                    <td class="num">{money(r.cost_usd ?? 0)}</td>
                  </tr>
                )}
              </For>
            </tbody>
            <tfoot>
              <tr>
                <th scope="row" colspan="4">
                  All runs
                </th>
                <td class="num">{clock(total().seconds)}</td>
                <td class="num">{total().turns}</td>
                <td class="num">{kTokens(total().tokens)}</td>
                <td class="num">{money(total().cost)}</td>
              </tr>
            </tfoot>
          </table>
        </Show>
      </Show>
    </div>
  );
}
