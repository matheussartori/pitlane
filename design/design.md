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
- Frame: `--color-frame` around the content panel. Ground: `--color-paper` (near-black graphite, hue 255, low chroma), `-2` panels and cards, `-3` hover and menus, `-sunk` terminal, diff, inputs
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
- Shell: no title bar. A frame (`--color-frame`, one step darker than the panel) holds a 38px drag strip on top, where the traffic lights sit, and a 56px rail at the side: the repository as a 32px tile (its first letters; the menu of repositories opens beside it), the screens as 18px outline icons in 36px targets (Inbox, Board, Lanes, Ledger; a rule; Code, Terminal) with a count badge when something waits (accent when it waits on you), and at the foot the Claude quota as two thin bars and Settings. Hover names the screen and its key beside the rail. Everything else sits in one inset panel: 8px from the frame, 10px radius, a hairline. Research: docs/research/ui-trends.md
- The Tower (signature): a compact list like a terminal's tab list. Each row: the number in small mono, the branch, a mono sub-line (idle days, dirty files, ahead/behind). A row says nothing at the right unless it matters: "You" with its icon in accent, a run's clock, Failed, Done. No row is ever filled with colour. Never reorders on activity
- State marks (`.chip`): an icon and a word in the state's colour, no fill, no outline
- Board groups by who acts next (Needs you, Backlog, In progress, Landed); "By stage" is the alternate lens. Cards that need you carry a 2px accent stripe on the left and their mark in accent, never a wash or a full ring. A card linked to a lane carries the track at card size (8px stops) with the one stop that matters in words (next: main, mixed in dev, 1↓)
- Board columns are wells (paper-sunk, ring, radius 7), each head an icon, the name and the count; the Needs you icon and count turn accent when it holds cards. A card whose broken state waits on you (mixed, a failing check) takes the stripe in fail instead of accent. Landed cards step back: title in ink-2, regular weight
- Run all: the Backlog head's one action, quiet text with a play mark in accent (never a second primary). It starts every backlog card the planner would not hold back, one after another, staying on the Board; "Run ready" when the planner holds some back, its tooltip naming which. A paid option stops it and asks once for the rest
- Planner on the Board: a card that can start now says nothing more; one the planner would hold back carries one muted line in place of its why ("After #6: both touch invites/index.ts", "Waits for feat/x to land: both touch cookie.ts"). A prediction, never a lock
- Inbox items say why they wait, offer options with one recommended, answer with a single key, and record a decision. Questions group under one head per card, with that card's decisions so far as context
- Diff first: per-hunk Keep and Drop while the card waits on your review (Keep is a quiet mark that you decided; Drop, in the fail colour, reverts that change in the lane's worktree as a commit "review: drop a change in <file>" and refuses if the lane moved), a Viewed tick per file, scope this round or the whole card, Open sends the file to Code
- Lanes screen: one table grouped by who acts next (Waiting on you, In progress, Ready to release, Shipped, Parked). Waiting on you is only what is stuck without you (conflict, mixed with dev, a pull to settle, a review); a lane in dev and not yet in main is Ready to release, calm, with Ship to main as quiet text. Each lane carries the track, four stops (local, remote, integration, production) joined in ink where reached; only the stop your move reaches is accent, a count to settle is an ink ring with its figure, a remote deletion is a dashed stop, contamination is fail. One Next line per lane says the action or the proof (Squashed into main). On this screen the docked Tower folds away: the screen is the Tower at full width
- Run screen: the log (the agent's words, each tool as name, detail relative to the worktree and a mark), the clock ticking in mono in the head beside the state chip, the lane's track under the title, figures in a 2x2 grid (turns, cost, tokens in with cached, tokens out), the handoff once (beside the log, with Changed and Checked), tools that were not allowed with the flow.toml fix. A box tells the agent something without stopping it. Ledger: one table of runs with totals in the foot. Quota meters in the top bar only show figures an agent reported
- Review screen: the verdict in one sentence under the title (the Board's words), the primary action follows the evidence (Ask for changes when a check fails or the judge asks; Approve when all passes), the diff per file with a Viewed tick, judge findings at the line they concern (blockers and majors with a fail stripe, the rest quiet), a side column with checks ("1 of 2 pass") and the judge as a compact index of findings, the ask-for-changes composer under them saying which round it sends
- Merge queue: shipping always goes through the queue. A lane in line is in progress, not waiting on you; its Next cell says "2nd in line for dev" or "Shipping to main" with a spinner. A conflict the queue hit puts the lane back in Waiting on you with the file and a Clear action. The Lanes summary adds one clause ("Shipping feat/x to main, 1 more in line."); Ready to release offers "Queue all N for main" when more than one lane is ready for production
- Search field (`.search-field`): a quiet paper-2 fill with the search icon inside, no ring at rest, a hairline on hover, the focus outline when typing; the browser's own search chrome removed. Every in-app search uses it
- Code tab: VS Code (code-server, bundled) fills the panel and wears the design system, as Cursor reshapes VS Code: flat (no modern-UI floating panels), the activity bar as Pitlane's rail (18px icons in 36px rounded targets, no side marker), sentence-case headers, Geist and Geist Mono, sidebar rows with rounded inset selection, tabs as small rounded chips, a quiet status bar, popovers with 8px radius. The folder picker (repository or a lane's worktree) sits in VS Code's own side bar title. Its corners take the panel's 10px radius over the frame colour. Pitlane's colours are written into Dark Modern and Light Modern. The look goes in past VS Code's CSP as a constructed stylesheet and FontFace bytes (apps/desktop/src-tauri/src/vscode-look.css, vscode-look.js, vscode-place.js)
- Terminal screen: your own login shell in a real PTY, in the repository or any lane's worktree. Tabs at the side by default (240px, like Code's tree; on top as an option), each tab a tree of panes. A tab row: the place in small mono (a name you give it wins; the program's OSC title is only the second line, never the name), the folder under it, and at the right only what matters, in icon and word: Needs you and Waiting in accent text, Working muted, Exited in fail. Tabs never move by themselves. Panes split with ⌘D / ⌘⇧D and, as in Warp, the panes along that split share the room equally; each with a 28px bar (place, title, zoom, close) only when the tab has more than one; the focused bar carries a 1px accent underline and the others' text dims. A divider is a 1px rule with an 8px hit area; drag to resize, double-click to share the room equally. Drag a pane by its bar onto another pane's side (a half-pane wash in accent at 9%), its middle to swap, or onto a tab. Pane body: 12px top, 16px left, Geist Mono 12 on paper-sunk; every program's colour lifted to 4.5:1. Find (⌘F) floats top right in the pane. The empty state is centred: a head, one line, the places as a list, the three keys. The Ghostty key set (docs/research/terminals.md). The layout is kept and comes back after a restart with fresh shells in the same folders
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
