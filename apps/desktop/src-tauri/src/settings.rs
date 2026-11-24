//! Settings: the app's own preferences and list of repositories (desktop.json), a repository's
//! flow.toml, and how many agents fit on this Mac now.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use pit_core::default_homes;
use serde::{Deserialize, Serialize};

use crate::daemon::running;
use crate::{CmdResult, blocking, open};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub repos: Vec<RepoEntry>,
    pub last: Option<PathBuf>,
    #[serde(default)]
    pub prefs: Prefs,
}

pub use pit_core::prefs::Prefs;

#[derive(Debug, Serialize)]
pub struct Capacity {
    /// How many agents Run all may have working now.
    pub agents: u32,
    pub max: u32,
    /// The 1-minute load average and the cores it is measured against.
    pub load: f64,
    pub cores: u32,
    /// Free memory, as a share of the total (0..1), when macOS says.
    pub free_memory: Option<f64>,
}

/// Load per core past which Run all works with two agents fewer.
const LOAD_HEAVY: f64 = 2.0;
/// Load per core past which Run all works with one agent fewer.
const LOAD_BUSY: f64 = 1.25;
/// Share of free memory under which Run all works with a single agent.
const MEMORY_SCARCE: f64 = 0.05;
/// Share of free memory under which Run all works with one agent fewer.
const MEMORY_SHORT: f64 = 0.12;

/// How many agents fit now: the setting, lowered while the load runs past the cores or memory
/// runs short (each agent brings its own tests and builds), raised again when they ease.
#[tauri::command]
pub(crate) async fn capacity() -> CmdResult<Capacity> {
    blocking(|| {
        let prefs = load_settings().prefs;
        let max = prefs.max_agents.clamp(1, 16);
        let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(8);
        let mut avg = [0f64; 3];
        // SAFETY: getloadavg writes at most `nelem` doubles into the buffer we pass.
        let load = if unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) } >= 1 { avg[0] } else { 0.0 };
        let free_memory = free_memory_share();
        let mut agents = max;
        if prefs.adaptive_agents {
            let ratio = load / cores as f64;
            let by_load = if ratio > LOAD_HEAVY { max.saturating_sub(2) } else if ratio > LOAD_BUSY { max.saturating_sub(1) } else { max };
            let by_memory = match free_memory {
                Some(f) if f < MEMORY_SCARCE => 1,
                Some(f) if f < MEMORY_SHORT => max.saturating_sub(1),
                _ => max,
            };
            agents = by_load.min(by_memory).max(1);
        }
        // The daemon's Performance governor decides when it runs: Full or Light, and the memory.
        // Only a daemon already running: reading the capacity never starts one.
        if let Some(v) = running().and_then(|mut c| c.perf().ok())
            && v.get("governed").and_then(serde_json::Value::as_bool) == Some(true)
                && let Some(n) = v.get("allowed").and_then(serde_json::Value::as_u64) {
                    agents = (n as u32).max(1);
                }
        Ok(Capacity { agents, max, load, cores, free_memory })
    })
    .await
}

/// Free plus reclaimable memory over the total, from `vm_stat` and `hw.memsize`.
fn free_memory_share() -> Option<f64> {
    let total: f64 = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())?;
    let out = std::process::Command::new("/usr/bin/vm_stat").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let page: f64 = text.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |key: &str| -> f64 {
        text.lines().find(|l| l.starts_with(key)).and_then(|l| l.rsplit(':').next()).and_then(|v| v.trim().trim_end_matches('.').parse().ok()).unwrap_or(0.0)
    };
    let free = (pages("Pages free") + pages("Pages inactive") + pages("Pages speculative") + pages("Pages purgeable")) * page;
    (total > 0.0).then(|| (free / total).clamp(0.0, 1.0))
}

fn settings_path() -> PathBuf {
    default_homes().1.join(pit_core::prefs::FILE)
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Changes desktop.json: one change at a time (read, change, write under one lock, so two
/// commands never drop each other's change), written whole or not at all. A file that no longer
/// reads is kept beside it as `desktop.json.bad`, never silently overwritten with the defaults.
fn edit_settings<R>(change: impl FnOnce(&mut Settings) -> R) -> Result<(Settings, R)> {
    static EDITING: Mutex<()> = Mutex::new(());
    let _one = EDITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = settings_path();
    let mut s = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
            eprintln!("pitlane: {} does not read ({e}); kept as desktop.json.bad", path.display());
            let _ = std::fs::copy(&path, path.with_extension("json.bad"));
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    let out = change(&mut s);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&s)?)?;
    std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))?;
    Ok((s, out))
}

#[tauri::command]
pub(crate) async fn settings() -> CmdResult<Settings> {
    blocking(|| Ok(load_settings())).await
}

#[tauri::command]
pub(crate) async fn prefs_set(prefs: Prefs) -> CmdResult<()> {
    blocking(move || {
        edit_settings(|s| s.prefs = prefs).map(drop)
    })
    .await
}

/// Takes a repository off Pitlane's list. Nothing on disk changes.
#[tauri::command]
pub(crate) async fn remove_repo(path: String) -> CmdResult<Settings> {
    blocking(move || {
        let (s, ()) = edit_settings(|s| {
            s.repos.retain(|r| r.path != Path::new(&path));
            if s.last.as_deref() == Some(Path::new(&path)) {
                s.last = s.repos.first().map(|r| r.path.clone());
            }
        })?;
        Ok(s)
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct FlowOut {
    /// Whether .pitlane/flow.toml exists (else these are the defaults).
    exists: bool,
    path: PathBuf,
    flow: pit_core::flow::FlowFile,
}

#[tauri::command]
pub(crate) async fn flow_get(root: String) -> CmdResult<FlowOut> {
    blocking(move || {
        let r = Path::new(&root);
        Ok(FlowOut { exists: r.join(pit_core::flow::FLOW_PATH).exists(), path: r.join(pit_core::flow::FLOW_PATH), flow: pit_core::flow::FlowFile::load(r)? })
    })
    .await
}

/// One value of the repository's flow.toml, from Settings. Comments and layout stay.
#[tauri::command]
pub(crate) async fn flow_set(root: String, table: String, key: String, value: serde_json::Value) -> CmdResult<()> {
    blocking(move || {
        pit_core::flow::set_json(Path::new(&root), &table, &key, &value)
    })
    .await
}

/// Where Pitlane keeps its own files (~/.pitlane, or $PITLANE_HOME).
#[tauri::command]
pub(crate) fn pitlane_home() -> String {
    default_homes().1.to_string_lossy().into_owned()
}

/// Adds a repository (any folder inside it) and makes it the current one.
#[tauri::command]
pub(crate) async fn add_repo(path: String) -> CmdResult<RepoEntry> {
    blocking(move || {
        let repo = open(&path)?;
        let entry = RepoEntry { path: repo.root.clone(), name: repo.name.clone() };
        edit_settings(|s| {
            if !s.repos.iter().any(|r| r.path == entry.path) {
                s.repos.push(entry.clone());
                s.repos.sort_by(|a, b| a.name.cmp(&b.name));
            }
            s.last = Some(entry.path.clone());
        })?;
        Ok(entry)
    })
    .await
}

#[tauri::command]
pub(crate) async fn select_repo(path: String) -> CmdResult<()> {
    blocking(move || {
        edit_settings(|s| s.last = Some(PathBuf::from(path))).map(drop)
    })
    .await
}
