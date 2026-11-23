//! Approvals: what the agent asks a person, and what the person decides.


use anyhow::{Context, Result};
use pit_core::Repo;
use pit_core::flow;
use pit_core::lanes::now_secs;
use pit_core::runs::{self, Approval, Decision, RunEvent, RunMeta};
use serde_json::Value;
use tokio::sync::oneshot;

use super::{Supervisor, home_dir};

impl Supervisor {
    /// The agent asks to use a tool no rule allows. Answers at once when a rule of this run or of
    /// the repository allows it; otherwise waits for a person. Returns Claude Code's answer:
    /// `{"behavior":"allow","updatedInput":…}` or `{"behavior":"deny","message":…}`.
    pub async fn ask(
        &self,
        run: &str,
        tool: &str,
        input: Value,
        tool_use_id: Option<String>,
    ) -> Value {
        let allow =
            |input: &Value| serde_json::json!({ "behavior": "allow", "updatedInput": input });
        let deny = |msg: &str| serde_json::json!({ "behavior": "deny", "message": msg });
        let (repo, run_rules, autopilot) = {
            let live = self.live.lock().unwrap();
            let Some(l) = live.get(run) else {
                return deny("This run is not live any more.");
            };
            (l.meta.repo.clone(), l.rules.clone(), l.meta.autopilot)
        };
        // Run all: allowed at once, and written down so the review can see what ran.
        if autopilot {
            self.emit(
                run,
                RunEvent::Note {
                    text: format!("Allowed {tool} without asking (Run all)"),
                },
            );
            return allow(&input);
        }
        let repo_rules = Repo::open(&repo, &home_dir(), &self.cfg.home)
            .map(|r| r.flow.run.allow)
            .unwrap_or_default();
        if run_rules
            .iter()
            .chain(repo_rules.iter())
            .any(|r| runs::rule_allows(r, tool, &input))
        {
            return allow(&input);
        }
        let mut approval = Approval {
            id: tool_use_id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()),
            tool: tool.to_owned(),
            detail: runs::read_claude_line(&serde_json::json!({
                "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "", "name": tool, "input": input }] } }))
            .into_iter()
            .find_map(|e| match e {
                RunEvent::Tool { detail, .. } => Some(detail),
                _ => None,
            })
            .unwrap_or_default(),
            rule: runs::rule_for(tool, &input),
            input: input.clone(),
            asked_at: now_secs(),
        };
        // Said first, so it is the first thing you read: this one would run unsandboxed.
        if runs::unsandboxed(&input) {
            approval.detail = format!("Outside the sandbox: {}", approval.detail);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut live = self.live.lock().unwrap();
            let Some(l) = live.get_mut(run) else {
                return deny("This run is not live any more.");
            };
            // An id already waiting (a repeated or forged tool_use_id) gets one of its own, so it
            // never takes the place of the request a person is looking at.
            if l.pending.contains_key(&approval.id) {
                approval.id = uuid::Uuid::new_v4().simple().to_string();
            }
            l.pending.insert(approval.id.clone(), tx);
            l.meta.waiting.push(approval.clone());
        }
        self.update(run, |_| {});
        self.emit(
            run,
            RunEvent::Approval {
                approval: approval.clone(),
            },
        );

        let answer = rx.await;
        self.update(run, |m| m.waiting.retain(|a| a.id != approval.id));
        let Ok((decision, message)) = answer else {
            return deny("The run ended before anyone answered.");
        };
        match decision {
            Decision::Run => {
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Always => {
                if let Err(e) = flow::add_run_allow(&repo, &approval.rule) {
                    self.emit(
                        run,
                        RunEvent::Note {
                            text: format!("Could not save {} to flow.toml: {e:#}", approval.rule),
                        },
                    );
                }
                if let Some(l) = self.live.lock().unwrap().get_mut(run) {
                    l.rules.push(approval.rule.clone());
                }
            }
            Decision::Once | Decision::Deny => {}
        }
        self.emit(
            run,
            RunEvent::Decided {
                id: approval.id.clone(),
                decision,
                message: message.clone(),
            },
        );
        if decision == Decision::Deny {
            deny(
                message
                    .as_deref()
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or(
                        "The person declined this. Find another way or report it in the handoff.",
                    ),
            )
        } else {
            allow(&input)
        }
    }

    /// Answers a waiting approval: the one named, or the oldest of the run.
    pub fn decide(
        &self,
        run: &str,
        approval: Option<&str>,
        decision: Decision,
        message: Option<String>,
    ) -> Result<String> {
        let mut live = self.live.lock().unwrap();
        let l = live
            .get_mut(run)
            .with_context(|| format!("run {run} is not live"))?;
        let id = match approval {
            Some(a) => a.to_owned(),
            None => l
                .meta
                .waiting
                .first()
                .map(|a| a.id.clone())
                .context("nothing is waiting in this run")?,
        };
        let tx = l
            .pending
            .remove(&id)
            .with_context(|| format!("no approval {id} waiting in run {run}"))?;
        tx.send((decision, message))
            .map_err(|_| anyhow::anyhow!("the agent stopped waiting"))?;
        Ok(id)
    }

    /// Everything waiting on a person, across live runs.
    pub fn approvals(&self) -> Vec<(RunMeta, Approval)> {
        self.live
            .lock()
            .unwrap()
            .values()
            .flat_map(|l| l.meta.waiting.iter().map(|a| (l.meta.clone(), a.clone())))
            .collect()
    }
}
