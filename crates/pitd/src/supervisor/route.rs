//! The route: which account and model a run gets, and where a card goes when its run hits a usage
//! limit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::accounts::{self, Choice, Pick, Routing, Situation, Usage};
use pit_core::time::now_secs;
use pit_core::runs::{RunEvent, RunMeta, RunState};

use super::card::{block_card, checkpoint};
use super::{NEEDS_API_KEY, StartOpts, Supervisor, home_dir};

impl Supervisor {
    /// Usage per account, in percent, from the last events seen.
    fn usage(&self) -> HashMap<String, Usage> {
        self.quota
            .lock()
            .unwrap()
            .iter()
            .map(|(acct, evs)| {
                let mut u = Usage::default();
                for e in evs {
                    if let RunEvent::Quota { window, used, .. } = e {
                        match window.as_str() {
                            "five_hour" => u.five_hour = Some(used * 100.0),
                            "seven_day" => u.seven_day = Some(used * 100.0),
                            _ => {}
                        }
                    }
                }
                (acct.clone(), u)
            })
            .collect()
    }

    /// The route's choice for a new run of this repository. A resumed run keeps its account and
    /// model (the session lives there); a continuation skips the accounts that hit their limit.
    pub(super) fn choose(
        &self,
        r: &Repo,
        resume: Option<&RunMeta>,
        opts: &StartOpts,
    ) -> Result<(Choice, String, Vec<String>)> {
        let routing = Routing::load(&self.cfg.home)?;
        if let Some(prev) = resume {
            let a = routing
                .account(&prev.account)
                .with_context(|| format!("account {} is gone from routing.toml", prev.account))?;
            let choice = Choice {
                account: a.id.clone(),
                kind: a.kind,
                model: prev.model.clone(),
                config_dir: accounts::profile_dir(&a.id, &self.cfg.home),
                api_key_helper: a.api_key_helper.clone(),
            };
            return Ok((
                choice,
                format!("resumes {} in the same session", prev.id),
                prev.exhausted.clone(),
            ));
        }
        let exhausted = match &opts.handoff_from {
            Some(prev) => {
                let p = self.get(prev)?;
                let mut e = p.exhausted.clone();
                e.push(p.account.clone());
                e
            }
            None => Vec::new(),
        };
        let choices = routing.choices(&r.root, &home_dir(), &self.cfg.home)?;
        let running: HashMap<String, usize> =
            self.live()
                .values()
                .fold(HashMap::new(), |mut m, l| {
                    *m.entry(l.meta.account.clone()).or_default() += 1;
                    m
                });
        let max_parallel: HashMap<String, u32> = routing
            .accounts
            .iter()
            .filter_map(|a| a.max_parallel.map(|n| (a.id.clone(), n)))
            .collect();
        let usage = self.usage();
        let claude = self.cfg.claude_bin.clone();
        let home = self.cfg.home.clone();
        let routing_ref = &routing;
        // Signed out only when the official CLI says so; when it cannot tell, the run will say.
        let logged_in = move |c: &Choice| {
            routing_ref.account(&c.account).is_none_or(|a| {
                let s = accounts::status(a, &home, &claude);
                s.logged_in || s.problem.is_some()
            })
        };
        let pick = accounts::pick(
            &choices,
            &routing.route.run.switch_at,
            &Situation {
                usage: &usage,
                running: &running,
                max_parallel: &max_parallel,
                logged_in: &logged_in,
                allow_api_key: opts.allow_api_key,
                skip: &exhausted,
            },
        );
        match pick {
            Pick::Run { choice, reason } => Ok((choice, reason, exhausted)),
            Pick::NeedsApiKey { reason, .. } => bail!("{NEEDS_API_KEY}{reason}"),
            Pick::Nothing { reason } => bail!("{reason}"),
        }
    }

    /// The run hit a usage limit: commit what it did, close it, and continue the card with the
    /// route's next option. When that option is an API key, it waits for the person's yes.
    pub(super) async fn hand_off(
        self: Arc<Self>,
        id: String,
        repo: PathBuf,
        card: u32,
        code: Option<i32>,
    ) -> Result<()> {
        let meta = self.get(&id)?;
        let worktree = meta.worktree.clone();
        let label = meta.card;
        let also: Vec<PathBuf> = meta.related.iter().map(|l| l.worktree.clone()).collect();
        let checkpoint = tokio::task::spawn_blocking(move || {
            for w in &also {
                let _ = checkpoint(w, label);
            }
            checkpoint(&worktree, label)
        })
        .await?;
        if let Ok(Some(sha)) = &checkpoint {
            self.emit(
                &id,
                RunEvent::Note {
                    text: format!("Committed the work so far as {sha} before handing off."),
                },
            );
        }
        let why = format!("Hit the usage limit on {}.", meta.account);
        self.update(&id, |m| {
            m.state = RunState::Failed;
            m.ended_at = Some(now_secs());
            m.pid = None;
            m.waiting.clear();
            m.error = Some(why.clone());
        });
        self.emit(
            &id,
            RunEvent::Exited {
                code,
                state: RunState::Failed,
            },
        );
        self.live().remove(&id);

        match self.start(
            repo,
            card,
            None,
            StartOpts {
                allow_api_key: false,
                handoff_from: Some(id.clone()),
                round: meta.round,
                autopilot: meta.autopilot,
                extensions: meta.extensions,
                ..Default::default()
            },
        ) {
            Ok(next) => {
                let text = format!(
                    "{why} Continues as run {} with {}.",
                    next.id,
                    next.reason.clone().unwrap_or_default()
                );
                self.update_disk(&id, |m| m.error = Some(text.clone()));
                Ok(())
            }
            Err(e) => {
                let e = format!("{e:#}");
                let (next, text) = match e.strip_prefix(NEEDS_API_KEY) {
                    // The reason reads "<option>: <why the others were passed over>".
                    Some(reason) => {
                        let option = reason
                            .split_once(':')
                            .map_or(reason, |(o, _)| o)
                            .trim()
                            .to_owned();
                        (
                            Some(option.clone()),
                            format!(
                                "{why} The next option, {option}, is paid per token and waits for your yes."
                            ),
                        )
                    }
                    None => (
                        None,
                        format!("{why} Nothing else in the route can take it now: {e}"),
                    ),
                };
                // The card first: whoever sees the run's next option finds the card already waiting.
                let home = self.cfg.home.clone();
                let detail = text.clone();
                let reason = if next.is_some() { pit_core::cards::BlockReason::ApiKey } else { pit_core::cards::BlockReason::NoRoute };
                tokio::task::spawn_blocking(move || block_card(&meta, reason, &detail, &home))
                .await??;
                self.update_disk(&id, |m| {
                    m.error = Some(text.clone());
                    m.next = next.clone();
                });
                Ok(())
            }
        }
    }

    /// Continues a run that hit its limit; `allow_api_key` is the person's yes to the paid option.
    pub fn continue_run(self: &Arc<Self>, id: &str, allow_api_key: bool) -> Result<RunMeta> {
        let prev = self.get(id)?;
        if prev.state.is_live() {
            bail!("run {id} is still running");
        }
        let next = self.start(
            prev.repo.clone(),
            prev.card,
            None,
            StartOpts {
                allow_api_key,
                handoff_from: Some(prev.id.clone()),
                round: prev.round,
                ..Default::default()
            },
        )?;
        self.update_disk(id, |m| m.next = None);
        Ok(next)
    }
}
