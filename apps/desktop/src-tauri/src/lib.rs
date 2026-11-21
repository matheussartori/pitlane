//! Pitlane desktop: Tauri commands over `pit-core`. Every git call runs off the UI thread.
//! Until `pitd` exists the app calls the library directly, like the CLI does.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pit_core::actions::{self, CleanOutcome, CleanPlan, NewOptions, NewReport, ShipPlan, ShipReport, UndoReport};
use pit_core::lanes::{self, Yard, YardOptions};
use pit_core::cards::{self, CardOut, CardPatch, CardState, NewCard};
use pit_core::accounts::{self, AccountStatus, Kind, Routing, Usage};
use pit_core::runs::{self, Client, RunEvent, RunMeta};
use std::collections::HashMap;
use pit_core::{Repo, default_homes, trash};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

mod files;
mod vscode;
mod term;
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
    #[serde(default)]
    pub prefs: Prefs,
}

/// The app's own preferences (a repository's flow lives in its .pitlane/flow.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "system", "light" or "dark".
    pub theme: String,
    /// System notifications, each when Pitlane is not in front.
    pub notify_approvals: bool,
    pub notify_runs: bool,
    pub notify_terminals: bool,
    /// Load VS Code in the background when a repository opens.
    pub code_prewarm: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { theme: "system".into(), notify_approvals: true, notify_runs: true, notify_terminals: true, code_prewarm: true }
    }
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

#[tauri::command]
async fn prefs_set(prefs: Prefs) -> CmdResult<()> {
    blocking(move || {
        let mut s = load_settings();
        s.prefs = prefs;
        save_settings(&s)
    })
    .await
}

/// Takes a repository off Pitlane's list. Nothing on disk changes.
#[tauri::command]
async fn remove_repo(path: String) -> CmdResult<Settings> {
    blocking(move || {
        let mut s = load_settings();
        s.repos.retain(|r| r.path != Path::new(&path));
        if s.last.as_deref() == Some(Path::new(&path)) {
            s.last = s.repos.first().map(|r| r.path.clone());
        }
        save_settings(&s)?;
        Ok(s)
    })
    .await
}

#[derive(Serialize)]
struct FlowOut {
    /// Whether .pitlane/flow.toml exists (else these are the defaults).
    exists: bool,
    path: PathBuf,
    flow: pit_core::flow::FlowFile,
}

#[tauri::command]
async fn flow_get(root: String) -> CmdResult<FlowOut> {
    blocking(move || {
        let r = Path::new(&root);
        Ok(FlowOut { exists: r.join(pit_core::flow::FLOW_PATH).exists(), path: r.join(pit_core::flow::FLOW_PATH), flow: pit_core::flow::FlowFile::load(r)? })
    })
    .await
}

/// One value of the repository's flow.toml, from Settings. Comments and layout stay.
#[tauri::command]
async fn flow_set(root: String, table: String, key: String, value: serde_json::Value) -> CmdResult<()> {
    blocking(move || {
        pit_core::flow::set_json(Path::new(&root), &table, &key, &value)
    })
    .await
}

/// Where Pitlane keeps its own files (~/.pitlane, or $PITLANE_HOME).
#[tauri::command]
fn pitlane_home() -> String {
    default_homes().1.to_string_lossy().into_owned()
}

/// Shows a file or folder in the Finder.
#[tauri::command]
fn reveal(path: String) -> CmdResult<()> {
    std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).status().map_err(|e| e.to_string())?;
    Ok(())
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

// ---------------------------------------------------------------- runs (through the daemon)

/// The `pit` executable that starts the daemon when nobody answers on its socket.
fn pit_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PITLANE_PIT_BIN") {
        return Some(PathBuf::from(p));
    }
    // The one bundled beside the app's own binary (Pitlane.app/Contents/MacOS/pit), then the PATH.
    let bundled = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("pit"))).filter(|p| p.is_file());
    bundled.or_else(|| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("pit")).find(|c| c.is_file())))
}

fn daemon() -> Result<Client> {
    Client::connect(&default_homes().1, pit_bin().as_deref())
}

