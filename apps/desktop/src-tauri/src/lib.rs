//! Pitlane desktop: Tauri commands over `pit-core`. Every git call runs off the UI thread.
//! Until `pitd` exists the app calls the library directly, like the CLI does.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pit_core::actions::{self, CleanOutcome, CleanPlan, NewOptions, NewReport, ShipPlan, ShipReport, UndoReport};
use pit_core::lanes::{self, Yard, YardOptions};
use pit_core::cards::{self, CardOut, CardPatch, CardState, NewCard};
use pit_core::{Repo, default_homes, trash};
use serde::{Deserialize, Serialize};

type CmdResult<T> = Result<T, String>;

async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

fn open(path: &str) -> Result<Repo> {
    Repo::open_default(Path::new(path))
}

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub repos: Vec<RepoEntry>,
    pub last: Option<PathBuf>,
}

fn settings_path() -> PathBuf {
    default_homes().1.join("desktop.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) -> Result<()> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s)?)?;
    std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
}

#[tauri::command]
async fn settings() -> CmdResult<Settings> {
    blocking(|| Ok(load_settings())).await
}

/// Adds a repository (any folder inside it) and makes it the current one.
#[tauri::command]
async fn add_repo(path: String) -> CmdResult<RepoEntry> {
    blocking(move || {
        let repo = open(&path)?;
        let entry = RepoEntry { path: repo.root.clone(), name: repo.name.clone() };
        let mut s = load_settings();
        if !s.repos.iter().any(|r| r.path == entry.path) {
            s.repos.push(entry.clone());
            s.repos.sort_by(|a, b| a.name.cmp(&b.name));
        }
        s.last = Some(entry.path.clone());
        save_settings(&s)?;
        Ok(entry)
    })
    .await
}

#[tauri::command]
async fn select_repo(path: String) -> CmdResult<()> {
    blocking(move || {
        let mut s = load_settings();
        s.last = Some(PathBuf::from(path));
        save_settings(&s)
    })
    .await
}

// ---------------------------------------------------------------- lanes

#[tauri::command]
async fn yard(path: String, fetch: bool) -> CmdResult<Yard> {
    blocking(move || lanes::yard(&open(&path)?, YardOptions { fetch, github: fetch })).await
}

#[tauri::command]
async fn new_lane(path: String, branch: String) -> CmdResult<NewReport> {
    blocking(move || actions::new_lane(&open(&path)?, branch.trim(), NewOptions { fetch: true, setup: true }))
        .await
}

#[tauri::command]
async fn ship_plan(path: String, branch: String, to: Option<String>) -> CmdResult<ShipPlan> {
    blocking(move || actions::plan_ship(&open(&path)?, &branch, to.as_deref(), true)).await
}

/// Ships only if neither the lane nor the target moved since the user reviewed the plan.
#[tauri::command]
async fn ship(
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
async fn clean_plan(path: String) -> CmdResult<CleanPlan> {
    blocking(move || {
        let repo = open(&path)?;
        let y = lanes::yard(&repo, YardOptions { fetch: true, github: true })?;
        actions::plan_clean(&repo, &y)
    })
    .await
}

/// Re-checks every lane right before deleting; only the chosen ones that still qualify go.
#[tauri::command]
async fn clean(path: String, branches: Vec<String>) -> CmdResult<Vec<CleanOutcome>> {
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
async fn undo_clean(path: String, branch: String) -> CmdResult<UndoReport> {
    blocking(move || actions::undo_clean(&open(&path)?, &branch)).await
}

#[tauri::command]
async fn trash_list(path: String) -> CmdResult<Vec<trash::Entry>> {
    blocking(move || trash::list(&open(&path)?)).await
}

// ---------------------------------------------------------------- cards

#[derive(Debug, Serialize)]
pub struct CardList {
    pub cards: Vec<CardOut>,
    /// Card files that did not parse, with why.
    pub problems: Vec<String>,
}

#[tauri::command]
async fn cards_list(path: String) -> CmdResult<CardList> {
    blocking(move || {
        let (all, problems) = cards::list(&open(&path)?)?;
        Ok(CardList { cards: all.into_iter().map(CardOut::from).collect(), problems })
    })
    .await
}

#[tauri::command]
async fn card_add(path: String, title: String, body: String) -> CmdResult<CardOut> {
    blocking(move || Ok(cards::add(&open(&path)?, NewCard { title, body, ..Default::default() })?.into())).await
}

/// `lane`: a branch to link, an empty string to unlink, absent to leave as is.
#[tauri::command]
async fn card_update(
    path: String,
    id: u32,
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    lane: Option<String>,
) -> CmdResult<CardOut> {
    blocking(move || {
        let patch = CardPatch {
            title,
            body,
            state: state.map(|s| CardState::parse(&s)).transpose()?,
            lane: lane.map(|l| Some(l).filter(|l| !l.trim().is_empty())),
            tags: None,
        };
        Ok(cards::update(&open(&path)?, id, patch)?.into())
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct AnswerOut {
    pub decision: String,
    pub card: CardOut,
}

#[tauri::command]
async fn card_answer(path: String, id: u32, question: u32, answer: String) -> CmdResult<AnswerOut> {
    blocking(move || {
        let a = cards::answer(&open(&path)?, id, question, &answer)?;
        Ok(AnswerOut { decision: a.decision, card: a.card.into() })
    })
    .await
}

/// Apps opened from Finder get a bare PATH; git, gh and setup commands live in Homebrew.
fn extend_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = current.split(':').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !parts.iter().any(|p| p == extra) && Path::new(extra).exists() {
            parts.insert(0, extra.to_owned());
        }
    }
    // SAFETY: called first thing in `run`, before Tauri or any other thread starts.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

pub fn run() {
    extend_path();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            settings,
            add_repo,
            select_repo,
            yard,
            new_lane,
            ship_plan,
            ship,
            clean_plan,
            clean,
            undo_clean,
            trash_list,
            cards_list,
            card_add,
            card_update,
            card_answer
        ])
        .run(tauri::generate_context!())
        .expect("Pitlane failed to start");
}
