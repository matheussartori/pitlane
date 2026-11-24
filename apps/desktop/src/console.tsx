// One Claude Code session as a terminal: the log as Claude Code shows it (● a call, ⎿ its result,
// an edit as red and green lines), the questions it asks at its end, a prompt that talks to it
// while it works (Ctrl+C stops it, / for commands) and the model and effort under the prompt.
// The same console wherever a session shows: Terminal › Agents and a card's run.
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, untrack, type JSX } from "solid-js";
import { api, isLive, type RunEvent, type RunMeta } from "./api";
import { plural, t } from "./i18n";
import { Spinner, toast, tailFollower } from "./ui";
import { ImageViewer } from "./viewer";
import { ApprovalBlock } from "./approvals";
import { Icon } from "./icons";
import { Dictate } from "./dictate";
import { Prose } from "./prose";
import { money, msg } from "./format";
import { highlight, langOf } from "./highlight";
import { TERMINAL_FACES, terminalPrefs } from "./terminals/model";
import { sessions } from "./store/agents";
import { holdTimeline, timelineOf } from "./store/timelines";

type Done = Extract<RunEvent, { kind: "tool-done" }>;

/** The models `/model` offers: Claude Code's aliases for the newest of each. */
const MODELS: { id: string; hint: string }[] = [
  { id: "opus", hint: "Most capable, for the hard parts" },
  { id: "sonnet", hint: "Fast and strong, for most work" },
  { id: "haiku", hint: "Fastest, for small steps" },
  { id: "fable", hint: "The newest family" },
];
const EFFORTS: { id: string; hint: string }[] = [
  { id: "low", hint: "Quick, little thinking" },
  { id: "medium", hint: "Balanced" },
  { id: "high", hint: "Thinks more before acting" },
  { id: "xhigh", hint: "Thinks a lot" },
  { id: "max", hint: "Thinks the most, slowest" },
];

/** A model as Claude Code names it: `claude-opus-5-5` is Opus 5.5, `sonnet` is Sonnet. */
export function modelLabel(m: string | null | undefined): string {
  if (!m) return t("default model");
  const wide = /\[1m\]$/i.test(m) ? " (1M)" : "";
  const id = m.replace(/\[1m\]$/i, "");
  const full = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/.exec(id);
  const cap = (x: string) => x.charAt(0).toUpperCase() + x.slice(1);
  if (full) return `${cap(full[1]!)} ${full[2]}${full[3] ? `.${full[3]}` : ""}${wide}`;
  return /^[a-z]+$/.test(id) ? cap(id) + wide : m;
}

interface Suggestion {
  label: string;
  hint: string;
  /** What the field holds once picked; `run`: picking it also sends it. */
  value: string;
  run: boolean;
  on?: boolean;
}

