// Browser preview (vite dev without Tauri): answers the commands from a fixture made by
// `pit lanes --json` on a throwaway demo repo, so the UI can be designed and linted in a browser.
// Never part of a production build: api.ts only imports it under import.meta.env.DEV.
import cards from "./cards.json";
import runMeta from "./run-meta.json";
import runTimeline from "./run-timeline.json";
import yard from "./yard.json";

const repo = { path: "/Users/you/Projects/demo", name: "demo" };

type Ev = Record<string, unknown>;
const pairs = (evs: unknown[]) => evs.map((e, i) => [i, e]);
const now = () => Math.floor(Date.now() / 1000);
const zero = { input: 0, output: 0, cache_read: 0, cache_write: 0 };
/** The real run's events, moved to another card's lane. */
const moved = (branch: string, dir: string) =>
  JSON.parse(JSON.stringify(runTimeline).replaceAll("feat/9-describe-the-demo-in-the-readme", branch).replaceAll("feat-9-describe-the-demo-in-the-readme", dir)) as Ev[];

// A live run made from the first steps of the real one, on card #3's lane.
const liveRun = () => ({ ...runMeta, id: "3-live", card: 3, card_title: "Strip scripts from SVG avatars", branch: "fix/avatar-svg",
  worktree: "/Users/you/.pitlane/wt/demo/fix-avatar-svg", state: "running", started_at: now() - 95, ended_at: null,
  effort: "high", cost_usd: null, duration_ms: null, turns: null, handoff: null, tokens: zero, denied: [], error: null,
  waiting: [{ id: "toolu_demo", tool: "Bash", detail: "npm test -- avatar", input: { command: "npm test -- avatar" },
    rule: "Bash(npm test:*)", asked_at: now() - 12 }] });
const liveTimeline = () =>
  pairs([...moved("fix/avatar-svg", "fix-avatar-svg").slice(0, 5),
    { kind: "approval", approval: liveRun().waiting[0] }]);

// A failed run: the agent was refused a command and stopped with an error.
const failedRun = () => ({ ...runMeta, id: "5-failed", card: 5, card_title: "Kart drift tier 3", branch: "feat/kart-drift",
  worktree: "/Users/you/.pitlane/wt/demo/feat-kart-drift", state: "failed", started_at: now() - 3600, ended_at: now() - 3540,
  cost_usd: 0.41, duration_ms: 60000, turns: 9, handoff: null, denied: ["Bash: npm test -- drift"],
  error: "The agent stopped with an error: tests could not run without npm test." });
const failedTimeline = () => {
  const evs = moved("feat/kart-drift", "feat-kart-drift").slice(0, 5);
  evs.push({ kind: "tool", id: "x1", name: "Bash", detail: "npm test -- drift" });
  evs.push({ kind: "tool-done", id: "x1", ok: false, detail: "This command requires approval" });
  evs.push({ kind: "finished", ok: false, cost_usd: 0.41, duration_ms: 60000, turns: 9, tokens: zero, handoff: null, denied: ["Bash: npm test -- drift"] });
  evs.push({ kind: "exited", code: 1, state: "failed" });
  return pairs(evs);
};

let nextTerm = 3;

// Performance: a 32 GB Mac with two cards working and one paused for the mode.
const GB = 1024 ** 3;
const perfCfg = { mode: "light", auto: true, away_minutes: 10, light_agents: 2, light_background: true };
// Presence: here, so card 3's agent waits on a question mid-run.
const presenceCfg = { mode: "here", auto: false, wait_minutes: 10, per_run: 3 };
for (const q of (cards as { id: number; questions?: { run?: string; asked_at?: string }[] }[]).find((c) => c.id === 3)?.questions ?? []) {
  if (q.run) q.asked_at = new Date(Date.now() - 252_000).toISOString();
}
const perfHistory = () =>
  Array.from({ length: 180 }, (_, i) => {
    const w = Math.sin(i / 14) * 0.5 + 0.5;
    return { t: now() - (179 - i) * 2, cpu: 30 + w * 35 + (i % 7), mem_used: (19 + w * 4) * GB, agents_mem: (4 + w * 3) * GB, agents_cpu: 120 + w * 260 };
  });
