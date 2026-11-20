// The Lanes screen: every lane grouped by who acts next, each with its place on the track
// (local, remote, integration, production). The Tower is the same lanes, compact, for the other screens.
import { For, Show, createEffect, type JSX } from "solid-js";
import { isLive, isOpenJob, type Lane, type RunMeta, type ShipJob, type Yard } from "./api";
import { clock, elapsed } from "./run";
import { Icon } from "./icons";
import { ago, group, groupOrder, groupTitle, laneView, next, pad2, shipTarget, track, worktreeName, type Group } from "./lane";
import { ErrorLine, Spinner } from "./ui";

export interface LanesProps {
  yard: Yard | null;
  fetching: boolean;
  error: string | null;
  fetchedAt: number | null;
  now: number;
  selected: string | null;
  onSelect: (branch: string) => void;
  onRefresh: () => void;
  onShip: (lane: Lane, to: string) => void;
  /** Remote-only lane: create a local worktree tracking it. */
  onCheckout: (lane: Lane) => void;
  /** Branch being checked out right now. */
  checkingOut: string | null;
  onClean: () => void;
  /** The merge queue of this repository. */
  jobs: ShipJob[];
  onQueueAll: (lanes: Lane[], to: string) => void;
  onDequeue: (id: string) => void;
  onNew: () => void;
}

export function LanesView(props: LanesProps) {
  const rows = new Map<string, HTMLTableRowElement>();
  createEffect(() => {
    const b = props.selected;
    if (b) rows.get(b)?.scrollIntoView({ block: "nearest" });
  });

  const groups = () => {
    const y = props.yard;
    if (!y) return [];
    return groupOrder
      .map((g) => ({ key: g, lanes: y.lanes.filter((l) => queuedGroup(l, props.jobs) === g) }))
      .filter((g) => g.lanes.length > 0);
  };
  const waiting = () => groups().find((g) => g.key === "waiting")?.lanes.length ?? 0;
  const deletable = () => props.yard?.lanes.filter((l) => l.status === "can-delete").length ?? 0;
  const open = () => props.jobs.filter(isOpenJob);
  const shipping = () => open().find((j) => j.state === "shipping");
  /** Lanes ready for production that are not in line yet. */
  const readyForMain = () =>
    (props.yard?.lanes ?? []).filter((l) => l.status === "needs-production" && !open().some((j) => j.branch === l.branch));

  return (
    <div class="lanes">
      <header class="lanes-head">
        <h1 class="title">Lanes</h1>
        <Show when={props.yard && props.yard.lanes.length > 0 ? props.yard : null}>
          {(y) => (
            <p class="summary">
              {y().lanes.length} {y().lanes.length === 1 ? "lane" : "lanes"},{" "}
              <Show when={waiting() > 0} fallback="nothing waiting on you.">
                <b>{waiting()} waiting on you.</b>
              </Show>
              <Show when={open().length > 0}>
                {" "}
                <Show when={shipping()} fallback={`${open().length} in the merge queue.`}>
                  {(j) => `Shipping ${j().branch} to ${j().target}${open().length > 1 ? `, ${open().length - 1} more in line` : ""}.`}
                </Show>
              </Show>
            </p>
          )}
        </Show>
        <span class="spacer" />
        <button
          type="button"
          class="btn btn-ghost"
          onClick={props.onRefresh}
          disabled={props.fetching}
          aria-busy={props.fetching}
          title="Fetch the remote and check every lane again (⌘R)"
        >
          <Show when={props.fetching} fallback={<Icon name="refresh" small />}>
            <Spinner />
          </Show>
          {props.fetching ? "Fetching" : props.fetchedAt ? `Fetched ${ago(props.now - props.fetchedAt)}` : "Fetch"}
        </button>
        <button type="button" class="btn btn-primary" onClick={props.onNew}>
          <Icon name="plus" small />
          New lane <span class="kbd">N</span>
        </button>
      </header>

      <Show when={props.error}>
        {(e) => (
          <ErrorLine>{e()}</ErrorLine>
        )}
      </Show>

      <Show when={props.yard} fallback={<Show when={!props.error}><Skeleton /></Show>}>
        {(y) => (
          <Show when={y().lanes.length > 0} fallback={<Empty base={y().production} onNew={props.onNew} />}>
            <table class="yard">
              <colgroup>
                <col class="c-no" />
                <col class="c-lane" />
                <col class="c-track" />
                <col class="c-wt" />
                <col class="c-next" />
              </colgroup>
              <thead>
                <tr>
                  <th scope="col">
                    <span class="visually-hidden">Lane number</span>
                  </th>
                  <th scope="col">Lane</th>
                  <th scope="col">
                    <span class="rail rail-head">
                      <For each={track(y().lanes[0]!, y(), props.now)}>{(n) => <span>{n.stage}</span>}</For>
                    </span>
                  </th>
                  <th scope="col">Worktree</th>
                  <th scope="col" class="end">
                    Next
                  </th>
                </tr>
              </thead>
              <For each={groups()}>
                {(g) => (
                  <tbody data-group={g.key}>
                    <tr class="group-row">
                      <th scope="rowgroup" colspan="5">
                        <span class="group-head">
                          <GroupTitle group={g.key} count={g.lanes.length} />
                          <Show when={g.key === "waiting" && readyForMain().length > 1}>
                            <button type="button" class="btn" onClick={() => props.onQueueAll(readyForMain(), y().production)}>
                              <Icon name="ship" small />
                              Queue all {readyForMain().length} for {y().production}
                            </button>
                          </Show>
                          <Show when={g.key === "shipped" && deletable() > 0}>
                            <button type="button" class="btn" onClick={props.onClean}>
                              <Icon name="trash" small />
                              Clean up {deletable()}
                            </button>
                          </Show>
                        </span>
                      </th>
                    </tr>
                    <For each={g.lanes}>
                      {(l) => <LaneRow lane={l} yard={y()} props={props} ref={(el) => rows.set(l.branch, el)} />}
                    </For>
                  </tbody>
                )}
              </For>
            </table>
          </Show>
        )}
      </Show>

      <Show when={props.yard && props.yard.only_in_integration.length > 0 ? props.yard : null}>
        {(y) => (
          <section class="strays" aria-labelledby="strays-title">
            <h2 id="strays-title" class="group-head">
              <span class="group-title">Only in {y().integration}</span>
              <span class="count">{y().only_in_integration.length}</span>
            </h2>
            <p class="strays-note">
              Committed straight to {y().integration}, outside any lane, so the flow never carries them to{" "}
              {y().production}.
            </p>
            <ul class="commit-list">
              <For each={y().only_in_integration}>
                {(c) => (
                  <li class="commit">
                    <span class="sha selectable">{c.short}</span>
                    <span class="selectable">{c.subject}</span>
                    <span class="who">
                      {c.author}, {c.days_ago === 0 ? "today" : `${c.days_ago}d ago`}
                    </span>
                  </li>
                )}
              </For>
            </ul>
          </section>
        )}
      </Show>
    </div>
  );
}

