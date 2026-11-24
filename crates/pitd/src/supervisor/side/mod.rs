//! Side sessions beside the runs: analyses, talks, the judge's session, the closing step and the
//! board assistant.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice, Routing};
use pit_core::actions;
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::time::now_secs;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;

use crate::supervisor::control::{Release, end_group};
use crate::supervisor::launch::{mode_for, read_only_args};
use crate::supervisor::{Supervisor, home_dir};

mod analysis;
mod aside;
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
    /// Picks the run's Claude Code session back up (`--resume`) instead of starting it.
    pub(super) resume: bool,
    /// Starts as a copy of this Claude Code session (`--resume <it> --fork-session`): it knows
    /// all that session did and said, and nothing it does reaches that session.
    pub(super) fork: Option<String>,
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
        let id = self.open_side(&side)?;
        self.drive_side(id, side).await
    }

    /// Records a side session about to start and returns its id.
    pub(super) fn open_side(&self, side: &Side) -> Result<String> {
        let now = now_secs();
        let tag = match side.kind {
            SessionKind::Analysis => "analysis",
            SessionKind::Judge => "judge",
            SessionKind::Run => "run",
            SessionKind::Finish => "finish",
            SessionKind::Assist => "assist",
            SessionKind::Aside => "aside",
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
        Ok(id)
    }

    /// Runs a recorded side session's Claude Code to its end and records how it ended.
    pub(super) async fn drive_side(&self, id: String, side: Side) -> Result<String> {
        let mut cmd = self.side_command(&id, &side)?;
        let work = async {
            let first = runs::claude_user_line_with(&self.in_their_language(&side.prompt), &side.images);
            let (child, stopped_meanwhile) = self.spawn_session(&id, &mut cmd, &first).await?;
            // A stop that came while it was starting found no process to signal: end it now.
            if stopped_meanwhile && let Some(pid) = child.id() {
                end_group(pid);
            }
            let ended = self.read_to_end(&id, child).await?;
            anyhow::Ok((ended.last, ended.ok, ended.status, ended.stderr))
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
        self.end_session(&id, state, code, |m| {
            if m.error.is_none() {
                m.error = error;
            }
        });
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
