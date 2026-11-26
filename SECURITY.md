# Security policy

Pitlane runs coding agents on your machine with access to your repositories and shell, so security reports are welcome and taken seriously.

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through GitHub: go to the repository's **Security** tab and choose **Report a vulnerability**.

Include what you found, how to reproduce it, and what an attacker could do with it. You'll get an answer within a week. Once a fix ships, you'll be credited in the release notes unless you'd rather not be.

## Supported versions

Pitlane is pre-release. Only the latest commit on `main` gets security fixes.

## In scope

- Anything that lets an agent or a web page escape the permissions Pitlane shows you (approvals, `flow.toml` rules, the sandbox setting).
- Access to the daemon socket (`~/.pitlane/pitd.sock`) from another user, or anything that leaks credentials.
- Pitlane reading, storing or sending login tokens. It must never do this.

Bugs in Claude Code itself should go to [Anthropic](https://github.com/anthropics/claude-code/security).
