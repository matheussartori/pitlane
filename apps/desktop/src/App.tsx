// The shell: top bar, the current screen, the Tower. Inbox, Board and Lanes read the repository; Ledger arrives with runs.
import { For, Match, Show, Switch, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { systemNotify } from "./notify";
import { NEEDS_API_KEY, api, isLive, onRun, type Lane, type RepoEntry, type RunMeta, type Settings, isOpenJob } from "./api";
import { AccountsSheet } from "./accounts";
import { LedgerView } from "./ledger";
import { PerformanceView } from "./performance";
import { ReviewScreen } from "./review";
import { RunScreen } from "./run";
import { BoardView, NewCardSheet } from "./board";
import { CardSheet, QuestionsSheet } from "./cardsheet";
import { openQuestions, place } from "./cards";
import { Palette, type Command } from "./palette";
import { InboxView } from "./inbox";
import { Icon, type IconName } from "./icons";
import { LanesView, Tower, queuedGroup } from "./lanes";
import { TerminalScreen, openTerminal, startTerminals, terminalAt, terminalsWanting } from "./terminals";
import { st as termFold, toggleFold } from "./terminals/model";
import { AgentDock } from "./agents";
import { agentsWanting, agentsWorking, kindLabel, selectAgent, setAgentsRepo, startAgents } from "./store/agents";
import { setAnalysisAsker } from "./store/analysis";
import { PreviewScreen } from "./preview";
import { CodeScreen, prewarmCode, rethemeCode, setCodePlaces } from "./code";
import { codeRoot, openInCode, resetCodeRoot } from "./store/code";
import { SettingsScreen } from "./settings";
import { ReportsView } from "./reports";
import { loadPrefs, prefs } from "./prefs";
import { CleanSheet, NewLaneSheet, ProjectSheet, RunAllSheet, ShipSheet } from "./sheets";
import { ScreenBoundary, Toasts, dismissable, installTips, toast } from "./ui";
import { TOWER_W } from "./layout";
import { plural, t } from "./i18n";
import { msg, tilde } from "./format";
import { cards, dropCardPoll, dropLoads, error, fetchedAt, fetching, jobs, load, loadCards, loadQueue, loadReports, loadRuns, problems, queueAll, dequeue, quota, reports, repo, runs, setCards, setError, setFetchedAt, setFetching, setJobs, setProblems, setQuota, setRepo, setReports, setRuns, setYard, yard } from "./store/repo";
import { createRunAll, batchIds, phaseIn, agentShare } from "./runall";
import { shortcuts } from "./shortcuts";

export type Tab = "inbox" | "board" | "lanes" | "ledger" | "reports" | "performance" | "code" | "preview" | "terminal" | "settings";
export type SheetState =
  | { kind: "ship"; lane: Lane; to: string }
  | { kind: "clean" }
  | { kind: "new" }
  | { kind: "card"; id: number }
  | { kind: "new-card" }
  | { kind: "accounts" }
  | { kind: "project" }
  /** `cards`: questions from cards analysed on their own (not Run all's). */
  | { kind: "questions"; cards?: number[] }
  | { kind: "run-all"; ids: number[] }
  | null;

const TAB_KEY = "pitlane.tab";
const isTab = (t: string | null): t is Tab => t === "inbox" || t === "board" || t === "lanes" || t === "ledger" || t === "reports" || t === "performance" || t === "code" || t === "preview" || t === "terminal" || t === "settings";
/** The screen lives in the URL hash (#board), falling back to the last one used. */
function savedTab(): Tab {
  const hash = location.hash.replace(/^#\/?/, "");
  if (isTab(hash)) return hash;
  try {
    const t = localStorage.getItem(TAB_KEY);
    return isTab(t) ? t : "lanes";
  } catch {
    return "lanes";
  }
}

/** `/Users/me/Projects/a/b/c/repo` → `~/Projects/…/c/repo`: short, and still the right folder. */
function shortPath(p: string): string {
  const home = tilde(p);
  const parts = home.split("/").filter(Boolean);
  return parts.length > 4 ? [parts[0], parts[1], "…", ...parts.slice(-2)].join("/") : home;
}


export default function App() {
  const [settings, setSettings] = createSignal<Settings>({ repos: [], last: null });
  const [tab, setTabSignal] = createSignal<Tab>(savedTab());
  // The Tower stays folded until you open it; the rail's Lanes count already says when a lane needs you.
  const [towerOpen, setTowerOpenSignal] = createSignal((() => {
    try {
      return localStorage.getItem("pitlane.tower") === "open";
    } catch {
      return false;
    }
  })());
  const toggleTower = () => {
    const open = !towerOpen();
    setTowerOpenSignal(open);
    try {
      localStorage.setItem("pitlane.tower", open ? "open" : "closed");
    } catch {
      /* private window: the choice simply is not remembered */
    }
  };
  // On the Terminal screen, which has no Tower, ⌘\ and the rail's button fold its sidebar instead.
  const sideFolds = () => tab() === "terminal";
  const toggleSide = () => (sideFolds() ? toggleFold() : toggleTower());
  const towerHere = () => towerOpen() && !["lanes", "terminal", "code", "preview", "settings"].includes(tab());
  const setTab = (t: Tab) => {
    setTabSignal(t);
    setRunView(null);
    setReviewView(null);
    history.replaceState(null, "", `#${t}`);
    try {
      localStorage.setItem(TAB_KEY, t);
    } catch {
      /* private window: the tab simply is not remembered */
    }
  };
  const runFromHash = () => {
    const m = /^#run\/(.+)$/.exec(location.hash);
    return m ? decodeURIComponent(m[1]!) : null;
  };
  const [runView, setRunView] = createSignal<string | null>(runFromHash());
  const openRun = (id: string) => {
    location.hash = `#run/${encodeURIComponent(id)}`;
  };
  const reviewFromHash = () => {
    const m = /^#review\/(\d+)$/.exec(location.hash);
    return m ? Number(m[1]) : null;
  };
  const [reviewView, setReviewView] = createSignal<number | null>(reviewFromHash());
  const openReview = (card: number) => {
    location.hash = `#review/${card}`;
  };
  /** The card's latest run: the highest review round, then the newest. */
  const lastRun = (card: number) =>
    (runs() ?? []).filter((r) => r.card === card).sort((a, b) => (b.round ?? 0) - (a.round ?? 0) || b.started_at - a.started_at)[0];
  const [now, setNow] = createSignal(Date.now());
  const [selected, setSelected] = createSignal<string | null>(null);
  // The browser preview opens a sheet from the address (`?sheet=accounts`), to look at it without clicking.
  const [sheet, setSheet] = createSignal<SheetState>(import.meta.env.DEV && new URLSearchParams(location.search).get("sheet") === "accounts" ? { kind: "accounts" } : null);
  const [menu, setMenu] = createSignal(false);
  const [palette, setPalette] = createSignal(false);
  /** Presence in force, read each time ⌘K opens so its entry offers the other mode. */
  const [presence, setPresence] = createSignal<"here" | "away" | null>(null);
  createEffect(on(palette, (open) => open && void api.presence().then((p) => setPresence(p.mode)).catch(() => setPresence(null))));
  let repoBox: HTMLDivElement | undefined;
  dismissable(() => repoBox, menu, () => setMenu(false));
  const [booted, setBooted] = createSignal(false);
  const [checkingOut, setCheckingOut] = createSignal<string | null>(null);

  async function checkout(lane: Lane) {
    const path = repo();
    if (!path || checkingOut()) return;
    setCheckingOut(lane.branch);
    try {
      const r = await api.newLane(path, lane.branch);
      toast(
        {
          tone: r.setup_ok === false ? "fail" : "done",
          title: t("{branch} checked out, tracking {base}", { branch: r.branch, base: r.base }),
          body: r.setup_ok === false ? t("{path}. The setup command failed.", { path: r.path }) : r.path,
          action: { label: t("Copy path"), run: () => void navigator.clipboard.writeText(r.path) },
        },
        12000,
      );
      await load(false);
    } catch (e) {
      toast({ tone: "fail", title: t("{branch} not checked out", { branch: lane.branch }), body: msg(e) });
    } finally {
      setCheckingOut(null);
    }
  }

  createEffect(on([tab, repo], ([t]) => t === "reports" && void loadReports()));

  /** A file of a lane (its worktree) or of the repository, in the Code screen. */
  function codeFile(branch: string | null | undefined, file: string) {
    const path = repo();
    if (!path) return;
    const wt = yard()?.lanes.find((l) => l.branch === branch)?.worktree?.path;
    openInCode(wt ?? path, file);
    setRunView(null);
    setReviewView(null);
    setTab("code");
  }

  // A screen change closes the repository menu.
  createEffect(on(tab, () => setMenu(false), { defer: true }));

  // VS Code loads in the background on the repository, so the Code tab opens at once.
  createEffect(on(repo, (r) => r && prewarmCode(r)));
  createEffect(on(repo, resetCodeRoot, { defer: true }));
  createEffect(on(repo, (r) => setAgentsRepo(r)));
  // VS Code's own folder picker offers the repository and the lanes' worktrees.
  createEffect(() => {
    const r = repo();
    if (r) setCodePlaces(r, current()?.name ?? t("repository"), yard()?.lanes ?? [], yard()?.head);
  });

  async function terminalIn(lane: Lane | undefined) {
    const path = repo();
    if (!path) return;
    const ok = lane?.worktree
      ? await openTerminal(lane.worktree.path, lane.branch)
      : await openTerminal(path, current()?.name ?? t("repository"));
    if (ok) setTab("terminal");
  }

  /** Lanes that finished while you were away, told together once the last one ends. */
  const doneAway: RunMeta[] = [];

  function notify(run: string, detail: string) {
    if (!prefs().notify_approvals) return;
    void systemNotify(() => {
      const m = (runs() ?? []).find((r) => r.id === run);
      return m ? t("#{card} {title} is waiting for you", { card: m.card, title: m.card_title }) : t("An agent is waiting for you");
    }, detail);
  }

  /** `fresh`: start over instead of resuming the card's stopped session. */
  async function startRun(card: number, allowApiKey = false, fresh = false) {
    const path = repo();
    if (!path) return;
    try {
      const m = await api.runStart(path, card, allowApiKey, null, false, null, false, fresh);
      void api.runFollow(m.id, 0).catch(() => {});
      await loadRuns();
      void loadCards();
      setSheet(null);
      openRun(m.id);
    } catch (e) {
      const text = msg(e);
      if (text.startsWith(NEEDS_API_KEY)) {
        // The route reached a paid option: never taken without a yes.
        toast(
          {
            tone: "info",
            title: t("Only the API key can take this card now"),
            body: t("{why}. It is paid per token.", { why: text.slice(NEEDS_API_KEY.length) }),
            action: { label: t("Use the API key"), run: () => void startRun(card, true, fresh) },
          },
          0,
        );
        return;
      }
      toast({ tone: "fail", title: t("Card #{card} did not start", { card }), body: text });
    }
  }

  // Run all: its session, its steps and the effects that keep it going live in runall.ts.
  const { plan, phase, queue, joining, analyse, analyseInto, analysed, startDev, finishBatch, stopRunAll } = createRunAll({ sheet, setSheet, setTab, switchTo, nameOf: (path) => settings().repos.find((r) => r.path === path)?.name });

  // Cards analysed on their own: their questions come up once the cards are read again, at once
  // when you are in that repository with nothing else open, else from the toast's Answer.
  setAnalysisAsker({
    auto: (path, ids) => {
      if (path !== repo() || sheet()) return;
      void loadCards().then(() => path === repo() && !sheet() && setSheet({ kind: "questions", cards: ids }));
    },
    open: (path, ids) => void Promise.resolve(path === repo() ? loadCards() : switchTo(path)).then(() => setSheet({ kind: "questions", cards: ids })),
  });

  async function switchTo(path: string) {
    setMenu(false);
    if (path === repo()) return;
    // Run all belongs to the repository it started in, and keeps going there while another one is open.
    dropLoads();
    setRepo(path);
    setYard(null);
    setCards(null);
    setRuns(null);
    // Nothing of the last repository stays on screen while this one's loads.
    setJobs([]);
    setQuota(null);
    setReports(null);
    setProblems([]);
    setError(null);
    setFetchedAt(null);
    setSelected(null);
    setFetching(false);
    void api.selectRepo(path).catch(() => {});
    void loadCards();
    void loadRuns();
    void loadQueue();
    await load(false); // local refs first: instant
    await load(true); // then fetch and GitHub
  }

  async function addRepo() {
    setMenu(false);
    const dir = await openDialog({ directory: true, multiple: false, title: t("Choose a git repository") });
    if (typeof dir !== "string") return;
    try {
      const entry = await api.addRepo(dir);
      setSettings(await api.settings());
      await switchTo(entry.path);
    } catch (e) {
      toast({ tone: "fail", title: t("Not added"), body: msg(e) });
    }
  }

  onMount(() => {
    installTips();
    void startTerminals();
    void startAgents();
    // Boot without holding the listeners below: the first fetch can be slow or offline.
    void (async () => {
      try {
        const s = await api.settings();
        setSettings(s);
        loadPrefs(s.prefs);
        const start = s.last ?? s.repos[0]?.path;
        if (start) await switchTo(start);
      } catch (e) {
        setError(msg(e));
      } finally {
        setBooted(true);
      }
      // A daemon from another build (started by an older pit): said, never restarted from here.
      try {
        const other = await api.daemonMismatch();
        if (other)
          toast(
            {
              tone: "fail",
              title: t("The daemon is from another build of Pitlane"),
              body: t("It may not understand this app. Once its agents finish, quit and reopen Pitlane so it starts its own. ({detail})", { detail: other }),
            },
            0,
          );
      } catch {
        /* the browser preview */
      }
      // Pitlane closed with work in the merge queue: it waits for a yes.
      try {
        const n = await api.queuePaused();
        if (n > 0)
          toast(
            {
              tone: "info",
              title: t("The merge queue is paused ({n} waiting)", { n }),
              body: t("It paused when Pitlane closed. Nothing ships until you resume it."),
              action: { label: t("Resume"), run: () => void api.queueResume().then(() => loadQueue(), (e) => toast({ tone: "fail", title: t("Could not resume the queue"), body: msg(e) })) },
            },
            0,
          );
      } catch {
        /* the browser preview */
      }
    })();

    // The clock ticks every second while an agent runs, every 30 s otherwise.
    const tick = setInterval(() => {
      if ((runs() ?? []).some((r) => isLive(r.state)) || Date.now() - now() >= 30_000) setNow(Date.now());
    }, 1000);
    let offRun = () => {};
    let unmounted = false;
    void onRun(
      (run, _i, e) => {
        if (e.kind !== "approval" && e.kind !== "decided") return;
        void loadRuns();
        // Not looking at Pitlane: say so where the person is.
        if (e.kind === "approval" && !document.hasFocus()) notify(run, e.approval.detail || e.approval.tool);
      },
      async (_run, m) => {
        const fresh = loadRuns();
        const cardsFresh = loadCards();
        void load(false);
        if (!m || !prefs().notify_runs || document.hasFocus()) return;
        // A failure needs you now. A finished lane waits until every running lane is done:
        // one notification when the whole batch is ready, not one per card.
        if (m.state === "failed") {
          // Unless Pitlane carries on by itself: more turns in the same session, or a retry it set.
          if (m.goes_on) return;
          await cardsFresh;
          if (m.repo === repo() && cards()?.find((c) => c.id === m.card)?.blocked?.retry_at) return;
          void systemNotify(t("#{card} {title} stopped", { card: m.card, title: m.card_title }), t("The run failed. Open it in Pitlane to see why."));
          return;
        }
        if (m.state !== "done") return;
        doneAway.push(m);
        await fresh;
        if ((runs() ?? []).some((r) => r.id !== m.id && isLive(r.state))) return;
        const done = doneAway.splice(0);
        if (done.length === 1) void systemNotify(t("#{card} {title} is ready for review", { card: m.card, title: m.card_title }), t("The agent finished. Its checks and the review are waiting in Pitlane."));
        else void systemNotify(t("All {n} lanes finished", { n: done.length }), done.map((d) => `#${d.card} ${d.card_title}`).join(" · "));
      },
    ).then((off) => {
      // Unmounted before the listener came back: let it go at once.
      if (unmounted) off();
      else offRun = off;
    });
    const onHash = () => {
      const r = runFromHash();
      setRunView(r);
      const rv = reviewFromHash();
      setReviewView(rv);
      if (!r && rv == null) {
        const t = location.hash.replace(/^#\/?/, "");
        if (isTab(t)) setTabSignal(t);
      }
    };
    window.addEventListener("hashchange", onHash);
    // A file opened from a terminal (or elsewhere) shows the Code screen.
    const onShowCode = () => {
      setRunView(null);
      setReviewView(null);
      setTab("code");
    };
    window.addEventListener("pitlane-show-code", onShowCode);
    // A card made elsewhere (a failed command in a terminal) asks to be opened.
    const onOpenCard = (e: Event) => setSheet({ kind: "card", id: (e as CustomEvent<number>).detail });
    window.addEventListener("pitlane-open-card", onOpenCard);
    const onTheme = () => rethemeCode();
    window.addEventListener("pitlane-theme", onTheme);
    // Coming back to the window: recheck local state (cheap, no network).
    const onFocus = () => {
      doneAway.length = 0; // you are back: the board shows what finished
      void loadCards();
      void loadRuns();
      void loadQueue();
      if (!fetching() && !sheet()) void load(false);
    };
    // Agents write cards through MCP while the app is open: pick them up within a few seconds.
    // One of each at a time: a new read drops the answer of the one on its way, so when the daemon
    // answers slower than the pace (a loaded Mac) polling over it would never show anything.
    const reading = new Set<string>();
    const once = (what: string, read: () => Promise<void>) => {
      if (reading.has(what)) return;
      reading.add(what);
      void read().finally(() => reading.delete(what));
    };
    const poll = setInterval(() => {
      if (document.visibilityState !== "visible") return;
      if ((tab() === "inbox" || tab() === "board") && !sheet()) once("cards", loadCards);
      if ((runs() ?? []).some((r) => isLive(r.state))) once("runs", loadRuns);
      if (jobs().some(isOpenJob)) once("queue", loadQueue);
    }, 4000);
    const onKey = shortcuts({ tab, setTab, sheet, setSheet, selected, setMenu, toggleTower: toggleSide, terminalIn });
    // ⌘P everywhere (Warp's key), ⌘K too except on the Terminal, where it clears the screen. Taken
    // while capturing, so no field that keeps its keys (an agent's prompt, a terminal) hides it.
    const onPaletteKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if (e.metaKey && k === "j" && !e.shiftKey && !e.altKey && !e.ctrlKey) {
        e.preventDefault();
        e.stopPropagation();
        void terminalHere();
        return;
      }
      if (!e.metaKey || e.shiftKey || e.altKey || e.ctrlKey || !(k === "p" || (k === "k" && !here("terminal")))) return;
      e.preventDefault();
      e.stopPropagation();
      setPalette((p) => !p);
    };
    window.addEventListener("keydown", onPaletteKey, true);
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      clearInterval(tick);
      unmounted = true;
      offRun();
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("pitlane-show-code", onShowCode);
      window.removeEventListener("pitlane-open-card", onOpenCard);
      window.removeEventListener("pitlane-theme", onTheme);
      clearInterval(poll);
      window.removeEventListener("keydown", onPaletteKey, true);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKey);
    });
  });

  const current = (): RepoEntry | undefined => settings().repos.find((r) => r.path === repo());
  const reviewing = () => (cards() ?? []).filter((c) => c.state === "review" && c.lane).map((c) => c.lane!);
  const needs = () =>
    yard()?.lanes.filter(
      (l) =>
        queuedGroup(l, jobs(), reviewing()) === "waiting" ||
        (runs() ?? []).some((r) => r.branch === l.branch && isLive(r.state) && r.waiting.length > 0),
    ).length ?? 0;
  const approvals = () =>
    (runs() ?? []).filter((r) => isLive(r.state)).flatMap((r) => r.waiting.map((approval) => ({ run: r, approval })));
  /** The screen in view, for the rail (a run or a review sits over the screen it came from). */
  const here = (t: Tab) => !runView() && reviewView() == null && tab() === t;
  const asking = () => openQuestions(cards() ?? []).length + approvals().length;
  const openCard = () => {
    const s = sheet();
    return s?.kind === "card" ? (cards() ?? []).find((c) => c.id === s.id) : undefined;
  };
  const select = (b: string) => {
    setSelected(b);
    setTab("lanes");
  };

  /** A screen, leaving a run or a review that sits over it. */
  const leaveTo = (to: Tab) => {
    setRunView(null);
    setReviewView(null);
    setTab(to);
    if (location.hash.startsWith("#run/") || location.hash.startsWith("#review/")) location.hash = `#${to}`;
  };

  // The Dock icon counts what waits on you (the Inbox and terminals that want you), as Mail counts
  // unread: seen from any Space without opening Pitlane. Nothing waiting, no badge.
  createEffect(
    on(
      () => asking() + terminalsWanting(),
      (n) => {
        if (!("__TAURI_INTERNALS__" in window)) return;
        void getCurrentWindow()
          .setBadgeCount(n > 0 ? n : undefined)
          .catch(() => {});
      },
    ),
  );

  /** ⌘J: a terminal where you are. A run or a card in review: its worktree; an open card: its lane's;
   *  Lanes: the selected lane's; Code: the folder it shows; anywhere else, the repository. On the
   *  Terminal it goes back where you came from. */
  let beforeTerminal: { tab: Tab; hash: string } | null = null;
  async function terminalHere() {
    const path = repo();
    if (!path) return;
    if (tab() === "terminal" && !runView() && reviewView() == null) {
      if (beforeTerminal) location.hash = beforeTerminal.hash || `#${beforeTerminal.tab}`;
      if (beforeTerminal && !beforeTerminal.hash.startsWith("#run/") && !beforeTerminal.hash.startsWith("#review/")) setTab(beforeTerminal.tab);
      beforeTerminal = null;
      return;
    }
    const lanes = yard()?.lanes ?? [];
    const byBranch = (b: string | null | undefined) => (b ? lanes.find((l) => l.branch === b && l.worktree) : undefined);
    const run = runView() ? (runs() ?? []).find((r) => r.id === runView()) : undefined;
    const card = reviewView() != null ? (cards() ?? []).find((c) => c.id === reviewView()) : openCard();
    let place: { path: string; label: string } | undefined;
    if (run?.worktree) place = { path: run.worktree, label: run.branch || t("Run") };
    else if (card && byBranch(card.lane)) place = { path: byBranch(card.lane)!.worktree!.path, label: card.lane! };
    else if (tab() === "lanes" && byBranch(selected())) place = { path: byBranch(selected())!.worktree!.path, label: selected()! };
    else if (tab() === "code" && codeRoot()) {
      const lane = lanes.find((l) => l.worktree?.path === codeRoot());
      place = { path: codeRoot()!, label: lane?.branch ?? current()?.name ?? t("repository") };
    }
    place ??= { path, label: current()?.name ?? t("repository") };
    const from = { tab: tab(), hash: location.hash };
    if (await terminalAt(place.path, place.label)) {
      if (sheet()) setSheet(null);
      beforeTerminal = from;
      leaveTo("terminal");
    }
  }

  /** Everything ⌘K reaches, built when it opens. */
  const commands = (): Command[] => {
    const out: Command[] = [];
    const go = (to: Tab) => () => leaveTo(to);
    const has = !!repo();
    if (has) {
      out.push(
        { id: "a-card", group: t("Actions"), icon: "plus", title: t("New card"), keys: "C", run: () => setSheet({ kind: "new-card" }) },
        { id: "a-lane", group: t("Actions"), icon: "branch", title: t("New lane"), keys: "N", run: () => setSheet({ kind: "new" }) },
        { id: "a-here", group: t("Actions"), icon: "term", title: t("Terminal where you are"), keys: "⌘J", run: () => void terminalHere() },
        { id: "a-term", group: t("Actions"), icon: "term", title: t("Terminal in the repository"), keys: "⌘T", searchOnly: true, run: () => void terminalIn(undefined) },
        { id: "a-fetch", group: t("Actions"), icon: "refresh", title: t("Fetch the remote"), keys: "⌘R", run: () => void (fetching() || load(true)) },
      );
    }
    out.push(
      sideFolds()
        ? { id: "a-tower", group: t("Actions"), icon: termFold.fold ? "side-open" : "side-close", title: termFold.fold ? t("Show the sidebar") : t("Hide the sidebar"), keys: "⌘\\", run: toggleSide }
        : { id: "a-tower", group: t("Actions"), icon: "tower", title: towerOpen() ? t("Fold the Tower") : t("Open the Tower"), keys: "⌘\\", run: toggleTower },
      { id: "a-settings", group: t("Actions"), icon: "settings", title: t("Settings"), keys: "⌘,", run: go("settings") },
    );
    const mode = presence();
    if (mode)
      out.push({
        id: "a-presence",
        group: t("Actions"),
        icon: "person",
        title: mode === "here" ? t("Go away (agents take their recommendation)") : t("I'm here (agents ask me their calls)"),
        run: () =>
          void api
            .presenceSet({ mode: mode === "here" ? "away" : "here" })
            .then((p) => toast({ tone: "done", title: p.mode === "here" ? t("You're here: agents ask you their calls") : t("Away: agents take their recommendation") }))
            .catch((e) => toast({ tone: "fail", title: t("Presence not changed"), body: msg(e) })),
      });
    if (has) {
      out.push(
        { id: "a-accounts", group: t("Actions"), icon: "person", title: t("Accounts"), searchOnly: true, run: () => setSheet({ kind: "accounts" }) },
        { id: "a-project", group: t("Actions"), icon: "note", title: t("Project context"), searchOnly: true, run: () => setSheet({ kind: "project" }) },
      );
    }
    // Agents at work: the ones waiting on you first.
    const liveRuns = (runs() ?? []).filter((r) => isLive(r.state)).sort((a, b) => b.waiting.length - a.waiting.length || b.started_at - a.started_at);
    for (const r of liveRuns)
      out.push({
        id: `r-${r.id}`,
        group: t("Agents"),
        icon: r.waiting.length ? "needs" : "run",
        title: `#${r.card} ${r.card_title}`,
        sub: r.waiting.length ? t("Needs you") : kindLabel(r),
        words: r.branch ?? "",
        run: () => openRun(r.id),
        alt: { label: t("in Terminal › Agents"), run: () => (selectAgent(r.id), go("terminal")()) },
      });
    // Cards: the ones that need you first, then in progress and the Backlog; landed ones only when searched for.
    const order = { needs: 0, progress: 1, queued: 2, backlog: 3, landed: 4 } as const;
    const placed = (cards() ?? []).map((c) => ({ c, p: place(c, yard()) })).sort((x, y) => order[x.p.column] - order[y.p.column]);
    for (const { c, p } of placed) {
      const last = lastRun(c.id);
      out.push({
        id: `c-${c.id}`,
        group: t("Cards"),
        icon: p.icon,
        title: `#${c.id} ${c.title}`,
        sub: p.label,
        words: `${c.id} ${c.lane ?? ""}`,
        searchOnly: p.column === "landed",
        run: () => (c.state === "review" && c.lane ? openReview(c.id) : setSheet({ kind: "card", id: c.id })),
        alt: last ? { label: t("its last run"), run: () => openRun(last.id) } : undefined,
      });
    }
    for (const l of yard()?.lanes ?? [])
      out.push({
        id: `l-${l.branch}`,
        group: t("Lanes"),
        icon: "branch",
        title: l.branch,
        sub: l.worktree ? shortPath(l.worktree.path) : t("no worktree"),
        searchOnly: true,
        run: () => select(l.branch),
        alt: l.worktree ? { label: t("a terminal in its worktree"), run: () => void terminalIn(l) } : undefined,
      });
    const screens: [Tab, IconName, string, string][] = [
      ["inbox", "inbox", t("Inbox"), "⌥1"],
      ["board", "board", t("Board"), "⌥2"],
      ["lanes", "lanes", t("Lanes"), "⌥3"],
      ["terminal", "term", t("Terminal"), "⌥4"],
      ["code", "code", t("Code"), "⌥5"],
      ["preview", "preview", t("Preview"), "⌥6"],
      ["ledger", "ledger", t("Ledger"), "⌥7"],
      ["reports", "note", t("Reports"), "⌥8"],
      ["performance", "gauge", t("Performance"), "⌥9"],
    ];
    for (const [to, icon, title, keys] of screens) out.push({ id: `s-${to}`, group: t("Go to"), icon, title, keys, run: go(to) });
    return out;
  };

  return (
    <div class="app">
      <div class="strip" data-tauri-drag-region>
          <div class="anchor strip-repo" ref={repoBox}>
            <button
              type="button"
              class="strip-repo-btn"
              aria-haspopup="menu"
              aria-expanded={menu()}
              title={current() ? t("Switch repository ({path})", { path: shortPath(current()!.path) }) : t("Add a repository")}
              onClick={() => setMenu(!menu())}
            >
              <Icon name="folder" small />
              <span class="strip-repo-name">{current()?.name ?? t("Add a repository")}</span>
              <Icon name="chev" small />
            </button>
            <Show when={menu()}>
              <div class="pop strip-pop" role="menu">
                <For each={settings().repos}>
                  {(r) => (
                    <div
                      class="menu-item"
                      role="menuitem"
                      tabindex="0"
                      aria-current={r.path === repo()}
                      onClick={() => switchTo(r.path)}
                      onKeyDown={(e) => e.key === "Enter" && switchTo(r.path)}
                    >
                      <Icon name="folder" small />
                      <span class="repo-entry">
                        <span class="repo-name">
                          {r.name}
                          <Show when={phaseIn(r.path) !== "idle"}>
                            <span class="repo-run" title={t("Run all is going in this repository")}>
                              {phaseIn(r.path) === "working" ? t("Run all") : phaseIn(r.path) === "asking" ? t("Questions") : t("Analysing")}
                            </span>
                          </Show>
                        </span>
                        <span class="repo-path" title={r.path}>
                          {shortPath(r.path)}
                        </span>
                      </span>
                    </div>
                  )}
                </For>
                <Show when={settings().repos.length > 0}>
                  <div class="menu-sep" />
                </Show>
                <Show when={repo()}>
                  <div
                    class="menu-item"
                    role="menuitem"
                    tabindex="0"
                    onClick={() => {
                      setMenu(false);
                      setSheet({ kind: "accounts" });
                    }}
                    onKeyDown={(e) => e.key === "Enter" && (setMenu(false), setSheet({ kind: "accounts" }))}
                  >
                    <Icon name="person" small />
                    {t("Accounts")}
                  </div>
                  <div
                    class="menu-item"
                    role="menuitem"
                    tabindex="0"
                    onClick={() => {
                      setMenu(false);
                      setSheet({ kind: "project" });
                    }}
                    onKeyDown={(e) => e.key === "Enter" && (setMenu(false), setSheet({ kind: "project" }))}
                  >
                    <Icon name="note" small />
                    {t("Project context")}
                  </div>
                </Show>
                <div class="menu-item" role="menuitem" tabindex="0" onClick={addRepo} onKeyDown={(e) => e.key === "Enter" && addRepo()}>
                  <Icon name="plus" small />
                  {t("Add repository")}
                </div>
              </div>
            </Show>
          </div>
          {/* The command center, as in VS Code and Zed: where ⌘K lives, for those who don't know the key yet. */}
          <button type="button" class="strip-search" onClick={() => setPalette(true)} aria-keyshortcuts="Meta+K">
            <Icon name="search" small />
            <span class="strip-search-text">{current() ? t("Search {name}", { name: current()!.name }) : t("Search")}</span>
            <span class="strip-search-keys">⌘K</span>
          </button>
      </div>
      <nav class="navrail" aria-label="Pitlane">
        <div class="navrail-items">
          <RailItem icon="inbox" label="Inbox" keys="⌥1" current={here("inbox")} onClick={() => setTab("inbox")} count={asking()} hot />
          <RailItem icon="board" label="Board" keys="⌥2" current={here("board")} onClick={() => setTab("board")} />
          <RailItem icon="lanes" label="Lanes" keys="⌥3" current={here("lanes")} onClick={() => setTab("lanes")} count={needs()} hot />
          <RailItem
            icon="term"
            label="Terminal"
            keys="⌥4 · ⌘T new"
            current={here("terminal")}
            onClick={() => setTab("terminal")}
            count={terminalsWanting() + agentsWanting()}
            hot
            live={agentsWorking() > 0}
            liveLabel={plural(agentsWorking(), "{n} agent working", "{n} agents working")}
          />
          <RailItem icon="code" label="Code" keys="⌥5" current={here("code")} onClick={() => setTab("code")} />
          <RailItem icon="preview" label="Preview" keys="⌥6" current={here("preview")} onClick={() => setTab("preview")} />
          <div class="navrail-sep" />
          <RailItem icon="ledger" label="Ledger" keys="⌥7" current={here("ledger")} onClick={() => setTab("ledger")} />
          <RailItem icon="note" label="Reports" keys="⌥8" current={here("reports")} onClick={() => setTab("reports")} />
          <RailItem icon="gauge" label="Performance" keys="⌥9" current={here("performance")} onClick={() => setTab("performance")} />
        </div>
        <div class="navrail-foot">
          <Show when={quota() && quota()!.events.length > 0 ? quota() : null}>
            {(qa) => (
              <button
                type="button"
                class="navrail-quota"
                aria-label={t("Usage of the {account} account. Open Accounts", { account: qa().account })}
                title={qa()
                  .events.map((q) => `${q.window === "five_hour" ? t("5-hour") : q.window === "seven_day" ? t("7-day") : q.window}: ${Math.round(q.used * 100)}%`)
                  .join("\n")}
                onClick={() => setSheet({ kind: "accounts" })}
              >
                <For each={qa().events}>
                  {(q) => (
                    <span
                      class="navrail-meter"
                      data-level={q.used >= 0.95 ? "out" : q.used * 100 >= (q.window === "seven_day" ? qa().seven_day_at : qa().five_hour_at) ? "high" : undefined}
                    >
                      <span class="navrail-meter-fill" style={{ "--v": String(q.used) }} />
                    </span>
                  )}
                </For>
              </button>
            )}
          </Show>
          <Show
            when={sideFolds()}
            fallback={<RailItem icon="tower" label={towerOpen() ? "Hide the Tower" : "Show the Tower"} keys="⌘\" current={towerOpen()} onClick={toggleTower} />}
          >
            <RailItem icon={termFold.fold ? "side-open" : "side-close"} label={termFold.fold ? "Show the sidebar" : "Hide the sidebar"} keys="⌘\" current={!termFold.fold} onClick={toggleSide} />
          </Show>
          <RailItem icon="settings" label="Settings" keys="⌘," current={tab() === "settings"} onClick={() => setTab("settings")} />
        </div>
      </nav>

      {/* On Lanes the screen is the Tower at full width, so the docked one folds away. */}
      <div class="panel">
      <div class="body" data-tower={towerHere() ? "" : undefined} style={{ "--tower-w": `${TOWER_W.get()}px` }}>
        <main class="main" data-flush={(tab() === "terminal" || tab() === "code" || tab() === "preview" || tab() === "settings") && !runView() && reviewView() == null ? "" : undefined}>
          <Show
            when={tab() !== "settings"}
            fallback={
              <SettingsScreen
                repo={repo()}
                repoName={current()?.name ?? t("repository")}
                repos={settings().repos}
                onAddRepo={addRepo}
                onReposChanged={async () => {
                  const s = await api.settings();
                  setSettings(s);
                  if (repo() && !s.repos.some((r) => r.path === repo())) {
                    const next = s.last ?? s.repos[0]?.path;
                    if (next) await switchTo(next);
                  }
                }}
                onAccounts={() => setSheet({ kind: "accounts" })}
              />
            }
          >
          <Show
            when={repo()}
            fallback={
              <Show when={booted()}>
                <div class="empty">
                  <div class="empty-title">{t("Every lane in view")}</div>
                  <p class="empty-body">
                    {t("Add a git repository to see each branch, its worktree and where it has really landed.")}
                  </p>
                  <button type="button" class="btn btn-primary btn-lg" onClick={addRepo}>
                    <Icon name="folder" small />
                    {t("Add repository")}
                  </button>
                </div>
              </Show>
            }
          >
            {(path) => (
              <ScreenBoundary resetOn={`${path()}|${tab()}|${runView() ?? ""}|${reviewView() ?? ""}`}>
              <Switch>
                <Match when={reviewView() != null && reviewView()}>
                  {(card) => (
                    <ReviewScreen
                      repo={path()}
                      card={(cards() ?? []).find((c) => c.id === card())}
                      run={lastRun(card())}
                      now={now()}
                      integration={yard()?.integration ?? null}
                      production={yard()?.production ?? null}
                      onBack={() => setTab("board")}
                      onOpenRun={openRun}
                      onOpenFile={(branch, file) => codeFile(branch, file)}
                      onChanged={() => {
                        void loadQueue();
                        void loadRuns();
                        void loadCards();
                        void load(false);
                      }}
                    />
                  )}
                </Match>
                <Match when={runView()}>
                  {(id) => (
                    <RunScreen
                      repo={path()}
                      card={(cards() ?? []).find((c) => c.id === (runs() ?? []).find((r) => r.id === id())?.card)}
                      onOpenCard={(cid) => setSheet({ kind: "card", id: cid })}
                      meta={(runs() ?? []).find((r) => r.id === id())}
                      yard={yard()}
                      now={now()}
                      onOpenFile={(branch, file) => codeFile(branch, file)}
                      onTerminal={(wt, label) => void openTerminal(wt, label).then((ok) => ok && leaveTo("terminal"))}
                      onCode={(wt) => (openInCode(wt, ""), leaveTo("code"))}
                      onBack={() => setTab(tab() === "lanes" ? "board" : tab())}
                      onChanged={() => {
                        void loadQueue();
                        void loadRuns();
                        void loadCards();
                      }}
                    />
                  )}
                </Match>
                <Match when={tab() === "lanes"}>
                  <LanesView
                    yard={yard()}
                    fetching={fetching()}
                    error={error()}
                    fetchedAt={fetchedAt()}
                    now={now()}
                    selected={selected()}
                    onSelect={setSelected}
                    onRefresh={() => void load(true)}
                    onShip={(lane, to) => setSheet({ kind: "ship", lane, to })}
                    onCheckout={checkout}
                    onTerminal={(l) => void terminalIn(l)}
                    checkingOut={checkingOut()}
                    onClean={() => setSheet({ kind: "clean" })}
                    onNew={() => setSheet({ kind: "new" })}
                    jobs={jobs()}
                    reviewing={reviewing()}
                    onQueueAll={(ls, to) => void queueAll(ls, to)}
                    onDequeue={(id) => void dequeue(id)}
                  />
                  <Switch>
                    <Match when={sheet()?.kind === "ship" && (sheet() as { lane: Lane; to: string })}>
                      {(s) => (
                        <ShipSheet
                          repo={path()}
                          lane={s().lane}
                          to={s().to}
                          onDone={() => void loadQueue()}
                          onClose={() => setSheet(null)}
                        />
                      )}
                    </Match>
                    <Match when={sheet()?.kind === "clean"}>
                      <CleanSheet repo={path()} onDone={() => void load(false)} onClose={() => setSheet(null)} />
                    </Match>
                  </Switch>
                </Match>
                <Match when={tab() === "inbox"}>
                  <InboxView
                    repo={path()}
                    cards={cards()}
                    active={tab() === "inbox" && !sheet()}
                    onChanged={() => void loadCards()}
                    onOpenCard={(id) => setSheet({ kind: "card", id })}
                    approvals={approvals()}
                    onOpenRun={openRun}
                    runs={runs() ?? []}
                    now={now()}
                  />
                </Match>
                <Match when={tab() === "board"}>
                  <div class="board-pane">
                  <div class="board-scroll">
                  <BoardView
                    repo={repo()}
                    onChanged={() => void loadCards()}
                    cards={cards()}
                    problems={problems()}
                    yard={yard()}
                    now={now()}
                    runs={runs() ?? []}
                    plan={plan()}
                    onOpen={(id) => {
                      const live = (runs() ?? []).find((r) => r.card === id && isLive(r.state));
                      if (live) openRun(live.id);
                      else if ((cards() ?? []).find((c) => c.id === id)?.state === "review") openReview(id);
                      else setSheet({ kind: "card", id });
                    }}
                    onNew={() => setSheet({ kind: "new-card" })}
                    onAnalyse={(ids) => void analyse(ids)}
                    onQuestions={() => setSheet({ kind: "questions" })}
                    onJoin={(ids) => void analyseInto(ids)}
                    batch={batchIds()}
                    joining={joining()}
                    onStartDev={(ids) => setSheet({ kind: "run-all", ids })}
                    onFinish={() => void finishBatch()}
                    onStopAll={stopRunAll}
                    phase={phase()}
                    queued={queue()}
                    share={agentShare()}
                    onReorder={(ids) => {
                      const path = repo();
                      if (!path) return;
                      // A card poll already on its way would bring back the old order: drop it.
                      dropCardPoll();
                      void api.cardReorder(path, ids).then(
                        () => loadCards(),
                        (e) => toast({ tone: "fail", title: t("The order was not saved"), body: msg(e) }),
                      );
                    }}
                  />
                  </div>
                  <AgentDock
                    onOpen={(id) => {
                      selectAgent(id);
                      setTab("terminal");
                    }}
                    onOpenAll={() => setTab("terminal")}
                  />
                  </div>
                </Match>
                <Match when={tab() === "reports"}>
                  <ReportsView reports={reports()} now={now()} onChanged={() => void loadReports()} onCardMade={() => void loadCards()} />
                </Match>
                <Match when={tab() === "performance"}>
                  <PerformanceView onOpen={openRun} />
                </Match>
                <Match when={tab() === "ledger"}>
                  <LedgerView runs={runs()} now={now()} onOpen={openRun} />
                </Match>
                <Match when={tab() === "preview"}>
                  <PreviewScreen onTerminal={() => void terminalHere()} />
                </Match>
                <Match when={tab() === "code"}>
                  <CodeScreen repo={path()} repoName={current()?.name ?? t("repository")} lanes={yard()?.lanes ?? []} />
                </Match>
                <Match when={tab() === "terminal"}>
                  <TerminalScreen repo={path()} repoName={current()?.name ?? t("repository")} lanes={yard()?.lanes ?? []} onOpenRun={openRun} />
                </Match>
              </Switch>
              </ScreenBoundary>
            )}
          </Show>
          </Show>
          <Show when={repo() ? openCard() : undefined}>
            {(card) => (
              <CardSheet
                repo={repo()!}
                card={card()}
                yard={yard()}
                now={now()}
                runs={(runs() ?? []).filter((r) => r.card === card().id)}
                onRun={(fresh) => void startRun(card().id, false, fresh)}
                onOpenRun={(id) => {
                  setSheet(null);
                  openRun(id);
                }}
                onChanged={() => void loadCards()}
                onClose={() => setSheet(null)}
              />
            )}
          </Show>
          <Show when={sheet()?.kind === "run-all" && (sheet() as { ids: number[] })}>
            {(s) => (
              <RunAllSheet
                repo={repo() ?? ""}
                count={s().ids.length}
                yard={yard()?.lanes ?? []}
                base={yard()?.production ?? "main"}
                production={yard()?.production ?? "main"}
                integration={yard()?.integration ?? null}
                selected={selected()}
                onClose={() => setSheet(null)}
                onStart={(c) => {
                  const ids = s().ids;
                  setSheet(null);
                  void startDev(ids, c);
                }}
              />
            )}
          </Show>
          <Show when={sheet()?.kind === "questions" && repo()}>
            {(path) => (
              <Show
                when={(() => {
                  const s = sheet();
                  return s?.kind === "questions" ? s.cards : undefined;
                })()}
                fallback={
                  <QuestionsSheet
                    repo={path()}
                    cards={(cards() ?? []).filter((c) => queue().includes(c.id))}
                    onChanged={() => void loadCards()}
                    onDone={() => {
                      setSheet(null);
                      if (phase() === "asking") analysed();
                    }}
                    onStop={stopRunAll}
                  />
                }
              >
                {(ids) => (
                  <QuestionsSheet
                    repo={path()}
                    cards={(cards() ?? []).filter((c) => ids().includes(c.id))}
                    onChanged={() => void loadCards()}
                    onDone={() => setSheet(null)}
                  />
                )}
              </Show>
            )}
          </Show>
          <Show when={sheet()?.kind === "project" && repo()}>
            {(path) => <ProjectSheet repo={path()} repoName={current()?.name ?? t("This repository")} onClose={() => setSheet(null)} />}
          </Show>
          <Show when={sheet()?.kind === "accounts" && repo()}>
            {(path) => (
              <AccountsSheet
                repo={path()}
                repoName={current()?.name ?? t("This repository")}
                onChanged={() => void loadRuns()}
                onClose={() => setSheet(null)}
              />
            )}
          </Show>
          <Show when={sheet()?.kind === "new-card" && repo()}>
            {(path) => (
              <NewCardSheet
                repo={path()}
                onDone={(id) => {
                  void loadCards();
                  if (tab() !== "board" && tab() !== "inbox") setTab("board");
                  void id;
                }}
                onClose={() => setSheet(null)}
              />
            )}
          </Show>
          <Show when={sheet()?.kind === "new" && repo()}>
            {(path) => (
              <NewLaneSheet
                repo={path()}
                base={yard()?.production ?? "main"}
                onDone={() => void load(false)}
                onClose={() => setSheet(null)}
              />
            )}
          </Show>
        </main>
        <Show when={towerHere()}>
          <Tower
            yard={yard()}
            runs={runs() ?? []}
            reviewing={reviewing()}
            jobs={jobs()}
            now={now()}
            selected={selected()}
            onSelect={select}
            onOpenRun={openRun}
          />
        </Show>
      </div>
      </div>
      <Show when={palette()}>
        <Palette commands={commands} onClose={() => setPalette(false)} />
      </Show>
      <Toasts />
    </div>
  );
}

/** One screen in the rail: its icon, a count when something waits, its name and key on hover. */
function RailItem(props: { icon: IconName; label: string; keys?: string; current: boolean; onClick: () => void; count?: number; hot?: boolean; live?: boolean; liveLabel?: string }) {
  return (
    <button
      type="button"
      class="navrail-item"
      aria-current={props.current ? "page" : undefined}
      aria-label={[t(props.label), props.count ? t("{n} waiting", { n: props.count }) : "", props.live ? props.liveLabel ?? t("working") : ""].filter(Boolean).join(", ")}
      data-tip={[props.keys ? `${t(props.label)}  ${t(props.keys)}` : t(props.label), props.live ? props.liveLabel : ""].filter(Boolean).join(" · ")}
      onClick={props.onClick}
    >
      <Icon name={props.icon} />
      <Show when={props.live && !props.count}>
        <span class="navrail-live" />
      </Show>
      <Show when={props.count}>
        <span class="navrail-count" data-hot={props.hot ? "" : undefined}>
          {props.count! > 9 ? "9+" : props.count}
        </span>
      </Show>
    </button>
  );
}