fn call(req: serde_json::Value) -> Result<serde_json::Value> {
    daemon()?.call(req)
}

#[tauri::command]
async fn runs_list(path: String) -> CmdResult<Vec<RunMeta>> {
    blocking(move || {
        let root = open(&path)?.root;
        // No daemon yet means no runs yet: read the records on disk instead of starting one.
        let home = default_homes().1;
        if Client::connect(&home, None).is_err() {
            let mut all: Vec<RunMeta> = std::fs::read_dir(runs::runs_dir(&home))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path().join("meta.json")).ok())
                .filter_map(|t| serde_json::from_str::<RunMeta>(&t).ok())
                .filter(|m| m.repo == root)
                .collect();
            all.sort_by_key(|m| std::cmp::Reverse(m.started_at));
            return Ok(all);
        }
        let v = call(serde_json::json!({ "op": "list", "repo": root }))?;
        Ok(serde_json::from_value(v["runs"].clone())?)
    })
    .await
}

#[tauri::command]
async fn run_start(path: String, card: u32, allow_api_key: Option<bool>) -> CmdResult<RunMeta> {
    blocking(move || {
        let root = open(&path)?.root;
        let v = call(serde_json::json!({ "op": "start", "repo": root, "card": card, "allow_api_key": allow_api_key.unwrap_or(false) }))?;
        Ok(serde_json::from_value(v["run"].clone())?)
    })
    .await
}

#[tauri::command]
async fn run_resume(run: String) -> CmdResult<RunMeta> {
    blocking(move || {
        let v = call(serde_json::json!({ "op": "resume", "run": run }))?;
        Ok(serde_json::from_value(v["run"].clone())?)
    })
    .await
}

#[tauri::command]
async fn run_say(run: String, text: String) -> CmdResult<()> {
    blocking(move || call(serde_json::json!({ "op": "say", "run": run, "text": text })).map(drop)).await
}

/// Answers what a run is waiting on: once, run, always or deny (with a reason the agent reads).
#[tauri::command]
async fn run_decide(run: String, approval: Option<String>, decision: String, message: Option<String>) -> CmdResult<()> {
    blocking(move || {
        call(serde_json::json!({ "op": "decide", "run": run, "approval": approval, "decision": decision, "message": message })).map(drop)
    })
    .await
}

#[tauri::command]
async fn run_stop(run: String) -> CmdResult<()> {
    blocking(move || call(serde_json::json!({ "op": "stop", "run": run })).map(drop)).await
}

#[derive(Debug, Serialize)]
struct QuotaOut {
    /// The account the meters are about: this repository's.
    account: String,
    events: Vec<RunEvent>,
    /// The route's thresholds, so every meter lights at the same point.
    five_hour_at: f64,
    seven_day_at: f64,
}

#[tauri::command]
async fn quota(path: String) -> CmdResult<QuotaOut> {
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
        if Client::connect(&pitlane_home, None).is_ok() {
            let v = call(serde_json::json!({ "op": "quota" }))?;
            let mut all: HashMap<String, Vec<RunEvent>> = serde_json::from_value(v["quota"].clone())?;
            out.events = all.remove(&account).unwrap_or_default();
        }
        Ok(out)
    })
    .await
}

/// What the checks found for a run (proofs and the judge), if they ran.
#[tauri::command]
async fn run_verification(run: String) -> CmdResult<Option<pit_core::verify::Verification>> {
    blocking(move || {
        let path = runs::runs_dir(&default_homes().1).join(&run).join("verify.json");
        Ok(std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()))
    })
    .await
}

/// What a lane changes against production: per-file counts and the patch (cut past 600 KB).
#[tauri::command]
async fn lane_diff(path: String, branch: String, from: Option<String>) -> CmdResult<pit_core::verify::LaneDiff> {
    blocking(move || pit_core::verify::lane_diff(&open(&path)?, &branch, 600_000, from.as_deref())).await
}

/// Your decision on a card in review: approve (ship into integration), changes (the run continues
/// with your comments) or discard (back to ready, lane kept).
#[tauri::command]
async fn card_review(path: String, card: u32, action: String, message: Option<String>) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let root = open(&path)?.root;
        call(serde_json::json!({ "op": "review", "repo": root, "card": card, "action": action, "message": message }))
    })
    .await
}

