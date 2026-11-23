//! Side sessions beside the runs: analyses, talks, the judge's session, the closing step and the
//! board assistant.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice, Routing};
use pit_core::actions;
use pit_core::cards::{self, CardState};
use pit_core::flow::RUN_BASE_TOOLS;
use pit_core::lanes::now_secs;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::control::{Release, end_group};
use super::launch::{mode_for, read_only_args};
use super::run::AFTER_EXIT_READ;
use super::{Supervisor, home_dir};

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
    /// Run all's analysis: for each card not analysed yet, a read-only Claude Code session in the
    /// repository returns questions and notes, written on the card. Three at a time.
    pub async fn analyze(self: &Arc<Self>, repo: PathBuf, ids: Vec<u32>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        // Never the API key: analysis is not worth a paid token without a yes.
        let choice = self.side_choice(&r.root)?;
        // The repository's model for agents, else yours (the judge's model is for judging).
        let model = r.flow.run.model.clone();
        let batch: Vec<cards::Card> = ids
            .into_iter()
            .filter_map(|id| cards::get(&r, id).ok())
            .collect();
        let todo: Vec<cards::Card> = batch
            .iter()
            .filter(|c| c.analyzed.is_none() && c.state != CardState::Done)
            .cloned()
            .collect();
        let gate = Arc::clone(&self.analysis_gate);
        let epoch = self.epoch.load(Ordering::SeqCst);
        // A card already being analysed (another Run all) is not analysed twice.
        let todo: Vec<cards::Card> = {
            let mut busy = self.analysing.lock().unwrap();
            todo.into_iter()
                .filter(|c| busy.insert((r.root.clone(), c.id)))
                .collect()
        };
        let mut tasks = Vec::new();
        for card in todo {
            let (gate, me, root, choice, model) = (
                gate.clone(),
                self.clone(),
                r.root.clone(),
                choice.clone(),
                model.clone(),
            );
            let prompt = pit_core::analyze::prompt(&r, &card, &batch);
            tasks.push(tokio::spawn(async move {
                let _busy = Release(&me.analysing, (root.clone(), card.id));
                let _slot = gate.acquire().await;
                if me.epoch.load(Ordering::SeqCst) != epoch {
                    return (card.id, Err(anyhow::anyhow!("stopped before it started")));
                }
                let out = me
                    .analyst(&root, &card, &prompt, choice.as_ref(), model.as_deref())
                    .await;
                (card.id, out)
            }));
        }
        let mut results = Vec::new();
        // Sibling repositories the cards' code lives in: what the analysts named, and what their
        // notes mention, that exist next to this one and are not configured yet.
        let mut found: Vec<String> = Vec::new();
        for t in tasks {
            let Ok((id, out)) = t.await else { continue };
            results.push(match out.and_then(|(a, cost)| {
                let (q, n) = (a.questions.len(), a.notes.len());
                let text = format!("{}\n{}", a.repos.join(" "), a.notes.join("\n"));
                for rel in pit_core::related::mentioned(&r, &text) {
                    if !found.contains(&rel) {
                        found.push(rel);
                    }
                }
                pit_core::analyze::record(&r, id, a)?;
                Ok((q, n, cost))
            }) {
                Ok((q, n, cost)) => serde_json::json!({ "card": id, "questions": q, "notes": n, "cost_usd": cost }),
                Err(e) => serde_json::json!({ "card": id, "error": format!("{e:#}") }),
            });
        }
        // Added now, shown on the Run sheet, where the person can take any of them off.
        let repos_error = if found.is_empty() {
            None
        } else {
            pit_core::related::add(&r, &found)
                .err()
                .map(|e| format!("{e:#}"))
        };
        Ok(
            serde_json::json!({ "analyzed": results, "repos_added": found, "repos_error": repos_error }),
        )
    }

    /// Claude's title and summary for a card just made (`title`) or whose spec changed (summary
    /// only). Always Sonnet, no tools, about a cent; written on the card when it is back, unless
    /// the spec changed meanwhile.
    pub async fn describe(self: &Arc<Self>, repo: PathBuf, id: u32, title: bool) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = cards::get(&r, id)?;
        // Never the API key, like the analysis: a summary is not worth a paid token without a yes.
        let choice = self.side_choice(&r.root)?;
        let language = pit_core::language::agent_language(&self.cfg.home);
        let system = format!("{}{}", pit_core::describe::RULES, pit_core::language::rule(&language));
        let out = self
            .one_shot(
                &r.root,
                choice.as_ref(),
                &system,
                &pit_core::describe::schema(title).to_string(),
                format!(
                    "{}{}",
                    pit_core::describe::prompt(&card, title),
                    pit_core::language::reminder(&language)
                ),
                "the description",
            )
            .await?;
        let d = pit_core::describe::read(&String::from_utf8_lossy(&out.stdout))?;
        let card = cards::describe(
            &r,
            id,
            &card.body,
            d.title.as_deref().filter(|_| title),
            &d.summary,
        )?;
        Ok(serde_json::json!({ "card": card.id, "title": card.title, "summary": card.summary }))
    }

    /// Claude's digest of report `id` (Sonnet, no tools, about a cent): saved on the report.
    pub async fn report_digest(self: &Arc<Self>, repo: PathBuf, id: String) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let report = pit_core::report::get(&self.cfg.home, &id)?;
        // Never the API key, like the card summaries.
        let choice = self.side_choice(&r.root)?;
        let language = pit_core::language::agent_language(&self.cfg.home);
        let system = format!("{}{}", pit_core::report::DIGEST_RULES, pit_core::language::rule(&language));
        let out = self
            .one_shot(
                &r.root,
                choice.as_ref(),
                &system,
                &pit_core::report::digest_schema().to_string(),
                format!("{}{}", pit_core::report::digest_prompt(&report), pit_core::language::reminder(&language)),
                "the digest",
            )
            .await?;
        let d = pit_core::report::read_digest(&String::from_utf8_lossy(&out.stdout))?;
        let report = pit_core::report::set_digest(&self.cfg.home, &id, d)?;
        Ok(serde_json::to_value(report)?)
    }

    async fn analyst(
        &self,
        repo: &Path,
        card: &cards::Card,
        prompt: &str,
        choice: Option<&Choice>,
        model: Option<&str>,
    ) -> Result<(pit_core::analyze::Analysis, Option<f64>)> {
        let mut args: Vec<String> = read_only_args(&mode_for(repo));
        args.extend([
            "--allowedTools".into(),
            READ_ONLY_TOOLS.into(),
            "--max-turns".into(),
            ANALYSIS_MAX_TURNS.into(),
        ]);
        // Read-only: it may look at sibling repositories, where a project's code often lives.
        if let Some(parent) = repo.parent() {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::analyze::RULES.into(),
            "--json-schema".into(),
            pit_core::analyze::schema().to_string(),
        ]);
        if let Some(m) = model {
            args.extend(["--model".into(), m.into()]);
        }
        let side = Side {
            kind: SessionKind::Analysis,
            repo: repo.to_owned(),
            cwd: repo.to_owned(),
            card: card.id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir.clone()),
            model: model.map(str::to_owned),
            args,
            prompt: prompt.to_owned(),
            limit: ANALYSIS_LIMIT,
            board: false,
            images: Repo::open(repo, &home_dir(), &self.cfg.home)
                .map(|r| cards::image_bytes(&r, card))
                .unwrap_or_default(),
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the analysis {e:#}"))?;
        pit_core::analyze::read(&stdout)
    }

    /// Talks one question over: what the person said goes on the question, a read-only Claude
    /// Code session answers it with the options as they stand now, and that answer goes on the
    /// question too (its options become the question's). Without a message it answers the last
    /// one again (after a failure). Waits for the answer; returns the card.
    pub async fn talk(self: &Arc<Self>, repo: PathBuf, id: u32, question: u32, message: Option<String>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = match message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => cards::say(&r, id, question, "you", m, None)?,
            None => cards::get(&r, id)?,
        };
        let last = card.questions.iter().find(|q| q.id == question).and_then(|q| q.talk.last());
        if last.is_none_or(|t| t.by != "you") {
            anyhow::bail!("there is nothing to answer on question {question} of card #{id}");
        }
        let prompt = pit_core::analyze::talk_prompt(&r, &card, question)?;
        // Like the analysis: never the API key without a yes.
        let choice = self.side_choice(&r.root)?;
        let model = r.flow.run.model.clone();
        let mut args: Vec<String> = read_only_args(&mode_for(&r.root));
        args.extend([
            "--allowedTools".into(),
            READ_ONLY_TOOLS.into(),
            "--max-turns".into(),
            TALK_MAX_TURNS.into(),
        ]);
        if let Some(parent) = r.root.parent() {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::analyze::TALK_RULES.into(),
            "--json-schema".into(),
            pit_core::analyze::talk_schema().to_string(),
        ]);
        if let Some(m) = &model {
            args.extend(["--model".into(), m.clone()]);
        }
        let side = Side {
            // An analysis to everything that counts sessions: it reads, it is no run.
            kind: SessionKind::Analysis,
            repo: r.root.clone(),
            cwd: r.root.clone(),
            card: id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir),
            model,
            args,
            prompt,
            limit: TALK_LIMIT,
            board: false,
            images: cards::image_bytes(&r, &card),
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the answer {e:#}"))?;
        let (reply, cost) = pit_core::analyze::read_talk(&stdout)?;
        let offer = cards::NewQuestion {
            text: String::new(),
            options: reply.options,
            recommended: reply.recommended,
            context: reply.context,
        };
        let card = cards::say(&r, id, question, "claude", &reply.reply, Some(offer))?;
        Ok(serde_json::json!({ "card": cards::CardOut::from(card), "cost_usd": cost }))
    }

    /// Stops the conversation going on a card's questions (it is an analysis session of that
    /// card), and nothing else: Run all's analyses waiting for a slot keep their place.
    pub fn stop_talk(self: &Arc<Self>, repo: &Path, id: u32) -> usize {
        self.stop_matching(|m| m.kind == SessionKind::Analysis && m.card == id && m.repo == repo)
    }

    /// Run all's closing step: one Claude Code session in the repository (or the target branch's
    /// worktree) that does what the person asked for when everything is done. It edits files and
    /// asks through Pitlane for anything no rule allows. Waits for it; returns its summary.
    pub async fn finish(
        self: &Arc<Self>,
        repo: PathBuf,
        ids: Vec<u32>,
        instructions: String,
        into: Option<String>,
    ) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let cwd = match into.as_deref() {
            Some(t) if !r.flow.flow.is_trunk(t) => {
                actions::ensure_worktree(&r, t).unwrap_or_else(|_| r.root.clone())
            }
            _ => r.root.clone(),
        };
        let choice = self.side_choice(&r.root)?;
        let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
        tools.extend(r.flow.run.allow.iter().cloned());
        let args: Vec<String> = vec![
            "--setting-sources".into(),
            "project,local".into(),
            "--permission-mode".into(),
            mode_for(&r.root),
            "--allowedTools".into(),
            tools.join(","),
            "--disallowedTools".into(),
            "AskUserQuestion".into(),
            "--max-turns".into(),
            FINISH_MAX_TURNS.into(),
        ];
        let side = Side {
            kind: SessionKind::Finish,
            repo: r.root.clone(),
            cwd,
            card: 0,
            card_title: "When it is all done".into(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.as_ref().and_then(|c| c.config_dir.clone()),
            model: None,
            args,
            prompt: pit_core::report::finish_prompt(&r, &ids, &instructions, into.as_deref()),
            limit: FINISH_LIMIT,
            board: false,
            images: Vec::new(),
        };
        let id_hint = now_secs();
        let out = self.side_session(side).await;
        // The session's id: the newest finish session of this repository.
        let run = self
            .sessions(Some(&r.root))?
            .into_iter()
            .find(|m| m.kind == SessionKind::Finish && m.started_at >= id_hint)
            .map(|m| m.id);
        Ok(match out {
            Ok(line) => {
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                serde_json::json!({
                    "run": run,
                    "ok": !v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    "summary": v.get("result").and_then(Value::as_str),
                    "cost_usd": v.get("total_cost_usd").and_then(Value::as_f64),
                })
            }
            Err(e) => serde_json::json!({ "run": run, "ok": false, "summary": format!("{e:#}") }),
        })
    }

    /// The Board's assistant: what the person typed or said ("make cards from this list") goes to
    /// a Claude Code session in the repository that reads the code and writes cards with
    /// Pitlane's board tools. It runs in the background, in view in the Agents dock; people can
    /// keep talking to it while it works.
    pub fn assist(self: &Arc<Self>, repo: PathBuf, text: String) -> Result<()> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        if self.cfg.pit_bin.is_none() {
            bail!("the board assistant needs the pit executable for its board tools");
        }
        let choice = self.side_choice(&r.root)?;
        let tools = format!("{READ_ONLY_TOOLS},mcp__pitlane__add_card,mcp__pitlane__list_cards,mcp__pitlane__get_card,mcp__pitlane__update_card,mcp__pitlane__delete_card,mcp__pitlane__add_note,mcp__pitlane__brief");
        let args: Vec<String> = vec![
            "--setting-sources".into(),
            "project,local".into(),
            "--permission-mode".into(),
            mode_for(&r.root),
            "--disallowedTools".into(),
            "Edit,Write,NotebookEdit,AskUserQuestion".into(),
            "--allowedTools".into(),
            tools,
            "--append-system-prompt".into(),
            ASSIST_RULES.into(),
        ];
        let side = Side {
            kind: SessionKind::Assist,
            repo: r.root.clone(),
            cwd: r.root.clone(),
            card: 0,
            card_title: "Board assistant".into(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.as_ref().and_then(|c| c.config_dir.clone()),
            model: None,
            args,
            prompt: text,
            // No time limit: it ends when the person stops it.
            limit: Duration::MAX,
            board: true,
            images: Vec::new(),
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let _ = me.side_session(side).await;
        });
        Ok(())
    }

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
            id: id.clone(),
            repo: side.repo.clone(),
            card: side.card,
            card_title: side.card_title.clone(),
            branch: String::new(),
            worktree: side.cwd.clone(),
            agent: "claude-code".into(),
            model: side.model.clone(),
            effort: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            pid: None,
            state: RunState::Running,
            started_at: now,
            ended_at: None,
            cost_usd: None,
            duration_ms: None,
            turns: None,
            tokens: Default::default(),
            handoff: None,
            error: None,
            denied: Vec::new(),
            resumed_from: None,
            waiting: Vec::new(),
            account: side.account.clone(),
            reason: None,
            handoff_from: None,
            exhausted: Vec::new(),
            next: None,
            resets_at: None,
            round: 0,
            verdict: None,
            proofs_ok: None,
            base_sha: None,
            brief: None,
            kind: side.kind,
            parent: side.parent.clone(),
            autopilot: false,
            extensions: 0,
            related: Vec::new(),
        };
        self.register(meta)?;

        let mut cmd = self.side_command(&id, &side)?;
        let work = async {
            let mut child = cmd
                .spawn()
                .with_context(|| format!("could not start {}", self.cfg.claude_bin))?;
            let mut stdin = child.stdin.take().context("no stdin")?;
            stdin
                .write_all(
                    format!(
                        "{}\n",
                        runs::claude_user_line_with(
                            &format!(
                                "{}{}",
                                side.prompt,
                                pit_core::language::reminder(&pit_core::language::agent_language(&self.cfg.home))
                            ),
                            &side.images
                        )
                    )
                    .as_bytes(),
                )
                .await?;
            stdin.flush().await?;
            {
                let mut live = self.live.lock().unwrap();
                if let Some(l) = live.get_mut(&id) {
                    l.stdin = Some(stdin);
                    l.turn = true;
                    l.meta.pid = child.id();
                }
            }
            self.update(&id, |_| {});
            let mut err = child.stderr.take().context("no stderr")?;
            let err_task = tokio::spawn(async move {
                let mut s = String::new();
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut err, &mut s).await;
                s
            });
            let mut raw = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir(&id).join("events.jsonl"))?;
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
                let _ = writeln!(raw, "{line}");
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                self.note_background(&id, &v);
                if v.get("type").and_then(Value::as_str) == Some("result") {
                    last = Some(line.clone());
                }
                for ev in runs::read_claude_line(&v) {
                    let mut done = None;
                    self.absorb(&id, &ev, &mut done).await;
                    ok = done.or(ok);
                    self.emit(&id, ev);
                }
                self.keep_shots(&id, &v);
            }
            let status = match status {
                Some(s) => s,
                None => child.wait().await?,
            };
            if let Some(l) = self.live.lock().unwrap().get_mut(&id) {
                l.meta.pid = None;
            }
            if let Some(pid) = pid {
                end_group(pid);
            }
            let stderr = err_task.await.unwrap_or_default();
            if !stderr.trim().is_empty() {
                let _ = fs::write(self.dir(&id).join("stderr.log"), &stderr);
            }
            anyhow::Ok((last, ok, status, stderr))
        };
        let outcome = tokio::time::timeout(side.limit, work).await;
        let stopped = self
            .live
            .lock()
            .unwrap()
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
                    let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
                    match last {
                        Some(_) => "ended with an error".to_owned(),
                        None if tail.is_empty() => format!("exited with {status}"),
                        None => tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
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
        self.live.lock().unwrap().remove(&id);
        result
    }
}

const ASSIST_RULES: &str = "You are Pitlane's board assistant. The person tells you, typed or dictated (so expect \
spoken phrasing and transcription slips), what work they want on this repository's board. You turn it into cards with \
the pitlane tools: one card per piece of work someone could finish on its own, a short imperative title, and a spec in \
markdown that says what and why, with acceptance in a line or two. Read the code when it helps you write a precise spec \
(paths, names), but never change files and never do the work itself. Before adding, list the cards to avoid duplicates; \
update an existing card instead when it is the same work. When they ask to change, annotate or delete cards, do that. End with a \
one-line summary per card you added or changed (#id and title).";
