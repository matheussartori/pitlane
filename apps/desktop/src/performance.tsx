// Performance: how much of this Mac the agents take. Full while you are away, Light while you use
// it; the daemon's governor pauses, resumes and throttles the agents to fit, without losing a thing.
import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api, type PerfEvent, type PerfAgent, type PerfSample, type PerfSnapshot, type PresenceConfig, type PresenceSnapshot } from "./api";
import { Icon, type IconName } from "./icons";
import { locale, plural, t, tx } from "./i18n";
import { Seg, Spinner, toast } from "./ui";
import { msg, timeOfDay } from "./format";
import { KIND } from "./store/agents";


/** 1.2 GB, 420 MB. */
function bytes(n: number): string {
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
const pausedBy = { you: "by you", mode: "by the mode", memory: "for memory", heat: "for heat" } as const;

export function PerformanceView(props: { onOpen: (id: string) => void }) {
  // A store, reconciled by id: the figures change every two seconds, and each row takes its new
  // numbers in place instead of the whole list being rebuilt under the cursor.
  const [read, setRead] = createStore<{ snap: PerfSnapshot | null }>({ snap: null });
  const snap = () => read.snap;
  const setSnap = (s: PerfSnapshot) => setRead("snap", reconcile(s, { key: "id" }));
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
    for (const p of ["cpu", "gpu", "battery"] as Part[]) {
      const v = s.temps?.[p];
      if (v != null && levelOf(p, v) === "critical") parts.push(t("{part} is too hot ({v}).", { part: partName(p), v: celsius(v) }));
    }
    if (s.tight) parts.push(s.critical ? t("Memory is critical: fewer agents until it eases.") : t("Memory is tight: one fewer agent until it eases."));
    if (s.hot) parts.push(s.hot_critical ? t("The Mac is overheating: one agent until it cools.") : t("The Mac is too warm: one fewer agent until it cools."));
    return parts.join(" ");
  };

  return (
    <div class="perf">
      <header class="screen-head perf-head">
        <h1 class="title">{t("Performance")}</h1>
        <Show when={snap()}>
          <p class="summary">{summary()}</p>
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
              <div class="perf-controls">
                <ModeCard snap={s()} onSet={(p) => void set(p)} />
                <PresenceCard />
              </div>
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

// ---------------------------------------------------------------- settings

/** Mode and Presence share one shape: a title, the two choices, "by itself", what the choice
 *  means, then one row per setting with its label on the left. */
function SettingsCard(props: { title: string; seg: JSX.Element; auto: boolean; onAuto: (on: boolean) => void; line: string; children: JSX.Element; foot?: JSX.Element }) {
  return (
    <section class="perf-card" aria-label={t(props.title)}>
      <header class="perf-card-head">
        <h2 class="perf-card-title">{t(props.title)}</h2>
        {props.seg}
        <label class="perf-auto">
          <input type="checkbox" class="switch" role="switch" checked={props.auto} onChange={(e) => props.onAuto(e.currentTarget.checked)} />
          {t("Switch by itself")}
        </label>
      </header>
      <p class="perf-card-line">{props.line}</p>
      <dl class="perf-rows">{props.children}</dl>
      <Show when={props.foot}>
        <p class="perf-card-foot">{props.foot}</p>
      </Show>
    </section>
  );
}

function Row(props: { label: string; on?: boolean; children: JSX.Element }) {
  return (
    <div class="perf-row" data-on={props.on ? "" : undefined}>
      <dt>{props.label}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

function NumInput(props: { value: number; min: number; max: number; label: string; fallback: number; onSet: (n: number) => void }) {
  return (
    <input
      class="input perf-num"
      type="number"
      min={props.min}
      max={props.max}
      aria-label={t(props.label)}
      value={props.value}
      onChange={(e) => props.onSet(Math.min(props.max, Math.max(props.min, Math.round(Number(e.currentTarget.value)) || props.fallback)))}
    />
  );
}

function ModeCard(props: { snap: PerfSnapshot; onSet: (p: Parameters<typeof api.perfSet>[0]) => void }) {
  const c = () => props.snap.config;
  const max = () => props.snap.max ?? 3;
  const mode = () => props.snap.mode;
  return (
    <SettingsCard
      title="Mode"
      seg={
        <Seg
          label="Mode"
          value={mode()}
          options={[
            { v: "light", label: tx("Light", "perf") },
            { v: "full", label: tx("Full", "perf") },
          ]}
          onChange={(m) => props.onSet({ mode: m })}
        />
      }
      auto={c().auto}
      onAuto={(on) => props.onSet({ auto: on })}
      line={
        mode() === "full"
          ? t("You are away: the agents take the Mac, and it stays awake while they work.")
          : t("You are using the Mac: fewer agents, on the efficiency cores, so nothing you do stutters.")
      }
      foot={
        <>
          <span>{props.snap.background ? t("Agents at background priority") : t("Agents at full priority")}</span>
          <span>{props.snap.awake ? t("Kept awake while agents work") : t("Sleeps as usual")}</span>
          <span>{props.snap.adaptive ? t("Fewer agents while the Mac is loaded") : t("Fixed number of agents (Settings)")}</span>
        </>
      }
    >
      <Row label={tx("Light", "perf")} on={mode() === "light"}>
        <span class="perf-inline">
          <NumInput value={Math.min(c().light_agents, max())} min={1} max={max()} label="Cards at once in Light" fallback={1} onSet={(n) => props.onSet({ light_agents: n })} />
          {t("at once")}
        </span>
        <label class="perf-inline">
          <input type="checkbox" class="switch" role="switch" checked={c().light_background} onChange={(e) => props.onSet({ light_background: e.currentTarget.checked })} />
          {t("background priority")}
        </label>
        <span class="muted">{t("pauses early when memory gets tight")}</span>
      </Row>
      <Row label={tx("Full", "perf")} on={mode() === "full"}>
        <span>{plural(max(), "{n} card at once", "{n} cards at once")}</span>
        <span>{t("full priority")}</span>
        <span class="muted">{t("pauses only when memory runs out")}</span>
      </Row>
      <Row label={t("By itself")}>
        <span class="perf-inline">
          <span>{t("Full after")}</span>
          <NumInput value={c().away_minutes} min={1} max={120} label="Minutes away before Full" fallback={10} onSet={(n) => props.onSet({ away_minutes: n })} />
          {t("min without a key or the mouse.")}
        </span>
        <span class="muted">{t("Away for {t}", { t: away(props.snap.idle_secs ?? 0) })}</span>
      </Row>
    </SettingsCard>
  );
}

/** Here: a run asks you the calls that are yours. Away: it takes its recommendation. */
function PresenceCard() {
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
  return (
    <Show when={p()}>
      {(s) => (
        <SettingsCard
          title="Presence"
          seg={
            <Seg
              label="Presence"
              value={s().mode}
              options={[
                { v: "here", label: tx("Here", "presence") },
                { v: "away", label: tx("Away", "presence") },
              ]}
              onChange={(mode) => void set({ mode })}
            />
          }
          auto={s().config.auto}
          onAuto={(auto) => void set({ auto })}
          line={
            s().mode === "here"
              ? t("You are here: an agent asks you the calls that are yours, and takes its recommendation when no answer comes in time.")
              : t("You are away: agents take their own recommendation and stop only for what they cannot do or what is costly to undo. What they decide for you is on the card.")
          }
        >
          <Row label={t("Wait")}>
            <span class="perf-inline">
              <NumInput value={s().config.wait_minutes} min={1} max={120} label="Minutes to wait for an answer" fallback={10} onSet={(n) => void set({ wait_minutes: n })} />
              {t("min for an answer")}
            </span>
          </Row>
          <Row label={t("Questions")}>
            <span class="perf-inline">
              <span>{t("at most")}</span>
              <NumInput value={s().config.per_run} min={1} max={20} label="Questions per run" fallback={3} onSet={(n) => void set({ per_run: n })} />
              {t("per run")}
            </span>
          </Row>
          <Row label={t("By itself")}>
            <span>{t("Here while you use the Mac, Away after {n} min away", { n: String(s().away_minutes) })}</span>
          </Row>
        </SettingsCard>
      )}
    </Show>
  );
}

function away(secs: number): string {
  if (secs < 60) return t("{n} s", { n: String(Math.floor(secs)) });
  return t("{n} min", { n: String(Math.floor(secs / 60)) });
}

// ---------------------------------------------------------------- the Mac

type Share = { cls: string; v: number };

/** Every tile reads the same way: label and badge, the figure, a bar, then one row per share. */
function Tile(props: { label: string; badge?: JSX.Element; big: JSX.Element; small?: string; bar?: Share[]; barLabel?: string; children?: JSX.Element }) {
  return (
    <div class="perf-tile">
      <div class="perf-tile-head">
        <span>{t(props.label)}</span>
        {props.badge}
      </div>
      <div class="perf-big">
        {props.big}
        <Show when={props.small}>
          <small>{props.small}</small>
        </Show>
      </div>
      <div class="perf-stack" role={props.barLabel ? "img" : undefined} aria-label={props.barLabel} aria-hidden={props.barLabel ? undefined : "true"}>
        <For each={props.bar ?? []}>{(b) => <span class={b.cls} style={{ "flex-grow": Math.max(0, b.v) }} />}</For>
      </div>
      <div class="perf-legend">{props.children}</div>
    </div>
  );
}

function Leg(props: { sw?: string; label: string; value: string }) {
  return (
    <span>
      <Show when={props.sw} fallback={<i class="sw sw-none" />}>
        <i class={`sw ${props.sw}`} />
      </Show>
      {props.label}
      <b>{props.value}</b>
    </span>
  );
}

const thermalView = {
  nominal: { chip: "done", icon: "done", label: "Normal" },
  fair: { chip: "needs", icon: "needs", label: "Moderate" },
  serious: { chip: "needs", icon: "needs", label: "High" },
  critical: { chip: "failed", icon: "fail", label: "Critical" },
} as const satisfies Record<string, { chip: string; icon: IconName; label: string }>;

/** Where each part goes from fine to hot to too hot, in °C. Apple publishes no die limit:
 *  under sustained load Apple silicon settles near 100 °C and throttles from there (M3 Air
 *  stress tests peak at 107 °C CPU, 103 °C GPU), and 90 °C is where tools start to warn. The
 *  GPU shares the die, so it shares the CPU's limits. Apple gives 10–35 °C as the room to work
 *  in and 45 °C as the top even for storage, and a cell over 40 °C wears faster. */
const TEMP_LIMITS = {
  cpu: { high: 85, critical: 100 },
  gpu: { high: 85, critical: 100 },
  battery: { high: 40, critical: 45 },
} as const;
type Part = keyof typeof TEMP_LIMITS;
type Level = "ok" | "high" | "critical";
const levelOf = (part: Part, v: number): Level => (v >= TEMP_LIMITS[part].critical ? "critical" : v >= TEMP_LIMITS[part].high ? "high" : "ok");
const rank = { ok: 0, high: 1, critical: 2 } as const;
const partName = (p: Part) => (p === "battery" ? t("Battery") : p.toUpperCase());

const celsius = (v: number) => `${Math.round(v)} °C`;

/** The temperature tile: the hottest part big, every part on its row marked by its own limits,
 *  and a line saying what to do when one runs hot. */
function TempTile(props: { temps: NonNullable<PerfSnapshot["temps"]> }) {
  const parts = () =>
    (["cpu", "gpu", "battery"] as Part[])
      .filter((p) => props.temps[p] != null)
      .map((p) => ({ part: p, v: props.temps[p]!, level: levelOf(p, props.temps[p]!) }));
  // The part in the worst state leads; while all are fine, the CPU does (the first listed).
  const worst = () => parts().reduce((a, b) => (rank[b.level] > rank[a.level] ? b : a));
  const level = (): Level => {
    const own = parts().reduce<Level>((l, p) => (rank[p.level] > rank[l] ? p.level : l), "ok");
    const th = props.temps.thermal;
    const mac: Level = th === "critical" ? "critical" : th === "serious" ? "high" : "ok";
    return rank[mac] > rank[own] ? mac : own;
  };
  const badge = () =>
    level() === "critical"
      ? { chip: "failed", icon: "fail" as IconName, label: "Critical" }
      : level() === "high"
        ? { chip: "needs", icon: "needs" as IconName, label: "High" }
        : props.temps.thermal === "fair"
          ? thermalView.fair
          : thermalView.nominal;
  // The bar runs from a cold Mac (30 °C) to the leading part's critical limit.
  const fill = () => Math.min(1, Math.max(0.02, (worst().v - 30) / (TEMP_LIMITS[worst().part].critical - 30)));
  // Apple silicon runs its dies near 100 °C under load by design; whether the Mac is actually
  // being slowed down is macOS's thermal pressure, which is also what the governor acts on.
  const slowing = () => props.temps.thermal === "serious" || props.temps.thermal === "critical";
  const note = () => {
    const w = parts().filter((p) => p.level !== "ok").sort((a, b) => rank[b.level] - rank[a.level])[0];
    if (w?.level === "critical")
      return w.part === "battery"
        ? t("The battery is past {n}: unplug it or give it air; heat this high wears it for good.", { n: celsius(TEMP_LIMITS.battery.critical) })
        : slowing()
          ? t("{part} is at its limit and macOS is slowing the Mac down: Pitlane runs fewer agents until it cools.", { part: partName(w.part) })
          : t("{part} is at the top of its range: macOS slows the Mac down if it stays there.", { part: partName(w.part) });
    if (w?.level === "high")
      return w.part === "battery"
        ? t("The battery runs warm: fine for a while, but it ages faster above {n}.", { n: celsius(TEMP_LIMITS.battery.high) })
        : t("{part} runs hot but within limits: normal under heavy load.", { part: partName(w.part) });
    if (slowing()) return t("macOS says the Mac is too warm and is slowing it down: Pitlane runs fewer agents until it cools.");
    return t("Within limits.");
  };
  return (
    <Tile
      label="Temperature"
      badge={
        <span class="chip" data-state={badge().chip}>
          <Icon name={badge().icon} small />
          {t(badge().label)}
        </span>
      }
      big={celsius(worst().v)}
      small={partName(worst().part)}
      bar={[
        { cls: `perf-stack-heat-${levelOf(worst().part, worst().v)}`, v: fill() },
        { cls: "perf-stack-free", v: 1 - fill() },
      ]}
      barLabel={t("{part} at {v}, critical from {c}", { part: partName(worst().part), v: celsius(worst().v), c: celsius(TEMP_LIMITS[worst().part].critical) })}
    >
      <For each={parts()}>
        {(p) => (
          <span data-level={p.level} title={t("High from {h}, critical from {c}", { h: celsius(TEMP_LIMITS[p.part].high), c: celsius(TEMP_LIMITS[p.part].critical) })}>
            <i class={`sw sw-heat-${p.level}`} />
            {partName(p.part)}
            <b>{celsius(p.v)}</b>
          </span>
        )}
      </For>
      <span class="perf-tile-note" data-level={level()}>{note()}</span>
    </Tile>
  );
}

function MacPanel(props: { snap: PerfSnapshot; sys: NonNullable<PerfSnapshot["system"]> }) {
  const s = () => props.sys;
  const agentsMem = () => props.snap.agents_mem ?? 0;
  const othersMem = () => Math.max(0, s().mem_used - agentsMem());
  // CPU as a share of the whole Mac, the way the chart draws it, not "648% of one core".
  const agentsCpu = () => Math.min(s().cpu, (props.snap.agents_cpu ?? 0) / Math.max(1, s().cores));
  const pressure = () =>
    s().pressure === "critical"
      ? { chip: "failed", icon: "fail" as IconName, label: "Critical" }
      : s().pressure === "warn"
        ? { chip: "needs", icon: "needs" as IconName, label: "Tight" }
        : { chip: "done", icon: "done" as IconName, label: "Normal" };
  const temps = () => props.snap.temps;
  return (
    <section class="perf-mac" aria-label={t("This Mac")}>
      <h2 class="perf-h2">{t("This Mac")}</h2>
      <div class="perf-tiles">
        <Tile
          label="Memory"
          badge={
            <span class="chip" data-state={pressure().chip}>
              <Icon name={pressure().icon} small />
              {t("Pressure: {level}", { level: t(pressure().label) })}
            </span>
          }
          big={bytes(s().mem_used)}
          small={t("of {total}", { total: bytes(s().mem_total) })}
          bar={[
            { cls: "perf-stack-agents", v: agentsMem() },
            { cls: "perf-stack-others", v: othersMem() },
            { cls: "perf-stack-free", v: s().mem_total - s().mem_used },
          ]}
          barLabel={t("Agents {a}, everything else {o}, free {f}", { a: bytes(agentsMem()), o: bytes(othersMem()), f: pct(s().free_pct) })}
        >
          <Leg sw="sw-agents" label={t("Agents")} value={bytes(agentsMem())} />
          <Leg sw="sw-others" label={t("Everything else")} value={bytes(othersMem())} />
          <Leg sw="sw-free" label={t("Free")} value={pct(s().free_pct)} />
        </Tile>
        <Tile
          label="CPU"
          badge={<span class="muted">{plural(s().cores, "{n} core", "{n} cores")}</span>}
          big={pct(s().cpu)}
          bar={[
            { cls: "perf-stack-agents", v: agentsCpu() },
            { cls: "perf-stack-others", v: s().cpu - agentsCpu() },
            { cls: "perf-stack-free", v: 100 - s().cpu },
          ]}
        >
          <Leg sw="sw-agents" label={t("Agents")} value={pct(agentsCpu())} />
          <Leg sw="sw-others" label={t("Everything else")} value={pct(s().cpu - agentsCpu())} />
          <Leg label={t("Load")} value={s().load.toFixed(1)} />
        </Tile>
        <Show
          when={temps() && (temps()!.cpu != null || temps()!.gpu != null)}
          fallback={
            <Tile label="Temperature" big="—" bar={[{ cls: "perf-stack-free", v: 1 }]}>
              <span class="perf-tile-note">{t("This Mac does not share its sensors yet.")}</span>
            </Tile>
          }
        >
          <TempTile temps={temps()!} />
        </Show>
        <Tile
          label="Swap"
          big={bytes(s().swap_used)}
          small={s().swap_total > 0 ? t("of {total}", { total: bytes(s().swap_total) }) : undefined}
          bar={[
            { cls: s().swap_used > 1024 ** 3 ? "perf-stack-warm" : "perf-stack-others", v: s().swap_used },
            { cls: "perf-stack-free", v: Math.max(s().swap_total - s().swap_used, s().swap_total ? 0 : 1) },
          ]}
        >
          <span class="perf-tile-note">{s().swap_used > 1024 ** 3 ? t("The Mac is paging to disk: it feels slower.") : t("Little or none: memory fits.")}</span>
        </Tile>
      </div>
      <div class="perf-charts">
        <Chart title="Memory" unit="bytes" history={props.snap.history} total={s().mem_total} mac={(h) => h.mem_used} agents={(h) => h.agents_mem} />
        <Chart title="CPU" unit="pct" history={props.snap.history} total={100} mac={(h) => h.cpu} agents={(h) => Math.min(h.cpu, h.agents_cpu / Math.max(1, s().cores))} />
      </div>
    </section>
  );
}

/** The last minutes as two lines on one axis: the whole Mac and the agents' share of it. The
 *  scale sits in its own gutter, so no line runs over a label, and nothing is drawn outside. */
function Chart(props: { title: string; unit: "bytes" | "pct"; history: PerfSample[]; total: number; mac: (h: PerfSample) => number; agents: (h: PerfSample) => number }) {
  const W = 600;
  const H = 120;
  const [hover, setHover] = createSignal<number | null>(null);
  const pts = () => props.history;
  const end = () => (pts().length ? pts()[pts().length - 1]!.t : 0);
  // The window is what the history holds: samples come every two seconds or a little slower.
  const span = () => Math.max(60, end() - (pts()[0]?.t ?? end()));
  const minutes = () => Math.max(1, Math.round(span() / 60));
  const x = (tt: number) => Math.max(0, W - ((end() - tt) / span()) * W);
  const y = (v: number) => H - (Math.max(0, Math.min(v, props.total)) / Math.max(1, props.total)) * H;
  const line = (f: (h: PerfSample) => number) => pts().map((h, i) => `${i ? "L" : "M"}${x(h.t).toFixed(1)},${y(f(h)).toFixed(1)}`).join("");
  const area = (f: (h: PerfSample) => number) => (pts().length ? `${line(f)}L${x(end()).toFixed(1)},${H}L${x(pts()[0]!.t).toFixed(1)},${H}Z` : "");
  const fmt = (v: number) => (props.unit === "bytes" ? bytes(v) : pct(v));
  const at = () => (hover() == null ? null : pts()[hover()!]);
  const onMove = (e: PointerEvent) => {
    const r = (e.currentTarget as SVGElement).getBoundingClientRect();
    const tt = end() - (1 - (e.clientX - r.left) / r.width) * span();
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
  // The tip stays inside the plot: anchored left near the start, right near the end.
  const tipStyle = (h: PerfSample) => {
    const f = x(h.t) / W;
    return { left: `${f * 100}%`, transform: f < 0.2 ? "translateX(8px)" : f > 0.8 ? "translateX(calc(-100% - 8px))" : "translateX(-50%)" };
  };
  return (
    <figure class="perf-chart">
      <figcaption class="perf-tile-head">
        <span>{t(props.title)}</span>
        <span class="perf-legend perf-legend-row">
          <span><i class="sw sw-mac" />{t("This Mac")}</span>
          <span><i class="sw sw-agents" />{t("Agents")}</span>
        </span>
      </figcaption>
      <div class="perf-plot">
        <div class="perf-yaxis" aria-hidden="true">
          <span style={{ top: "0%" }}>{fmt(props.total)}</span>
          <span style={{ top: "50%" }}>{fmt(props.total / 2)}</span>
          <span style={{ top: "100%" }}>0</span>
        </div>
        <div class="perf-plot-area">
          <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" onPointerMove={onMove} onPointerLeave={() => setHover(null)} role="img" aria-label={t("{title} over the last {n} minutes", { title: t(props.title), n: minutes() })}>
            <line class="perf-grid" x1="0" x2={W} y1={0} y2={0} />
            <line class="perf-grid" x1="0" x2={W} y1={H / 2} y2={H / 2} />
            <line class="perf-grid perf-grid-base" x1="0" x2={W} y1={H} y2={H} />
            <path class="perf-area-agents" d={area(props.agents)} />
            <path class="perf-line-mac" d={line(props.mac)} />
            <path class="perf-line-agents" d={line(props.agents)} />
            <Show when={at()}>{(h) => <line class="perf-cross" x1={x(h().t)} x2={x(h().t)} y1="0" y2={H} />}</Show>
          </svg>
          <Show when={at()}>
            {(h) => (
              <div class="perf-tip" style={tipStyle(h())}>
                <span class="muted">{new Date(h().t * 1000).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</span>
                <span><i class="sw sw-mac" />{t("This Mac")} <b>{fmt(props.mac(h()))}</b></span>
                <span><i class="sw sw-agents" />{t("Agents")} <b>{fmt(props.agents(h()))}</b></span>
              </div>
            )}
          </Show>
        </div>
      </div>
      <div class="perf-axis-x">
        <span>{t("{n} min ago", { n: minutes() })}</span>
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
                        {tx(KIND[a.kind] ?? a.kind, "kind")} · <span class="mono">{a.branch || a.model || ""}</span>
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
    case "paused_heat":
      return n ? t("Paused {card}: the Mac is overheating") : t("Paused {card}: the Mac is too warm");
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
                <span class="mono muted">{timeOfDay(e.t)}</span>
                <span>{eventText(e)}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}
