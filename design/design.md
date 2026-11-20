# Design: Pitlane

The locked system. Every UI change reads this first; surfaces share it. Change it on purpose; the file is the rule.
Live specimen: `design/index.html`. Research behind it: `docs/research/ux-research.md`.

## System
- Genre: modern-minimal, app scope (a tool people work in, not a page they read)
- Theme: custom. Idea: "a pit wall: every car in view, only the ones that need you light up"
- Axes: dark/grotesk-sans/neutral. Posture: dense and quiet, a desktop tool in the line of Warp and Linear, never a page. Modes: dark (default) + light (re-derived, never inverted)
- Revised 20 nov 2026 after the person found the first take childish: everything smaller, no pills, no orange fills. Size never signals importance; a small mark does

## Tokens
`design/tokens.css` is the source of truth. Roles:
- Ground: `--color-paper` (near-black graphite, hue 255, low chroma), `-2` panels and cards, `-3` hover and menus, `-sunk` terminal, diff, inputs
- Ink: `--color-ink`, `--color-ink-2`, `--color-muted`; rules `--color-rule`, `--color-rule-strong`
- Accent `--color-accent` (signal orange, hue 58) means "your move" only: the one primary button, focus, and small marks (a 2px stripe, an icon and its word, a lane number). Never a fill behind a row or a card. Under 2% of a screen
- States: `--color-run`, `--color-done`, `--color-fail` (+ `-wash`). Never colour alone: icon + word + colour
- Faces: Geist (variable) for everything, 400/500 in the UI and 600 only for titles; Geist Mono for code, terminal, branch names, lane numbers and figures that compare (tabular-nums)
- Scale: 13px UI, 12 secondary, 11 meta; prose 14; titles 15 (sheets) and 16 (screens); nothing above 18. Space: 4px grid (`--space-*`), rows 30 to 38, controls 26 (30 prominent). Radius 3/5/7, no pills (round only for switches and meters)
- Depth on dark is lightness + inset hairline (`--ring`), never drop shadow

## Controls
- Primary: accent fill, accent-ink text, radius 5, height 26 (30 when prominent). One per view
- Secondary: paper-3 fill + hairline ring, same radius and height. Ghost: text only. Destructive: secondary with fail text, always undoable
- Every interactive control ships 8 states: default, hover, focus-visible (instant, never animated), active (scale .97), disabled (.55), loading, error, success
- Inputs sit in `paper-sunk` with a ring; focus is an outline, never a border change

## Product patterns (see the specimen)
- Shell: top bar (wordmark, repo, Inbox / Board / Lanes / Ledger / Code / Terminal, ⌘K, quota meters) + the Tower docked right (folded away on Lanes, Code and Terminal)
- The Tower (signature): a compact list like a terminal's tab list. Each row: the number in small mono, the branch, a mono sub-line (idle days, dirty files, ahead/behind). A row says nothing at the right unless it matters: "You" with its icon in accent, a run's clock, Failed, Done. No row is ever filled with colour. Never reorders on activity
- State marks (`.chip`): an icon and a word in the state's colour, no fill, no outline
- Board groups by who acts next (Needs you, Backlog, In progress, Landed); "By stage" is the alternate lens. Cards that need you carry a 2px accent stripe on the left and their mark in accent, never a wash or a full ring. A card linked to a lane carries the track at card size (8px stops) with the one stop that matters in words (next: main, mixed in dev, 1↓)
- Inbox items say why they wait, offer options with one recommended, answer with a single key, and record a decision. Questions group under one head per card, with that card's decisions so far as context
- Diff first: per-hunk accept/reject, viewed checkbox, scope = last turn, card or file; pending vs applied is always visible
- Lanes screen: one table grouped by who acts next (Waiting on you, In progress, Ready to release, Shipped, Parked). Waiting on you is only what is stuck without you (conflict, mixed with dev, a pull to settle, a review); a lane in dev and not yet in main is Ready to release, calm, with Ship to main as quiet text. Each lane carries the track, four stops (local, remote, integration, production) joined in ink where reached; only the stop your move reaches is accent, a count to settle is an ink ring with its figure, a remote deletion is a dashed stop, contamination is fail. One Next line per lane says the action or the proof (Squashed into main). On this screen the docked Tower folds away: the screen is the Tower at full width
- Run screen: the log (the agent's words, each tool as name, detail relative to the worktree and a mark), the clock ticking in mono in the head beside the state chip, the lane's track under the title, figures in a 2x2 grid (turns, cost, tokens in with cached, tokens out), the handoff once (beside the log, with Changed and Checked), tools that were not allowed with the flow.toml fix. A box tells the agent something without stopping it. Ledger: one table of runs with totals in the foot. Quota meters in the top bar only show figures an agent reported
- Review screen: the verdict in one sentence under the title (the Board's words), the primary action follows the evidence (Ask for changes when a check fails or the judge asks; Approve when all passes), the diff per file with a Viewed tick, judge findings at the line they concern (blockers and majors with a fail stripe, the rest quiet), a side column with checks ("1 of 2 pass") and the judge as a compact index of findings, the ask-for-changes composer under them saying which round it sends
- Merge queue: shipping always goes through the queue. A lane in line is in progress, not waiting on you; its Next cell says "2nd in line for dev" or "Shipping to main" with a spinner. A conflict the queue hit puts the lane back in Waiting on you with the file and a Clear action. The Lanes summary adds one clause ("Shipping feat/x to main, 1 more in line."); Ready to release offers "Queue all N for main" when more than one lane is ready for production
- Code screen: files of the repository or a lane's worktree on the left (a folder picker, a find field for ⌘P, a tree in the body face at 12px), Monaco on the right edge to edge in paper-sunk, themed from the tokens, Geist Mono 12.5. Open files keep their models and unsaved edits while you move between screens; a tab says "unsaved" in accent, the Code tab counts them. ⌘S saves atomically, and nothing outside the chosen folder can be read or written. Review and Run open their files here
- Terminal screen: your own login shell in a real PTY, in the repository or any lane's worktree, in tabs like a terminal's own (a small mono label, close on hover, + for the place). The surface runs edge to edge in paper-sunk with Geist Mono 12; colours come from the tokens and every program's colour is lifted to 4.5:1. Terminals live with the app, not the screen: switching screens never loses a line, a reloaded window draws them again. ⌘T opens one where you are (the selected lane on Lanes); each Lanes row offers a terminal in its worktree when you reach for the row
- Lanes proves merge state (ancestry, squash patch-id, PR) and never offers to delete a live or locked worktree
- Every data region has loading (skeleton in the shape of the content), empty (what goes here + one action), error (what broke + what to do), full

## Motion
- Character: brisk and still. 120ms press and colour, 200ms hover and menus, 360ms panels and meters
- transform and opacity only. No pulsing dots: a running agent shows its clock ticking in mono. The only loops are the spinner and the skeleton

## Voice
- English UI (pt later via i18n). Say what happened, why, and the next step, in the user's nouns
- Never "Oops", exclamation marks, or announcing the AI. Never a generic "waiting for input"

## Exports
`design/tokens.css`, `design/base.css`, `design/components.css`. Tailwind or DTCG blocks get added here on request.
