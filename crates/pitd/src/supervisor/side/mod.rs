//! Side sessions beside the runs: analyses, talks, the judge's session, the closing step and the
//! board assistant.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice, Routing};
use pit_core::actions;
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::time::now_secs;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::supervisor::control::{Release, end_group};
use crate::supervisor::launch::{mode_for, read_only_args};
use crate::supervisor::session::{AFTER_EXIT_READ, capture_stderr, stderr_tail};
use crate::supervisor::{Supervisor, home_dir};

mod analysis;
mod assist;
mod finish;
mod reading;
mod talk;

/// What a read-only session may use: read and search the code, and look at its history.
const READ_ONLY_TOOLS: &str = "Read,Grep,Glob,Bash(git log:*),Bash(git show:*),Bash(ls:*)";
/// Turns an analysis may take.
const ANALYSIS_MAX_TURNS: &str = "25";
/// How long an analysis may take.
const ANALYSIS_LIMIT: Duration = Duration::from_secs(20 * 60);
/// Turns an answer on a question may take.
const TALK_MAX_TURNS: &str = "15";
/// How long an answer on a question may take.
const TALK_LIMIT: Duration = Duration::from_secs(10 * 60);
/// Turns Run all's closing step may take.
const FINISH_MAX_TURNS: &str = "80";
/// How long Run all's closing step may take.
const FINISH_LIMIT: Duration = Duration::from_secs(90 * 60);

/// What a side session needs: who it is for, where it runs, and Claude Code's arguments
/// (without the stream-json ones, which `side_command` adds).
pub(super) struct Side {
    pub(super) kind: SessionKind,
    pub(super) repo: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) card: u32,
    pub(super) card_title: String,
    pub(super) parent: Option<String>,
    pub(super) account: String,
    pub(super) config_dir: Option<PathBuf>,
    pub(super) model: Option<String>,
    pub(super) args: Vec<String>,
    pub(super) prompt: String,
    pub(super) limit: Duration,
    /// Give it Pitlane's board tools too (`pit mcp`: add_card, list_cards, update_card, add_note).
    pub(super) board: bool,
    /// Images sent with the prompt (the card's screenshots, for the analysis).
    pub(super) images: Vec<(&'static str, Vec<u8>)>,
}

impl Supervisor {
    /// The route's first option that is not an API key: a side session never spends a paid
    /// token without a yes.
    fn side_choice(&self, root: &Path) -> Result<Option<Choice>> {
        let routing = Routing::load(&self.cfg.home)?;
        Ok(routing
            .choices(root, &home_dir(), &self.cfg.home)?
            .into_iter()
            .find(|c| c.kind != accounts::Kind::ApiKey))
    }

    /// A read-only Claude Code session beside the runs (analysis, judge), recorded and followed
    /// like a run, so the app shows it live and people can write to it while it works. Returns
    /// the last `result` line it printed.
    pub(super) async fn side_session(&self, side: Side) -> Result<String> {
        let now = now_secs();
        let tag = match side.kind {
            SessionKind::Analysis => "analysis",
            SessionKind::Judge => "judge",
            SessionKind::Run => "run",
            SessionKind::Finish => "finish",
            SessionKind::Assist => "assist",
        };
        let id = format!(
            "{}-{tag}-{now}-{}",
            side.card,
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        );
        let meta = RunMeta {
            card_title: side.card_title.clone(),
            worktree: side.cwd.clone(),
            model: side.model.clone(),
            session_id: uuid::Uuid::new_v4().to_string(),
            state: RunState::Running,
            account: side.account.clone(),
            parent: side.parent.clone(),
            ..RunMeta::new(id.clone(), side.repo.clone(), side.card, side.kind, now)
        };
        self.register(meta)?;
        // The Board's assistant opens with what the person typed or dictated, at the top of its
        // terminal, as a message they sent later shows.
        if side.kind == SessionKind::Assist {
            self.emit(&id, RunEvent::Said { text: side.prompt.clone() });
        }

        let mut cmd = self.side_command(&id, &side)?;
        let work = async {
            let first = runs::claude_user_line_with(&self.in_their_language(&side.prompt), &side.images);
            let (mut child, stdin) = self.spawn_session(&id, &mut cmd, &first).await?;
            if let Some(l) = self.live().get_mut(&id) {
                l.stdin = stdin;
                l.turn = true;
            }
            self.update(&id, |_| {});
            let err_task = capture_stderr(child.stderr.take().context("no stderr")?, self.dir(&id).join("stderr.log"));
            let mut raw = self.raw_log(&id)?;
            let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
            let mut last = None::<String>;
            let mut ok = None::<bool>;
            let pid = child.id();
            // Read until the agent exits (not until the pipe closes: a child it left could hold
            // that open), then what is left for a moment.
            let mut status = None;
            loop {
                let line = tokio::select! {
                    l = lines.next_line() => match l { Ok(Some(l)) => l, _ => break },
                    st = child.wait(), if status.is_none() => { status = Some(st?); continue; }
                    _ = tokio::time::sleep(AFTER_EXIT_READ), if status.is_some() => break,
                };
                let read = self.read_line(&id, &mut raw, &line).await;
                if read.result {
                    last = Some(line);
                }
                ok = read.done.or(ok);
            }
            let status = match status {
                Some(s) => s,
                None => child.wait().await?,
            };
            if let Some(l) = self.live().get_mut(&id) {
                l.meta.pid = None;
            }
            if let Some(pid) = pid {
                end_group(pid);
            }
            let stderr = stderr_tail(err_task).await;
            anyhow::Ok((last, ok, status, stderr))
        };
        let outcome = tokio::time::timeout(side.limit, work).await;
        let stopped = self.live()
            .get(&id)
            .is_some_and(|l| l.stop_requested);
        let (state, code, result) = match outcome {
            Ok(Ok((Some(line), Some(true), status, _))) => {
                (RunState::Done, status.code(), Ok(line))
            }
            Ok(Ok((last, _, status, stderr))) => {
                let why = if stopped {
                    "was stopped".to_owned()
                } else {
                    let tail = &stderr[stderr.len().saturating_sub(4)..];
                    match last {
                        Some(_) => "ended with an error".to_owned(),
                        None if tail.is_empty() => format!("exited with {status}"),
                        None => tail.join("\n"),
                    }
                };
                (
                    if stopped {
                        RunState::Stopped
                    } else {
                        RunState::Failed
                    },
                    status.code(),
                    last.ok_or_else(|| anyhow::anyhow!(why)),
                )
            }
            Ok(Err(e)) => (RunState::Failed, None, Err(e)),
            Err(_) => {
                if let Some(pid) = self.get(&id).ok().and_then(|m| m.pid) {
                    end_group(pid);
                }
                (
                    RunState::Failed,
                    None,
                    Err(anyhow::anyhow!(
                        "took longer than {} minutes",
                        side.limit.as_secs() / 60
                    )),
                )
            }
        };
        let error = result.as_ref().err().map(|e| format!("{e:#}"));
        self.update(&id, |m| {
            m.state = state;
            m.ended_at = Some(now_secs());
            m.pid = None;
            if m.error.is_none() {
                m.error = error;
            }
        });
        self.emit(&id, RunEvent::Exited { code, state });
        self.live().remove(&id);
        result
    }
}

/// The folder holding a repository and its siblings, for read-only sessions to look around in;
/// none when that folder is your home or above it, which would open ~/.ssh and the rest of
/// your files to a session that only meant to read the code next door.
fn siblings_dir(repo: &Path) -> Option<&Path> {
    let parent = repo.parent()?;
    let home = home_dir();
    (!home.starts_with(parent)).then_some(parent)
}