const perfSnap = () => ({
  governed: true, mode: perfCfg.mode, config: perfCfg, allowed: perfCfg.mode === "full" ? 3 : perfCfg.light_agents, at: now(), cap: 2, max: 3, adaptive: true,
  idle_secs: 42, tight: false, critical: false, background: perfCfg.mode === "light", awake: perfCfg.mode === "full",
  system: { mem_total: 32 * GB, mem_used: 22.4 * GB, free_pct: 30, pressure: "normal", swap_used: 0.6 * GB, swap_total: 2 * GB, cpu: 46, cores: 12, p_cores: 8, load: 7.4 },
  agents: [
    { id: "3-live", card: 3, title: "Strip scripts from SVG avatars", kind: "run", repo: repo.path, branch: "fix/avatar-svg", model: "opus", started_at: now() - 900, pid: 4242,
      state: "background", paused_by: null, paused_since: null, mem: 3.1 * GB, cpu: 212, procs: 9,
      top: [{ name: "cargo", mem: 1.6 * GB, cpu: 180 }, { name: "claude", mem: 0.38 * GB, cpu: 3 }, { name: "node", mem: 0.31 * GB, cpu: 21 }, { name: "rustc", mem: 0.6 * GB, cpu: 8 }] },
    { id: "4-live", card: 4, title: "Invite limits per workspace", kind: "run", repo: repo.path, branch: "feat/invite-limits", model: "opus", started_at: now() - 600, pid: 4300,
      state: "background", paused_by: null, paused_since: null, mem: 1.2 * GB, cpu: 64, procs: 4,
      top: [{ name: "node", mem: 0.7 * GB, cpu: 60 }, { name: "claude", mem: 0.41 * GB, cpu: 4 }] },
    { id: "6-live", card: 6, title: "Kart drift tier 3", kind: "run", repo: repo.path, branch: "feat/kart-drift", model: "sonnet", started_at: now() - 300, pid: 4400,
      state: "paused", paused_by: "mode", paused_since: now() - 140, mem: 0.9 * GB, cpu: 0, procs: 3,
      top: [{ name: "claude", mem: 0.36 * GB, cpu: 0 }, { name: "npm", mem: 0.5 * GB, cpu: 0 }] },
    { id: "j-1", card: 2, title: "Cookie banner copy", kind: "judge", repo: repo.path, branch: "feat/cookie-copy", model: "sonnet", started_at: now() - 60, pid: 4500,
      state: "background", paused_by: null, paused_since: null, mem: 0.3 * GB, cpu: 2, procs: 1, top: [{ name: "claude", mem: 0.3 * GB, cpu: 2 }] },
  ],
  agents_mem: 5.5 * GB, agents_cpu: 278,
  others: [{ name: "Google Chrome", mem: 6.2 * GB, cpu: 38 }, { name: "Pitlane", mem: 1.4 * GB, cpu: 6 }, { name: "Figma", mem: 1.1 * GB, cpu: 4 },
    { name: "Slack", mem: 0.8 * GB, cpu: 1 }, { name: "WindowServer", mem: 0.6 * GB, cpu: 12 }, { name: "Finder", mem: 0.2 * GB, cpu: 0 }],
  history: perfHistory(),
  events: [
    { t: now() - 140, kind: "paused_mode", card: 6, title: "Kart drift tier 3", n: 2 },
    { t: now() - 700, kind: "back" },
    { t: now() - 3600, kind: "resumed", card: 4, title: "Invite limits per workspace", n: 95 },
    { t: now() - 3700, kind: "paused_memory", card: 4, title: "Invite limits per workspace", n: 9 },
    { t: now() - 7200, kind: "away", n: 10 },
  ],
});