/** Paths inside the worktree read relative to it; anything under a Pitlane worktree root too. */
function relTo(worktree: string | null | undefined) {
  return (s: string) => {
    let d = worktree ? s.replaceAll(`${worktree}/`, "").replaceAll(worktree, ".") : s;
    d = d.replace(/\/[^\s"']*?\/wt\/[^/\s]+\/[^/\s]+\//g, "");
    return d;
  };
}

/** How many of a log's last lines are drawn when it opens, and how many more each request adds. */
const LOG_WINDOW = 400;

export function AgentConsole(props: {
  m: RunMeta;
  /** The session's whole timeline, in order. */
  events: RunEvent[];
  /** The subagent in view (its Task call's id), or the session itself. */
  sub: string | null;
  onSub: (id: string | null) => void;
  /** What the session is ("Run", "Analysis"…), for the way back from a subagent and the stop toast. */
  kind: string;
  /** The state it ended in, in a word, for the prompt once it ended. */
  ended: string;
  /** A message or a command went to it (the Agents list keeps such sessions open). */
  onTalked?: () => void;
  /** In a page (a card's run): a well with its own edge. In the Terminal it fills the pane. */
  framed?: boolean;
  /** Written to once it had stopped: its session goes on in this new run, to show instead. */
  onResumed?: (m: RunMeta) => void;
}) {
  const m = () => props.m;
  const live = () => isLive(m().state);
  // Once ended, the Board assistant and a card's run pick their session back up when you write
  // to them, as `claude --resume` does: the assistant in the same record, a run in a new one.
  const resumesRun = () => !live() && !props.sub && (m().kind ?? "run") === "run";
  const writable = () => live() || (!props.sub && m().kind === "assist") || resumesRun();
  // Enter talks to the session itself, as in Claude Code: it reads the message after its current
  // step and goes on with its work. On a card's run at work, ⌘Enter asks on the side instead: a
  // copy of its conversation answers, in parallel, and the run never sees it.
  const asksAside = () => live() && !props.sub && (m().kind ?? "run") === "run";
  // Keyed on the values themselves: the session's record is read anew every few seconds, and
  // nothing below should start over for a record that says the same.
  const runId = createMemo(() => m().id);
  // The runs this session went on from (stopped, then written to or run again): their logs come
  // first, so the whole conversation is in one place, as in Claude Code.
  const earlier = createMemo(
    () => {
      const out: string[] = [];
      const seen = new Set<string>([m().id]);
      let p = m().resumed_from;
      while (p && !seen.has(p) && out.length < 20) {
        seen.add(p);
        out.unshift(p);
        const id: string = p;
        p = sessions().find((x) => x.id === id)?.resumed_from ?? null;
      }
      return out;
    },
    undefined,
    { equals: (a, b) => a.join() === b.join() },
  );
  createEffect(on(earlier, (ids) => ids.forEach((id) => onCleanup(holdTimeline(id)))));
  const history = createMemo(() => earlier().flatMap((id) => timelineOf(id) ?? []));
  const events = createMemo(() => (history().length ? [...history(), ...props.events] : props.events));
  const worktree = createMemo(() => m().worktree);
  const rel = createMemo(() => relTo(worktree()));

  // One relative copy per event, so the log keeps its rows while the timeline grows.
  const relCache = new WeakMap<RunEvent, RunEvent>();
  const relative = (e: RunEvent): RunEvent => {
    let r = relCache.get(e);
    if (r) return r;
    const f = rel();
    if (e.kind === "tool" || e.kind === "tool-done") r = { ...e, detail: f(e.detail) };
    else if (e.kind === "note") r = { ...e, text: f(e.text) };
    else if (e.kind === "approval") r = { ...e, approval: { ...e.approval, detail: f(e.approval.detail) } };
    else r = e;
    relCache.set(e, r);
    return r;
  };
  // What the whole timeline says, kept as it grows: each event is looked at once, when it
  // arrives, instead of the timeline being walked again for every new line (a long run has
  // thousands). Starts over for another session, or when the timeline was read anew.
  const fresh = () => ({
    run: "",
    hist: 0,
    rel: rel(),
    n: 0,
    /** Tool calls, by id. */
    calls: new Set<string>(),
    /** Their results, by the call's id. */
    results: new Map<string, Done>(),
    /** Asides asked and not answered yet. */
    asking: new Set<string>(),
    /** Where the agent started (setup notes before it are dropped), or -1. */
    started: -1,
    /** A turn that ended in failure: the exit after it says nothing new. */
    failed: false,
  });
  let seen = fresh();
  const scanned = createMemo(
    () => {
      const all = events();
      const hist = history().length;
      if (seen.run !== runId() || seen.rel !== rel() || seen.hist !== hist || all.length < seen.n) seen = { ...fresh(), run: runId(), hist };
      for (let i = seen.n; i < all.length; i++) {
        const e = all[i]!;
        if (e.kind === "tool") seen.calls.add(e.id);
        else if (e.kind === "tool-done") seen.results.set(e.id, relative(e) as Done);
        else if (e.kind === "aside") e.by === "you" ? seen.asking.add(e.session) : seen.asking.delete(e.session);
        else if (e.kind === "started" && seen.started < 0) seen.started = i;
        else if (e.kind === "finished" && !e.ok) seen.failed = true;
      }
      seen.n = all.length;
      return seen;
    },
    undefined,
    { equals: false },
  );
  const results = () => scanned().results;
  const asking = () => scanned().asking;
  // Results show under their call, as in Claude Code (one with no call in view, a failure say,
  // still shows on its own); a subagent's steps show behind its Task; setup notes only until the
  // agent starts.
  const shown = createMemo(() => {
    const all = events();
    const { calls, started, failed } = scanned();
    const s = props.sub;
    return all
      .filter((e, i) => {
        if (e.kind === "quota") return false;
        const p = "parent" in e ? e.parent : undefined;
        if (s ? p !== s : p) return false;
        if (e.kind === "tool-done" && calls.has(e.id)) return false;
        if (e.kind === "note" && started >= 0 && i < started) return false;
        // "Finished" already said it; only an exit that says something new stays.
        if (e.kind === "exited" && (e.state === "done" || (e.state === "failed" && failed))) return false;
        return true;
      })
      .map(relative);
  });
  const subLabel = () => {
    const s = props.sub;
    const call = s ? events().find((e) => e.kind === "tool" && e.id === s) : undefined;
    return call && call.kind === "tool" ? call.detail || call.name : t("subagent");
  };
  // Working, said once at the end: while no call is open, nothing waits on you and the session
  // has not answered (an assistant that answered waits for your next message).
  const working = createMemo(() => {
    if (!live() || m().waiting.length > 0) return false;
    const list = shown();
    if (list.at(-1)?.kind === "finished") return false;
    return !list.some((e) => e.kind === "tool" && !results().has(e.id));
  });

  const [text, setText] = createSignal("");
  let input: HTMLInputElement | undefined;
  /** When a key was last pressed in the prompt, and a pointer last went down anywhere. */
  let typedAt = 0;
  let pointedAt = 0;
  let root: HTMLDivElement | undefined;
  const tail = tailFollower(32);

  // A long log opens on its last lines: drawing thousands of rows at once is what makes opening
  // a long run slow. The earlier ones come in on request, a screenful at a time; the first line
  // drawn stays put while the log grows, so nothing moves under what you are reading.
  const [from, setFrom] = createSignal(0);
  const visible = createMemo(() => (from() > 0 ? shown().slice(from()) : shown()));
  let logEl: HTMLElement | undefined;
  createEffect(on(() => shown().length, (len, prev) => len - (prev ?? 0) > LOG_WINDOW && setFrom(len - LOG_WINDOW)));
  createEffect(on([runId, () => props.sub], () => setFrom(Math.max(0, untrack(() => shown().length) - LOG_WINDOW)), { defer: true }));
  const showEarlier = () => {
    const before = logEl?.scrollHeight ?? 0;
    setFrom(Math.max(0, from() - LOG_WINDOW));
    // Keep what was on screen where it was: the new rows went in above it.
    if (logEl) logEl.scrollTop += logEl.scrollHeight - before;
  };

  createEffect(on([() => shown().length, () => props.sub], () => tail.follow()));
  // A new session in view: follow it from its end. The prompt takes the keys, unless a question
  // waits: then 1-4 answer it.
  // Keyed on the id itself: `on` runs whenever the run's record is read anew (every update of
  // the run), and that pulled you back to the end and took the focus while you read above.
  const sessionId = createMemo(() => m().id);
  createEffect(
    on(
      sessionId,
      () => {
        tail.restart();
        requestAnimationFrame(() => (m().waiting.length > 0 ? root?.focus() : input?.focus()));
      },
    ),
  );
  createEffect(
    on(
      () => m().waiting.length,
      (n, prev) => {
        // Never while you are mid-sentence: the next key you type would answer the request.
        if (n > (prev ?? 0) && document.activeElement === input && !input?.value && performance.now() - typedAt > 4000) root?.focus();
        if (n > (prev ?? 0)) tail.follow();
      },
    ),
  );

  // Slash commands, as in Claude Code: /model and /effort change the live session.
  const [pick, setPick] = createSignal(0);
  const suggestions = createMemo<Suggestion[]>(() => {
    const v = text();
    if (!v.startsWith("/") || !live()) return [];
    const [cmd, ...rest] = v.slice(1).split(" ");
    const arg = rest.join(" ").trim().toLowerCase();
    if (rest.length === 0) {
      return [
        { label: "/model", hint: t("Switch the model ({now})", { now: modelLabel(m().model) }), value: "/model ", run: false },
        { label: "/effort", hint: t("Set how hard it thinks ({now})", { now: m().effort ?? t("default") }), value: "/effort ", run: false },
      ].filter((x) => x.label.startsWith(`/${cmd}`));
    }
    const list = cmd === "model" ? MODELS : cmd === "effort" ? EFFORTS : [];
    const current = cmd === "model" ? (m().model ?? "") : (m().effort ?? "");
    return list
      .filter((x) => x.id.startsWith(arg))
      .map((x) => ({
        label: cmd === "model" ? modelLabel(x.id) : x.id,
        hint: t(x.hint),
        value: `/${cmd} ${x.id}`,
        run: true,
        on: current === x.id || current.includes(`-${x.id}-`),
      }));
  });
  createEffect(on(() => suggestions().length, () => setPick(0)));
  const tune = async (what: { model?: string; effort?: string }) => {
    try {
      await api.runTune(m().id, what);
      setText("");
      props.onTalked?.();
    } catch (e) {
      toast({ tone: "fail", title: t("Could not change it"), body: msg(e) });
    }
  };
  /** A slash command it knows: run it and say true. Others (a skill, /compact) go to Claude Code as typed. */
  const command = (line: string): boolean => {
    const mt = /^\/(model|effort)(?:\s+(\S+))?\s*$/.exec(line);
    if (!mt) return false;
    const [, cmd, arg] = mt;
    if (!arg) {
      setText(`/${cmd} `);
      input?.focus();
      return true;
    }
    if (cmd === "effort" && !EFFORTS.some((x) => x.id === arg.toLowerCase())) {
      toast({ tone: "fail", title: t("No such effort"), body: t("Use one of: {list}", { list: EFFORTS.map((x) => x.id).join(", ") }) });
      return true;
    }
    void tune(cmd === "model" ? { model: arg } : { effort: arg.toLowerCase() });
    return true;
  };
  const choose = (sg: Suggestion) => {
    if (sg.run) command(sg.value);
    else (setText(sg.value), input?.focus());
  };
  const ask = (cmd: "model" | "effort") => {
    setText(`/${cmd} `);
    input?.focus();
  };

  // ↑ and ↓ walk what you wrote to this conversation (earlier runs of the session too), newest
  // first, as in a shell; past the newest, ↓ gives back what you were typing.
  const [sentHere, setSentHere] = createSignal<string[]>([]);
  const mine = createMemo(() => {
    const out: string[] = [];
    for (const e of events()) {
      const said = e.kind === "said" ? e.text : e.kind === "aside" && e.by === "you" ? e.text : "";
      if (said.trim() && out[out.length - 1] !== said) out.push(said);
    }
    for (const x of sentHere()) if (out[out.length - 1] !== x && !out.includes(x)) out.push(x);
    return out;
  });
  let recall: number | null = null;
  let draft = "";
  const walk = (step: -1 | 1): boolean => {
    const all = mine();
    if (all.length === 0) return false;
    if (recall === null) {
      if (step === 1) return false;
      draft = text();
      recall = all.length;
    }
    const next = recall + step;
    if (next < 0) return true;
    if (next >= all.length) {
      recall = null;
      setText(draft);
    } else {
      recall = next;
      setText(all[next]!);
    }
    requestAnimationFrame(() => input?.setSelectionRange(input.value.length, input.value.length));
    return true;
  };

  // One message at a time (Enter twice sends once), and the box is cleared when it goes, so
  // what you type while it is on its way stays yours.
  let sending = false;
  const send = async (aside = false) => {
    const msgText = text().trim();
    if (!msgText || sending) return;
    if (command(msgText)) return;
    sending = true;
    setText("");
    recall = null;
    setSentHere((xs) => [...xs.filter((x) => x !== msgText), msgText].slice(-50));
    try {
      if (resumesRun()) {
        const next = await api.runResume(m().id, msgText);
        props.onResumed?.(next);
      } else if (asksAside() && aside) await api.runAside(m().id, msgText);
      else await api.runSay(m().id, msgText);
      props.onTalked?.();
    } catch (e) {
      // Not sent: the words come back, unless something new was typed meanwhile.
      if (!text()) setText(msgText);
      toast({ tone: "fail", title: t("The agent did not get it"), body: msg(e) });
    } finally {
      sending = false;
    }
  };

  const stop = async () => {
    if (!live()) return;
    try {
      await api.runStop(m().id);
      toast({ tone: "info", title: t("Stopping #{card} {kind}", { card: m().card, kind: props.kind }), body: t("It ends its current step, then stops.") });
    } catch (e) {
      toast({ tone: "fail", title: t("Could not stop it"), body: msg(e) });
    }
  };
  // While you type here the keys stay here: when the field loses the focus without you clicking
  // elsewhere or pressing Tab (a redraw, a field disabled for an instant, the page taking it),
  // it takes it back, so a sentence is never cut off halfway.
  const onPointer = () => (pointedAt = performance.now());
  window.addEventListener("pointerdown", onPointer, true);
  onCleanup(() => window.removeEventListener("pointerdown", onPointer, true));
  const keepFocus = (e: FocusEvent) => {
    if (e.relatedTarget) return; // the focus went to something else on purpose
    const at = performance.now();
    if (at - typedAt > 4000 || at - pointedAt < 500) return;
    requestAnimationFrame(() => {
      if (!input?.isConnected || input.disabled || !document.hasFocus()) return;
      const now = document.activeElement;
      if (now && now !== document.body && !(now === root && input.value)) return;
      input.focus();
    });
  };

  const onKey = (e: KeyboardEvent) => {
    // Ctrl+C stops the agent, like in a terminal (unless you are copying a selection).
    if (e.ctrlKey && !e.metaKey && e.key.toLowerCase() === "c" && !window.getSelection()?.toString()) {
      e.preventDefault();
      void stop();
    }
  };

  // The log wears the Terminal's face and size, so an agent reads like the shell beside it.
  const face = () => {
    const p = terminalPrefs();
    return { "--term-face": (TERMINAL_FACES.find((f) => f.v === p.face) ?? TERMINAL_FACES[0]).css, "--term-size": `${p.font}px` };
  };

  return (
    <div class="console" ref={root} tabindex="-1" data-framed={props.framed ? "" : undefined} style={face()} onKeyDown={onKey}>
      <Show when={props.sub}>
        <div class="console-crumb">
          <button type="button" class="ag-crumb" onClick={() => props.onSub(null)}>
            ‹ {props.kind}
          </button>
          <span class="ag-bar-sep">›</span>
          <span class="console-crumb-sub">{subLabel()}</span>
        </div>
      </Show>
      <div class="ag-log" ref={(el) => (tail.ref(el), (logEl = el))} aria-live="polite">
        <Show when={from() > 0}>
          <button type="button" class="ag-earlier" onClick={showEarlier}>
            {plural(from(), "Show {n} earlier line", "Show {n} earlier lines")}
          </button>
        </Show>
        <For each={visible()} fallback={<div class="ag-line ag-muted">{live() ? t("Starting…") : t("Nothing recorded.")}</div>}>
          {(e) => <Line e={e} m={m()} done={e.kind === "tool" ? results().get(e.id) : undefined} asking={e.kind === "aside" && asking().has(e.session)} onSub={props.onSub} />}
        </For>
        <Show when={working()}>
          <div class="cc-call cc-working">
            <Spinner />
            <span>{t("Working…")}</span>
          </div>
        </Show>
        <Show when={live() && m().waiting.length > 0}>
          <div class="ag-asks">
            <For each={m().waiting}>{(a, i) => <ApprovalBlock run={m().id} approval={a} keys={i() === 0} />}</For>
          </div>
        </Show>
        {/* Floats over the log's end while you read above it; no room taken, so nothing shifts. */}
        <Show when={!tail.following() && shown().length > 0}>
          <div class="ag-jump-dock">
            <button type="button" class="ag-jump" onClick={() => tail.restart()}>
              <Icon name="chev" small />
              {t("Jump to present")}
            </button>
          </div>
        </Show>
      </div>
      <div class="ag-input" data-off={writable() ? undefined : ""}>
        <Show when={suggestions().length > 0}>
          <div class="ag-suggest" role="listbox" aria-label={t("Commands")}>
            <For each={suggestions()}>
              {(sg, i) => (
                <button
                  type="button"
                  role="option"
                  class="ag-suggest-item"
                  aria-selected={pick() === i()}
                  data-on={sg.on ? "" : undefined}
                  onPointerDown={(e) => e.preventDefault()}
                  onPointerEnter={() => setPick(i())}
                  onClick={() => choose(sg)}
                >
                  <span class="ag-suggest-label">{sg.label}</span>
                  <span class="ag-suggest-hint">{sg.hint}</span>
                  <Show when={sg.on}>
                    <span class="ag-suggest-on">{t("current")}</span>
                  </Show>
                </button>
              )}
            </For>
          </div>
        </Show>
        <span class="ag-prompt">›</span>
        <input
          ref={input}
          class="ag-field"
          disabled={!writable()}
          placeholder={
            live()
              ? props.sub
                ? t("Message the agent that opened this subagent (Enter). Ctrl+C stops it")
                : asksAside()
                  ? t("Tell the run more, without stopping it (Enter) · ask on the side (⌘Enter). Ctrl+C stops it")
                  : t("Tell the agent more: context, a check to make… (Enter). Ctrl+C stops it")
              : resumesRun()
                ? t("Write to pick this session back up where it stopped (Enter)")
                : writable()
                ? t("Ask something else; it picks this conversation back up (Enter)")
                : t("This session ended ({state})", { state: props.ended.toLowerCase() })
          }
          aria-label={t("Message the agent")}
          autocomplete="off"
          value={text()}
          onInput={(e) => ((recall = null), setText(e.currentTarget.value))}
          onBlur={keepFocus}
          onKeyDown={(e) => {
            typedAt = performance.now();
            const list = suggestions();
            if (list.length > 0 && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
              e.preventDefault();
              setPick((pick() + (e.key === "ArrowDown" ? 1 : -1) + list.length) % list.length);
            } else if (list.length > 0 && (e.key === "Tab" || (e.key === "Enter" && !e.isComposing))) {
              e.preventDefault();
              choose(list[Math.min(pick(), list.length - 1)]!);
            } else if (list.length > 0 && e.key === "Escape") {
              e.preventDefault();
              setText("");
            } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.isComposing && !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey) {
              if (walk(e.key === "ArrowUp" ? -1 : 1)) e.preventDefault();
            } else if (e.key === "Enter" && !e.isComposing) (e.preventDefault(), void send(e.metaKey || e.ctrlKey));
            // Escape with nothing typed belongs to the screen (the run goes back to the Board).
            if (!(e.key === "Escape" && !text())) e.stopPropagation();
            onKey(e);
          }}
        />
        <Show when={writable()}>
          <Dictate field={() => input} value={text} onChange={(v) => ((recall = null), setText(v))} />
        </Show>
      </div>
      <div class="ag-status" data-off={live() ? undefined : ""}>
        <button type="button" class="ag-chip ag-chip-model" disabled={!live()} title={live() ? t("Switch the model (/model)") : (m().reason ?? undefined)} onClick={() => ask("model")}>
          <span class="ag-chip-mark" aria-hidden="true">✻</span>
          {modelLabel(m().model)}
        </button>
        <button type="button" class="ag-chip" disabled={!live()} title={live() ? t("Set the effort (/effort)") : undefined} onClick={() => ask("effort")}>
          <span class="ag-effort" data-level={m().effort ?? "default"} aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
            <i />
          </span>
          {t("{level} effort", { level: m().effort ?? t("default") })}
        </button>
        <Show when={m().account}>
          <span class="ag-status-hint" title={m().reason ?? undefined}>
            {t("on {account}", { account: m().account! })}
          </span>
        </Show>
        <span class="ag-bar-fill" />
        <Show when={live()}>
          <span class="ag-status-hint">
            <span class="kbd">/</span> {t("for commands")}
            <span class="kbd">⌃C</span> {t("stops")}
          </span>
        </Show>
      </div>
    </div>
  );
}

/** Claude Code's names for its tools, as its own terminal shows them. */
const TOOL_LABEL: Record<string, string> = { Edit: "Update", MultiEdit: "Update", Grep: "Search", WebFetch: "Fetch", WebSearch: "Web search", NotebookEdit: "Update" };
// The browser's tools (`mcp__browser__browser_take_screenshot`) read as "Browser · take screenshot".
const toolLabel = (name: string) =>
  name.startsWith("mcp__browser__browser_") ? `${t("Browser")} · ${name.slice(22).replace(/_/g, " ")}` : t(TOOL_LABEL[name] ?? name);

/** What a finished tool came to, in one line, the way Claude Code sums it up: what it printed
 * in the log's ink, and what only counts (lines read, results found, lines left out) dimmed. */
function resultLine(name: string, d: Done): { text: string; more?: string; dim?: boolean } {
  const n = d.lines ?? (d.detail ? 1 : 0);
  if (!d.ok) return { text: d.detail || t("Failed") };
  if (name === "Read") return { text: plural(n, "Read {n} line", "Read {n} lines"), dim: true };
  if (name === "Edit" || name === "MultiEdit" || name === "Write" || name === "NotebookEdit") return { text: d.detail || t("Done") };
  if (name === "Grep" || name === "Glob") return n > 1 ? { text: t("Found {n} results", { n }), dim: true } : { text: d.detail || t("No matches"), dim: !d.detail };
  return { text: d.detail || t("Done"), more: n > 1 ? `  … ${plural(n - 1, "+{n} line", "+{n} lines")}` : undefined, dim: !d.detail };
}

/** An edit as Claude Code shows it: removed lines, then added ones, in the file's syntax colours. */
function Diff(props: { change: { old: string; new: string; cut: number }; path: string }) {
  const old = () => (props.change.old ? props.change.old.split("\n") : []);
  const add = () => (props.change.new ? props.change.new.split("\n") : []);
  const lang = () => langOf(props.path);
  return (
    <div class="cc-diff">
      <For each={old()}>{(l) => <div class="cc-del"><span class="cc-sign">-</span>{highlight(l || " ", lang())}</div>}</For>
      <For each={add()}>{(l) => <div class="cc-add"><span class="cc-sign">+</span>{highlight(l || " ", lang())}</div>}</For>
      <Show when={props.change.cut > 0}>
        <div class="cc-more">… {plural(props.change.cut, "{n} more line", "{n} more lines")}</div>
      </Show>
    </div>
  );
}

/** A screenshot a tool gave back, under its call: a thumbnail that opens full size. */
function Shot(props: { run: string; file: string }) {
  // Keyed by a string, so a fresh record of the run (every update) neither fetches it again nor
  // closes it while it is open full size.
  const [src] = createResource(() => `${props.run}\n${props.file}`, (k) => {
    const [r, f] = k.split("\n") as [string, string];
    return api.runShot(r, f).catch(() => "");
  });
  const [big, setBig] = createSignal(false);
  return (
    <Show when={src()}>
      {(u) => (
        <div class="cc-shot">
          <span class="cc-sign">⎿</span>
          <button type="button" class="cc-shot-thumb" title={t("Open full size")} onClick={() => setBig(true)}>
            <img src={u()} alt={t("Screenshot")} loading="lazy" />
          </button>
          <ImageViewer open={big()} src={u()} name={props.file} alt={t("Screenshot")} label={t("Screenshot")} onClose={() => setBig(false)} />
        </div>
      )}
    </Show>
  );
}

const EXITED: Record<string, string> = { stopped: "Stopped", failed: "Failed", interrupted: "Interrupted", done: "Done" };

/** A line that is not the agent's own: a glyph in the gutter, then the words. */
function Sys(props: { glyph: string; tone?: "ok" | "fail" | "needs" | "ink"; children: JSX.Element }) {
  return (
    <div class="cc-call cc-sys" data-tone={props.tone}>
      <span class="cc-glyph" aria-hidden="true">{props.glyph}</span>
      <span>{props.children}</span>
    </div>
  );
}

/** One event of a session, as Claude Code's terminal shows it. */
function Line(props: { e: RunEvent; m: RunMeta; done?: Done; asking?: boolean; onSub: (id: string) => void }) {
  const e = props.e;
  const live = () => isLive(props.m.state);
  switch (e.kind) {
    case "text":
      return (
        <div class="cc-msg">
          <span class="cc-dot">●</span>
          <div class="cc-text">
            <Prose text={e.text.trim()} />
          </div>
        </div>
      );
    case "tool": {
      // Read through props each time: the result arrives after the call is drawn.
      const d = () => props.done;
      const dot = () => {
        const r = d();
        return r ? (r.ok ? "ok" : "fail") : live() ? "work" : "ended";
      };
      if (e.name === "Task" || e.name === "Agent")
        return (
          <button type="button" class="ag-line ag-spawn cc-call" title={t("Open what this subagent did")} onClick={() => props.onSub(e.id)}>
            <span class="cc-dot" data-s={dot()}>●</span>
            <span>
              <b class="cc-name">Task</b>
              <span class="cc-arg">({e.detail || e.name})</span> <span class="ag-muted">› {t("open")}</span>
            </span>
          </button>
        );
      return (
        <div class="cc-tool">
          <div class="cc-call">
            <span class="cc-dot" data-s={dot()}>●</span>
            <span>
              <b class="cc-name">{toolLabel(e.name)}</b>
              <Show when={e.detail}>
                <span class="cc-arg">({e.detail})</span>
              </Show>
            </span>
          </div>
          <Show when={e.change}>{(c) => <Diff change={c()} path={e.detail ?? ""} />}</Show>
          <Show when={d()} fallback={<Show when={live()}><div class="cc-result ag-muted">⎿  {t("Running…")}</div></Show>}>
            {(d) => {
              const r = () => resultLine(e.name, d());
              return (
                <div class="cc-result" classList={{ "ag-fail": !d().ok, "ag-muted": d().ok && !!r().dim }}>
                  ⎿  {r().text}
                  <Show when={r().more}><span class="cc-more">{r().more}</span></Show>
                </div>
              );
            }}
          </Show>
        </div>
      );
    }
    case "tool-done":
      return e.ok ? <div class="cc-result">⎿  {e.detail}</div> : <Sys glyph="✕" tone="fail">{e.detail}</Sys>;
    case "shot":
      return <Shot run={props.m.id} file={e.file} />;
    case "aside":
      if (e.by === "failed") return <Sys glyph="×" tone="fail">{t("Not answered on the side: {why}", { why: e.text })}</Sys>;
      if (e.by === "agent")
        return (
          <div class="cc-msg cc-aside">
            <span class="cc-dot">◇</span>
            <div class="cc-text">
              <div class="cc-aside-tag">{t("Answered on the side; the run did not read this")}</div>
              <Prose text={e.text.trim()} />
            </div>
          </div>
        );
      return (
        <>
          <div class="cc-call cc-said">
            <span class="cc-glyph" aria-hidden="true">›</span>
            <span>{e.text}</span>
          </div>
          <Show when={props.asking}>
            <div class="cc-call cc-working">
              <Spinner />
              <span>{t("Answering on the side; the run goes on…")}</span>
            </div>
          </Show>
        </>
      );
    case "said":
      return (
        <div class="cc-call cc-said">
          <span class="cc-glyph" aria-hidden="true">›</span>
          <span>{e.text}</span>
        </div>
      );
    case "note":
      return <Sys glyph="·">{e.text}</Sys>;
    case "tuned":
      return (
        <Sys glyph="·">
          {[e.model ? t("Model set to {model}", { model: modelLabel(e.model) }) : null, e.effort ? t("Effort set to {level}", { level: e.effort }) : null].filter(Boolean).join(" · ")}
        </Sys>
      );
    case "started":
      return <Sys glyph="·">{e.model ? t("Session started on {model}", { model: modelLabel(e.model) }) : t("Session started")}</Sys>;
    case "approval": {
      // While it waits, the question at the end of the log says it; after, this line keeps it.
      const pending = () => live() && props.m.waiting.some((w) => w.id === e.approval.id);
      return (
        <Show when={!pending()}>
          <Sys glyph="?">
            {e.approval.tool === "Bash" ? t("Asked to run") : t("Asked to use {tool}", { tool: e.approval.tool })} <code>{e.approval.detail || e.approval.tool}</code>
          </Sys>
        </Show>
      );
    }
    case "decided":
      return (
        <div class="cc-result ag-muted">
          ⎿  {t({ once: "You allowed it once.", run: "You allowed it for this run.", always: "You allowed it in this repository.", deny: "You denied it." }[e.decision] ?? e.decision)}
          {e.message ? ` ${e.message}` : ""}
        </div>
      );
    case "proof":
      return (
        <Sys glyph={e.ok ? "✓" : "✕"} tone={e.ok ? "ok" : "fail"}>
          <code>{e.command}</code>
        </Sys>
      );
    case "judged":
      return (
        <Sys glyph="●" tone={e.verdict === "approve" ? "ok" : "needs"}>
          {e.verdict === "approve"
            ? t("The judge approves: {summary}", { summary: e.summary })
            : t("The judge asks for changes ({findings}): {summary}", { findings: plural(e.findings, "{n} finding", "{n} findings"), summary: e.summary })}
        </Sys>
      );
    case "limited":
      return (
        <Sys glyph="!" tone="needs">
          {props.m.account
            ? t("Hit the {window} usage limit on {account}.", { window: e.window.replace("_", "-"), account: props.m.account })
            : t("Hit the {window} usage limit.", { window: e.window.replace("_", "-") })}
        </Sys>
      );
    case "finished":
      return (
        <Sys glyph={e.ok ? "✓" : "✕"} tone={e.ok ? "ok" : "fail"}>
          {[e.ok ? t("Finished") : t("Stopped with an error"), plural(e.turns, "{n} turn", "{n} turns"), money(e.cost_usd)].join(" · ")}
        </Sys>
      );
    case "exited":
      return (
        <Sys glyph="—" tone={e.state === "failed" ? "fail" : undefined}>
          {t("The agent exited: {state}.", { state: t(EXITED[e.state] ?? e.state).toLowerCase() })}
        </Sys>
      );
    default:
      return null;
  }
}
