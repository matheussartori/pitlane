//! Accounts: which exist, signing in to them, and which one a repository uses.

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::runs::{Client, RunEvent};

use super::pitlane_home;
use crate::term::{ACCENT, DONE, MUTED, claude_bin, paint};

pub fn accounts(repo: &Repo) -> Result<ExitCode> {
    let (home, pitlane_home) = pit_core::default_homes();
    let routing = pit_core::accounts::Routing::load(&pitlane_home)?;
    let mine = routing.repo_account(&repo.root, &home);
    let quota = daemon_quota();
    let claude = claude_bin();
    for a in &routing.accounts {
        let st = pit_core::accounts::status(a, &pitlane_home, &claude);
        let signed = if st.logged_in {
            paint(DONE, "signed in")
        } else if st.problem.is_some() {
            paint(MUTED, "unknown")
        } else {
            paint(ACCENT, &format!("not signed in: pit account login {}", a.id))
        };
        let usage = quota
            .get(&a.id)
            .map(|u| {
                let f = |v: Option<f64>| v.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "?".into());
                format!("5h {} 7d {}", f(u.five_hour), f(u.seven_day))
            })
            .unwrap_or_default();
        let here = if a.id == mine { paint(ACCENT, "this repo") } else { String::new() };
        println!("{:<14} {:<13} {signed}  {}  {here}", a.id, format!("{:?}", a.kind).to_lowercase(), paint(MUTED, &usage));
    }
    println!("{}", paint(MUTED, &format!("route: {}", routing.route.run.prefer.join(" → "))));
    Ok(ExitCode::SUCCESS)
}

pub fn add(id: String, kind: String, api_key_helper: Option<String>) -> Result<ExitCode> {
    let pitlane_home = pitlane_home();
    let kind = match kind.as_str() {
        "subscription" => pit_core::accounts::Kind::Subscription,
        "team" => pit_core::accounts::Kind::Team,
        "api-key" => pit_core::accounts::Kind::ApiKey,
        k => bail!("unknown kind {k}; subscription, team or api-key"),
    };
    let dir = pit_core::accounts::add_account(&pitlane_home, &id, kind, api_key_helper.as_deref())?;
    println!("{} account {id} added to {}", paint(DONE, "✓"), pit_core::accounts::Routing::path(&pitlane_home).display());
    if let Some(d) = dir
        && kind != pit_core::accounts::Kind::ApiKey
    {
        println!("  its Claude Code profile is {}", d.display());
        println!("  sign in with: pit account login {id}");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn login(id: String) -> Result<ExitCode> {
    let pitlane_home = pitlane_home();
    let routing = pit_core::accounts::Routing::load(&pitlane_home)?;
    let a = routing.account(&id).with_context(|| format!("no account {id}; add it with pit account add {id}"))?;
    if a.kind.is_api_key() {
        bail!("{id} is an api-key account: it signs in through its api_key_helper");
    }
    let mut cmd = std::process::Command::new(claude_bin());
    cmd.args(["auth", "login"]).env_remove("CLAUDECODE");
    if let Some(dir) = pit_core::accounts::profile_dir(&id, &pitlane_home) {
        std::fs::create_dir_all(&dir)?;
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    // The official CLI signs you in; Pitlane never sees the result.
    let st = cmd.status().context("could not run claude auth login")?;
    Ok(if st.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

pub fn use_for_repo(repo: &Repo, id: String) -> Result<ExitCode> {
    let pitlane_home = pitlane_home();
    pit_core::accounts::set_repo_account(&pitlane_home, &repo.root, &id)?;
    println!("{} {} uses {id} from its next run", paint(DONE, "✓"), repo.name);
    Ok(ExitCode::SUCCESS)
}

/// Usage per account from the daemon, if it is running.
fn daemon_quota() -> std::collections::HashMap<String, pit_core::accounts::Usage> {
    let Ok(mut c) = Client::connect(&pitlane_home(), None) else { return Default::default() };
    let Ok(v) = c.call(serde_json::json!({ "op": "quota" })) else { return Default::default() };
    let all: std::collections::HashMap<String, Vec<RunEvent>> = serde_json::from_value(v["quota"].clone()).unwrap_or_default();
    all.into_iter()
        .map(|(acct, evs)| {
            let mut u = pit_core::accounts::Usage::default();
            for e in evs {
                if let RunEvent::Quota { window, used, .. } = e {
                    match window.as_str() {
                        "five_hour" => u.five_hour = Some(used * 100.0),
                        "seven_day" => u.seven_day = Some(used * 100.0),
                        _ => {}
                    }
                }
            }
            (acct, u)
        })
        .collect()
}
