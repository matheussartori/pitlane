//! Lanes: the yard, new lanes, shipping and cleaning them, a lane's diff, and the plan of which
//! cards can run together.

use std::path::Path;

use anyhow::bail;
use pit_core::actions::{self, CleanOutcome, CleanPlan, NewOptions, NewReport, ShipPlan, ShipReport, UndoReport};
use pit_core::lanes::{self, Yard, YardOptions};
use pit_core::{default_homes, trash};

use crate::{CmdResult, blocking, files, open};

#[tauri::command]
pub(crate) async fn yard(path: String, fetch: bool) -> CmdResult<Yard> {
    blocking(move || lanes::yard(&open(&path)?, YardOptions { fetch, github: fetch })).await
}

#[tauri::command]
pub(crate) async fn new_lane(path: String, branch: String) -> CmdResult<NewReport> {
    blocking(move || actions::new_lane(&open(&path)?, branch.trim(), NewOptions { fetch: true, setup: true }))
        .await
}

#[tauri::command]
pub(crate) async fn ship_plan(path: String, branch: String, to: Option<String>) -> CmdResult<ShipPlan> {
    blocking(move || actions::plan_ship(&open(&path)?, &branch, to.as_deref(), true)).await
}

/// Ships only if neither the lane nor the target moved since the user reviewed the plan.
#[tauri::command]
pub(crate) async fn ship(
    path: String,
    branch: String,
    to: String,
    sha: String,
    target_sha: String,
) -> CmdResult<ShipReport> {
    blocking(move || {
        let repo = open(&path)?;
        let plan = actions::plan_ship(&repo, &branch, Some(&to), true)?;
        if plan.sha != sha || plan.target_sha != target_sha {
            bail!("{branch} or {to} moved since you reviewed it. Review the new plan.");
        }
        actions::execute_ship(&repo, &plan)
    })
    .await
}

#[tauri::command]
pub(crate) async fn clean_plan(path: String) -> CmdResult<CleanPlan> {
    blocking(move || {
        let repo = open(&path)?;
        let y = lanes::yard(&repo, YardOptions { fetch: true, github: true })?;
        actions::plan_clean(&repo, &y)
    })
    .await
}

/// Re-checks every lane right before deleting; only the chosen ones that still qualify go.
#[tauri::command]
pub(crate) async fn clean(path: String, branches: Vec<String>) -> CmdResult<Vec<CleanOutcome>> {
    blocking(move || {
        let repo = open(&path)?;
        let y = lanes::yard(&repo, YardOptions { fetch: true, github: true })?;
        let fresh = actions::plan_clean(&repo, &y)?;
        let plan = CleanPlan {
            items: fresh.items.into_iter().filter(|i| branches.contains(&i.branch)).collect(),
            skipped: Vec::new(),
        };
        actions::execute_clean(&repo, &plan)
    })
    .await
}

#[tauri::command]
pub(crate) async fn undo_clean(path: String, branch: String) -> CmdResult<UndoReport> {
    blocking(move || actions::undo_clean(&open(&path)?, &branch)).await
}

#[tauri::command]
pub(crate) async fn trash_list(path: String) -> CmdResult<Vec<trash::Entry>> {
    blocking(move || trash::list(&open(&path)?)).await
}

/// Where a lane's patch is cut for the review screen, in bytes.
const DIFF_LIMIT: usize = 600_000;

/// What a lane changes against production: per-file counts and the patch (cut past 600 KB).
#[tauri::command]
pub(crate) async fn lane_diff(path: String, branch: String, from: Option<String>) -> CmdResult<pit_core::verify::LaneDiff> {
    blocking(move || pit_core::verify::lane_diff(&open(&path)?, &branch, DIFF_LIMIT, from.as_deref())).await
}

/// Which open cards can run together, and which would touch the same files.
#[tauri::command]
pub(crate) async fn plan(path: String) -> CmdResult<pit_core::plan::Plan> {
    blocking(move || pit_core::plan::plan(&open(&path)?, &default_homes().1)).await
}

/// Reverts one hunk of a lane in its worktree and commits it ("review: drop a change in …").
#[tauri::command]
pub(crate) async fn lane_drop_hunk(path: String, branch: String, base: String, head: String, file: String, header: String) -> CmdResult<String> {
    blocking(move || pit_core::verify::drop_hunk(&open(&path)?, &branch, &base, &head, &file, &header)).await
}

// ---------------------------------------------------------------- files for the editor

#[tauri::command]
pub(crate) async fn files_list(root: String) -> CmdResult<Vec<String>> {
    blocking(move || files::list(Path::new(&root))).await
}
