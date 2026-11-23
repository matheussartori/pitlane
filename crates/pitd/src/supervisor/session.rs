//! What every Claude Code session shares, a card's run or a side session: its stdin, started
//! with the first message, its stdout read into the record and the timeline, and its stderr
//! kept for when it fails.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use pit_core::runs;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::task::JoinHandle;

use super::Supervisor;

/// How long the stdout of an agent that exited is still read (a tool's child process can keep
/// it open).
pub(super) const AFTER_EXIT_READ: Duration = Duration::from_secs(3);
/// How long its stderr is waited for, for the same reason.
pub(super) const STDERR_READ: Duration = Duration::from_secs(1);
/// Lines of stderr kept to say why a session failed.
const STDERR_TAIL: usize = 8;

/// A session's stdin, shared by everyone who writes to it (a message, `/model`, the first
/// message): writes go one at a time and none takes the pipe away from the others. Closing it
/// is how a stream-json session ends, and is final. The default is a closed one.
#[derive(Clone, Default)]
pub(super) struct Stdin(Arc<Pipe>);

#[derive(Default)]
struct Pipe {
    open: AtomicBool,
    pipe: tokio::sync::Mutex<Option<ChildStdin>>,
}

impl Stdin {
    pub(super) fn new(pipe: ChildStdin) -> Self {
        Self(Arc::new(Pipe { open: AtomicBool::new(true), pipe: tokio::sync::Mutex::new(Some(pipe)) }))
    }

    pub(super) fn is_open(&self) -> bool {
        self.0.open.load(Ordering::SeqCst)
    }

    /// Writes `text` whole, after any write under way. False when the session was closed.
    pub(super) async fn write(&self, text: &str) -> std::io::Result<bool> {
        if !self.is_open() {
            return Ok(false);
        }
        let mut pipe = self.0.pipe.lock().await;
        let Some(p) = pipe.as_mut().filter(|_| self.is_open()) else {
            return Ok(false);
        };
        p.write_all(text.as_bytes()).await?;
        p.flush().await?;
        Ok(true)
    }

    /// Closed from now on for anyone who writes: call it under the lock that decided to close,
    /// then `close`.
    pub(super) fn mark_closed(&self) {
        self.0.open.store(false, Ordering::SeqCst);
    }

    /// Closes the pipe once any write under way is done.
    pub(super) async fn close(&self) {
        self.mark_closed();
        if let Some(mut p) = self.0.pipe.lock().await.take() {
            let _ = p.shutdown().await;
        }
    }
}

/// What one line of the agent's output meant.
pub(super) struct Read {
    /// The work ended with this line: well (`Some(true)`) or not.
    pub done: Option<bool>,
    /// The line is Claude Code's `result`.
    pub result: bool,
}

impl Supervisor {
    /// Starts the session's process and writes its first message; the pid is in the record
    /// before anything can fail, so Stop and shutdown reach it. The stdin comes back open.
    pub(super) async fn spawn_session(&self, id: &str, cmd: &mut Command, first: &str) -> Result<(Child, Stdin)> {
        let mut child = cmd.spawn().with_context(|| format!("could not start {}", self.cfg.claude_bin))?;
        if let Some(l) = self.live().get_mut(id) {
            l.meta.pid = child.id();
        }
        let stdin = Stdin::new(child.stdin.take().context("no stdin")?);
        if let Err(e) = stdin.write(&format!("{first}\n")).await {
            if let Some(pid) = child.id() {
                super::control::end_group(pid);
            }
            return Err(e).context("could not write the first message");
        }
        Ok((child, stdin))
    }

    /// The first message's text: `text`, reminded of the language the person wants answers in.
    pub(super) fn in_their_language(&self, text: &str) -> String {
        let lang = pit_core::language::agent_language(&self.cfg.home);
        format!("{text}{}", pit_core::language::reminder(&lang))
    }

    /// The session's raw output log (`events.jsonl`), appended to.
    pub(super) fn raw_log(&self, id: &str) -> Result<File> {
        let path = self.dir(id).join("events.jsonl");
        OpenOptions::new().create(true).append(true).open(&path).with_context(|| format!("could not open {}", path.display()))
    }

    /// One line of the agent's stream-json: kept raw, then turned into the record's changes and
    /// the timeline's events.
    pub(super) async fn read_line(&self, id: &str, raw: &mut File, line: &str) -> Read {
        let _ = writeln!(raw, "{line}");
        let mut read = Read { done: None, result: false };
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return read;
        };
        self.note_background(id, &v);
        read.result = v.get("type").and_then(Value::as_str) == Some("result");
        for ev in runs::read_claude_line(&v) {
            let mut done = None;
            self.absorb(id, &ev, &mut done).await;
            read.done = done.or(read.done);
            self.emit(id, ev);
        }
        self.keep_shots(id, &v);
        read
    }
}

/// Reads a session's stderr into `path` (made at its first line) and keeps its last lines.
pub(super) fn capture_stderr(stderr: ChildStderr, path: PathBuf) -> JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        let mut file = None;
        while let Ok(Some(l)) = lines.next_line().await {
            if file.is_none() {
                file = OpenOptions::new().create(true).append(true).open(&path).ok();
            }
            if let Some(f) = file.as_mut() {
                let _ = writeln!(f, "{l}");
            }
            tail.push(l);
            if tail.len() > STDERR_TAIL {
                tail.remove(0);
            }
        }
        tail
    })
}

/// The stderr's last lines, waiting for them only `STDERR_READ`.
pub(super) async fn stderr_tail(task: JoinHandle<Vec<String>>) -> Vec<String> {
    match tokio::time::timeout(STDERR_READ, task).await {
        Ok(Ok(t)) => t,
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn writes_at_once_all_land_and_a_closed_stdin_says_so() {
        let mut child = Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = Stdin::new(child.stdin.take().unwrap());
        let writes: Vec<_> = (0..20)
            .map(|i| {
                let s = stdin.clone();
                tokio::spawn(async move { s.write(&format!("line {i}\n")).await.unwrap() })
            })
            .collect();
        for w in writes {
            assert!(w.await.unwrap(), "no write finds the pipe taken");
        }
        stdin.close().await;
        assert!(!stdin.is_open());
        assert!(!stdin.write("late\n").await.unwrap());
        let mut out = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut child.stdout.take().unwrap(), &mut out).await.unwrap();
        assert_eq!(out.lines().count(), 20);
        assert!(!out.contains("late"));
        assert!(!Stdin::default().is_open());
    }
}
