//! Thin wrapper over the `git` CLI. We shell out on purpose: `merge-tree --write-tree`,
//! `cherry`, `worktree` and `rerere` all live there, and fetch/push then use the user's own
//! SSH keys, credential helpers and hooks.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Same repository, run from another worktree.
    pub fn at(&self, dir: impl Into<PathBuf>) -> Self {
        Self::new(dir)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
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
        child.stdin.take().context("no stdin")?.write_all(input.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
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
