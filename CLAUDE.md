# Pitlane

AI coding-agent orchestrator: terminal + editor + kanban in one local desktop app. One Rust daemon (`pitd`) owns boards, worktrees, runs, the merge queue and quotas; thin UIs (Tauri desktop, `pit` CLI, `pit mcp`) talk to it. Overview and commands: `README.md`.

## Read first
- `RETOMADA.md`: current status, decisions already made (don't reopen them without a reason), pending work. Add a short entry when a delivery lands.
- `PLANO.html`: the product plan (flow, brain, account and model routing, launching sessions, branches and worktrees, roadmap, risks). Published as an artifact; republish details are in `RETOMADA.md`.
- `design/design.md`: the locked design system. Any UI work follows it; `design/index.html` is the live specimen.
- `docs/research/ux-research.md`: what devs love, hate and miss in editors, terminals, agent managers and git tools. Principles P1 to P10 come from it.

## Hard rules
- Agents are launched through their official, unmodified CLIs (`claude -p` stream-json). Pitlane never reads, copies, stores or proxies login tokens, and never uses subscription OAuth through an SDK.
- No automatic switching between accounts of the same provider. Automatic fallback only across models, providers, or to an API key (which always asks first).
- V1 is Claude Code only. Codex (`codex app-server`) and other agents are V2; don't add them unasked.
- Branch flow per repo comes from `.pitlane/flow.toml`; new branches start from a freshly fetched `origin/main`. Merges go through the queue by git plumbing, never touching checkouts.
- Nothing Pitlane starts is invisible or unstoppable, and nothing runs after the app closes.
- Local-first: no login, telemetry opt-in. Pitlane writes nothing into a repo except `.pitlane/flow.toml`; boards, runs and worktrees live in `~/.pitlane/`.

## Layout
- `crates/pit-core`: all logic, no UI. Git via the `git` CLI, not libgit2/gix.
- `crates/pitd`: daemon (tokio), run supervisor, Unix socket `~/.pitlane/pitd.sock`.
- `crates/pit`: CLI and the stdio MCP server (`pit mcp`, no SDK).
- `apps/desktop`: Tauri 2 + Solid + Vite. `src/` one file per screen; `src-tauri/` backend; `src/dev/` browser fixtures.

## Commands
- `cargo test` / `cargo clippy --workspace` (keep clippy clean); narrow with `-p pit-core` etc.
- `cd apps/desktop && npm run build` type-checks the UI; `npm run dev` previews it in a browser with fixtures (`?state=loading|empty|error`).
- `cd apps/desktop && npx tauri dev` / `npx tauri build`. After changing `pit`, `cargo install --path crates/pit` so Claude Code's MCP uses the new binary.
- Homebrew rustup is keg-only: `/opt/homebrew/opt/rustup/bin` must be on `PATH`.

## Conventions
- Never run `cargo fmt`: the repo isn't rustfmt-formatted and it rewrites dozens of files.
- Other sessions edit this same checkout: stage only your own files, never `git add -A`.
- Work on `main`; commit and push at the end of each delivery. Commit messages are prose describing the user-visible change; no `Co-Authored-By` trailer.
- UI copy is English, through `t()`; Portuguese lives in `apps/desktop/src/locales/pt`. Reply to the user in Portuguese.
- This repo's board is outside the repo: read and change cards only through the `pitlane` MCP tools.
