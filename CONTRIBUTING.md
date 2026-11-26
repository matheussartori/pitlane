# Contributing to Pitlane

Thanks for helping. Pitlane is young and moves fast, so a short conversation before a big change saves everyone time.

## Before you start

- **Bugs:** open an [issue](../../issues/new/choose) with the steps to reproduce, your macOS version and `claude --version`.
- **Features or larger changes:** open an issue first and describe the problem, not just the solution. Small fixes (typos, copy, obvious bugs) can go straight to a pull request.
- **Security issues:** don't open an issue. See [SECURITY.md](SECURITY.md).

## Set up

Follow [Getting started](README.md#getting-started) in the README. To work on the UI without the daemon, run `cd apps/desktop && npm run dev` and open it in a browser with fixture data.

## Before you open a pull request

```bash
cargo test                          # or narrow it: cargo test -p pit-core
cargo clippy --workspace            # must stay clean
cd apps/desktop && npm run build    # type-checks the UI
```

- **Don't run `cargo fmt`.** The repository isn't rustfmt-formatted, and it rewrites dozens of files. Keep the diff to what you changed.
- **UI text** is English and goes through `t()`. Add the Portuguese string to `apps/desktop/src/locales/pt`.
- **UI work** follows the design system in [`design/design.md`](design/design.md).
- **Screenshots:** if your change alters a screen shown in the README, retake them with `tools/screenshots.sh`.
- **Commit messages** are prose that describes the change as a user sees it ("Approving a card ships it into its run's branch"), not a list of files.

## Rules a change must keep

These come from the [design principles](README.md#design-principles) and aren't up for negotiation in a pull request:

- Agents run through their official, unmodified CLIs. Pitlane never reads, stores or proxies login tokens.
- Pitlane never switches between two accounts of the same provider by itself.
- Nothing Pitlane starts is invisible or unstoppable, and nothing runs after the app closes.
- Pitlane writes nothing into a user's repository except `.pitlane/flow.toml`.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
