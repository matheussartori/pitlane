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
