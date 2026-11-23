//! Runs, through the daemon: agent sessions, their timelines and live events, Performance, the
//! merge queue and reports.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use pit_core::cards::CardOut;
use pit_core::default_homes;
use pit_core::protocol::{Changed, ReviewAction, StartArgs};
use pit_core::runs::{self, RunEvent, RunMeta};
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::daemon::{daemon, running};
use crate::{CmdResult, blocking, open};

/// The sessions recorded on disk that `keep` keeps, newest first: what there is with no daemon up.
fn runs_on_disk(keep: impl Fn(&RunMeta) -> bool) -> Vec<RunMeta> {
    let mut all: Vec<RunMeta> = std::fs::read_dir(runs::runs_dir(&default_homes().1))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("meta.json")).ok())
        .filter_map(|t| serde_json::from_str::<RunMeta>(&t).ok())
        .filter(keep)
        .collect();
    all.sort_by_key(|m| std::cmp::Reverse(m.started_at));
    all
}

#[tauri::command]
pub(crate) async fn runs_list(path: String) -> CmdResult<Vec<RunMeta>> {
    blocking(move || {
        let root = open(&path)?.root;
        // No daemon yet means no runs yet: read the records on disk instead of starting one.
        if running().is_none() {
            return Ok(runs_on_disk(|m| m.repo == root && m.kind == runs::SessionKind::Run));
        }
        daemon()?.list(Some(&root))
    })
    .await
}

/// Every agent session of the repository, newest first: runs, Run all's analyses and judges.
#[tauri::command]
pub(crate) async fn sessions_list(path: String) -> CmdResult<Vec<RunMeta>> {
    blocking(move || {
        let root = open(&path)?.root;
        if running().is_none() {
            return Ok(runs_on_disk(|m| m.repo == root));
        }
        daemon()?.sessions(Some(&root))
    })
    .await
}

// The frontend sends these as named arguments; a struct would change what it sends.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn run_start(path: String, card: u32, allow_api_key: Option<bool>, into: Option<String>, direct: Option<bool>, instructions: Option<String>, autopilot: Option<bool>, fresh: Option<bool>) -> CmdResult<RunMeta> {
    blocking(move || {
        let root = open(&path)?.root;
        let args = StartArgs {
            repo: root,
            card,
            allow_api_key: allow_api_key.unwrap_or(false),
            no_brief: false,
            into,
            direct: direct.unwrap_or(false),
            instructions,
            autopilot: autopilot.unwrap_or(false),
            fresh: fresh.unwrap_or(false),
        };
        daemon()?.start(args)
    })
    .await
}

#[tauri::command]
pub(crate) async fn run_resume(run: String) -> CmdResult<RunMeta> {
    blocking(move || daemon()?.resume(&run)).await
}

#[tauri::command]
pub(crate) async fn run_say(run: String, text: String) -> CmdResult<()> {
    blocking(move || daemon()?.say(&run, text)).await
}

/// `/model` and `/effort` for a live session.
#[tauri::command]
pub(crate) async fn run_tune(run: String, model: Option<String>, effort: Option<String>) -> CmdResult<()> {
    blocking(move || daemon()?.tune(&run, model, effort)).await
}

/// Answers what a run is waiting on: once, run, always or deny (with a reason the agent reads).
#[tauri::command]
pub(crate) async fn run_decide(run: String, approval: Option<String>, decision: String, message: Option<String>) -> CmdResult<()> {
    blocking(move || daemon()?.decide(&run, approval, runs::Decision::parse(&decision)?, message).map(drop)).await
}

#[tauri::command]
pub(crate) async fn run_stop(run: String) -> CmdResult<()> {
    blocking(move || daemon()?.stop(&run)).await
}

/// Performance: the Mac, the agents' share of it, the mode and what the governor did.
#[tauri::command]
pub(crate) async fn perf() -> CmdResult<serde_json::Value> {
    blocking(|| daemon()?.perf()).await
}

/// Changes Performance's settings (mode, auto, away_minutes, light_agents, light_background).
#[tauri::command]
pub(crate) async fn perf_set(config: serde_json::Value) -> CmdResult<serde_json::Value> {
    blocking(move || daemon()?.perf_set(config)).await
}

