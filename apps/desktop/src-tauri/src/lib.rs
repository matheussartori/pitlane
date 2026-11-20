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
use std::sync::Mutex;
use tauri::{Emitter, Manager};

mod files;
mod lsp;
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

// ---------------------------------------------------------------- runs (through the daemon)

/// The `pit` executable that starts the daemon when nobody answers on its socket.
fn pit_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PITLANE_PIT_BIN") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join("pit")).find(|c| c.is_file()))
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

#[tauri::command]
async fn file_read(root: String, path: String) -> CmdResult<files::FileText> {
    blocking(move || files::read(Path::new(&root), &path)).await
}

#[tauri::command]
async fn file_write(root: String, path: String, text: String) -> CmdResult<()> {
    blocking(move || files::write(Path::new(&root), &path, &text)).await
}

#[tauri::command]
async fn files_ignored(root: String) -> CmdResult<Vec<String>> {
    blocking(move || files::ignored(Path::new(&root))).await
}

#[tauri::command]
async fn dir_list(root: String, path: String) -> CmdResult<Vec<String>> {
    blocking(move || files::dir(Path::new(&root), &path)).await
}

#[tauri::command]
async fn files_search(root: String, query: String) -> CmdResult<Vec<files::Hit>> {
    blocking(move || files::search(Path::new(&root), &query, 400)).await
}

/// How a file differs from where its lane left production (the merge-base with it), or from
/// HEAD outside a lane.
#[tauri::command]
async fn file_changes(root: String, path: String) -> CmdResult<files::Changes> {
    blocking(move || {
        let dir = Path::new(&root);
        let base = Repo::open_default(dir)
            .ok()
            .and_then(|repo| {
                let prod = lanes::Target::resolve(&repo, &repo.flow.flow.production).ok().flatten()?;
                repo.git.run(&["merge-base", "HEAD", &prod.sha]).ok()
            })
            .unwrap_or_else(|| "HEAD".into());
        files::changes(dir, &path, &base)
    })
    .await
}

// ---------------------------------------------------------------- language servers

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LspStart {
    Started(lsp::Started),
    Missing(lsp::Missing),
    /// A language with no server Pitlane knows (Markdown, CSS…): Monaco's own support only.
    None,
}

#[tauri::command]
async fn lsp_start(app: tauri::AppHandle, root: String, lang: String) -> CmdResult<LspStart> {
    let Some(family) = lsp::family(&lang) else { return Ok(LspStart::None) };
    blocking(move || {
        let dir = Path::new(&root);
        match lsp::find(dir, family, &default_homes().1) {
            Err(m) => Ok(LspStart::Missing(m)),
            Ok((name, program, args)) => {
                let lsps = app.state::<lsp::Lsps>();
                Ok(LspStart::Started(lsps.start(lsp::app_sink(&app), dir, family, &program, &args, &name)?))
            }
        }
    })
    .await
}

#[tauri::command]
fn lsp_send(lsps: tauri::State<'_, lsp::Lsps>, id: u32, msg: String) -> CmdResult<()> {
    lsps.send(id, &msg).map_err(|e| format!("{e:#}"))
}

/// Installs the server for a language, on the person's click. Only the command `find` names.
#[tauri::command]
async fn lsp_install(root: String, lang: String) -> CmdResult<()> {
    let Some(family) = lsp::family(&lang) else { return Err(format!("no language server for {lang}")) };
    blocking(move || {
        let home = default_homes().1;
        let Err(missing) = lsp::find(Path::new(&root), family, &home) else { return Ok(()) };
        let Some(cmd) = missing.install else { bail!("{}", missing.hint) };
        std::fs::create_dir_all(lsp::own_dir(&home))?;
        let out = std::process::Command::new(&cmd[0]).args(&cmd[1..]).current_dir(lsp::own_dir(&home)).output().with_context(|| format!("could not run {}", cmd[0]))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("{} failed: {}", cmd.join(" "), err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        Ok(())
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
fn extend_path() {
    let current = std::env::var("PATH").unwrap_or_default();
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
        .manage(lsp::Lsps::default())
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
            file_read,
            file_write,
            files_search,
            files_ignored,
            dir_list,
            file_changes,
            term_open,
            lsp_start,
            lsp_send,
            lsp_install,
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
                app.state::<lsp::Lsps>().close_all();
            }
        });
}
