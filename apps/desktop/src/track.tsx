// A lane's track at card size: four stops and the one that matters, in words. On a card on the
// Board, in its sheet and beside a run.
import { For, Show } from "solid-js";
import type { Card, Yard } from "./api";
import { track } from "./lane";
import { t } from "./i18n";

/** The one stop worth naming at card size: your move, a count to settle, or what is wrong. */
function stopNote(nodes: { stage: string; state: string; figure?: string }[]): string | null {
  const n = nodes.find((x) => x.state === "fail") ?? nodes.find((x) => x.state === "next") ?? nodes.find((x) => x.state === "count");
  if (!n) return null;
  if (n.state === "next") return t("next: {stage}", { stage: n.stage });
  if (n.state === "fail") return t("mixed in {stage}", { stage: n.stage });
  return n.figure ?? n.stage;
}

/** The lane's track at card size: four stops and the one that matters, in words. */
export function MiniTrack(props: { card: Card; yard: Yard | null; now: number }) {
  return <LaneTrack branch={props.card.lane} yard={props.yard} now={props.now} />;
}

/** The track of the lane on `branch`, small, with the one stop that matters in words. */
export function LaneTrack(props: { branch: string | undefined; yard: Yard | null; now: number }) {
  const lane = () => (props.branch ? props.yard?.lanes.find((l) => l.branch === props.branch) : undefined);
  return (
    <Show when={lane() && props.yard ? lane() : null}>
      {(l) => (
        <span class="mini-track">
          <span
            class="rail rail-mini"
            role="img"
            aria-label={t("Lane {branch}: {stops}", { branch: l().branch, stops: track(l(), props.yard!, props.now).map((n) => `${n.stage} ${n.state === "on" ? t("reached") : n.state === "off" ? t("not reached") : n.state}`).join(", ") })}
          >
            <For each={track(l(), props.yard!, props.now)}>
              {(n) => (
                <span class="node" data-state={n.state} title={`${n.stage}: ${n.title}`}>
                  <span class="dot" aria-hidden="true" />
                </span>
              )}
            </For>
          </span>
          <Show when={stopNote(track(l(), props.yard!, props.now))}>
            {(note) => <span class="stop-note" aria-hidden="true">{note()}</span>}
          </Show>
        </span>
      )}
    </Show>
  );
}
