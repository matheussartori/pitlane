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

export async function preview<T>(cmd: string): Promise<T> {
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
      return [liveRun(), failedRun(), runMeta] as T;
    }
    case "run_timeline":
      return (location.hash.includes("3-live") ? liveTimeline() : location.hash.includes("5-failed") ? failedTimeline() : pairs(runTimeline)) as T;
    case "run_follow":
      return undefined as T;
    case "quota":
      return [
        { kind: "quota", window: "five_hour", used: 0.14, resets_at: Math.floor(Date.now() / 1000) + 3 * 3600 },
        { kind: "quota", window: "seven_day", used: 0.27, resets_at: Math.floor(Date.now() / 1000) + 4 * 86400 },
      ] as T;
    case "cards_list":
      return { cards: new URLSearchParams(location.search).get("state") === "empty" ? [] : cards, problems: [] } as T;
    default:
      throw `Preview mode: "${cmd}" runs only in the desktop app.`;
  }
}