/** A lane in the merge queue is in progress, not waiting on you; one the queue stopped on a conflict is. */
export function queuedGroup(l: Lane, jobs: ShipJob[]): Group {
  const mine = jobs.filter((j) => j.branch === l.branch);
  if (mine.some(isOpenJob)) return "progress";
  if (mine.some((j) => j.state === "conflict" || j.state === "failed")) return "waiting";
  return group(l);
}

const ordinal = (n: number) => `${n}${n % 10 === 1 && n % 100 !== 11 ? "st" : n % 10 === 2 && n % 100 !== 12 ? "nd" : n % 10 === 3 && n % 100 !== 13 ? "rd" : "th"}`;

function GroupTitle(props: { group: Group; count: number }) {
  return (
    <>
      <span class="group-title">{groupTitle[props.group]}</span>
      <span class={props.group === "waiting" ? "count count-hot" : "count"}>{props.count}</span>
    </>
  );
}

function LaneRow(p: { lane: Lane; yard: Yard; props: LanesProps; ref: (el: HTMLTableRowElement) => void }) {
  const props = p.props;
  const l = p.lane;
  const needs = () => queuedGroup(l, props.jobs) === "waiting";
  // The lane's place in the merge queue: in line, shipping, or the conflict that stopped it.
  const job = () => {
    const mine = p.props.jobs.filter((j) => j.branch === l.branch);
    return mine.find(isOpenJob) ?? mine.find((j) => j.state === "conflict" || j.state === "failed");
  };
  const place = () => {
    const j = job();
    if (!j || j.state !== "queued") return 0;
    return p.props.jobs.filter((x) => x.state === "queued" && x.queued_at <= j.queued_at).length;
  };
  const target = () => (job() && isOpenJob(job()!) ? null : shipTarget(l, p.yard));
  const n = () => next(l, p.yard);
  const wt = () => worktreeName(l);

  let action: () => JSX.Element = () => null;
  if (!l.local) {
    action = () => (
      <button
        type="button"
        class="btn"
        disabled={p.props.checkingOut === l.branch}
        aria-busy={p.props.checkingOut === l.branch}
        onClick={(e) => {
          e.stopPropagation();
          props.onCheckout(l);
        }}
      >
        <Show when={p.props.checkingOut === l.branch} fallback={<Icon name="branch" small />}>
          <Spinner />
        </Show>
        Check out
      </button>
    );
  }

  return (
    <tr
      ref={p.ref}
      data-needs={needs() ? "" : undefined}
      aria-selected={p.props.selected === l.branch}
      onClick={() => p.props.onSelect(l.branch)}
    >
      <td class="lane-no">{pad2(l.number)}</td>
      <td>
        <span class="lane-name">
          <span class="branch selectable">{l.branch}</span>
          <Show
            when={l.tracks_other}
            fallback={
              <span class="subject" title={l.status === "new" ? undefined : l.subject}>
                {l.status === "new" ? `No commits yet, started from ${p.yard.production}` : l.subject}
              </span>
            }
          >
            {(other) => (
              <span class="subject subject-warn" title={`git pull and git push on this branch go to ${other()}, another branch`}>
                Tracks {other()}, not its own branch
              </span>
            )}
          </Show>
        </span>
      </td>
      <td>
        <span class="rail">
          <For each={track(l, p.yard, p.props.now)}>
            {(node) => (
              <span class="node" data-state={node.state} title={node.title}>
                <span class="dot" aria-hidden="true" />
                <span class="visually-hidden">
                  {node.stage}: {node.title}
                </span>
                <Show when={node.figure}>
                  <span class="fig" aria-hidden="true">
                    {node.figure}
                  </span>
                </Show>
              </span>
            )}
          </For>
        </span>
      </td>
      <td>
        <Show when={wt()}>
          <span class="wt" title={l.worktree!.path} data-dirty={l.worktree!.dirty > 0 ? "" : undefined}>
            {l.worktree!.dirty > 0 ? `${l.worktree!.dirty} changed` : "Clean"}
          </span>
        </Show>
      </td>
      <td>
        <div class="next">
          <Show when={job()}>
            {(j) => (
              <span
                class="next-text"
                data-tone={j().state === "conflict" || j().state === "failed" ? "failed" : "plain"}
                title={j().conflicts.length ? j().conflicts.join("\n") : (j().error ?? undefined)}
              >
                <Show when={j().state === "shipping"}>
                  <Spinner />
                </Show>
                <Show when={j().state === "conflict" || j().state === "failed"}>
                  <Icon name="fail" small />
                </Show>
                {j().state === "shipping"
                  ? `Shipping to ${j().target}`
                  : j().state === "queued"
                    ? `${ordinal(place())} in line for ${j().target}`
                    : j().state === "conflict"
                      ? `Conflicts with ${j().target} in ${j().conflicts.map((c) => c.split("/").pop()).join(", ")}`
                      : `Did not ship to ${j().target}`}
              </span>
            )}
          </Show>
          <Show when={job() && !isOpenJob(job()!) ? job() : null}>
            {(j) => (
              <button
                type="button"
                class="btn"
                title={j().error ?? undefined}
                onClick={(e) => {
                  e.stopPropagation();
                  p.props.onDequeue(j().id);
                }}
              >
                Clear
              </button>
            )}
          </Show>
          <Show when={!job() && n()}>
            {(x) => (
              <span class="next-text" data-tone={x().tone}>
                <Show when={x().tone === "failed"}>
                  <Icon name="fail" small />
                </Show>
                {x().text}
              </span>
            )}
          </Show>
          <Show when={target()}>
            {(t) => (
              <button
                type="button"
                class="btn"
                onClick={(e) => {
                  e.stopPropagation();
                  props.onShip(l, t());
                }}
              >
                <Icon name="ship" small />
                Ship to {t()}
              </button>
            )}
          </Show>
          {action()}
        </div>
      </td>
    </tr>
  );
}

