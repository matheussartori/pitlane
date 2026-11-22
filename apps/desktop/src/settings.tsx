// Settings: the app's own preferences, the repositories it knows, and the current repository's
// flow (its .pitlane/flow.toml, versioned with it). One section at a time, a list of sections at
// the side, as in Linear and Raycast. Every change saves as it is made.
import { For, Match, Show, Switch, createResource, createSignal, type JSX } from "solid-js";
import { getVersion } from "@tauri-apps/api/app";
import { api, vscode, type FlowFile, type RepoEntry } from "./api";
import { Icon } from "./icons";
import { prefs, setPrefs } from "./prefs";
import { setTerminalFont, setTerminalSide, terminalPrefs } from "./terminals";
import { toast } from "./ui";

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
const shortPath = (p: string) => p.replace(/^\/Users\/[^/]+/, "~");

type Section = "general" | "repositories" | "agents" | "accounts" | "terminal" | "code" | "shortcuts" | "about";
const SECTIONS: { id: Section; label: string; icon: string }[] = [
  { id: "general", label: "General", icon: "mark" },
  { id: "repositories", label: "Repositories", icon: "folder" },
  { id: "agents", label: "Agents", icon: "run" },
  { id: "accounts", label: "Accounts", icon: "person" },
  { id: "terminal", label: "Terminal", icon: "term" },
  { id: "code", label: "Code", icon: "code" },
  { id: "shortcuts", label: "Shortcuts", icon: "search" },
  { id: "about", label: "About", icon: "verify" },
];

const [section, setSection] = createSignal<Section>("general");
export const openSettingsAt = (s: Section) => setSection(s);

// ---------------------------------------------------------------- building blocks

function Row(props: { label: string; help?: JSX.Element; children: JSX.Element; for?: string }) {
  return (
    <div class="set-row">
      <div class="set-text">
        <label class="set-label" for={props.for}>
          {props.label}
        </label>
        <Show when={props.help}>
          <div class="set-help">{props.help}</div>
        </Show>
      </div>
      <div class="set-control">{props.children}</div>
    </div>
  );
}

function Group(props: { title?: string; note?: JSX.Element; children: JSX.Element }) {
  return (
    <section class="set-group">
      <Show when={props.title}>
        <h3 class="set-group-title">{props.title}</h3>
      </Show>
      <Show when={props.note}>
        <p class="set-note">{props.note}</p>
      </Show>
      <div class="set-rows">{props.children}</div>
    </section>
  );
}

function Toggle(props: { id: string; on: boolean; onChange: (v: boolean) => void; label: string }) {
  return <input id={props.id} type="checkbox" class="switch" role="switch" aria-label={props.label} checked={props.on} onChange={(e) => props.onChange(e.currentTarget.checked)} />;
}