/// Presence: whether runs ask you their calls (Here) or go with their recommendation (Away).
#[tauri::command]
pub(crate) async fn presence() -> CmdResult<serde_json::Value> {
    blocking(|| daemon()?.presence()).await
}

/// Changes Presence's settings (mode, auto, wait_minutes, per_run).
#[tauri::command]
pub(crate) async fn presence_set(config: serde_json::Value) -> CmdResult<serde_json::Value> {
    blocking(move || daemon()?.presence_set(config)).await
}

/// Pauses or resumes one session by hand.
#[tauri::command]
pub(crate) async fn perf_hold(run: String, hold: bool) -> CmdResult<Changed> {
    blocking(move || daemon()?.perf_hold(&run, hold)).await
}

/// Stop all: every agent session the daemon runs, and the analyses waiting for a slot.
#[tauri::command]
pub(crate) async fn runs_stop_all() -> CmdResult<usize> {
    blocking(move || {
        // No daemon, nothing running.
        let Some(mut c) = running() else { return Ok(0) };
        c.stop_all()
    })
    .await
}

/// Stops Run all's analyses (and the ones waiting for a slot); runs keep going.
#[tauri::command]
pub(crate) async fn runs_stop_analyses() -> CmdResult<usize> {
    blocking(move || {
        let Some(mut c) = running() else { return Ok(0) };
        c.stop_analyses()
    })
    .await
}

/// Run all's closing step: Claude does what the person asked for when it is all done. Waits.
#[tauri::command]
pub(crate) async fn run_all_finish(path: String, cards: Vec<u32>, instructions: String, into: Option<String>) -> CmdResult<pit_core::report::Finish> {
    blocking(move || {
        let root = open(&path)?.root;
        let v = daemon()?.finish(&root, cards, instructions, into)?;
        Ok(pit_core::report::Finish {
            run: v["run"].as_str().map(str::to_owned),
            ok: v["ok"].as_bool().unwrap_or(false),
            summary: v["summary"].as_str().map(str::to_owned),
            cost_usd: v["cost_usd"].as_f64(),
        })
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn report_create(
    path: String,
    cards: Vec<u32>,
    since: u64,
    into: Option<String>,
    direct: bool,
    instructions: String,
    finish: Option<pit_core::report::Finish>,
) -> CmdResult<pit_core::report::Report> {
    blocking(move || pit_core::report::create(&open(&path)?, &default_homes().1, &cards, since, into, direct, &instructions, finish)).await
}

#[tauri::command]
pub(crate) async fn reports_list(path: String) -> CmdResult<Vec<pit_core::report::Report>> {
    blocking(move || Ok(pit_core::report::list(&default_homes().1, &open(&path)?.root))).await
}

/// Claude's short reading of a report, written on it; returns the report.
#[tauri::command]
pub(crate) async fn report_digest(path: String, id: String) -> CmdResult<pit_core::report::Report> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.report_digest(&root, &id)
    })
    .await
}

#[tauri::command]
pub(crate) async fn report_delete(id: String) -> CmdResult<()> {
    blocking(move || pit_core::report::delete(&default_homes().1, &id)).await
}

/// A report's suggestion becomes a card in the backlog.
#[tauri::command]
pub(crate) async fn report_suggest_card(path: String, id: String, index: usize) -> CmdResult<CardOut> {
    blocking(move || Ok(pit_core::report::suggest_card(&open(&path)?, &default_homes().1, &id, index)?.into())).await
}

#[tauri::command]
pub(crate) async fn report_restore(id: String) -> CmdResult<()> {
    blocking(move || pit_core::report::restore(&default_homes().1, &id)).await
}

/// The Board's assistant: Claude turns what you typed or said into cards, in the background.
#[tauri::command]
pub(crate) async fn board_assist(path: String, text: String) -> CmdResult<()> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.assist(&root, text)
    })
    .await
}

