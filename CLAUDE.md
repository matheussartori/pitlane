# Pitlane

AI coding-agent orchestrator: terminal + editor + kanban in one local desktop app. One Rust daemon owns boards, worktrees, runs and quotas; thin UIs (Tauri desktop, `pit` CLI/TUI) talk to it.

## Read first
- `PLANO.html`: the product plan (flow, brain, account and model routing, launching sessions, branches and worktrees, roadmap, risks). Published as an artifact.
- `design/design.md`: the locked design system. Any UI work follows it; `design/index.html` is the live specimen.
- `docs/research/ux-research.md`: what devs love, hate and miss in editors, terminals, agent managers and git tools. Principles P1 to P10 come from it.

## Hard rules
- Agents are launched through their official, unmodified CLIs (`claude -p` stream-json, `codex app-server`). Pitlane never reads, copies, stores or proxies login tokens, and never uses subscription OAuth through an SDK.
- No automatic switching between accounts of the same provider. Automatic fallback only across models, providers, or to an API key.
- Branch flow per repo comes from `.pitlane/flow.toml`; new branches start from a freshly fetched `origin/main`.
- Local-first: no login, telemetry opt-in.
