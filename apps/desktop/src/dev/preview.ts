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
  cost_usd: null, duration_ms: null, turns: null, handoff: null, tokens: zero, denied: [], error: null,
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

export async function preview<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((r) => setTimeout(r, 250));
  switch (cmd) {
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
    case "run_timeline":
      return (location.hash.includes("3-live") ? liveTimeline() : location.hash.includes("5-failed") ? failedTimeline() : pairs(runTimeline)) as T;
    case "run_follow":
      return undefined as T;
    case "accounts_list":
      return {
        accounts: [
          { id: "default", kind: "subscription", status: { logged_in: true, method: "claude.ai", email: null, problem: null },
            usage: { five_hour: 14, seven_day: 27 }, here: true },
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
        files: [{ path: "README.md", added: 1, removed: 1 }],
        patch: "diff --git a/README.md b/README.md\nindex 1b5a4e9..3c1f0a2 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-demo\n+This is a demo repository for Pitlane.\n",
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
    case "file_read":
      return {
        path: "src/kart/drift.ts",
        text: "import { integrate } from \"./physics\";\n\n/** The drift angle a kart reaches at each tier, in degrees. */\nexport function driftAngle(tier: number): number {\n  if (tier < 1) return 0;\n  return Math.min(12 + tier * 6, 36);\n}\n\nexport function applyDrift(kart: Kart, dt: number) {\n  const angle = driftAngle(kart.tier);\n  kart.heading += angle * dt;\n  integrate(kart, dt);\n}\n\ninterface Kart {\n  tier: number;\n  heading: number;\n}\n",
        refused: null,
      } as T;
    case "files_ignored":
      return [".env", "dist/", "node_modules/"] as T;
    case "dir_list":
      return ["chunks/", "index.js", "index.js.map"] as T;
    case "files_search": {
      const q = String((args as { query: string }).query).toLowerCase();
      const lines: [string, number, string][] = [
        ["src/kart/drift.ts", 4, "export function driftAngle(tier: number): number {"],
        ["src/kart/drift.ts", 10, "  const angle = driftAngle(kart.tier);"],
        ["src/kart/drift.test.ts", 3, "import { driftAngle } from \"./drift\";"],
        ["src/kart/physics.ts", 1, "export function integrate(kart: Kart, dt: number) {"],
      ];
      return lines.filter(([, , t]) => t.toLowerCase().includes(q)).map(([path, line, text]) => ({ path, line, col: text.toLowerCase().indexOf(q) + 1, text })) as T;
    }
    case "file_changes":
      return {
        base: "a1b2c3d4",
        original: "import { integrate } from \"./physics\";\n\nexport function driftAngle(tier: number): number {\n  if (tier < 1) return 0;\n  return 12 + tier * 6;\n}\n\nexport function applyDrift(kart: Kart, dt: number) {\n  kart.heading += driftAngle(kart.tier) * dt;\n  integrate(kart, dt);\n}\n\ninterface Kart {\n  tier: number;\n  heading: number;\n}\n",
        added: [[2, 2], [10, 10]],
        changed: [[6, 6]],
        removed: [],
      } as T;
    case "file_write":
      return null as T;
    case "term_list":
      return [
        { id: 1, cwd: "/Users/you/demo", label: "demo", exited: null },
        { id: 2, cwd: "/Users/you/.pitlane/wt/demo/fix-avatar-svg", label: "fix/avatar-svg", exited: null },
      ] as T;
    case "term_buffer":
      return "\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m git status --short\r\n M src/kart/drift.ts\r\n?? notes.md\r\n\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m npm test\r\n\r\n> demo@0.1.0 test\r\n> vitest run\r\n\r\n \x1b[32m✓\x1b[0m src/kart/drift.test.ts (4 tests) 12ms\r\n\r\n Test Files  \x1b[32m1 passed\x1b[0m (1)\r\n      Tests  \x1b[32m4 passed\x1b[0m (4)\r\n\r\n\x1b[38;5;244m~/demo\x1b[0m main \x1b[38;5;214m❯\x1b[0m " as T;
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
      return { cards: new URLSearchParams(location.search).get("state") === "empty" ? [] : cards, problems: [] } as T;
    default:
      throw `Preview mode: "${cmd}" runs only in the desktop app.`;
  }
}