/// What the checks found for a run (proofs and the judge), if they ran.
#[tauri::command]
pub(crate) async fn run_verification(run: String) -> CmdResult<Option<pit_core::verify::Verification>> {
    blocking(move || {
        let path = runs::run_dir(&default_homes().1, &run)?.join("verify.json");
        Ok(std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()))
    })
    .await
}

/// Your decision on a card in review: approve (ship into integration), changes (the run continues
/// with your comments) or discard (back to ready, lane kept).
#[tauri::command]
pub(crate) async fn card_review(path: String, card: u32, action: String, message: Option<String>) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.review(&root, card, ReviewAction::parse(&action)?, message)
    })
    .await
}

/// The merge queue of this repository: open jobs, then recent ones.
#[tauri::command]
pub(crate) async fn queue_list(path: String) -> CmdResult<Vec<pit_core::queue::ShipJob>> {
    blocking(move || {
        let root = open(&path)?.root;
        // No daemon: nothing is shipping; the saved queue still says what waits.
        if running().is_none() {
            return Ok(pit_core::queue::load(&default_homes().1)?.into_iter().filter(|j| j.repo == root).collect());
        }
        daemon()?.queue(Some(&root))
    })
    .await
}

/// The merge queue paused when Pitlane closed, with how many jobs wait in it.
#[tauri::command]
pub(crate) async fn queue_paused() -> CmdResult<usize> {
    blocking(move || {
        let home = default_homes().1;
        if !pit_core::queue::paused(&home) {
            return Ok(0);
        }
        Ok(pit_core::queue::load(&home)?.iter().filter(|j| j.state.is_open()).count())
    })
    .await
}

#[tauri::command]
pub(crate) async fn queue_resume() -> CmdResult<()> {
    blocking(move || daemon()?.queue_resume()).await
}

#[tauri::command]
pub(crate) async fn queue_add(path: String, branch: String, to: Option<String>) -> CmdResult<pit_core::queue::ShipJob> {
    blocking(move || {
        let root = open(&path)?.root;
        daemon()?.enqueue(&root, branch, to)
    })
    .await
}

#[tauri::command]
pub(crate) async fn queue_remove(id: String) -> CmdResult<()> {
    blocking(move || daemon()?.dequeue(&id)).await
}

#[tauri::command]
pub(crate) async fn run_continue(run: String, allow_api_key: bool) -> CmdResult<RunMeta> {
    blocking(move || daemon()?.continue_run(&run, allow_api_key)).await
}

/// A run's events so far, read from its timeline on disk (no daemon needed for history).
#[tauri::command]
pub(crate) async fn run_timeline(run: String) -> CmdResult<Vec<(usize, RunEvent)>> {
    blocking(move || {
        let path = runs::run_dir(&default_homes().1, &run)?.join("timeline.jsonl");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Ok(text
            .lines()
            .enumerate()
            .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|e| (i, e)))
            .collect())
    })
    .await
}

#[derive(Default)]
pub(crate) struct Following(Mutex<HashSet<String>>);

#[derive(Clone, Serialize)]
struct RunEventOut {
    run: String,
    i: usize,
    event: RunEvent,
}

#[derive(Clone, Serialize)]
struct RunEndOut {
    run: String,
    meta: Option<RunMeta>,
}

/// Streams a live run to the window as `run-event`s from `from`, then a `run-end`.
/// One follower per run, however many times the screen asks.
#[tauri::command]
pub(crate) fn run_follow(app: tauri::AppHandle, state: tauri::State<'_, Following>, run: String, from: usize) -> CmdResult<()> {
    if !state.0.lock().unwrap_or_else(PoisonError::into_inner).insert(run.clone()) {
        return Ok(());
    }
    std::thread::spawn(move || {
        let result = daemon().and_then(|mut c| {
            c.follow(&run, from, |i, event| app.emit("run-event", RunEventOut { run: run.clone(), i, event }).is_ok())
        });
        let meta = result.ok().flatten();
        let _ = app.emit("run-end", RunEndOut { run: run.clone(), meta });
        if let Some(f) = app.try_state::<Following>() {
            f.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&run);
        }
    });
    Ok(())
}
