//! Checking finished work: the repository's proofs, then the judge.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use pit_core::Repo;
use pit_core::accounts::{self, Routing};
use pit_core::cards::{self, CardState};
use pit_core::time::now_secs;
use pit_core::runs::{RunEvent, RunMeta, RunState, SessionKind};

use super::card::set_card_state;
use super::launch::{mode_for, read_only_args};
use super::side::Side;
use super::{Supervisor, home_dir};

/// The most of a lane's diff the judge is given, in bytes.
const DIFF_BUDGET: usize = 200_000;
/// What the judge may use: read and search the code, and look at the diff and the history.
const JUDGE_TOOLS: &str = "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)";
/// Turns the judge may take.
const JUDGE_MAX_TURNS: &str = "30";
/// How long the judge may take.
const JUDGE_LIMIT: Duration = Duration::from_secs(20 * 60);

impl Supervisor {
    /// Proofs, then the judge, on the run's lane. The run stays live (state `verifying`) meanwhile,
    /// so everyone following it sees each step. Failures here are recorded, never fatal.
    pub(super) async fn verify(self: &Arc<Self>, id: &str, card: u32) {
        let meta = match self.get(id) {
            Ok(m) => m,
            Err(_) => return,
        };
        let home = self.cfg.home.clone();
        let Ok(r) = Repo::open(&meta.repo, &home_dir(), &home) else {
            return;
        };
        let cfg = r.flow.verify.clone();
        if cfg.commands.is_empty() && !cfg.judge {
            return;
        }
        self.update(id, |m| m.state = RunState::Verifying);
        let _ = set_card_state(&meta, CardState::Verifying, &home);
        let mut v = pit_core::verify::Verification {
            run: id.to_owned(),
            started_at: now_secs(),
            no_commands: cfg.commands.is_empty(),
            max_rounds: cfg.max_rounds,
            ..Default::default()
        };

        for cmd in &cfg.commands {
            self.emit(
                id,
                RunEvent::Note {
                    text: format!("Checking: {cmd}"),
                },
            );
            if self.stop_requested(id) {
                break;
            }
            let (wt, c, me, rid) = (
                meta.worktree.clone(),
                cmd.clone(),
                Arc::clone(self),
                id.to_owned(),
            );
            let proof = tokio::task::spawn_blocking(move || {
                pit_core::verify::run_proof(&wt, &c, &|| me.stop_requested(&rid))
            })
            .await
            .unwrap_or_else(|e| pit_core::verify::Proof {
                command: cmd.clone(),
                ok: false,
                code: None,
                duration_ms: 0,
                tail: e.to_string(),
            });
            self.emit(
                id,
                RunEvent::Proof {
                    command: proof.command.clone(),
                    ok: proof.ok,
                    duration_ms: proof.duration_ms,
                },
            );
            v.proofs.push(proof);
        }
        if !v.proofs.is_empty() {
            let ok = v.proofs_ok();
            self.update(id, move |m| m.proofs_ok = Some(ok));
        }

        if cfg.judge && !self.stop_requested(id) {
            let model = cfg.judge_model.clone();
            self.emit(
                id,
                RunEvent::Note {
                    text: format!(
                        "The judge reads the diff ({}), in a session of its own.",
                        model.as_deref().unwrap_or("default model")
                    ),
                },
            );
            let judge = self.judge(&meta, &r, card, &v.proofs, model.clone()).await;
            if let Some(j) = &judge.judgement {
                let (verdict, summary, n) = (j.verdict, j.summary.clone(), j.findings.len());
                self.update(id, move |m| m.verdict = Some(verdict));
                self.emit(
                    id,
                    RunEvent::Judged {
                        verdict,
                        summary,
                        findings: n,
                    },
                );
            } else if let Some(e) = &judge.error {
                self.emit(
                    id,
                    RunEvent::Note {
                        text: format!("The judge could not finish: {e}"),
                    },
                );
            }
            v.judge = Some(judge);
        }
        v.ended_at = Some(now_secs());
        let _ = fs::write(
            self.dir(id).join("verify.json"),
            serde_json::to_string_pretty(&v).unwrap_or_default(),
        );
    }

