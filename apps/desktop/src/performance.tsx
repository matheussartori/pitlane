// Performance: how much of this Mac the agents take. Full while you are away, Light while you use
// it; the daemon's governor pauses, resumes and throttles the agents to fit, without losing a thing.
import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { api, type PerfEvent, type PerfAgent, type PerfMode, type PerfSample, type PerfSnapshot, type PresenceConfig, type PresenceSnapshot } from "./api";
import { Seg } from "./settings";
import { Icon, type IconName } from "./icons";
import { locale, plural, t, tx } from "./i18n";
import { Spinner, toast } from "./ui";
import { msg } from "./format";


/** 1.2 GB, 420 MB. */
export function bytes(n: number): string {
  if (n >= 1000 * 1024 ** 2) return `${(n / 1024 ** 3).toFixed(n >= 10 * 1024 ** 3 ? 0 : 1)} GB`;
  if (n >= 1024 ** 2) return `${Math.round(n / 1024 ** 2)} MB`;
  return `${Math.round(n / 1024)} KB`;
}
const pct = (n: number) => `${Math.round(n)}%`;

const stateView: Record<PerfAgent["state"], { chip: string; icon: IconName; label: string }> = {
  working: { chip: "running", icon: "run", label: "Working" },
  background: { chip: "running", icon: "run", label: "Background" },
  paused: { chip: "queued", icon: "pause", label: "Paused" },
  waiting: { chip: "queued", icon: "queued", label: "Waiting for room" },
  starting: { chip: "queued", icon: "queued", label: "Starting" },
};
const pausedBy = { you: "by you", mode: "by the mode", memory: "for memory" } as const;
const kindLabel: Record<string, string> = { run: "Run", analysis: "Analysis", judge: "Judge", finish: "Closing step", assist: "Assistant" };

