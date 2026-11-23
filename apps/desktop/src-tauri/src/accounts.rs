//! Accounts: which one runs a repository's agents, their usage, and the quota meters.

use std::collections::HashMap;

use pit_core::accounts::{self, AccountStatus, Kind, Routing, Usage};
use pit_core::default_homes;
use pit_core::runs::RunEvent;
use serde::Serialize;

use crate::daemon::running;
use crate::{CmdResult, blocking, open};

#[derive(Debug, Serialize)]
pub(crate) struct QuotaOut {
    /// The account the meters are about: this repository's.
    account: String,
    events: Vec<RunEvent>,
    /// The route's thresholds, so every meter lights at the same point.
    five_hour_at: f64,
    seven_day_at: f64,
}

#[tauri::command]
pub(crate) async fn quota(path: String) -> CmdResult<QuotaOut> {
    blocking(move || {
        let (home, pitlane_home) = default_homes();
        let root = open(&path)?.root;
        let routing = Routing::load(&pitlane_home)?;
        let account = routing.repo_account(&root, &home);
        let mut out = QuotaOut {
            account: account.clone(),
            events: Vec::new(),
            five_hour_at: routing.route.run.switch_at.five_hour,
            seven_day_at: routing.route.run.switch_at.seven_day,
        };
        if let Some(mut c) = running() {
            let mut all = c.quota()?;
            out.events = all.remove(&account).unwrap_or_default();
        }
        Ok(out)
    })
    .await
}

#[derive(Debug, Serialize)]
struct AccountOut {
    id: String,
    kind: Kind,
    status: AccountStatus,
    /// Last usage seen in a run, percent of each window.
    usage: Option<Usage>,
    /// The account this repository uses.
    here: bool,
    /// Every usage window its runs reported (5-hour, 7-day, per model…), with when it resets.
    windows: Vec<Window>,
    /// Extra usage past the plan, as the last report on this account put it.
    overage: Option<Overage>,
}

#[derive(Debug, Serialize)]
struct Window {
    window: String,
    /// Percent used.
    used: f64,
    /// Unix seconds; 0 when not reported.
    resets_at: u64,
}

#[derive(Debug, Serialize)]
struct Overage {
    /// `allowed`, `allowed_warning` or `rejected`, as Claude Code reports it.
    status: String,
    /// Why it is off (`org_level_disabled`…), when it is.
    reason: Option<String>,
    /// Whether runs spend extra usage right now.
    using: bool,
    /// When this was reported (unix seconds).
    at: u64,
}

/// The newest rate-limit report among this account's recent runs, read from the runs' own event
/// logs (what Claude Code printed; no login is read). Only the tail of each log is read.
fn overage_of(pitlane_home: &std::path::Path, account: &str) -> Option<Overage> {
    use std::io::{Read, Seek, SeekFrom};
    let dir = pitlane_home.join("runs");
    let mut runs: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let ev = e.path().join("events.jsonl");
            let t = std::fs::metadata(&ev).and_then(|m| m.modified()).ok()?;
            Some((t, e.path()))
        })
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.0));
    for (t, run) in runs.into_iter().take(40) {
        let Some(meta) = std::fs::read(run.join("meta.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { continue };
        if meta.get("account").and_then(|a| a.as_str()).unwrap_or("default") != account {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(run.join("events.jsonl")) else { continue };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(1 << 20)));
        let mut tail = String::new();
        if f.read_to_string(&mut tail).is_err() {
            let mut bytes = Vec::new();
            let _ = f.seek(SeekFrom::Start(len.saturating_sub(1 << 20)));
            let _ = f.read_to_end(&mut bytes);
            tail = String::from_utf8_lossy(&bytes).into_owned();
        }
        for line in tail.lines().rev().filter(|l| l.contains("\"rate_limit_event\"")) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let info = &v["rate_limit_info"];
            let Some(status) = info.get("overageStatus").and_then(|x| x.as_str()) else { continue };
            return Some(Overage {
                status: status.to_owned(),
                reason: info.get("overageDisabledReason").and_then(|x| x.as_str()).map(str::to_owned),
                using: info.get("isUsingOverage").and_then(|x| x.as_bool()).unwrap_or(false),
                at: t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            });
        }
    }
    None
}

/// Whether Claude Code has Pitlane's MCP server (`claude mcp get pitlane`, run in the repository so
/// every scope counts). The CLI answers about its own config; Pitlane reads only the exit status.
#[tauri::command]
pub(crate) async fn mcp_connected(path: String) -> CmdResult<bool> {
    blocking(move || {
        let root = open(&path)?.root;
        let ok = std::process::Command::new("claude")
            .args(["mcp", "get", "pitlane"])
            .current_dir(&root)
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        Ok(ok)
    })
    .await
}

#[derive(Debug, Serialize)]
pub(crate) struct AccountsOut {
    accounts: Vec<AccountOut>,
    route: Vec<String>,
    five_hour_at: f64,
    seven_day_at: f64,
    file: String,
}

/// Your accounts as the official CLI reports them (signed in or not), their last usage, and the
/// route. Reading `claude auth status` is the CLI reading its own login; Pitlane only reads the answer.
#[tauri::command]
pub(crate) async fn accounts_list(path: String) -> CmdResult<AccountsOut> {
    blocking(move || {
        let (home, pitlane_home) = default_homes();
        let root = open(&path)?.root;
        let routing = Routing::load(&pitlane_home)?;
        let mine = routing.repo_account(&root, &home);
        let quota: HashMap<String, Vec<RunEvent>> = running().and_then(|mut c| c.quota().ok()).unwrap_or_default();
        let accounts = routing
            .accounts
            .iter()
            .map(|a| AccountOut {
                id: a.id.clone(),
                kind: a.kind,
                status: accounts::status(a, &pitlane_home, "claude"),
                usage: quota.get(&a.id).map(|evs| {
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
                    u
                }),
                here: a.id == mine,
                windows: quota
                    .get(&a.id)
                    .map(|evs| {
                        let mut w: Vec<Window> = evs
                            .iter()
                            .filter_map(|e| match e {
                                RunEvent::Quota { window, used, resets_at } => Some(Window { window: window.clone(), used: used * 100.0, resets_at: *resets_at }),
                                _ => None,
                            })
                            .collect();
                        // The 5-hour window first, then the weekly ones.
                        w.sort_by_key(|x| (x.window != "five_hour", x.window.clone()));
                        w.dedup_by(|a, b| a.window == b.window);
                        w
                    })
                    .unwrap_or_default(),
                overage: if a.kind.is_api_key() { None } else { overage_of(&pitlane_home, &a.id) },
            })
            .collect();
        Ok(AccountsOut {
            accounts,
            route: routing.route.run.prefer.clone(),
            five_hour_at: routing.route.run.switch_at.five_hour,
            seven_day_at: routing.route.run.switch_at.seven_day,
            file: Routing::path(&pitlane_home).to_string_lossy().into_owned(),
        })
    })
    .await
}

/// Your choice of account for this repository's next runs. Never made by Pitlane on its own.
#[tauri::command]
pub(crate) async fn account_use(path: String, id: String) -> CmdResult<()> {
    blocking(move || {
        let root = open(&path)?.root;
        accounts::set_repo_account(&default_homes().1, &root, &id)
    })
    .await
}