export async function preview<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((r) => setTimeout(r, 250));
  switch (cmd) {
    case "perf":
      return perfSnap() as T;
    case "perf_set":
      Object.assign(perfCfg, (args?.config ?? {}) as object);
      if (args?.config && "mode" in (args.config as object) && !("auto" in (args.config as object))) perfCfg.auto = false;
      return { config: perfCfg } as T;
    case "perf_hold":
      return { changed: true } as T;
    case "presence":
      return { config: presenceCfg, mode: presenceCfg.mode, away_minutes: perfCfg.away_minutes } as T;
    case "presence_set":
      Object.assign(presenceCfg, (args?.config ?? {}) as object);
      if (args?.config && "mode" in (args.config as object) && !("auto" in (args.config as object))) presenceCfg.auto = false;
      return { config: presenceCfg, mode: presenceCfg.mode, away_minutes: perfCfg.away_minutes } as T;
    case "settings":
      return { repos: [repo], last: repo.path } as T;
    case "select_repo":
      return undefined as T;
    case "yard": {
      // ?state=loading|empty|error previews the other three states of the screen.
      const state = new URLSearchParams(location.search).get("state");
      if (state === "loading") return new Promise<T>(() => {});
      if (state === "error") throw "Could not fetch origin: ssh: connect to host github.com port 22: Network is unreachable";
      if (state === "empty") return { ...yard, fetched: true, lanes: [], only_in_integration: [] } as T;
      return { ...yard, fetched: true } as T;
    }
    case "runs_list": {
      if (new URLSearchParams(location.search).get("state") === "empty") return [] as T;
      return [liveRun(), failedRun(), { ...runMeta, round: 0, verdict: "changes", proofs_ok: false }] as T;
    }
    case "sessions_list": {
      if (new URLSearchParams(location.search).get("state") === "empty") return [] as T;
      const now = Math.floor(Date.now() / 1000);
      const live = liveRun();
      return [
        { ...live, id: "7-analysis-1", kind: "analysis", card: 7, card_title: "Rate-limit the invite endpoint", state: "running", started_at: now - 40, model: "sonnet", cost_usd: 0.03 },
        { ...live, id: "8-analysis-1", kind: "analysis", card: 8, card_title: "Avatar upload refuses SVG", state: "running", started_at: now - 35, model: "sonnet", waiting: [] },
        { ...live, started_at: now - 90 },
      ] as T;
    }
    case "run_timeline":
      if (String(args?.run ?? "").includes("analysis"))
        return pairs([
          { kind: "started", model: "sonnet", session: "s" },
          { kind: "text", text: "Reading the card and the invite routes. The limit lives in `src/invites/limit.ts`; **two** callers skip it:\n\n- `sendInvite` in the API\n- the resend job" },
          { kind: "tool", id: "a1", name: "Grep", detail: "rateLimit" },
          { kind: "tool-done", id: "a1", ok: true, detail: "src/invites/limit.ts", lines: 3 },
          { kind: "tool", id: "r1", name: "Read", detail: "src/invites/limit.ts" },
          { kind: "tool-done", id: "r1", ok: true, detail: "export const LIMIT = 10;", lines: 48 },
          { kind: "tool", id: "e1", name: "Edit", detail: "src/invites/limit.ts", change: { old: "export const LIMIT = 10;", new: "export const LIMIT = 10;\nexport const WINDOW_MS = 60_000;", cut: 0 } },
          { kind: "tool-done", id: "e1", ok: true, detail: "The file src/invites/limit.ts has been updated." },
          { kind: "tool", id: "b1", name: "Bash", detail: "npm test -- invites" },
          { kind: "tool-done", id: "b1", ok: false, detail: "FAIL src/invites/limit.test.ts", lines: 12 },
          { kind: "tool", id: "task1", name: "Task", detail: "Find every caller of sendInvite" },
          { kind: "tool", id: "s1", name: "Grep", detail: "sendInvite(", parent: "task1" },
          { kind: "text", text: "Found two callers.", parent: "task1" },
          { kind: "said", text: "Make sure it also covers resend." },
        ]) as T;
      return (location.hash.includes("3-live") ? liveTimeline() : location.hash.includes("5-failed") ? failedTimeline() : pairs(runTimeline)) as T;
    case "mcp_connected":
      return (location.search.includes("mcp=no") ? false : true) as T;
    case "run_follow":
      return undefined as T;
    case "accounts_list":
      return {
        accounts: [
          { id: "default", kind: "subscription", status: { logged_in: true, method: "claude.ai", email: "you@example.com", plan: "max_20x", plan_name: "Ultra", org: "Your organization", login_expires_at: new Date(Date.now() + 26 * 864e5).toISOString(), problem: null },
            usage: { five_hour: null, seven_day: 27 }, here: true, live: true,
            windows: [
              { window: "weekly_all", used: 27, resets_at: now() + 3 * 86400 + 5000 },
              { window: "weekly_scoped", model: "Fable", used: 41, resets_at: now() + 3 * 86400 + 5000 },
            ],
            overage: { status: "rejected", reason: "org_level_disabled", using: false, at: now() - 300 } },
          { id: "work", kind: "team", status: { logged_in: false, method: null, email: null, problem: null }, usage: null, here: false },
          { id: "anthropic-api", kind: "api-key", status: { logged_in: true, method: "api key helper", email: null, problem: null }, usage: null, here: false },
        ],
        route: ["opus@repo", "sonnet@repo", "opus@anthropic-api"],
        five_hour_at: 90,
        seven_day_at: 95,
        file: "/Users/you/.pitlane/routing.toml",
      } as T;
    case "run_verification":
      return {
        run: runMeta.id,
        no_commands: false,
        max_rounds: 3,
        started_at: now() - 600,
        ended_at: now() - 560,
        proofs: [
          { command: "test -f README.md", ok: true, code: 0, duration_ms: 12, tail: "" },
          { command: "npm run lint", ok: false, code: 1, duration_ms: 2140,
            tail: "README.md:1:1  MD041  First line in a file should be a top-level heading\n✖ 1 problem (1 error, 0 warnings)" },
        ],
        judge: {
          model: "sonnet", cost_usd: 0.04, duration_ms: 18000, error: null,
          judgement: {
            verdict: "changes",
            summary: "The sentence is right, but the README lost its heading, which the lint rule in this repo requires.",
            findings: [
              { file: "README.md", line: 1, severity: "major", text: "The first line is no longer a top-level heading; keep `# demo` and put the sentence under it." },
              { file: "README.md", line: 1, severity: "nit", text: "End the sentence without the trailing space." },
            ],
          },
        },
      } as T;
    case "lane_diff":
      return {
        base: "8e924ea0", head: "951104d0",
        files: [
          { path: "README.md", added: 1, removed: 1 },
          { path: "src/kart/drift.ts", added: 6, removed: 3 },
          { path: "src/kart/drift.test.ts", added: 4, removed: 0 },
          { path: "src/lobby/old-timer.ts", added: 0, removed: 3 },
        ],
        patch: [
          "diff --git a/README.md b/README.md\nindex 1b5a4e9..3c1f0a2 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-demo\n+This is a demo repository for Pitlane.",
          "diff --git a/src/kart/drift.ts b/src/kart/drift.ts\nindex 1..2 100644\n--- a/src/kart/drift.ts\n+++ b/src/kart/drift.ts\n@@ -10,8 +10,11 @@ export function drift(k: Kart) {\n   const speed = k.speed;\n-  const grip = 0.8;\n-  const angle = k.angle * grip;\n-  return angle;\n+  const grip = surfaceGrip(k.surface);\n+  const angle = k.angle * grip;\n+  if (k.tier >= 3) {\n+    return angle * 1.2;\n+  }\n+  return angle;\n }\n \n export const DRIFT_TIERS = 3;",
          "diff --git a/src/kart/drift.test.ts b/src/kart/drift.test.ts\nnew file mode 100644\nindex 0000000..3\n--- /dev/null\n+++ b/src/kart/drift.test.ts\n@@ -0,0 +1,4 @@\n+import { drift } from './drift';\n+test('tier 3 drifts further', () => {\n+  expect(drift(kart(3))).toBeGreaterThan(drift(kart(2)));\n+});",
          "diff --git a/src/lobby/old-timer.ts b/src/lobby/old-timer.ts\ndeleted file mode 100644\nindex 4..0000000\n--- a/src/lobby/old-timer.ts\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-export function timer() {\n-  return 0;\n-}",
        ].join("\n") + "\n",
        truncated: false,
      } as T;
    case "plan":
      return {
        footprints: [],
        clashes: [{ card: 6, with_card: 7, with_lane: null, files: ["src/invites/index.ts"] }],
        batches: [[6], [7]],
        waits: [],
      } as T;
    case "lane_drop_hunk":
      return "a1b2c3d4" as T;
    case "files_list":
      return ["README.md", "package.json", "src/app.ts", "src/kart/drift.ts", "src/kart/drift.test.ts", "src/kart/physics.ts", "src/lobby/timer.ts", "src/ui/avatar.tsx"] as T;
    case "prefs_set":
    case "reveal":
    case "flow_set":
      return null as T;
    case "pitlane_home":
      return "/Users/you/.pitlane" as T;
    case "remove_repo":
      return { repos: [repo], last: repo.path } as T;
    case "flow_get":
      return {
        exists: true,
        path: "/Users/you/Projects/demo/.pitlane/flow.toml",
        flow: {
          flow: { base: "main", integration: "dev", production: "main", remote: "origin", naming: ["feat/*", "fix/*", "chore/*"], stale_days: 14 },
          run: { allow: [], max_turns: 40, model: null, branch_prefix: "feat/", brief: true, browser: false, sandbox: true },
          verify: { commands: ["npm test", "npm run lint"], judge: true, judge_model: "sonnet", max_rounds: 3 },
        },
      } as T;
    case "vscode_show":
      return true as T;
    case "vscode_theme":
    case "vscode_prewarm":
    case "vscode_places":
    case "vscode_hide":
    case "vscode_open":
      return null as T;
    case "term_list":
      return [
        { id: 1, cwd: "/Users/you/demo", label: "demo", exited: null },
        { id: 2, cwd: "/Users/you/.pitlane/wt/demo/fix-avatar-svg", label: "fix/avatar-svg", exited: null },
      ] as T;
    case "term_buffer":
      // The lane's terminal: one command marked as a block (OSC 133) that failed.
      if ((args as { id?: number }).id === 2)
        return { seq: 0, text: "  \x1b[32mVITE\x1b[0m v7.3.8  ready in 412 ms\r\n  ➜  Local:   \x1b[36mhttp://localhost:5173/\x1b[0m\r\n\r\n\x1b]133;A\x07\x1b[38;5;244m~/…/fix-avatar-svg\x1b[0m fix/avatar-svg \x1b[38;5;214m❯\x1b[0m \x1b]133;B\x07npm test -- avatar\r\n\x1b]633;E;npm test -- avatar\x07\x1b]133;C\x07\r\n > demo@0.1.0 test\r\n > vitest run avatar\r\n\r\n \x1b[31m×\x1b[0m src/avatar/svg.test.ts > strips <script> from an SVG\r\n   AssertionError: expected '<svg><script>' not to contain '<script>'\r\n\r\n Test Files  \x1b[31m1 failed\x1b[0m (1)\r\n\x1b]133;D;1\x07\x1b]133;A\x07\x1b[38;5;244m~/…/fix-avatar-svg\x1b[0m fix/avatar-svg \x1b[38;5;214m❯\x1b[0m \x1b]133;B\x07" } as T;
      return { seq: 0, text: "\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m git status --short\r\n M src/kart/drift.ts\r\n?? notes.md\r\n\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m npm test\r\n\r\n> demo@0.1.0 test\r\n> vitest run\r\n\r\n \x1b[32m✓\x1b[0m src/kart/drift.test.ts (4 tests) 12ms\r\n\r\n Test Files  \x1b[32m1 passed\x1b[0m (1)\r\n      Tests  \x1b[32m4 passed\x1b[0m (4)\r\n\r\n\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m " } as T;
    case "card_add":
      return { ...structuredClone(cards[0]), id: 10, title: (args as { title: string }).title, state: "inbox", questions: [], lane: undefined } as T;
    case "term_open":
      return { id: nextTerm++, cwd: (args as { cwd: string }).cwd, label: (args as { label: string }).label, exited: null } as T;
    case "term_write":
    case "term_resize":
    case "term_close":
      return null as T;
    case "queue_list":
      return [
        { id: "q1", repo: "/demo", branch: "feat/billing-retry", target: "main", state: "shipping", queued_at: now() - 40, finished_at: null, merge_sha: null, conflicts: [], error: null, card: null },
        { id: "q2", repo: "/demo", branch: "fix/avatar-svg", target: "dev", state: "queued", queued_at: now() - 20, finished_at: null, merge_sha: null, conflicts: [], error: null, card: null },
        { id: "q3", repo: "/demo", branch: "feat/kart-drift", target: "dev", state: "conflict", queued_at: now() - 900, finished_at: now() - 860, merge_sha: null, conflicts: ["src/kart/physics.ts"], error: null, card: null },
      ] as T;
    case "queue_add":
    case "queue_remove":
      return null as T;
    case "quota":
      return {
        account: "default",
        five_hour_at: 90,
        seven_day_at: 95,
        events: [
          { kind: "quota", window: "five_hour", used: 0.14, resets_at: Math.floor(Date.now() / 1000) + 3 * 3600 },
          { kind: "quota", window: "seven_day", used: 0.27, resets_at: Math.floor(Date.now() / 1000) + 4 * 86400 },
        ],
      } as T;
    case "cards_list":
      return { cards: new URLSearchParams(location.search).get("state") === "empty" ? [] : structuredClone(cards).sort((a, b) => ((a as { rank?: number }).rank ?? a.id) - ((b as { rank?: number }).rank ?? b.id)), problems: [] } as T;
    case "card_reorder": {
      const ids = (args as { ids: number[] }).ids;
      ids.forEach((id, i) => {
        const c = cards.find((x) => x.id === id) as { rank?: number } | undefined;
        if (c) c.rank = i + 1;
      });
      return null as T;
    }
    case "card_move": {
      // The daemon's rules, roughly: enough to drag and Move to… in the preview.
      const a = args as { id: number; to: "backlog" | "queue" | "hold"; reason: string | null };
      const c = cards.find((x) => x.id === a.id) as unknown as { state: string; held?: string; lane?: string };
      if (c.state === "verifying") throw new Error(`card #${a.id} is being checked; wait for its checks to end`);
      if (a.to === "queue" && (c.state === "review" || c.state === "done" || c.state === "in-dev")) throw new Error(`card #${a.id} cannot go to the queue from ${c.state}`);
      if (a.to === "hold") (c.state = c.state === "queued" ? "ready" : c.state), (c.held = a.reason ?? "");
      else {
        delete c.held;
        c.state = a.to === "queue" ? "queued" : c.state === "inbox" ? "inbox" : "ready";
      }
      return c as T;
    }
    case "card_note": {
      const a = args as { id: number; text: string };
      const c = cards.find((x) => x.id === a.id) as unknown as { notes?: { at: string; by: string; text: string }[] };
      (c.notes ??= []).push({ at: new Date().toISOString(), by: "you", text: a.text });
      return c as T;
    }
    case "card_answer":
    case "card_reanswer": {
      const a = args as { id: number; question: number; answer: string };
      const c = cards.find((x) => x.id === a.id) as unknown as { questions: { id: number; options?: string[]; answer?: string; decision?: string }[] };
      const q = c.questions.find((x) => x.id === a.question)!;
      const n = Number(a.answer);
      q.answer = q.options?.[n - 1] && String(n) === a.answer ? q.options[n - 1] : a.answer;
      q.decision ??= `D-${String(20 + a.question).padStart(2, "0")}`;
      return { decision: q.decision, card: c } as T;
    }
    // Claude answers a question's conversation after a while, with a third way among the options.
    case "card_talk": {
      const a = args as { id: number; question: number; message: string | null };
      const c = cards.find((x) => x.id === a.id) as unknown as {
        questions: { id: number; options?: string[]; recommended?: number; talk?: { at: string; by: string; text: string; options?: string[]; recommended?: number }[] }[];
      };
      const q = c.questions.find((x) => x.id === a.question)!;
      if (a.message) (q.talk ??= []).push({ at: new Date().toISOString(), by: "you", text: a.message });
      await new Promise((r) => setTimeout(r, 2500));
      const options = [...(q.options ?? []).slice(0, 2), "Per account, with a higher limit for workspace admins"];
      q.options = options;
      q.recommended = options.length - 1;
      (q.talk ??= []).push({
        at: new Date().toISOString(),
        by: "claude",
        text: "Good point. `src/invites/limit.ts` already knows the inviter's role, so a separate admin limit is one branch in `allowed()`, no schema change. I would go that way now: admins invite in bursts at onboarding, and a flat limit would block exactly them.",
        options,
        recommended: options.length - 1,
      });
      return { card: c } as T;
    }
    case "card_talk_stop":
      return 1 as T;
    case "cards_analyze":
      await new Promise((r) => setTimeout(r, 1200));
      return { analyzed: (args as { ids: number[] }).ids.map((card) => ({ card, questions: 0, notes: 1 })) } as T;
    default:
      throw `Preview mode: "${cmd}" runs only in the desktop app.`;
  }
}