function Empty(props: { base: string; onNew: () => void }) {
  return (
    <div class="empty">
      <div class="empty-title">No lanes yet</div>
      <p class="empty-body">A lane is a branch with its own worktree, started from a fresh origin/{props.base}.</p>
      <button type="button" class="btn" onClick={props.onNew}>
        <Icon name="plus" small />
        Start the first lane
      </button>
    </div>
  );
}

/** Same table, same columns and row height as the loaded yard, so nothing jumps when data lands. */
function Skeleton() {
  return (
    <table class="yard" aria-busy="true" aria-label="Loading lanes">
      <colgroup>
        <col class="c-no" />
        <col class="c-lane" />
        <col class="c-track" />
        <col class="c-wt" />
        <col class="c-next" />
      </colgroup>
      <thead>
        <tr>
          <th scope="col">
            <span class="visually-hidden">Lane number</span>
          </th>
          <th scope="col">Lane</th>
          <th scope="col" />
          <th scope="col">Worktree</th>
          <th scope="col" class="end">
            Next
          </th>
        </tr>
      </thead>
      <tbody>
        <tr class="group-row">
          <th scope="rowgroup" colspan="5">
            <span class="group-head">
              <span class="skeleton keep-motion" style={{ width: "120px" }} />
            </span>
          </th>
        </tr>
        <For each={[0, 1, 2, 3, 4, 5]}>
          {() => (
            <tr class="skeleton-row">
              <td class="lane-no">
                <span class="skeleton keep-motion" style={{ width: "20px", "margin-inline-start": "auto" }} />
              </td>
              <td>
                <span class="lane-name">
                  <span class="skeleton keep-motion" style={{ width: "180px" }} />
                  <span class="skeleton keep-motion" style={{ width: "240px", height: "10px" }} />
                </span>
              </td>
              <td>
                <span class="skeleton keep-motion" style={{ width: "200px", height: "2px", "margin-inline": "auto" }} />
              </td>
              <td />
              <td />
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

// ---------------------------------------------------------------- Tower

export function Tower(props: {
  yard: Yard | null;
  runs: RunMeta[];
  /** Branches whose card waits in review: their lanes wait on you too. */
  reviewing: string[];
  jobs: ShipJob[];
  now: number;
  selected: string | null;
  onSelect: (branch: string) => void;
  onOpenRun: (id: string) => void;
}) {
  // A lane counts once, whether its branch or its running agent is waiting on you.
  const needs = () =>
    props.yard?.lanes.filter(
      (l) =>
        queuedGroup(l, props.jobs) === "waiting" ||
        props.reviewing.includes(l.branch) ||
        props.runs.some((r) => r.branch === l.branch && isLive(r.state) && r.waiting.length > 0),
    ).length ?? 0;
  return (
    <aside class="tower-pane" aria-label="Tower">
      <div class="panel-head">
        <span class="panel-title">Tower</span>
        <span class="panel-meta">{props.yard ? `${props.yard.lanes.length} lanes` : ""}</span>
      </div>
      <div class="tower" role="listbox" aria-label="Lanes">
        <Show when={props.yard && props.yard.lanes.length === 0}>
          <p class="tower-empty">No lanes yet. Branches with their own worktree show here, numbered.</p>
        </Show>
        <For each={props.yard?.lanes ?? []}>
          {(l) => {
            const v = () => laneView(l, props.yard!);
            const waiting = () => queuedGroup(l, props.jobs) === "waiting" || props.reviewing.includes(l.branch);
            const chip = () => (waiting() ? (v().chip === "failed" ? "failed" : "needs") : v().chip);
            const sync = l.sync;
            const run = () => props.runs.find((r) => r.branch === l.branch && isLive(r.state));
            return (
              <div
                class="tower-row"
                role="option"
                tabindex="0"
                data-state={waiting() || (run()?.waiting.length ?? 0) > 0 ? "needs" : v().chip === "done" ? "done" : undefined}
                aria-selected={props.selected === l.branch}
                onClick={() => (run() ? props.onOpenRun(run()!.id) : props.onSelect(l.branch))}
                onKeyDown={(e) => e.key === "Enter" && (run() ? props.onOpenRun(run()!.id) : props.onSelect(l.branch))}
              >
                <span class="tower-lane">{pad2(l.number)}</span>
                <span class="tower-main">
                  <span class="tower-title">{l.branch}</span>
                  <span class="tower-sub">
                    <span>{l.idle_days}d</span>
                    <Show when={l.worktree && l.worktree.dirty > 0}>
                      <span>+{l.worktree!.dirty}</span>
                    </Show>
                    <Show when={sync.kind === "ahead"}>
                      <span>{(sync as { ahead: number }).ahead}↑</span>
                    </Show>
                    <Show when={sync.kind === "behind"}>
                      <span>{(sync as { behind: number }).behind}↓</span>
                    </Show>
                  </span>
                </span>
                <span class="tower-state">
                  <Show
                    when={run()}
                    fallback={
                      <span class="chip" data-state={chip()}>
                        <Icon name={waiting() && v().chip !== "failed" ? "needs" : v().icon} small />
                        {waiting() ? "You" : v().chip === "done" ? "Done" : v().chip === "running" ? "Work" : "Idle"}
                      </span>
                    }
                  >
                    {(r) => (
                      <Show
                        when={r().waiting.length === 0}
                        fallback={
                          <span class="chip" data-state="needs">
                            <Icon name="needs" small />
                            You
                          </span>
                        }
                      >
                        <span class="chip" data-state="running">
                          <Icon name="run" small />
                          <span class="figure">{clock(elapsed(r(), props.now))}</span>
                        </span>
                      </Show>
                    )}
                  </Show>
                </span>
              </div>
            );
          }}
        </For>
      </div>
      <div class="tower-foot">
        <Show when={needs() > 0} fallback="Nothing waiting on you.">
          <b>{needs()}</b> waiting on you.
        </Show>
      </div>
    </aside>
  );
}
