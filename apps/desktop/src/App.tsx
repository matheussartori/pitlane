// The shell: top bar, the current screen, the Tower. Inbox, Board and Lanes read the repository; Ledger arrives with runs.
import { For, Match, Show, Switch, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { NEEDS_API_KEY, api, isLive, onRun, type Quota, type Card, type Lane, type RepoEntry, type RunEvent, type Plan, type RunMeta, type Settings, type ShipJob, type Yard, isOpenJob } from "./api";
import { AccountsSheet } from "./accounts";
import { LedgerView } from "./ledger";
import { ReviewScreen } from "./review";
import { RunScreen } from "./run";
import { BoardView, CardSheet, NewCardSheet } from "./board";
import { openQuestions } from "./cards";
import { InboxView } from "./inbox";
import { Icon, type IconName } from "./icons";
import { LanesView, Tower, queuedGroup } from "./lanes";
import { TerminalScreen, openTerminal, startTerminals, terminalsWanting } from "./terminals";
import { CodeScreen, openInCode, prewarmCode, rethemeCode, setCodePlaces } from "./code";
import { SettingsScreen } from "./settings";
import { loadPrefs, prefs } from "./prefs";
import { CleanSheet, NewLaneSheet, ShipSheet } from "./sheets";
import { Toasts, dismissable, toast } from "./ui";

type Tab = "inbox" | "board" | "lanes" | "ledger" | "code" | "terminal" | "settings";
type SheetState =
  | { kind: "ship"; lane: Lane; to: string }
  | { kind: "clean" }
  | { kind: "new" }
  | { kind: "card"; id: number }
  | { kind: "new-card" }
  | { kind: "accounts" }
  | null;

const TAB_KEY = "pitlane.tab";
const isTab = (t: string | null): t is Tab => t === "inbox" || t === "board" || t === "lanes" || t === "ledger" || t === "code" || t === "terminal" || t === "settings";
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
  const home = p.replace(/^\/Users\/[^/]+/, "~");
  const parts = home.split("/").filter(Boolean);
  return parts.length > 4 ? [parts[0], parts[1], "…", ...parts.slice(-2)].join("/") : home;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

export default function App() {
  const [settings, setSettings] = createSignal<Settings>({ repos: [], last: null });
  const [repo, setRepo] = createSignal<string | null>(null);
  const [tab, setTabSignal] = createSignal<Tab>(savedTab());
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
  const [cards, setCards] = createSignal<Card[] | null>(null);
  const [runs, setRuns] = createSignal<RunMeta[] | null>(null);
  const [quota, setQuota] = createSignal<Quota | null>(null);
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
  const [problems, setProblems] = createSignal<string[]>([]);
  const [yard, setYard] = createSignal<Yard | null>(null);
  const [fetching, setFetching] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [fetchedAt, setFetchedAt] = createSignal<number | null>(null);
  const [now, setNow] = createSignal(Date.now());
  const [selected, setSelected] = createSignal<string | null>(null);
  const [sheet, setSheet] = createSignal<SheetState>(null);
  const [menu, setMenu] = createSignal(false);
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
          title: `${r.branch} checked out, tracking ${r.base}`,
          body: r.setup_ok === false ? `${r.path}. The setup command failed.` : r.path,
          action: { label: "Copy path", run: () => void navigator.clipboard.writeText(r.path) },
        },
        12000,
      );
      await load(false);
    } catch (e) {
      toast({ tone: "fail", title: `${lane.branch} not checked out`, body: msg(e) });
    } finally {
      setCheckingOut(null);
    }
  }

  // Loads are tagged so a slow answer for an old repo never overwrites a newer one.
  let generation = 0;
  async function load(fetch: boolean) {
    const path = repo();
    if (!path) return;
    const gen = ++generation;
    if (fetch) setFetching(true);
    try {
      const y = await api.yard(path, fetch);
      if (gen !== generation) return;
      setYard(y);
      setError(null);
      if (fetch) setFetchedAt(Date.now());
    } catch (e) {
      if (gen === generation) setError(msg(e));
    } finally {
      if (fetch && gen === generation) setFetching(false);
    }
  }

  // Cards are small files: re-read them often, on their own, without touching git.
  let cardsGen = 0;
  async function loadCards() {
    const path = repo();
    if (!path) return;
    const gen = ++cardsGen;
    try {
      const r = await api.cards(path);
      if (gen !== cardsGen || path !== repo()) return;
      setCards(r.cards);
      setProblems(r.problems);
    } catch (e) {
      if (gen === cardsGen) setProblems([msg(e)]);
    }
  }

  // Runs come from the daemon (or its records on disk). Live ones are followed, so their end
  // refreshes the screens even when nobody has the run open.
  let runsGen = 0;
  async function loadRuns() {
    const path = repo();
    if (!path) return;
    const gen = ++runsGen;
    try {
      const r = await api.runs(path);
      if (gen !== runsGen || path !== repo()) return;
      setRuns(r);
      for (const m of r) if (isLive(m.state)) void api.runFollow(m.id, 0).catch(() => {});
      setQuota(await api.quota(path));
    } catch {
      if (gen === runsGen) setRuns([]);
    }
  }

  // The merge queue: polled while something is in line; a job that finishes refreshes the lanes.
  const [jobs, setJobs] = createSignal<ShipJob[]>([]);
  let jobsGen = 0;
  async function loadQueue() {
    const path = repo();
    if (!path) return;
    const gen = ++jobsGen;
    try {
      const next = await api.queue(path);
      if (gen !== jobsGen || path !== repo()) return;
      const wasOpen = new Set(jobs().filter(isOpenJob).map((j) => j.id));
      const landed = next.filter((j) => wasOpen.has(j.id) && !isOpenJob(j));
      setJobs(next);
      for (const j of landed) {
        if (j.state === "done") toast({ tone: "done", title: `${j.branch} is in ${j.target}` });
        else
          toast({
            tone: "fail",
            title: `${j.branch} did not ship to ${j.target}`,
            body: j.state === "conflict" ? `It conflicts in ${j.conflicts.join(", ")}.` : (j.error ?? undefined),
          });
      }
      if (landed.length) {
        void load(false);
        void loadCards();
      }
    } catch {
      if (gen === jobsGen) setJobs([]);
    }
  }
  async function queueAll(lanes: Lane[], to: string) {
    const path = repo();
    if (!path) return;
    let n = 0;
    for (const l of lanes) {
      try {
        await api.enqueue(path, l.branch, to);
        n++;
      } catch (e) {
        toast({ tone: "fail", title: `${l.branch} stayed out of line`, body: msg(e) });
      }
    }
    if (n) toast({ tone: "done", title: `${n} ${n === 1 ? "lane is" : "lanes are"} in line for ${to}`, body: "The queue ships them one at a time." });
    void loadQueue();
  }
  async function dequeue(id: string) {
    try {
      await api.dequeue(id);
    } catch (e) {
      toast({ tone: "fail", title: "Not cleared", body: msg(e) });
    }
    void loadQueue();
  }

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
  // VS Code's own folder picker offers the repository and the lanes' worktrees.
  createEffect(() => {
    const r = repo();
    if (r) setCodePlaces(r, current()?.name ?? "repository", yard()?.lanes ?? [], yard()?.head);
  });

  // The planner, read again when the cards change: which open cards should wait, and for what.
  const [plan, setPlan] = createSignal<Plan | null>(null);
  createEffect(
    on(cards, () => {
      const path = repo();
      if (path) void api.plan(path).then(setPlan, () => setPlan(null));
    }),
  );

  async function terminalIn(lane: Lane | undefined) {
    const path = repo();
    if (!path) return;
    const ok = lane?.worktree
      ? await openTerminal(lane.worktree.path, lane.branch)
      : await openTerminal(path, current()?.name ?? "repository");
    if (ok) setTab("terminal");
  }

  async function say(title: string, body: string) {
    try {
      let ok = await isPermissionGranted();
      if (!ok) ok = (await requestPermission()) === "granted";
      if (ok) sendNotification({ title, body });
    } catch {
      /* the browser preview has no notifications */
    }
  }

  async function notify(run: string, detail: string) {
    if (!prefs().notify_approvals) return;
    try {
      let ok = await isPermissionGranted();
      if (!ok) ok = (await requestPermission()) === "granted";
      const m = (runs() ?? []).find((r) => r.id === run);
      if (ok) sendNotification({ title: m ? `#${m.card} ${m.card_title} is waiting for you` : "An agent is waiting for you", body: detail });
    } catch {
      /* the browser preview has no notifications */
    }
  }

  async function startRun(card: number, allowApiKey = false) {
    const path = repo();
    if (!path) return;
    try {
      const m = await api.runStart(path, card, allowApiKey);
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
            title: "Only the API key can take this card now",
            body: `${text.slice(NEEDS_API_KEY.length)}. It is paid per token.`,
            action: { label: "Use the API key", run: () => void startRun(card, true) },
          },
          0,
        );
        return;
      }
      toast({ tone: "fail", title: `Card #${card} did not start`, body: text });
    }
  }

  async function switchTo(path: string) {
    setMenu(false);
    if (path === repo()) return;
    generation++;
    setRepo(path);
    setYard(null);
    setCards(null);
    setRuns(null);
    setProblems([]);
    setError(null);
    setFetchedAt(null);
    setSelected(null);
    setFetching(false);
    void api.selectRepo(path);
    void loadCards();
    void loadRuns();
    void loadQueue();
    await load(false); // local refs first: instant
    await load(true); // then fetch and GitHub
  }

  async function addRepo() {
    setMenu(false);
    const dir = await openDialog({ directory: true, multiple: false, title: "Choose a git repository" });
    if (typeof dir !== "string") return;
    try {
      const entry = await api.addRepo(dir);
      setSettings(await api.settings());
      await switchTo(entry.path);
    } catch (e) {
      toast({ tone: "fail", title: "Not added", body: msg(e) });
    }
  }

  onMount(async () => {
    void startTerminals();
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

    // The clock ticks every second while an agent runs, every 30 s otherwise.
    const tick = setInterval(() => {
      if ((runs() ?? []).some((r) => isLive(r.state)) || Date.now() - now() >= 30_000) setNow(Date.now());
    }, 1000);
    const offRun = await onRun(
      (run, _i, e) => {
        if (e.kind !== "approval" && e.kind !== "decided") return;
        void loadRuns();
        // Not looking at Pitlane: say so where the person is.
        if (e.kind === "approval" && !document.hasFocus()) void notify(run, e.approval.detail || e.approval.tool);
      },
      (_run, m) => {
        void loadRuns();
        void loadCards();
        void load(false);
        // A lane's agent finished or stopped while you were elsewhere: say which, and what now.
        if (m && prefs().notify_runs && !document.hasFocus() && (m.state === "done" || m.state === "failed")) {
          void say(
            m.state === "done" ? `#${m.card} ${m.card_title} is ready for review` : `#${m.card} ${m.card_title} stopped`,
            m.state === "done" ? "The agent finished. Its checks and the review are waiting in Pitlane." : "The run failed. Open it in Pitlane to see why.",
          );
        }
      },
    );
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
    const onTheme = () => rethemeCode();
    window.addEventListener("pitlane-theme", onTheme);
    // Coming back to the window: recheck local state (cheap, no network).
    const onFocus = () => {
      void loadCards();
      void loadRuns();
      void loadQueue();
      if (!fetching() && !sheet()) void load(false);
    };
    // Agents write cards through MCP while the app is open: pick them up within a few seconds.
    const poll = setInterval(() => {
      if (document.visibilityState === "visible" && (tab() === "inbox" || tab() === "board") && !sheet()) void loadCards();
      if (document.visibilityState === "visible" && (runs() ?? []).some((r) => isLive(r.state))) void loadRuns();
      if (document.visibilityState === "visible" && jobs().some(isOpenJob)) void loadQueue();
    }, 4000);
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
      if (e.metaKey && e.key === ",") {
        e.preventDefault();
        setTab("settings");
      } else if (e.metaKey && e.key.toLowerCase() === "r") {
        e.preventDefault();
        if (!fetching()) void load(true);
      } else if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === "t" && repo()) {
        // A terminal where you are: the selected lane's worktree on Lanes, else the repository.
        e.preventDefault();
        const lane = tab() === "lanes" ? yard()?.lanes.find((l) => l.branch === selected() && l.worktree) : undefined;
        void terminalIn(lane);
      } else if (!typing && !e.metaKey && !e.ctrlKey && !sheet() && repo()) {
        if ((e.key === "n" || e.key === "N") && tab() === "lanes") {
          e.preventDefault();
          setSheet({ kind: "new" });
        } else if ((e.key === "c" || e.key === "C") && (tab() === "board" || tab() === "inbox")) {
          e.preventDefault();
          setSheet({ kind: "new-card" });
        } else if (/^[1-4]$/.test(e.key) && e.altKey) {
          setTab((["inbox", "board", "lanes", "ledger"] as const)[Number(e.key) - 1]!);
        }
      }
      if (e.key === "Escape") setMenu(false);
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      clearInterval(tick);
      offRun();
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("pitlane-show-code", onShowCode);
      window.removeEventListener("pitlane-theme", onTheme);
      clearInterval(poll);
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

  return (
    <div class="app">
      <div class="strip" data-tauri-drag-region>
          <div class="anchor strip-repo" ref={repoBox}>
            <button
              type="button"
              class="strip-repo-btn"
              aria-haspopup="menu"
              aria-expanded={menu()}
              title={current() ? `Switch repository (${shortPath(current()!.path)})` : "Add a repository"}
              onClick={() => setMenu(!menu())}
            >
              <Icon name="folder" small />
              <span class="strip-repo-name">{current()?.name ?? "Add a repository"}</span>
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
                        <span class="repo-name">{r.name}</span>
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
                    Accounts
                  </div>
                </Show>
                <div class="menu-item" role="menuitem" tabindex="0" onClick={addRepo} onKeyDown={(e) => e.key === "Enter" && addRepo()}>
                  <Icon name="plus" small />
                  Add repository
                </div>
              </div>
            </Show>
          </div>
      </div>
      <nav class="navrail" aria-label="Pitlane">
        <div class="navrail-items">
          <RailItem icon="inbox" label="Inbox" keys="⌥1" current={here("inbox")} onClick={() => setTab("inbox")} count={asking()} hot />
          <RailItem icon="board" label="Board" keys="⌥2" current={here("board")} onClick={() => setTab("board")} />
          <RailItem icon="lanes" label="Lanes" keys="⌥3" current={here("lanes")} onClick={() => setTab("lanes")} count={needs()} hot />
          <RailItem icon="ledger" label="Ledger" keys="⌥4" current={here("ledger")} onClick={() => setTab("ledger")} />
          <div class="navrail-sep" />
          <RailItem icon="code" label="Code" current={here("code")} onClick={() => setTab("code")} />
          <RailItem
            icon="term"
            label="Terminal"
            keys="⌘T new"
            current={here("terminal")}
            onClick={() => setTab("terminal")}
            count={terminalsWanting() > 0 ? terminalsWanting() : 0}
            hot
          />
        </div>
        <div class="navrail-foot">
          <Show when={quota() && quota()!.events.length > 0 ? quota() : null}>
            {(qa) => (
              <button
                type="button"
                class="navrail-quota"
                aria-label={`Usage of the ${qa().account} account. Open Accounts`}
                title={qa()
                  .events.map((q) => `${q.window === "five_hour" ? "5-hour" : q.window === "seven_day" ? "7-day" : q.window}: ${Math.round(q.used * 100)}%`)
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
          <RailItem icon="settings" label="Settings" keys="⌘," current={tab() === "settings"} onClick={() => setTab("settings")} />
        </div>
      </nav>

      {/* On Lanes the screen is the Tower at full width, so the docked one folds away. */}
      <div class="panel">
      <div class="body" data-tower={!["lanes", "terminal", "code", "settings"].includes(tab()) ? "" : undefined}>
        <main class="main" data-flush={(tab() === "terminal" || tab() === "code" || tab() === "settings") && !runView() && reviewView() == null ? "" : undefined}>
          <Show
            when={tab() !== "settings"}
            fallback={
              <SettingsScreen
                repo={repo()}
                repoName={current()?.name ?? "repository"}
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
                  <div class="empty-title">Every lane in view</div>
                  <p class="empty-body">
                    Add a git repository to see each branch, its worktree and where it has really landed.
                  </p>
                  <button type="button" class="btn btn-primary btn-lg" onClick={addRepo}>
                    <Icon name="folder" small />
                    Add repository
                  </button>
                </div>
              </Show>
            }
          >
            {(path) => (
              <Switch>
                <Match when={reviewView() != null && reviewView()}>
                  {(card) => (
                    <ReviewScreen
                      repo={path()}
                      card={(cards() ?? []).find((c) => c.id === card())}
                      run={lastRun(card())}
                      now={now()}
                      integration={yard()?.integration ?? null}
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
                      meta={(runs() ?? []).find((r) => r.id === id())}
                      yard={yard()}
                      now={now()}
                      onOpenFile={(branch, file) => codeFile(branch, file)}
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
                  />
                </Match>
                <Match when={tab() === "board"}>
                  <BoardView
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
                  />
                </Match>
                <Match when={tab() === "ledger"}>
                  <LedgerView runs={runs()} now={now()} onOpen={openRun} />
                </Match>
                <Match when={tab() === "code"}>
                  <CodeScreen repo={path()} repoName={current()?.name ?? "repository"} lanes={yard()?.lanes ?? []} />
                </Match>
                <Match when={tab() === "terminal"}>
                  <TerminalScreen repo={path()} repoName={current()?.name ?? "repository"} lanes={yard()?.lanes ?? []} />
                </Match>
              </Switch>
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
                onRun={() => void startRun(card().id)}
                onOpenRun={(id) => {
                  setSheet(null);
                  openRun(id);
                }}
                onChanged={() => void loadCards()}
                onClose={() => setSheet(null)}
              />
            )}
          </Show>
          <Show when={sheet()?.kind === "accounts" && repo()}>
            {(path) => (
              <AccountsSheet
                repo={path()}
                repoName={current()?.name ?? "This repository"}
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
        <Show when={!["lanes", "terminal", "code", "settings"].includes(tab())}>
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
      <Toasts />
    </div>
  );
}

/** One screen in the rail: its icon, a count when something waits, its name and key on hover. */
function RailItem(props: { icon: IconName; label: string; keys?: string; current: boolean; onClick: () => void; count?: number; hot?: boolean }) {
  return (
    <button
      type="button"
      class="navrail-item"
      aria-current={props.current ? "page" : undefined}
      aria-label={props.count ? `${props.label}, ${props.count} waiting` : props.label}
      data-tip={props.keys ? `${props.label}  ${props.keys}` : props.label}
      onClick={props.onClick}
    >
      <Icon name={props.icon} />
      <Show when={props.count}>
        <span class="navrail-count" data-hot={props.hot ? "" : undefined}>
          {props.count! > 9 ? "9+" : props.count}
        </span>
      </Show>
    </button>
  );
}