function Seg<T extends string>(props: { value: T; options: { v: T; label: string }[]; label: string; onChange: (v: T) => void }) {
  return (
    <div class="seg" role="group" aria-label={props.label}>
      <For each={props.options}>
        {(o) => (
          <button type="button" aria-pressed={props.value === o.v} onClick={() => props.onChange(o.v)}>
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}

// ---------------------------------------------------------------- the screen

export function SettingsScreen(props: {
  repo: string | null;
  repoName: string;
  repos: RepoEntry[];
  onAddRepo: () => void;
  onReposChanged: () => void;
  onAccounts: () => void;
}) {
  return (
    <div class="settings">
      <nav class="settings-nav" aria-label="Settings">
        <div class="settings-nav-title">Settings</div>
        <For each={SECTIONS}>
          {(s) => (
            <button type="button" class="settings-nav-item" aria-current={section() === s.id ? "page" : undefined} onClick={() => setSection(s.id)}>
              <Icon name={s.icon} small />
              {s.label}
            </button>
          )}
        </For>
      </nav>
      <div class="settings-page">
        <Switch>
          <Match when={section() === "general"}>
            <General />
          </Match>
          <Match when={section() === "repositories"}>
            <Repositories {...props} />
          </Match>
          <Match when={section() === "agents"}>
            <Agents repo={props.repo} repoName={props.repoName} />
          </Match>
          <Match when={section() === "accounts"}>
            <h2 class="set-title">Accounts</h2>
            <Group note="Your Claude accounts, the route between them, and what each has used. Pitlane launches the official Claude Code CLI with an account's own login; it never reads or stores a token.">
              <Row label="Accounts and route" help="Add an account, set the order runs use them in, see each one's usage.">
                <button type="button" class="btn" onClick={props.onAccounts} disabled={!props.repo}>
                  Open accounts
                </button>
              </Row>
            </Group>
          </Match>
          <Match when={section() === "terminal"}>
            <Terminal />
          </Match>
          <Match when={section() === "code"}>
            <Code />
          </Match>
          <Match when={section() === "shortcuts"}>
            <Shortcuts />
          </Match>
          <Match when={section() === "about"}>
            <About />
          </Match>
        </Switch>
      </div>
    </div>
  );
}

function General() {
  const save = (patch: Parameters<typeof setPrefs>[0]) => void setPrefs(patch).catch((e) => toast({ tone: "fail", title: "Not saved", body: msg(e) }));
  return (
    <>
      <h2 class="set-title">General</h2>
      <Group>
        <Row label="Appearance" help="Light, dark, or as the system is. VS Code and the terminals follow.">
          <Seg
            label="Appearance"
            value={prefs().theme}
            options={[
              { v: "system", label: "System" },
              { v: "light", label: "Light" },
              { v: "dark", label: "Dark" },
            ]}
            onChange={(v) => save({ theme: v })}
          />
        </Row>
      </Group>
      <Group title="Notifications" note="Sent by macOS, only while Pitlane is not in front. The marks inside Pitlane stay either way.">
        <Row label="An agent asks for your approval" for="n-approvals">
          <Toggle id="n-approvals" label="An agent asks for your approval" on={prefs().notify_approvals} onChange={(v) => save({ notify_approvals: v })} />
        </Row>
        <Row label="A lane’s run ends" help="Ready for review, or stopped." for="n-runs">
          <Toggle id="n-runs" label="A lane’s run ends" on={prefs().notify_runs} onChange={(v) => save({ notify_runs: v })} />
        </Row>
        <Row label="A terminal needs you" help="A bell or a notification from the program, or a long run that went quiet." for="n-terms">
          <Toggle id="n-terms" label="A terminal needs you" on={prefs().notify_terminals} onChange={(v) => save({ notify_terminals: v })} />
        </Row>
      </Group>
    </>
  );
}

function Repositories(props: { repo: string | null; repos: RepoEntry[]; onAddRepo: () => void; onReposChanged: () => void }) {
  const remove = async (r: RepoEntry) => {
    if (!window.confirm(`Take ${r.name} off Pitlane’s list? Nothing on disk changes, and you can add it again.`)) return;
    try {
      await api.removeRepo(r.path);
      props.onReposChanged();
    } catch (e) {
      toast({ tone: "fail", title: "Not removed", body: msg(e) });
    }
  };
  return (
    <>
      <h2 class="set-title">Repositories</h2>
      <Group note="The repositories Pitlane knows. Removing one only takes it off this list.">
        <For each={props.repos} fallback={<p class="set-empty">No repository yet.</p>}>
          {(r) => (
            <div class="set-row">
              <div class="set-text">
                <div class="set-label">
                  {r.name}
                  <Show when={r.path === props.repo}>
                    <span class="set-tag">Open</span>
                  </Show>
                </div>
                <div class="set-help set-mono">{shortPath(r.path)}</div>
              </div>
              <div class="set-control">
                <button type="button" class="btn btn-ghost" onClick={() => void api.reveal(r.path)}>
                  Show in Finder
                </button>
                <button type="button" class="btn btn-ghost btn-danger" onClick={() => void remove(r)}>
                  Remove
                </button>
              </div>
            </div>
          )}
        </For>
      </Group>
      <button type="button" class="btn" onClick={props.onAddRepo}>
        <Icon name="plus" small />
        Add repository
      </button>
    </>
  );
}

function Agents(props: { repo: string | null; repoName: string }) {
  const [flow, { refetch }] = createResource(
    () => props.repo,
    (r) => api.flowGet(r),
  );
  const set = async (table: string, key: string, value: unknown) => {
    if (!props.repo) return;
    try {
      await api.flowSet(props.repo, table, key, value);
      void refetch();
    } catch (e) {
      toast({ tone: "fail", title: "flow.toml not changed", body: msg(e) });
      void refetch();
    }
  };
  const f = (): FlowFile | undefined => flow()?.flow;
  const num = (v: string, fallback: number) => (Number.isFinite(Number(v)) && Number(v) > 0 ? Math.round(Number(v)) : fallback);
  return (
    <>
      <h2 class="set-title">Agents</h2>
      <Show when={props.repo} fallback={<p class="set-empty">Open a repository to see how its agents run.</p>}>
        <p class="set-lead">
          How agents run on {props.repoName}. These live in <code class="set-code">.pitlane/flow.toml</code>, versioned with the repository, so the whole team shares them.
          <Show when={flow()}>
            {" "}
            <button type="button" class="link-btn" onClick={() => void api.reveal(flow()!.path)}>
              Show the file
            </button>
          </Show>
        </p>
        <Show when={f()}>
          {(fl) => (
            <>
              <Group title="Runs">
                <Row label="Model" help="A Claude Code model or alias. Empty: Claude Code’s own default." for="f-model">
                  <input id="f-model" class="input input-mono set-input" placeholder="default" value={fl().run.model ?? ""} onChange={(e) => void set("run", "model", e.currentTarget.value.trim())} />
                </Row>
                <Row label="Turns at most" help="A run that reaches it stops and hands back." for="f-turns">
                  <input id="f-turns" class="input input-mono set-input set-num" inputmode="numeric" value={fl().run.max_turns} onChange={(e) => void set("run", "max_turns", num(e.currentTarget.value, fl().run.max_turns))} />
                </Row>
                <Row label="Map of where to look" help="Each run’s first message lists the files and symbols most likely involved, from the repository’s index." for="f-brief">
                  <Toggle id="f-brief" label="Map of where to look" on={fl().run.brief} onChange={(v) => void set("run", "brief", v)} />
                </Row>
                <Row label="Branch prefix" help="For the branch a card gets when it has none: feat/ gives feat/41-avatar-limit." for="f-prefix">
                  <input id="f-prefix" class="input input-mono set-input" value={fl().run.branch_prefix} onChange={(e) => void set("run", "branch_prefix", e.currentTarget.value.trim())} />
                </Row>
              </Group>
              <Group title="Checks after a run">
                <Row label="Commands" help="Run in the lane’s worktree, one after another, before review. One per line." for="f-cmds">
                  <textarea
                    id="f-cmds"
                    class="input input-mono set-area"
                    rows={3}
                    placeholder={"npm test\nnpm run lint"}
                    value={fl().verify.commands.join("\n")}
                    onChange={(e) =>
                      void set(
                        "verify",
                        "commands",
                        e.currentTarget.value
                          .split("\n")
                          .map((x) => x.trim())
                          .filter(Boolean),
                      )
                    }
                  />
                </Row>
                <Row label="A second opinion" help="A separate Claude Code session reads the spec and the diff, and approves or asks for changes. It changes nothing." for="f-judge">
                  <Toggle id="f-judge" label="A second opinion" on={fl().verify.judge} onChange={(v) => void set("verify", "judge", v)} />
                </Row>
                <Row label="Its model" for="f-jmodel">
                  <input id="f-jmodel" class="input input-mono set-input" placeholder="your Claude Code model" value={fl().verify.judge_model ?? ""} disabled={!fl().verify.judge} onChange={(e) => void set("verify", "judge_model", e.currentTarget.value.trim())} />
                </Row>
                <Row label="Rounds of changes at most" help="After that, the card comes back to you." for="f-rounds">
                  <input id="f-rounds" class="input input-mono set-input set-num" inputmode="numeric" value={fl().verify.max_rounds} onChange={(e) => void set("verify", "max_rounds", num(e.currentTarget.value, fl().verify.max_rounds))} />
                </Row>
              </Group>
              <Group title="Branches" note="Where new branches start and where work goes. Change these in flow.toml itself: they shape every lane.">
                <Row label="New branches start from">
                  <span class="set-value">
                    {fl().flow.remote}/{fl().flow.base}
                  </span>
                </Row>
                <Row label="Tried in">
                  <span class="set-value">{fl().flow.integration || "no integration stage"}</span>
                </Row>
                <Row label="Production">
                  <span class="set-value">{fl().flow.production}</span>
                </Row>
              </Group>
            </>
          )}
        </Show>
      </Show>
    </>
  );
}

function Terminal() {
  const [p, setP] = createSignal(terminalPrefs());
  const side = (s: "left" | "top") => (setTerminalSide(s), setP(terminalPrefs()));
  const font = (n: number) => (setTerminalFont(n), setP(terminalPrefs()));
  return (
    <>
      <h2 class="set-title">Terminal</h2>
      <Group>
        <Row label="Tabs" help="A list at the side, with each tab’s place and state, or a strip on top.">
          <Seg
            label="Tabs"
            value={p().side}
            options={[
              { v: "left", label: "At the side" },
              { v: "top", label: "On top" },
            ]}
            onChange={side}
          />
        </Row>
        <Row label="Font size" help="⌘= and ⌘- change it in the terminal too.">
          <div class="stepper">
            <button type="button" class="btn btn-ghost btn-icon" aria-label="Smaller" onClick={() => font(p().font - 1)}>
              −
            </button>
            <span class="stepper-value">{p().font}</span>
            <button type="button" class="btn btn-ghost btn-icon" aria-label="Larger" onClick={() => font(p().font + 1)}>
              +
            </button>
          </div>
        </Row>
      </Group>
    </>
  );
}

function Code() {
  const [home] = createResource(() => api.pitlaneHome().catch(() => ""));
  const save = (patch: Parameters<typeof setPrefs>[0]) => void setPrefs(patch).catch((e) => toast({ tone: "fail", title: "Not saved", body: msg(e) }));
  const openSettings = () => {
    const h = home();
    if (!h) return;
    void vscode.open(`${h}/code-server/data/User/settings.json`).catch((e) => toast({ tone: "fail", title: "Could not open it", body: msg(e) }));
    window.dispatchEvent(new CustomEvent("pitlane-show-code"));
  };
  return (
    <>
      <h2 class="set-title">Code</h2>
      <Group note="The Code tab is VS Code (code-server, open source), bundled with Pitlane. Its own settings and extensions are VS Code’s.">
        <Row label="Load it in the background" help="When a repository opens, so the tab opens at once. Off: it loads when you open the tab." for="c-prewarm">
          <Toggle id="c-prewarm" label="Load it in the background" on={prefs().code_prewarm} onChange={(v) => save({ code_prewarm: v })} />
        </Row>
        <Row label="VS Code’s settings" help="Its settings.json, opened in the Code tab.">
          <button type="button" class="btn" onClick={openSettings} disabled={!home()}>
            Open settings.json
          </button>
        </Row>
        <Row label="Extensions" help="From Open VSX, installed from VS Code’s Extensions view.">
          <button type="button" class="btn btn-ghost" disabled={!home()} onClick={() => void api.reveal(`${home()}/code-server/extensions`)}>
            Show in Finder
          </button>
        </Row>
      </Group>
    </>
  );
}

const KEYS: { area: string; keys: [string, string][] }[] = [
  {
    area: "Everywhere",
    keys: [
      ["⌘,", "Settings"],
      ["⌘T", "A terminal where you are"],
      ["⌘R", "Read the repository again"],
      ["⌥1 … ⌥4", "Inbox, Board, Lanes, Ledger"],
    ],
  },
  { area: "Board and Lanes", keys: [["C", "New card"], ["N", "New lane"]] },
  {
    area: "Terminal",
    keys: [
      ["⌘D / ⇧⌘D", "Split right / down"],
      ["⌘W", "Close the pane"],
      ["⌥⌘ arrows", "Move between panes"],
      ["⌃⌘ arrows", "Resize the pane"],
      ["⌃⌘=", "Share the room equally"],
      ["⇧⌘↩", "Zoom the pane"],
      ["⌘1 … ⌘9", "Go to a tab"],
      ["⇧⌘U", "The tab that wants you"],
      ["⌘F / ⌘K", "Find / clear"],
      ["⌘-click a path", "Open it in Code at its line"],
    ],
  },
];

function Shortcuts() {
  return (
    <>
      <h2 class="set-title">Shortcuts</h2>
      <For each={KEYS}>
        {(g) => (
          <Group title={g.area}>
            <For each={g.keys}>
              {([k, what]) => (
                <div class="set-row set-row-tight">
                  <span class="set-label">{what}</span>
                  <span class="kbd set-kbd">{k}</span>
                </div>
              )}
            </For>
          </Group>
        )}
      </For>
    </>
  );
}

function About() {
  const [version] = createResource(() => getVersion().catch(() => "dev"));
  const [home] = createResource(() => api.pitlaneHome().catch(() => ""));
  return (
    <>
      <h2 class="set-title">About</h2>
      <Group>
        <Row label="Pitlane">
          <span class="set-value">{version()}</span>
        </Row>
        <Row label="Pitlane’s files" help="Settings, cards, runs, worktrees and VS Code’s data.">
          <span class="set-value set-mono">{shortPath(home() ?? "")}</span>
          <button type="button" class="btn btn-ghost" disabled={!home()} onClick={() => void api.reveal(home()!)}>
            Show in Finder
          </button>
        </Row>
        <Row label="Privacy" help="No account, no telemetry. Pitlane sends nothing anywhere; agents run through Claude Code’s own CLI and your own login.">
          <span class="set-value">Local only</span>
        </Row>
      </Group>
    </>
  );
}