/// The merge queue of this repository: open jobs, then recent ones.
#[tauri::command]
async fn queue_list(path: String) -> CmdResult<Vec<pit_core::queue::ShipJob>> {
    blocking(move || {
        let root = open(&path)?.root;
        // No daemon: nothing is shipping; the saved queue still says what waits.
        if Client::connect(&default_homes().1, None).is_err() {
            return Ok(pit_core::queue::load(&default_homes().1).into_iter().filter(|j| j.repo == root).collect());
        }
        let v = call(serde_json::json!({ "op": "queue", "repo": root }))?;
        Ok(serde_json::from_value(v["jobs"].clone())?)
    })
    .await
}

#[tauri::command]
async fn queue_add(path: String, branch: String, to: Option<String>) -> CmdResult<pit_core::queue::ShipJob> {
    blocking(move || {
        let root = open(&path)?.root;
        let v = call(serde_json::json!({ "op": "enqueue", "repo": root, "branch": branch, "to": to }))?;
        Ok(serde_json::from_value(v["job"].clone())?)
    })
    .await
}

#[tauri::command]
async fn queue_remove(id: String) -> CmdResult<()> {
    blocking(move || call(serde_json::json!({ "op": "dequeue", "id": id })).map(drop)).await
}

#[tauri::command]
async fn run_continue(run: String, allow_api_key: bool) -> CmdResult<RunMeta> {
    blocking(move || {
        let v = call(serde_json::json!({ "op": "continue", "run": run, "allow_api_key": allow_api_key }))?;
        Ok(serde_json::from_value(v["run"].clone())?)
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
struct AccountsOut {
    accounts: Vec<AccountOut>,
    route: Vec<String>,
    five_hour_at: f64,
    seven_day_at: f64,
    file: String,
}

/// Your accounts as the official CLI reports them (signed in or not), their last usage, and the
/// route. Reading `claude auth status` is the CLI reading its own login; Pitlane only reads the answer.
#[tauri::command]
async fn accounts_list(path: String) -> CmdResult<AccountsOut> {
    blocking(move || {
        let (home, pitlane_home) = default_homes();
        let root = open(&path)?.root;
        let routing = Routing::load(&pitlane_home)?;
        let mine = routing.repo_account(&root, &home);
        let quota: HashMap<String, Vec<RunEvent>> = Client::connect(&pitlane_home, None)
            .ok()
            .and_then(|mut c| c.call(serde_json::json!({ "op": "quota" })).ok())
            .and_then(|v| serde_json::from_value(v["quota"].clone()).ok())
            .unwrap_or_default();
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
async fn account_use(path: String, id: String) -> CmdResult<()> {
    blocking(move || {
        let root = open(&path)?.root;
        accounts::set_repo_account(&default_homes().1, &root, &id)
    })
    .await
}

/// A run's events so far, read from its timeline on disk (no daemon needed for history).
#[tauri::command]
async fn run_timeline(run: String) -> CmdResult<Vec<(usize, RunEvent)>> {
    blocking(move || {
        let path = runs::runs_dir(&default_homes().1).join(&run).join("timeline.jsonl");
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
struct Following(Mutex<HashSet<String>>);

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
fn run_follow(app: tauri::AppHandle, state: tauri::State<'_, Following>, run: String, from: usize) -> CmdResult<()> {
    if !state.0.lock().unwrap().insert(run.clone()) {
        return Ok(());
    }
    std::thread::spawn(move || {
        let result = daemon().and_then(|mut c| {
            c.follow(&run, from, |i, event| app.emit("run-event", RunEventOut { run: run.clone(), i, event }).is_ok())
        });
        let meta = result.ok().flatten();
        let _ = app.emit("run-end", RunEndOut { run: run.clone(), meta });
        if let Some(f) = app.try_state::<Following>() {
            f.0.lock().unwrap().remove(&run);
        }
    });
    Ok(())
}

/// Which open cards can run together, and which would touch the same files.
#[tauri::command]
async fn plan(path: String) -> CmdResult<pit_core::plan::Plan> {
    blocking(move || pit_core::plan::plan(&open(&path)?, &default_homes().1)).await
}

/// Reverts one hunk of a lane in its worktree and commits it ("review: drop a change in …").
#[tauri::command]
async fn lane_drop_hunk(path: String, branch: String, base: String, head: String, file: String, header: String) -> CmdResult<String> {
    blocking(move || pit_core::verify::drop_hunk(&open(&path)?, &branch, &base, &head, &file, &header)).await
}

// ---------------------------------------------------------------- files for the editor

#[tauri::command]
async fn files_list(root: String) -> CmdResult<Vec<String>> {
    blocking(move || files::list(Path::new(&root))).await
}

// ---------------------------------------------------------------- VS Code (code-server) in the Code tab

/// VS Code views, one per folder, most recent first. Up to three stay loaded, so coming back to
/// the tab, or to a recent lane, is instant. A view is never seen before VS Code has drawn itself:
/// it loads (login, workbench) out of sight, and moves onto the tab once it is ready.
#[derive(Default)]
struct CodeView {
    views: Mutex<Vec<View>>,
    /// The folders VS Code's picker offers: [{root, label, kind}].
    places: Mutex<serde_json::Value>,
    /// The folder the tab wants shown, and where (logical pixels).
    shown: Mutex<Option<(String, (f64, f64, f64, f64))>>,
    next: std::sync::atomic::AtomicU32,
}

struct View {
    folder: String,
    label: String,
    ready: Arc<std::sync::atomic::AtomicBool>,
}

const CODE_VIEWS: usize = 3;
/// Where a view waits: loaded and drawing, out of sight.
const PARKED: (f64, f64) = (-20_000.0, 0.0);

/// The bundled code-server, unpacked on first use (or, in a checkout without it, one on the PATH).
fn code_server(app: &tauri::AppHandle) -> Result<PathBuf> {
    vscode::ensure(app.path().resource_dir().ok().as_deref(), &default_homes().1)
}

/// Pitlane's colours for VS Code's dark and light themes.
#[tauri::command]
async fn vscode_theme(dark: serde_json::Map<String, serde_json::Value>, light: serde_json::Map<String, serde_json::Value>, mode: Option<String>) -> CmdResult<()> {
    blocking(move || vscode::write_theme(&default_homes().1, dark, light, mode.as_deref().unwrap_or("system"))).await
}

async fn code_server_running(app: &tauri::AppHandle) -> CmdResult<vscode::Running> {
    let a = app.clone();
    blocking(move || {
        let bin = code_server(&a)?;
        a.state::<vscode::Vscode>().start(&bin, a.path().resource_dir().ok().as_deref(), &default_homes().1)
    })
    .await
}

#[derive(Clone, Serialize)]
struct CodeReady {
    folder: String,
}

/// Puts the view of the folder the tab wants onto the tab, if it is ready; parks the others.
fn place_views(app: &tauri::AppHandle) {
    use tauri::{LogicalPosition, LogicalSize};
    let state = app.state::<CodeView>();
    let shown = state.shown.lock().unwrap().clone();
    let views = state.views.lock().unwrap();
    for v in views.iter() {
        let Some(wv) = app.get_webview(&v.label) else { continue };
        match &shown {
            Some((folder, (x, y, w, h))) if *folder == v.folder && v.ready.load(std::sync::atomic::Ordering::SeqCst) => {
                let _ = wv.set_size(LogicalSize::new(*w, *h));
                let _ = wv.set_position(LogicalPosition::new(*x, *y));
                let _ = wv.set_focus();
            }
            _ => {
                let _ = wv.set_position(LogicalPosition::new(PARKED.0, PARKED.1));
            }
        }
    }
}

/// Asks the page, every 120 ms, whether VS Code has drawn its workbench.
fn watch_ready(wv: tauri::Webview, ready: Arc<std::sync::atomic::AtomicBool>, folder: String, expect: Option<String>) {
    // Drawn, and with which editor background: on its very first run VS Code paints its own theme
    // before it reads the settings, so "drawn" alone would show that theme for a moment.
    const PROBE: &str = "(function(){var w=document.querySelector('.monaco-workbench');if(!w)return '';var drawn=!!document.querySelector('.part.sidebar .monaco-list-row, .part.editor .monaco-editor, .part.editor .editor-group-watermark')||document.querySelectorAll('.part.statusbar .statusbar-item').length>3;if(!drawn)return '';return 'bg:'+getComputedStyle(w).getPropertyValue('--vscode-editor-background').trim().toLowerCase();})()";
    let first = Arc::new(Mutex::new(None::<Instant>));
    std::thread::spawn(move || {
        let t = Instant::now();
        while !ready.load(std::sync::atomic::Ordering::SeqCst) && t.elapsed() < Duration::from_secs(60) {
            let (r, f, w, expect, first) = (ready.clone(), folder.clone(), wv.clone(), expect.clone(), first.clone());
            let _ = wv.eval_with_callback(PROBE, move |out| {
                let out = out.trim().trim_matches('"').to_owned();
                let Some(bg) = out.strip_prefix("bg:") else { return };
                let since = *first.lock().unwrap().get_or_insert_with(Instant::now);
                // Pitlane's ground, or (a theme of your own choosing) a few seconds after it drew.
                let settled = expect.as_deref().is_none_or(|e| e.eq_ignore_ascii_case(bg)) || since.elapsed() > Duration::from_secs(4);
                if settled && !r.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    eprintln!("pitlane: VS Code drawn on {f} in {} ms", t.elapsed().as_millis());
                    let app = w.app_handle().clone();
                    place_views(&app);
                    let _ = app.emit("vscode-ready", CodeReady { folder: f.clone() });
                }
            });
            std::thread::sleep(Duration::from_millis(120));
        }
    });
}

/// The view for `folder`, made out of sight if there is none. `bg` paints it while it loads.
fn code_view(app: &tauri::AppHandle, r: &vscode::Running, folder: &str, size: (f64, f64), bg: Option<&str>) -> CmdResult<bool> {
    use tauri::webview::{Color, PageLoadEvent, WebviewBuilder};
    use tauri::{LogicalPosition, LogicalSize, WebviewUrl};
    let e = |e: tauri::Error| e.to_string();
    let mut url = tauri::Url::parse(&format!("http://127.0.0.1:{}/", r.port)).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("folder", folder);
    let state = app.state::<CodeView>();
    let mut views = state.views.lock().unwrap();
    if let Some(i) = views.iter().position(|v| v.folder == folder) {
        let v = views.remove(i);
        let ready = v.ready.load(std::sync::atomic::Ordering::SeqCst);
        if let Some(wv) = app.get_webview(&v.label) {
            // A server started again since (a new port): load the folder from it.
            if !wv.url().is_ok_and(|u| u.port() == Some(r.port)) {
                v.ready.store(false, std::sync::atomic::Ordering::SeqCst);
                wv.navigate(url).map_err(e)?;
                views.insert(0, v);
                return Ok(false);
            }
            views.insert(0, v);
            return Ok(ready);
        }
    }
    let label = format!("vscode-{}", state.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let window = app.get_window("main").ok_or("no main window")?;
    let (rr, ff, expect) = (ready.clone(), folder.to_owned(), bg.map(|b| b.to_lowercase()));
    let places = state.places.lock().unwrap().clone();
    let nav_app = app.clone();
    let mut builder = WebviewBuilder::new(&label, WebviewUrl::External(url))
        .focused(false)
        .initialization_script(vscode::look_script(&places))
        // The folder picker inside VS Code asks for another folder with a pitlane:// address.
        .on_navigation(move |u| {
            if u.scheme() != "pitlane" {
                return true;
            }
            if let Some((_, root)) = u.query_pairs().find(|(k, _)| k == "root") {
                let _ = nav_app.emit("vscode-place", root.into_owned());
            }
            false
        })
        .on_page_load(move |wv, p| {
        if p.event() != PageLoadEvent::Finished {
            return;
        }
        if p.url().path().starts_with("/login") {
            // code-server's login page: type the password made for this run.
            if let Some(r) = wv.app_handle().state::<vscode::Vscode>().current() {
                let _ = wv.eval(vscode::login_script(&r.password));
            }
        } else {
            watch_ready(wv, rr.clone(), ff.clone(), expect.clone());
        }
    });
    if let Some(c) = bg.and_then(hex_colour) {
        builder = builder.background_color(Color(c.0, c.1, c.2, 255));
    }
    // Drawn out of sight (not hidden: a hidden view may not draw), moved in when ready.
    window.add_child(builder, LogicalPosition::new(PARKED.0, PARKED.1), LogicalSize::new(size.0.max(400.0), size.1.max(300.0))).map_err(e)?;
    views.insert(0, View { folder: folder.to_owned(), label, ready });
    while views.len() > CODE_VIEWS {
        if let Some(old) = views.pop() {
            if let Some(v) = app.get_webview(&old.label) {
                let _ = v.close();
            }
        }
    }
    Ok(false)
}

fn hex_colour(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

/// The folders VS Code's picker offers (the repository and the lanes' worktrees), now and in
/// every view already loaded.
#[tauri::command]
fn vscode_places(app: tauri::AppHandle, places: serde_json::Value) {
    let state = app.state::<CodeView>();
    *state.places.lock().unwrap() = places.clone();
    for v in state.views.lock().unwrap().iter() {
        if let Some(wv) = app.get_webview(&v.label) {
            let _ = wv.eval(vscode::places_script(&places));
        }
    }
}

/// Starts VS Code on `folder` out of sight, so the Code tab opens at once.
#[tauri::command]
async fn vscode_prewarm(app: tauri::AppHandle, folder: String, w: f64, h: f64, bg: Option<String>) -> CmdResult<()> {
    let r = code_server_running(&app).await?;
    code_view(&app, &r, &folder, (w, h), bg.as_deref())?;
    Ok(())
}

/// Shows VS Code over the Code tab's area (logical pixels of the window), on `folder`. Says whether
/// it is already drawn; if not, it moves in by itself when it is (and `vscode-ready` says so).
#[tauri::command]
async fn vscode_show(app: tauri::AppHandle, folder: String, x: f64, y: f64, w: f64, h: f64, bg: Option<String>) -> CmdResult<bool> {
    *app.state::<CodeView>().shown.lock().unwrap() = Some((folder.clone(), (x, y, w, h)));
    let r = code_server_running(&app).await?;
    let ready = code_view(&app, &r, &folder, (w, h), bg.as_deref())?;
    place_views(&app);
    Ok(ready)
}

/// Takes VS Code off the tab (another tab, or an app menu over it) and gives the keyboard back.
#[tauri::command]
async fn vscode_hide(app: tauri::AppHandle) -> CmdResult<()> {
    *app.state::<CodeView>().shown.lock().unwrap() = None;
    place_views(&app);
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_focus();
    }
    Ok(())
}

/// Opens a file at a line in the VS Code that is running.
#[tauri::command]
async fn vscode_open(app: tauri::AppHandle, path: String, line: Option<u32>, col: Option<u32>) -> CmdResult<()> {
    blocking(move || {
        let bin = code_server(&app)?;
        app.state::<vscode::Vscode>().open_file(&bin, &default_homes().1, Path::new(&path), line, col)
    })
    .await
}

// ---------------------------------------------------------------- free terminals

#[tauri::command]
fn term_open(app: tauri::AppHandle, terms: tauri::State<'_, term::Terms>, cwd: String, label: String, cols: u16, rows: u16) -> CmdResult<term::TermInfo> {
    terms.open(term::app_sink(&app), &cwd, &label, cols, rows).map_err(|e| format!("{e:#}"))
}

/// Opens a web link in the person's browser (links clicked in a terminal). Only http and https.
#[tauri::command]
fn open_external(url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url} is not a web link"));
    }
    std::process::Command::new("/usr/bin/open").arg(&url).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// The app menu. Like the default one, except ⌘W: it belongs to the screen (a terminal pane, a
/// file), so Close Window moves to ⇧⌘W.
fn menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem as P, Submenu};
    let name = app.package_info().name.clone();
    let close = MenuItem::with_id(app, "close-window", "Close Window", true, Some("CmdOrCtrl+Shift+W"))?;
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                &name,
                true,
                &[
                    &P::about(app, None, Some(AboutMetadata::default()))?,
                    &P::separator(app)?,
                    &P::services(app, None)?,
                    &P::separator(app)?,
                    &P::hide(app, None)?,
                    &P::hide_others(app, None)?,
                    &P::show_all(app, None)?,
                    &P::separator(app)?,
                    &P::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(app, "File", true, &[&close])?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &P::undo(app, None)?,
                    &P::redo(app, None)?,
                    &P::separator(app)?,
                    &P::cut(app, None)?,
                    &P::copy(app, None)?,
                    &P::paste(app, None)?,
                    &P::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(app, "View", true, &[&P::fullscreen(app, None)?])?,
            &Submenu::with_items(app, "Window", true, &[&P::minimize(app, None)?, &P::maximize(app, None)?])?,
        ],
    )
}

#[tauri::command]
fn term_write(terms: tauri::State<'_, term::Terms>, id: u32, data: String) -> CmdResult<()> {
    terms.write(id, &data).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn term_resize(terms: tauri::State<'_, term::Terms>, id: u32, cols: u16, rows: u16) -> CmdResult<()> {
    terms.resize(id, cols, rows).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn term_close(terms: tauri::State<'_, term::Terms>, id: u32) -> CmdResult<()> {
    terms.close(id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn term_list(terms: tauri::State<'_, term::Terms>) -> Vec<term::TermInfo> {
    terms.list()
}

#[tauri::command]
fn term_buffer(terms: tauri::State<'_, term::Terms>, id: u32) -> CmdResult<String> {
    terms.buffer(id).map_err(|e| format!("{e:#}"))
}

/// Apps opened from Finder get a bare PATH; git, gh and setup commands live in Homebrew.
/// The PATH your login shell sets up (nvm, asdf, Homebrew…): an app opened from the Finder only
/// gets the system's, and agents need your tools. At most a few seconds; None when it fails.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", "printf '__PITLANE_PATH__%s__END__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let t = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t.elapsed() < std::time::Duration::from_secs(4) => std::thread::sleep(std::time::Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let start = out.find("__PITLANE_PATH__")? + "__PITLANE_PATH__".len();
    let end = out[start..].find("__END__")? + start;
    Some(out[start..end].to_owned()).filter(|p| !p.is_empty())
}

fn extend_path() {
    let current = login_shell_path().or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
    let mut parts: Vec<String> = current.split(':').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    let cargo = default_homes().0.join(".cargo/bin").to_string_lossy().into_owned();
    let local = default_homes().0.join(".local/bin").to_string_lossy().into_owned();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", local.as_str(), cargo.as_str()] {
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
        .plugin(tauri_plugin_notification::init())
        .manage(Following::default())
        .manage(term::Terms::default())
        .manage(vscode::Vscode::default())
        .manage(CodeView::default())
        .menu(menu)
        .on_menu_event(|app, e| {
            if e.id().as_ref() == "close-window" {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            settings,
            prefs_set,
            remove_repo,
            flow_get,
            flow_set,
            reveal,
            pitlane_home,
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
            card_answer,
            runs_list,
            run_start,
            run_resume,
            run_say,
            run_stop,
            run_decide,
            run_continue,
            run_verification,
            lane_diff,
            card_review,
            queue_list,
            lane_drop_hunk,
            plan,
            files_list,
            term_open,
            vscode_theme,
            vscode_prewarm,
            vscode_places,
            vscode_show,
            vscode_hide,
            vscode_open,
            open_external,
            term_write,
            term_resize,
            term_close,
            term_list,
            term_buffer,
            queue_add,
            queue_remove,
            accounts_list,
            account_use,
            run_timeline,
            run_follow,
            quota
        ])
        .build(tauri::generate_context!())
        .expect("Pitlane failed to start")
        .run(|app, event| {
            // No shell outlives the app.
            if let tauri::RunEvent::Exit = event {
                app.state::<term::Terms>().close_all();
                app.state::<vscode::Vscode>().stop();
            }
        });
}
