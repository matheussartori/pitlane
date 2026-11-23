//! Free terminals: the commands over `term`, and "Continue in Claude Code".

use anyhow::{Context, bail};
use pit_core::accounts::{self, Routing};
use pit_core::default_homes;
use pit_core::runs::{self, RunMeta};
use tauri::Manager;

use crate::{CmdResult, blocking, term};

#[tauri::command]
pub(crate) fn term_open(app: tauri::AppHandle, terms: tauri::State<'_, term::Terms>, cwd: String, label: String, cols: u16, rows: u16) -> CmdResult<term::TermInfo> {
    terms.open(term::app_sink(&app), &cwd, &label, cols, rows).map_err(|e| format!("{e:#}"))
}

/// "Continue in Claude Code": the official CLI resumes a finished agent session in a terminal,
/// in the folder the agent worked in and on the account it ran on (Claude Code keeps sessions per
/// folder and per configuration directory). Pitlane only points it at them; the login stays Claude Code's.
#[tauri::command]
pub(crate) async fn term_resume(app: tauri::AppHandle, run: String, cols: u16, rows: u16) -> CmdResult<term::TermInfo> {
    blocking(move || {
        let home = default_homes().1;
        let text = std::fs::read_to_string(runs::run_dir(&home, &run)?.join("meta.json")).with_context(|| format!("no run {run}"))?;
        let meta: RunMeta = serde_json::from_str(&text)?;
        if meta.state.is_live() {
            bail!("the agent is still working; stop it first, so two Claude Codes never write the same session");
        }
        let sid = meta.session_id.trim();
        if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            bail!("this session has no Claude Code session to resume");
        }
        let mut args = vec!["claude".to_owned(), "--resume".to_owned(), sid.to_owned()];
        if let Some(m) = meta.model.as_deref().filter(|m| !m.is_empty()) {
            args.extend(["--model".into(), m.to_owned()]);
        }
        if let Some(e) = meta.effort.as_deref().filter(|e| !e.is_empty()) {
            args.extend(["--effort".into(), e.to_owned()]);
        }
        let mut env = Vec::new();
        if let Some(dir) = accounts::profile_dir(&meta.account, &home) {
            env.push(("CLAUDE_CONFIG_DIR".to_owned(), dir.to_string_lossy().into_owned()));
        }
        if let Some(helper) = Routing::load(&home).ok().and_then(|r| r.account(&meta.account).and_then(|a| a.api_key_helper.clone())) {
            args.extend(["--settings".into(), serde_json::json!({ "apiKeyHelper": helper }).to_string()]);
        }
        let cwd = [&meta.worktree, &meta.repo].into_iter().find(|p| !p.as_os_str().is_empty() && p.is_dir()).context("the folder the agent worked in is gone")?;
        let label = format!("#{} {}", meta.card, meta.card_title);
        let terms = app.state::<term::Terms>();
        terms.launch(term::app_sink(&app), &cwd.to_string_lossy(), &label, cols, rows, Some(term::Program { args, env, session: Some(sid.to_owned()) }))
    })
    .await
}

#[tauri::command]
pub(crate) fn term_write(terms: tauri::State<'_, term::Terms>, id: u32, data: String) -> CmdResult<()> {
    terms.write(id, &data).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) fn term_resize(terms: tauri::State<'_, term::Terms>, id: u32, cols: u16, rows: u16) -> CmdResult<()> {
    terms.resize(id, cols, rows).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) fn term_close(terms: tauri::State<'_, term::Terms>, id: u32) -> CmdResult<()> {
    terms.close(id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) fn term_list(terms: tauri::State<'_, term::Terms>) -> Vec<term::TermInfo> {
    terms.list()
}

#[tauri::command]
pub(crate) fn term_buffer(terms: tauri::State<'_, term::Terms>, id: u32) -> CmdResult<term::Kept> {
    terms.buffer(id).map_err(|e| format!("{e:#}"))
}
