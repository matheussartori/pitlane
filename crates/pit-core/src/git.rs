//! Thin wrapper over the `git` CLI. We shell out on purpose: `merge-tree --write-tree`,
//! `cherry`, `worktree` and `rerere` all live there, and fetch/push then use the user's own
//! SSH keys, credential helpers and hooks.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Git {
    dir: PathBuf,
    hooks: bool,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), hooks: true }
    }

    /// The same, with the repository's hooks off: for what Pitlane does by itself in a lane
    /// (a checkpoint, a local merge, a new worktree). A lane's hooks may be ones an agent wrote,
    /// in a hooks folder kept in the repository (husky, lefthook), and they would run as you,
    /// outside the agent's sandbox. Your own commits and pushes keep them.
    pub fn without_hooks(&self) -> Self {
        Self { dir: self.dir.clone(), hooks: false }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Same repository, run from another worktree (hooks as they were).
    pub fn at(&self, dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), hooks: self.hooks }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.dir)
            // Paths as they are (café.rs), not quoted and octal-escaped: the output is parsed.
            .args(["-c", "core.quotePath=false"]);
        if !self.hooks {
            cmd.args(["-c", "core.hooksPath=/dev/null"]);
        }
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            // Its messages in English whatever the Mac speaks: some are read (a rejected push).
            .env("LANGUAGE", "en")
            .env("LC_MESSAGES", "C")
            .stdin(Stdio::null());
        cmd
    }

    fn output(&self, args: &[&str]) -> Result<Output> {
        self.command(args)
            .output()
            .with_context(|| format!("could not run git {}", args.join(" ")))
    }

    /// Runs git with `input` on stdin (a patch for `git apply -`). Fails on a non-zero exit.
    pub fn run_stdin(&self, args: &[&str], input: &str) -> Result<String> {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not run git {}", args.join(" ")))?;
        // Written from its own thread: a large input whose output fills the pipe would otherwise
        // leave git waiting on us to read and us waiting on git to read.
        let mut stdin = child.stdin.take().context("no stdin")?;
        let input = input.to_owned();
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let out = child.wait_with_output()?;
        let written = writer.join().map_err(|_| anyhow::anyhow!("writing to git panicked"))?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        written?;
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
    }

    /// The first `limit` bytes of what git prints, and whether there was more: for output that
    /// can be any size (a diff with a generated file in it) of which only a screenful is shown.
    /// git is ended once the limit is read, so nothing waits on the rest. Fails on a non-zero
    /// exit when the output was read whole.
    pub fn run_head(&self, args: &[&str], limit: usize) -> Result<(String, bool)> {
        use std::io::Read;
        let mut child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("could not run git {}", args.join(" ")))?;
        let mut bytes = Vec::new();
        let mut stdout = child.stdout.take().context("no stdout")?;
        let read = (&mut stdout).take(limit as u64 + 1).read_to_end(&mut bytes);
        let more = bytes.len() > limit;
        if more {
            let _ = child.kill();
        }
        drop(stdout);
        let status = child.wait()?;
        read?;
        if !more && !status.success() {
            bail!("git {} failed", args.join(" "));
        }
        bytes.truncate(limit);
        let mut s = String::from_utf8_lossy(&bytes).into_owned();
        if !more {
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
        }
        Ok((s, more))
    }

    /// Runs git and returns stdout with the trailing newline removed. Fails on a non-zero exit.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        Ok(s)
    }

    /// Lines of stdout, empty lines dropped.
    pub fn lines(&self, args: &[&str]) -> Result<Vec<String>> {
        Ok(self
            .run(args)?
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// For predicates such as `merge-base --is-ancestor`: exit 0 is true, 1 is false,
    /// anything else is an error.
    pub fn test(&self, args: &[&str]) -> Result<bool> {
        let out = self.output(args)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        }
    }

    /// Returns the raw output whatever the exit code; the caller interprets it.
    pub fn raw(&self, args: &[&str]) -> Result<(i32, String, String)> {
        let out = self.output(args)?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    pub fn rev_parse(&self, rev: &str) -> Result<Option<String>> {
        // A name starting with '-' is an option to git, never a revision.
        if rev.starts_with('-') {
            return Ok(None);
        }
        let (code, out, _) = self.raw(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])?;
        Ok((code == 0).then(|| out.trim().to_owned()))
    }

    pub fn is_ancestor(&self, ancestor: &str, of: &str) -> Result<bool> {
        self.test(&["merge-base", "--is-ancestor", ancestor, of])
    }

    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        let (code, out, _) = self.raw(&["config", "--get", key])?;
        Ok((code == 0).then(|| out.trim().to_owned()))
    }

    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        self.run(&["config", key, value]).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_off_means_a_hook_in_the_repository_does_not_run() {
        let dir = std::env::temp_dir().join(format!("pit-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".hooks")).unwrap();
        let g = Git::new(&dir);
        g.run(&["init", "-q"]).unwrap();
        g.run(&["config", "core.hooksPath", ".hooks"]).unwrap();
        let hook = dir.join(".hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\ntouch ran\n").unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
        let commit = |g: &Git, m: &str| g.run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", m]).unwrap();
        commit(&g.without_hooks(), "quiet");
        assert!(!dir.join("ran").exists(), "the hook ran with hooks off");
        commit(&g, "loud");
        assert!(dir.join("ran").exists(), "the hook should run by default");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