    /// The judge: a fresh Claude Code session, read-only, on the same account as the run. It gets
    /// the spec, the decisions and the diff; nothing the working agent said.
    async fn judge(
        &self,
        meta: &RunMeta,
        r: &Repo,
        card: u32,
        proofs: &[pit_core::verify::Proof],
        model: Option<String>,
    ) -> pit_core::verify::JudgeRun {
        let mut out = pit_core::verify::JudgeRun {
            model: model.clone(),
            ..Default::default()
        };
        let prompt = (|| -> Result<String> {
            let c = cards::get(r, card)?;
            let diff = pit_core::verify::project_diff(r, &meta.branch, &meta.related, DIFF_BUDGET)?;
            let decisions = fs::read_to_string(cards::decisions_path(r)).unwrap_or_default();
            Ok(pit_core::verify::judge_prompt(
                &c, &decisions, &diff, proofs,
            ))
        })();
        let prompt = match prompt {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("{e:#}"));
                return out;
            }
        };
        let routing = Routing::load(&self.cfg.home).ok();
        let account = routing
            .as_ref()
            .and_then(|rt| rt.account(&meta.account).cloned());
        let mut args: Vec<String> = read_only_args(&mode_for(&meta.repo));
        args.extend([
            "--allowedTools".into(),
            JUDGE_TOOLS.into(),
            "--max-turns".into(),
            JUDGE_MAX_TURNS.into(),
        ]);
        for l in &meta.related {
            args.extend([
                "--add-dir".into(),
                l.worktree.to_string_lossy().into_owned(),
            ]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::verify::JUDGE_RULES.into(),
            "--json-schema".into(),
            pit_core::verify::verdict_schema().to_string(),
        ]);
        if let Some(m) = &model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(helper) = account.and_then(|a| a.api_key_helper) {
            args.extend([
                "--settings".into(),
                serde_json::json!({ "apiKeyHelper": helper }).to_string(),
            ]);
        }
        let side = Side {
            kind: SessionKind::Judge,
            repo: meta.repo.clone(),
            cwd: meta.worktree.clone(),
            card,
            card_title: meta.card_title.clone(),
            parent: Some(meta.id.clone()),
            account: meta.account.clone(),
            config_dir: accounts::profile_dir(&meta.account, &self.cfg.home),
            model: model.clone(),
            args,
            prompt,
            limit: JUDGE_LIMIT,
            board: false,
            images: Vec::new(),
            resume: false,
            fork: None,
        };
        match self.side_session(side).await {
            Ok(stdout) => match pit_core::verify::read_judge(&stdout) {
                Ok((j, cost, dur)) => {
                    out.judgement = Some(j);
                    out.cost_usd = cost;
                    out.duration_ms = dur;
                }
                Err(e) => out.error = Some(format!("{e:#}")),
            },
            Err(e) => out.error = Some(format!("the judge {e:#}")),
        }
        out
    }
}

/// What a fix round is asked to fix: the failing proofs' tails and the judge's findings.
pub(super) fn fix_request(dir: &Path) -> String {
    let Ok(v) = fs::read_to_string(dir.join("verify.json"))
        .map_err(anyhow::Error::from)
        .and_then(|t| Ok(serde_json::from_str::<pit_core::verify::Verification>(&t)?))
    else {
        return "The checks or the judge found problems; look at the diff again and fix what is wrong.".into();
    };
    let mut out = String::new();
    for p in v.proofs.iter().filter(|p| !p.ok) {
        out.push_str(&format!(
            "- `{}` fails:\n```\n{}\n```\n",
            p.command,
            p.tail.trim()
        ));
    }
    if let Some(j) = v.judge.as_ref().and_then(|j| j.judgement.as_ref()) {
        if !j.findings.is_empty() || j.verdict == pit_core::verify::Verdict::Changes {
            out.push_str(&format!("The reviewer says: {}\n", j.summary.trim()));
        }
        for f in &j.findings {
            let at = match (&f.file, f.line) {
                (Some(file), Some(l)) => format!(" ({file}:{l})"),
                (Some(file), None) => format!(" ({file})"),
                _ => String::new(),
            };
            out.push_str(&format!("- [{}]{at} {}\n", f.severity, f.text.trim()));
        }
    }
    if out.is_empty() {
        "The checks or the judge found problems; look at the diff again and fix what is wrong."
            .into()
    } else {
        out
    }
}