export function PerformanceView(props: { onOpen: (id: string) => void }) {
  const [snap, setSnap] = createSignal<PerfSnapshot | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);

  // One read at a time, and only the newest lands: a slow snapshot taken before a change
  // would otherwise put the old setting back on screen.
  let gen = 0;
  let reading = false;
  const load = async (force = false) => {
    if (reading && !force) return;
    const mine = ++gen;
    reading = true;
    try {
      const s = await api.perf();
      if (mine !== gen) return;
      setSnap(s);
      setError(null);
    } catch (e) {
      if (mine === gen) setError(msg(e));
    } finally {
      if (mine === gen) reading = false;
    }
  };
  onMount(() => {
    void load();
    const tick = window.setInterval(() => void load(), 2000);
    onCleanup(() => clearInterval(tick));
  });

  const set = async (patch: Parameters<typeof api.perfSet>[0]) => {
    try {
      await api.perfSet(patch);
      await load(true);
    } catch (e) {
      toast({ tone: "fail", title: t("Performance did not change"), body: msg(e) });
    }
  };
  const hold = async (a: PerfAgent, on: boolean) => {
    setBusy(a.id);
    try {
      await api.perfHold(a.id, on);
      await load(true);
    } catch (e) {
      toast({ tone: "fail", title: on ? t("Could not pause #{n}", { n: a.card }) : t("Could not resume #{n}", { n: a.card }), body: msg(e) });
    } finally {
      setBusy(null);
    }
  };

  const agents = () => snap()?.agents ?? [];
  const cards = () => agents().filter((a) => a.kind === "run");
  const working = () => cards().filter((a) => a.state === "working" || a.state === "background").length;
  const paused = () => cards().filter((a) => a.state === "paused").length;
  const waiting = () => cards().filter((a) => a.state === "waiting").length;

  const summary = () => {
    const s = snap();
    if (!s) return "";
    const mode = s.mode === "full" ? tx("Full", "perf") : tx("Light", "perf");
    const parts = [t("{mode}: {n} at once.", { mode, n: plural(s.allowed, "{n} card", "{n} cards") })];
    if (cards().length === 0) parts.push(t("No agent is working."));
    else parts.push(t("{working} working, {paused} paused, {waiting} waiting.", { working: String(working()), paused: String(paused()), waiting: String(waiting()) }));
    if (s.tight) parts.push(s.critical ? t("Memory is critical: fewer agents until it eases.") : t("Memory is tight: one fewer agent until it eases."));
    return parts.join(" ");
  };

  return (
    <div class="perf">
      <header class="screen-head perf-head">
        <div>
          <h1 class="title">{t("Performance")}</h1>
          <Show when={snap()}>
            <p class="summary">{summary()}</p>
          </Show>
        </div>
        <Show when={snap()}>
          {(s) => (
            <label class="perf-auto">
              <input
                type="checkbox"
                class="switch"
                role="switch"
                checked={s().config.auto}
                onChange={(e) => void set({ auto: e.currentTarget.checked })}
              />
              <span>
                {t("Switch by itself")}
                <small>{t("Light while you use the Mac, Full after {n} min away", { n: String(s().config.away_minutes) })}</small>
              </span>
            </label>
          )}
        </Show>
      </header>

      <Show when={error() && !snap()}>
        <div class="empty">
          <div class="empty-title">{t("Performance is not reachable")}</div>
          <p class="empty-body">{t("The Pitlane daemon did not answer: {why}. It starts with the app; reopen Pitlane if this stays.", { why: error()! })}</p>
        </div>
      </Show>
      <Show when={!error() || snap()}>
        <Show when={snap()} fallback={<div class="loading-line"><Spinner /> {t("Reading this Mac")}</div>}>
          {(s) => (
            <>
              <ModePicker snap={s()} onMode={(mode) => void set({ mode })} onSet={(p) => void set(p)} />
              <PresencePicker />
              <Show when={s().system}>{(sys) => <MacPanel snap={s()} sys={sys()} />}</Show>
              <AgentsTable agents={agents()} memTotal={s().system?.mem_total ?? 1} busy={busy()} onHold={hold} onOpen={props.onOpen} />
              <div class="perf-foot">
                <Others snap={s()} />
                <Log snap={s()} />
              </div>
            </>
          )}
        </Show>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------- modes

function ModePicker(props: { snap: PerfSnapshot; onMode: (m: PerfMode) => void; onSet: (p: Parameters<typeof api.perfSet>[0]) => void }) {
  const c = () => props.snap.config;
  const max = () => props.snap.max ?? 3;
  const Tile = (p: { mode: PerfMode; title: string; line: string; facts: JSX.Element }) => (
    <button type="button" class="perf-mode" aria-pressed={props.snap.mode === p.mode} onClick={() => props.onMode(p.mode)}>
      <span class="perf-mode-head">
        <span class="perf-mode-title">{tx(p.title, "perf")}</span>
        <Show when={props.snap.mode === p.mode}>
          <span class="chip" data-state="review">
            <Icon name="done" small />
            {c().auto ? t("On, by itself") : t("On")}
          </span>
        </Show>
      </span>
      <span class="perf-mode-line">{t(p.line)}</span>
      <span class="perf-mode-facts">{p.facts}</span>
    </button>
  );
  return (
    <section class="perf-modes" aria-label={t("Mode")}>
      <Tile
        mode="full"
        title="Full"
        line="You are away: the agents take the Mac, and it stays awake while they work."
        facts={
          <>
            <span>{plural(max(), "{n} card at once", "{n} cards at once")}</span>
            <span>{t("full priority")}</span>
            <span>{t("pauses only when memory runs out")}</span>
          </>
        }
      />
      <Tile
        mode="light"
        title="Light"
        line="You are using the Mac: fewer agents, on the efficiency cores, so nothing you do stutters."
        facts={
          <>
            <span class="perf-inline" onClick={(e) => e.stopPropagation()}>
              <input
                class="input perf-num"
                type="number"
                min="1"
                max={max()}
                aria-label={t("Cards at once in Light")}
                value={Math.min(c().light_agents, max())}
                onChange={(e) => props.onSet({ light_agents: Math.max(1, Math.round(Number(e.currentTarget.value)) || 1) })}
              />
              {t("at once")}
            </span>
            <span class="perf-inline" onClick={(e) => e.stopPropagation()}>
              <input type="checkbox" class="switch" role="switch" aria-label={t("Background priority")} checked={c().light_background} onChange={(e) => props.onSet({ light_background: e.currentTarget.checked })} />
              {t("background priority")}
            </span>
            <span>{t("pauses early when memory gets tight")}</span>
          </>
        }
      />
      <Show when={c().auto}>
        <p class="perf-note">
          {t("Full after")}{" "}
          <input
            class="input perf-num"
            type="number"
            min="1"
            max="120"
            aria-label={t("Minutes away before Full")}
            value={c().away_minutes}
            onChange={(e) => props.onSet({ away_minutes: Math.max(1, Math.round(Number(e.currentTarget.value)) || 10) })}
          />{" "}
          {t("min without a key or the mouse. You have been away {t}.", { t: away(props.snap.idle_secs ?? 0) })}
        </p>
      </Show>
    </section>
  );
}

// ---------------------------------------------------------------- presence

/** Here: a run asks you the calls that are yours. Away: it takes its recommendation. */
function PresencePicker() {
  const [p, setP] = createSignal<PresenceSnapshot | null>(null);
  let gen = 0;
  const load = async () => {
    const mine = ++gen;
    try {
      const s = await api.presence();
      if (mine === gen) setP(s);
    } catch {
      /* The daemon's own error shows above. */
    }
  };
  onMount(() => {
    void load();
    const tick = window.setInterval(() => void load(), 5000);
    onCleanup(() => clearInterval(tick));
  });
  const set = async (patch: Partial<PresenceConfig>) => {
    try {
      const s = await api.presenceSet(patch);
      ++gen;
      setP(s);
    } catch (e) {
      toast({ tone: "fail", title: t("Presence did not change"), body: msg(e) });
    }
  };
  const num = (v: string, fallback: number) => Math.max(1, Math.round(Number(v)) || fallback);
  return (
    <Show when={p()}>
      {(s) => (
        <section class="presence" aria-label={t("Presence")}>
          <div class="presence-head">
            <h2 class="presence-title">{t("Presence")}</h2>
            <Seg
              label="Presence"
              value={s().mode}
              options={[
                { v: "here", label: tx("Here", "presence") },
                { v: "away", label: tx("Away", "presence") },
              ]}
              onChange={(mode) => void set({ mode })}
            />
            <label class="perf-auto">
              <input type="checkbox" class="switch" role="switch" checked={s().config.auto} onChange={(e) => void set({ auto: e.currentTarget.checked })} />
              <span>
                {t("Switch by itself")}
                <small>{t("Here while you use the Mac, Away after {n} min away", { n: String(s().away_minutes) })}</small>
              </span>
            </label>
          </div>
          <p class="presence-line">
            {s().mode === "here"
              ? t("You are here: an agent asks you the calls that are yours, and takes its recommendation when no answer comes in time.")
              : t("You are away: agents take their own recommendation and stop only for what they cannot do or what is costly to undo. What they decide for you is on the card.")}
          </p>
          <p class="perf-note">
            {t("Wait")}{" "}
            <input class="input perf-num" type="number" min="1" max="120" aria-label={t("Minutes to wait for an answer")} value={s().config.wait_minutes} onChange={(e) => void set({ wait_minutes: num(e.currentTarget.value, 10) })} />{" "}
            {t("min for an answer, at most")}{" "}
            <input class="input perf-num" type="number" min="1" max="20" aria-label={t("Questions per run")} value={s().config.per_run} onChange={(e) => void set({ per_run: num(e.currentTarget.value, 3) })} />{" "}
            {t("questions per run.")}
          </p>
        </section>
      )}
    </Show>
  );
}

function away(secs: number): string {
  if (secs < 60) return t("{n} s", { n: String(Math.floor(secs)) });
  return t("{n} min", { n: String(Math.floor(secs / 60)) });
}

// ---------------------------------------------------------------- the Mac

function MacPanel(props: { snap: PerfSnapshot; sys: NonNullable<PerfSnapshot["system"]> }) {
  const s = () => props.sys;
  const agentsMem = () => props.snap.agents_mem ?? 0;
  const othersMem = () => Math.max(0, s().mem_used - agentsMem());
  const pressure = () =>
    s().pressure === "critical"
      ? { chip: "failed", icon: "fail" as IconName, label: "Critical" }
      : s().pressure === "warn"
        ? { chip: "needs", icon: "needs" as IconName, label: "Tight" }
        : { chip: "done", icon: "done" as IconName, label: "Normal" };
  return (
    <section class="perf-mac" aria-label={t("This Mac")}>
      <div class="perf-tile perf-tile-wide">
        <div class="perf-tile-head">
          <span>{t("Memory")}</span>
          <span class="chip" data-state={pressure().chip}>
            <Icon name={pressure().icon} small />
            {t("Pressure: {level}", { level: t(pressure().label) })}
          </span>
        </div>
        <div class="perf-big">
          {bytes(s().mem_used)} <small>{t("of {total}", { total: bytes(s().mem_total) })}</small>
        </div>
        <div class="perf-stack" role="img" aria-label={t("Agents {a}, everything else {o}, free {f}", { a: bytes(agentsMem()), o: bytes(othersMem()), f: pct(s().free_pct) })}>
          <span class="perf-stack-agents" style={{ "flex-grow": agentsMem() }} />
          <span class="perf-stack-others" style={{ "flex-grow": othersMem() }} />
          <span class="perf-stack-free" style={{ "flex-grow": Math.max(0, s().mem_total - s().mem_used) }} />
        </div>
        <div class="perf-legend">
          <span><i class="sw sw-agents" />{t("Agents")} <b>{bytes(agentsMem())}</b></span>
          <span><i class="sw sw-others" />{t("Everything else")} <b>{bytes(othersMem())}</b></span>
          <span><i class="sw sw-free" />{t("Free")} <b>{pct(s().free_pct)}</b></span>
        </div>
      </div>
      <div class="perf-tile">
        <div class="perf-tile-head">
          <span>{t("CPU")}</span>
          <span class="mono muted">{t("load {l} on {c} cores", { l: s().load.toFixed(1), c: String(s().cores) })}</span>
        </div>
        <div class="perf-big">{pct(s().cpu)}</div>
        <div class="perf-sub">{t("Agents: {n} of one core", { n: pct(props.snap.agents_cpu ?? 0) })}</div>
      </div>
      <div class="perf-tile">
        <div class="perf-tile-head">
          <span>{t("Swap")}</span>
        </div>
        <div class="perf-big">{bytes(s().swap_used)}</div>
        <div class="perf-sub">{s().swap_used > 1024 ** 3 ? t("The Mac is paging to disk: it feels slower.") : t("Little or none: memory fits.")}</div>
      </div>
      <div class="perf-tile">
        <div class="perf-tile-head">
          <span>{t("This Mac")}</span>
        </div>
        <div class="perf-facts">
          <span>{props.snap.background ? t("Agents at background priority") : t("Agents at full priority")}</span>
          <span>{props.snap.awake ? t("Kept awake while agents work") : t("Sleeps as usual")}</span>
          <span>{props.snap.adaptive ? t("Fewer agents while the Mac is loaded") : t("Fixed number of agents (Settings)")}</span>
        </div>
      </div>
      <Chart title="Memory" unit="bytes" history={props.snap.history} total={s().mem_total} mac={(h) => h.mem_used} agents={(h) => h.agents_mem} />
      <Chart title="CPU" unit="pct" history={props.snap.history} total={100} mac={(h) => h.cpu} agents={(h) => Math.min(100, h.agents_cpu / Math.max(1, s().cores))} />
    </section>
  );
}

/** The last minutes as two lines on one axis: the whole Mac and the agents' share of it. */
function Chart(props: { title: string; unit: "bytes" | "pct"; history: PerfSample[]; total: number; mac: (h: PerfSample) => number; agents: (h: PerfSample) => number }) {
  const W = 600;
  const H = 120;
  const [hover, setHover] = createSignal<number | null>(null);
  const span = 360; // seconds shown
  const pts = () => props.history;
  const end = () => (pts().length ? pts()[pts().length - 1]!.t : 0);
  const x = (tt: number) => W - ((end() - tt) / span) * W;
  const y = (v: number) => H - 2 - (Math.min(v, props.total) / Math.max(1, props.total)) * (H - 4);
  const line = (f: (h: PerfSample) => number) => pts().map((h, i) => `${i ? "L" : "M"}${x(h.t).toFixed(1)},${y(f(h)).toFixed(1)}`).join("");
  const area = (f: (h: PerfSample) => number) => (pts().length ? `${line(f)}L${x(end()).toFixed(1)},${H}L${x(pts()[0]!.t).toFixed(1)},${H}Z` : "");
  const fmt = (v: number) => (props.unit === "bytes" ? bytes(v) : pct(v));
  const at = () => (hover() == null ? null : pts()[hover()!]);
  const onMove = (e: PointerEvent) => {
    const r = (e.currentTarget as SVGElement).getBoundingClientRect();
    const tt = end() - (1 - (e.clientX - r.left) / r.width) * span;
    let best = -1;
    let d = Infinity;
    pts().forEach((h, i) => {
      if (Math.abs(h.t - tt) < d) {
        d = Math.abs(h.t - tt);
        best = i;
      }
    });
    setHover(best >= 0 ? best : null);
  };
  return (
    <figure class="perf-chart">
      <figcaption class="perf-tile-head">
        <span>{t(props.title)}</span>
        <span class="perf-legend">
          <span><i class="sw sw-mac" />{t("This Mac")}</span>
          <span><i class="sw sw-agents" />{t("Agents")}</span>
        </span>
      </figcaption>
      <div class="perf-plot">
        <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" onPointerMove={onMove} onPointerLeave={() => setHover(null)} role="img" aria-label={t("{title} over the last 6 minutes", { title: t(props.title) })}>
          <line class="perf-grid" x1="0" x2={W} y1={y(props.total / 2)} y2={y(props.total / 2)} />
          <path class="perf-area-agents" d={area(props.agents)} />
          <path class="perf-line-mac" d={line(props.mac)} />
          <path class="perf-line-agents" d={line(props.agents)} />
          <Show when={at()}>{(h) => <line class="perf-cross" x1={x(h().t)} x2={x(h().t)} y1="0" y2={H} />}</Show>
        </svg>
        <Show when={at()}>
          {(h) => (
            <div class="perf-tip" style={{ left: `${(x(h().t) / W) * 100}%` }}>
              <span class="muted">{new Date(h().t * 1000).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</span>
              <span><i class="sw sw-mac" />{t("This Mac")} <b>{fmt(props.mac(h()))}</b></span>
              <span><i class="sw sw-agents" />{t("Agents")} <b>{fmt(props.agents(h()))}</b></span>
            </div>
          )}
        </Show>
        <span class="perf-axis perf-axis-top">{fmt(props.total)}</span>
        <span class="perf-axis perf-axis-mid">{fmt(props.total / 2)}</span>
      </div>
      <div class="perf-axis-x">
        <span>{t("6 min ago")}</span>
        <span>{t("now")}</span>
      </div>
    </figure>
  );
}

// ---------------------------------------------------------------- agents

function AgentsTable(props: { agents: PerfAgent[]; memTotal: number; busy: string | null; onHold: (a: PerfAgent, on: boolean) => void; onOpen: (id: string) => void }) {
  const total = () => ({ mem: props.agents.reduce((n, a) => n + a.mem, 0), cpu: props.agents.reduce((n, a) => n + a.cpu, 0), procs: props.agents.reduce((n, a) => n + a.procs, 0) });
  return (
    <section class="perf-agents" aria-label={t("Agents")}>
      <h2 class="perf-h2">{t("Agents")}</h2>
      <Show
        when={props.agents.length > 0}
        fallback={
          <div class="empty">
            <div class="empty-title">{t("No agent is working")}</div>
            <p class="empty-body">{t("Every Claude session Pitlane runs (cards, analyses, the judge) shows here with what it and its commands take. Start one from the Board.")}</p>
          </div>
        }
      >
        <table class="table perf-table">
          <thead>
            <tr>
              <th scope="col">{t("Session")}</th>
              <th scope="col">{t("State")}</th>
              <th scope="col">{tx("Running", "perf")}</th>
              <th scope="col" class="end">{t("CPU")}</th>
              <th scope="col" class="end">{t("Memory")}</th>
              <th scope="col" class="end"><span class="visually-hidden">{t("Pause or resume")}</span></th>
            </tr>
          </thead>
          <tbody>
            <For each={props.agents}>
              {(a) => (
                <tr tabindex="0" onClick={() => props.onOpen(a.id)} onKeyDown={(e) => e.key === "Enter" && props.onOpen(a.id)}>
                  <td>
                    <span class="lane-name">
                      <span class="ledger-card">
                        <span class="card-id">#{a.card}</span> {a.title}
                      </span>
                      <span class="subject">
                        {a.kind === "run" ? tx("Run", "kind") : t(kindLabel[a.kind] ?? a.kind)} · <span class="mono">{a.branch || a.model || ""}</span>
                      </span>
                    </span>
                  </td>
                  <td>
                    <span class="chip" data-state={stateView[a.state].chip}>
                      <Icon name={stateView[a.state].icon} small />
                      {t(stateView[a.state].label)}
                      <Show when={a.state === "paused" && a.paused_by}>{(by) => <span class="muted"> {t(pausedBy[by()])}</span>}</Show>
                    </span>
                  </td>
                  <td>
                    <div class="perf-procs">
                    <Show when={a.top.length > 0} fallback={<span class="muted">{t("not started")}</span>}>
                      <For each={a.top}>
                        {(p) => (
                          <span class="mono">
                            {p.name} <span class="muted">{bytes(p.mem)}</span>
                          </span>
                        )}
                      </For>
                      <Show when={a.procs > a.top.length}>
                        <span class="muted">{t("+{n} more", { n: String(a.procs - a.top.length) })}</span>
                      </Show>
                    </Show>
                    </div>
                  </td>
                  <td class="num">{pct(a.cpu)}</td>
                  <td class="num perf-mem">
                    <span>{bytes(a.mem)}</span>
                    <span class="perf-bar" aria-hidden="true">
                      <span style={{ "--v": String(Math.min(1, a.mem / props.memTotal)) }} />
                    </span>
                  </td>
                  <td class="end">
                    <Show when={a.pid != null && a.state !== "waiting"}>
                      <button
                        type="button"
                        class="btn btn-ghost perf-hold"
                        disabled={props.busy === a.id}
                        aria-busy={props.busy === a.id}
                        title={a.state === "paused" ? t("Continue where it was") : t("Freeze it in place; nothing is lost")}
                        onClick={(e) => {
                          e.stopPropagation();
                          props.onHold(a, a.state !== "paused");
                        }}
                      >
                        <Icon name={a.state === "paused" ? "play" : "pause"} small />
                        {a.state === "paused" ? t("Resume") : t("Pause")}
                      </button>
                    </Show>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
          <tfoot>
            <tr>
              <th scope="row" colspan="2">{t("All agents")}</th>
              <td class="muted">{plural(total().procs, "{n} process", "{n} processes")}</td>
              <td class="num">{pct(total().cpu)}</td>
              <td class="num">{bytes(total().mem)}</td>
              <td />
            </tr>
          </tfoot>
        </table>
      </Show>
    </section>
  );
}

// ---------------------------------------------------------------- the rest

function Others(props: { snap: PerfSnapshot }) {
  const top = () => props.snap.others ?? [];
  const most = () => Math.max(1, ...top().map((o) => o.mem));
  return (
    <section class="perf-others" aria-label={t("Everything else")}>
      <h2 class="perf-h2">{t("Everything else on this Mac")}</h2>
      <ul class="perf-list">
        <For each={top()}>
          {(o) => (
            <li>
              <span class="perf-list-name">{o.name}</span>
              <span class="perf-bar" aria-hidden="true">
                <span class="perf-bar-other" style={{ "--v": String(o.mem / most()) }} />
              </span>
              <span class="num">{bytes(o.mem)}</span>
              <span class="num muted">{pct(o.cpu)}</span>
            </li>
          )}
        </For>
      </ul>
    </section>
  );
}

function eventText(e: PerfEvent): string {
  const card = e.card != null ? `#${e.card} ${e.title ?? ""}`.trim() : "";
  const n = e.n ?? 0;
  switch (e.kind) {
    case "full":
      return t("Full: the agents take the Mac");
    case "light":
      return t("Light: room left for you");
    case "away":
      return t("Full: you have been away {n} min", { n });
    case "back":
      return t("Light: you are back");
    case "paused_mode":
      return t("Paused {card}: the mode lets {n} at once", { card, n: plural(n, "{n} card", "{n} cards") });
    case "paused_memory":
      return t("Paused {card}: memory is tight ({n}% free)", { card, n });
    case "resumed":
      return t("Resumed {card} after {t}", { card, t: n < 60 ? t("{n} s", { n }) : t("{n} min", { n: Math.round(n / 60) }) });
    case "paused_you":
      return t("You paused {card}", { card });
    case "resumed_you":
      return t("You resumed {card}", { card });
  }
}

function Log(props: { snap: PerfSnapshot }) {
  return (
    <section class="perf-log" aria-label={t("What Performance did")}>
      <h2 class="perf-h2">{t("What Performance did")}</h2>
      <Show when={props.snap.events.length > 0} fallback={<p class="muted perf-quiet">{t("Nothing yet: every pause, resume and mode change lands here with its reason.")}</p>}>
        <ul class="perf-events">
          <For each={props.snap.events.slice(0, 12)}>
            {(e) => (
              <li>
                <span class="mono muted">{new Date(e.t * 1000).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" })}</span>
                <span>{eventText(e)}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}
