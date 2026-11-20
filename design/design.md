# Design: Pitlane

The locked system. Every UI change reads this first; surfaces share it. Change it on purpose; the file is the rule.
Live specimen: `design/index.html`. Research behind it: `docs/research/ux-research.md`.

## System
- Genre: modern-minimal, app scope (a tool people work in, not a page they read)
- Theme: custom. Idea: "a pit wall: every car in view, only the ones that need you light up"
- Axes: dark/grotesk-heavy/warm. Posture: restrained. Modes: dark (default) + light (re-derived, never inverted)

## Tokens
`design/tokens.css` is the source of truth. Roles:
- Ground: `--color-paper` (asphalt, hue 255), `-2` panels and cards, `-3` hover and menus, `-sunk` terminal, diff, inputs
- Ink: `--color-ink`, `--color-ink-2`, `--color-muted`; rules `--color-rule`, `--color-rule-strong`
- Accent `--color-accent` (signal orange, hue 58) means "your move" only: primary action, focus, selection, needs-you. Under 5% of a screen
- States: `--color-run`, `--color-done`, `--color-fail` (+ `-wash`). Never colour alone: icon + word + colour
- Faces: `--font-display` Archivo 600 with tight tracking (-0.022em) (wordmark, screen titles, lane numbers, the one big moment), `--font-body` Archivo 400/600, `--font-mono` JetBrains Mono (code, terminal, figures that compare, with tabular-nums)
- Scale 1.15 from 14px UI; prose 16px. Space: 4px grid (`--space-*`). Radius 4/6/10, pill only for chips
- Depth on dark is lightness + inset hairline (`--ring`), never drop shadow

## Controls
- Primary: accent fill, accent-ink text, radius 6, height 32 (40 when prominent). One per view
- Secondary: paper-3 fill + hairline ring, same radius and height. Ghost: text only. Destructive: secondary with fail text, always undoable
- Every interactive control ships 8 states: default, hover, focus-visible (instant, never animated), active (scale .97), disabled (.55), loading, error, success
- Inputs sit in `paper-sunk` with a ring; focus is an outline, never a border change

## Product patterns (see the specimen)
- Shell: top bar (wordmark, repo, Inbox / Board / Lanes / Ledger, ⌘K, quota meters) + the Tower docked right
- The Tower (signature): stable numbered lanes, live mono figures, only needs-you rows light up. Never reorders on activity
- Board groups by who acts next (Needs you, Running, To review, Landed); "By stage" is the alternate lens
- Inbox items say why they wait, offer options with one recommended, answer with a single key, and record a decision
- Diff first: per-hunk accept/reject, viewed checkbox, scope = last turn, card or file; pending vs applied is always visible
- Lanes screen: one table grouped by who acts next (Waiting on you, In progress, Shipped, Parked). Each lane carries the track, four stops (local, remote, integration, production) joined in ink where reached; only the stop your move reaches is accent, a count to settle is an ink ring with its figure, a remote deletion is a dashed stop, contamination is fail. One Next line per lane says the action or the proof (Squashed into main). On this screen the docked Tower folds away: the screen is the Tower at full width
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
