//! VS Code (code-server) in the Code tab.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use pit_core::default_homes;
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::{CmdResult, blocking, holes, vscode};

/// A place on the window in logical pixels: x, y, width, height.
type Rect = (f64, f64, f64, f64);

/// VS Code views, one per folder, most recent first. Up to three stay loaded, so coming back to
/// the tab, or to a recent lane, is instant. A view is never seen before VS Code has drawn itself:
/// it loads (login, workbench) out of sight, and moves onto the tab once it is ready.
///
/// No lock here is held while a webview is called: a call from another thread waits for the
/// main thread, which may itself be waiting for that lock. Read the views with `views()`.

#[derive(Default)]
pub(crate) struct CodeView {
    views: Mutex<Vec<View>>,
    /// The folders VS Code's picker offers: [{root, label, kind}].
    places: Mutex<serde_json::Value>,
    /// The folder the tab wants shown, and where (logical pixels).
    shown: Mutex<Option<(String, Rect)>>,
    /// What the page draws over VS Code (tooltips, toasts): window rects (x, y, w, h, radius).
    holes: Mutex<Vec<[f64; 5]>>,
    next: std::sync::atomic::AtomicU32,
    /// Held while a view is looked up and made, so two callers asking for one folder at once
    /// (the tab mounting, its first resize, the prewarm) end with one view. Only ever taken off
    /// the main thread (the commands are async), so the webview calls made under it cannot wait on it.
    making: Mutex<()>,
}

#[derive(Clone)]
struct View {
    folder: String,
    label: String,
    ready: Arc<std::sync::atomic::AtomicBool>,
}

/// How many VS Code views stay loaded.
const CODE_VIEWS: usize = 3;
/// Where a view waits: loaded and drawing, out of sight.
const PARKED: (f64, f64) = (-20_000.0, 0.0);
/// How often a loading view is asked whether VS Code has drawn.
const READY_POLL: Duration = Duration::from_millis(120);
/// How long a view is asked before the watch gives up on it.
const READY_GIVE_UP: Duration = Duration::from_secs(60);
/// How long after it drew a view with a background other than Pitlane's (a theme of your own) is shown anyway.
const THEME_GRACE: Duration = Duration::from_secs(4);

/// The bundled code-server, unpacked on first use (or, in a checkout without it, one on the PATH).
fn code_server(app: &tauri::AppHandle) -> Result<PathBuf> {
    vscode::ensure(app.path().resource_dir().ok().as_deref(), &default_homes().1)
}

/// Pitlane's colours for VS Code's dark and light themes.
#[tauri::command]
pub(crate) async fn vscode_theme(app: tauri::AppHandle, dark: serde_json::Map<String, serde_json::Value>, light: serde_json::Map<String, serde_json::Value>, mode: Option<String>) -> CmdResult<()> {
    let changed = blocking(move || vscode::write_theme(&default_homes().1, dark, light, mode.as_deref().unwrap_or("system"))).await?;
    // A loaded VS Code keeps the scheme it started with: reload it to follow Pitlane's (open
    // editors come back, unsaved text with them, from VS Code's own backup).
    if changed {
        for v in views(&app.state::<CodeView>()) {
            if let Some(wv) = app.get_webview(&v.label) {
                let _ = wv.eval("location.reload()");
            }
        }
    }
    Ok(())
}

async fn code_server_running(app: &tauri::AppHandle) -> CmdResult<vscode::Running> {
    let a = app.clone();
    let r = blocking(move || {
        let bin = code_server(&a)?;
        a.state::<vscode::Vscode>().start(&bin, a.path().resource_dir().ok().as_deref(), &default_homes().1)
    })
    .await?;
    watch_asks(app);
    Ok(r)
}

/// Picks up what the bridge extension leaves ("Ask Claude about this" on a selection) and hands each
/// request to the page as a `vscode-ask` event, once. A thread of the app's: it ends with it.
fn watch_asks(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let dir = vscode::asks_dir(&default_homes().1);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            files.sort();
            for f in files {
                let text = std::fs::read_to_string(&f).ok();
                let _ = std::fs::remove_file(&f);
                let ask = text.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object());
                if let Some(v) = ask {
                    let _ = app.emit("vscode-ask", v);
                }
            }
        }
    });
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
    for v in views(&state) {
        let Some(wv) = app.get_webview(&v.label) else { continue };
        match &shown {
            Some((folder, (x, y, w, h))) if *folder == v.folder && v.ready.load(std::sync::atomic::Ordering::SeqCst) => {
                let _ = wv.set_size(LogicalSize::new(*w, *h));
                let _ = wv.set_position(LogicalPosition::new(*x, *y));
                let _ = wv.set_focus();
                holes::cut(&wv, (*w, *h), local_holes(&state, *x, *y));
            }
            _ => {
                holes::cut(&wv, (0.0, 0.0), Vec::new());
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
        while !ready.load(std::sync::atomic::Ordering::SeqCst) && t.elapsed() < READY_GIVE_UP {
            let (r, f, w, expect, first) = (ready.clone(), folder.clone(), wv.clone(), expect.clone(), first.clone());
            let _ = wv.eval_with_callback(PROBE, move |out| {
                let out = out.trim().trim_matches('"').to_owned();
                let Some(bg) = out.strip_prefix("bg:") else { return };
                let since = *first.lock().unwrap().get_or_insert_with(Instant::now);
                // Pitlane's ground, or (a theme of your own choosing) a few seconds after it drew.
                let settled = expect.as_deref().is_none_or(|e| e.eq_ignore_ascii_case(bg)) || since.elapsed() > THEME_GRACE;
                if settled && !r.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    eprintln!("pitlane: VS Code drawn on {f} in {} ms", t.elapsed().as_millis());
                    let app = w.app_handle().clone();
                    place_views(&app);
                    let _ = app.emit("vscode-ready", CodeReady { folder: f.clone() });
                }
            });
            std::thread::sleep(READY_POLL);
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
    let _one = state.making.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // The list is never held while a webview is asked or changed: those calls wait on the main
    // thread, which itself takes the list (placing views, the holes, the places), so holding it
    // across them could freeze the app for good.
    let existing = {
        let mut views = state.views.lock().unwrap();
        views.iter().position(|v| v.folder == folder).map(|i| {
            let v = views.remove(i);
            let found = (v.label.clone(), v.ready.clone());
            views.insert(0, v);
            found
        })
    };
    if let Some((label, ready)) = existing {
        if let Some(wv) = app.get_webview(&label) {
            // A server started again since (a new port): load the folder from it.
            if !wv.url().is_ok_and(|u| u.port() == Some(r.port)) {
                ready.store(false, std::sync::atomic::Ordering::SeqCst);
                wv.navigate(url).map_err(e)?;
                return Ok(false);
            }
            return Ok(ready.load(std::sync::atomic::Ordering::SeqCst));
        }
        state.views.lock().unwrap().retain(|v| v.label != label);
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
                // Only this machine's code-server: the view never leaves for another site.
                let port = nav_app.state::<vscode::Vscode>().current().map(|r| r.port);
                return is_local_code_server(u, port);
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
            // code-server's login page: type the password made for this run, only on the
            // server Pitlane started (its address and port), never on another page called /login.
            if let Some(r) = wv.app_handle().state::<vscode::Vscode>().current()
                && p.url().host_str() == Some("127.0.0.1")
                && p.url().port() == Some(r.port)
            {
                let _ = wv.eval(vscode::login_script(&r.password));
            }
        } else {
            // The places may have come while the page was still loading (an eval then is lost, and
            // the start script holds what was known when the view was made): give it them again.
            let places = wv.app_handle().state::<CodeView>().places.lock().unwrap().clone();
            let _ = wv.eval(vscode::places_script(&places));
            watch_ready(wv, rr.clone(), ff.clone(), expect.clone());
        }
    });
    if let Some(c) = bg.and_then(hex_colour) {
        builder = builder.background_color(Color(c.0, c.1, c.2, 255));
    }
    // Drawn out of sight (not hidden: a hidden view may not draw), moved in when ready.
    window.add_child(builder, LogicalPosition::new(PARKED.0, PARKED.1), LogicalSize::new(size.0.max(CODE_VIEW_MIN.0), size.1.max(CODE_VIEW_MIN.1))).map_err(e)?;
    let dropped: Vec<View> = {
        let mut views = state.views.lock().unwrap();
        views.insert(0, View { folder: folder.to_owned(), label, ready });
        let keep = views.len().min(CODE_VIEWS);
        views.split_off(keep)
    };
    for old in dropped {
        if let Some(v) = app.get_webview(&old.label) {
            let _ = v.close();
        }
    }
    Ok(false)
}

/// The smallest size a Code view is made at, before the tab places it.
const CODE_VIEW_MIN: (f64, f64) = (400.0, 300.0);

/// Whether a Code view may go to `u`: only the code-server Pitlane started (this machine, its
/// port) and blank pages. Never another site, another local server, or a file: any of them could
/// pose as VS Code in a view that looks like Pitlane.
fn is_local_code_server(u: &tauri::Url, port: Option<u16>) -> bool {
    match u.scheme() {
        "http" | "https" | "ws" | "wss" => {
            matches!(u.host_str(), Some("127.0.0.1" | "localhost")) && port.is_some() && u.port_or_known_default() == port
        }
        "about" => true,
        _ => false,
    }
}

fn hex_colour(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

/// The folders VS Code's picker offers (the repository and the lanes' worktrees), now and in
/// every view already loaded.
#[tauri::command]
pub(crate) fn vscode_places(app: tauri::AppHandle, places: serde_json::Value) {
    let state = app.state::<CodeView>();
    *state.places.lock().unwrap() = places.clone();
    for v in views(&state) {
        if let Some(wv) = app.get_webview(&v.label) {
            let _ = wv.eval(vscode::places_script(&places));
        }
    }
}

/// Starts VS Code on `folder` out of sight, so the Code tab opens at once.
#[tauri::command]
pub(crate) async fn vscode_prewarm(app: tauri::AppHandle, folder: String, w: f64, h: f64, bg: Option<String>) -> CmdResult<()> {
    let r = code_server_running(&app).await?;
    code_view(&app, &r, &folder, (w, h), bg.as_deref())?;
    Ok(())
}

/// Shows VS Code over the Code tab's area (logical pixels of the window), on `folder`. Says whether
/// it is already drawn; if not, it moves in by itself when it is (and `vscode-ready` says so).
#[tauri::command]
pub(crate) async fn vscode_show(app: tauri::AppHandle, folder: String, x: f64, y: f64, w: f64, h: f64, bg: Option<String>) -> CmdResult<bool> {
    *app.state::<CodeView>().shown.lock().unwrap() = Some((folder.clone(), (x, y, w, h)));
    let r = code_server_running(&app).await?;
    let ready = code_view(&app, &r, &folder, (w, h), bg.as_deref())?;
    place_views(&app);
    Ok(ready)
}

/// Takes VS Code off the tab (another tab, or an app menu over it) and gives the keyboard back.
#[tauri::command]
pub(crate) async fn vscode_hide(app: tauri::AppHandle) -> CmdResult<()> {
    *app.state::<CodeView>().shown.lock().unwrap() = None;
    place_views(&app);
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_focus();
    }
    Ok(())
}

/// What the page draws over VS Code's area (window rects: x, y, w, h, corner radius), shown
/// through it: VS Code is a view of its own above the page, out of reach of any z-index.
#[tauri::command]
pub(crate) fn vscode_holes(app: tauri::AppHandle, holes: Vec<[f64; 5]>) {
    let state = app.state::<CodeView>();
    *state.holes.lock().unwrap() = holes;
    let shown = state.shown.lock().unwrap().clone();
    let Some((folder, (x, y, w, h))) = shown else { return };
    for v in views(&state).into_iter().filter(|v| v.folder == folder) {
        if let Some(wv) = app.get_webview(&v.label) {
            holes::cut(&wv, (w, h), local_holes(&state, x, y));
        }
    }
}

/// The views as they are now, read under the lock and used after it.
fn views(state: &CodeView) -> Vec<View> {
    state.views.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The holes, in the coordinates of a view placed at (x, y).
fn local_holes(state: &CodeView, x: f64, y: f64) -> Vec<[f64; 5]> {
    state.holes.lock().unwrap().iter().map(|[hx, hy, hw, hh, r]| [hx - x, hy - y, *hw, *hh, *r]).collect()
}

/// Opens a file at a line in the VS Code that is running.
#[tauri::command]
pub(crate) async fn vscode_open(app: tauri::AppHandle, path: String, line: Option<u32>, col: Option<u32>) -> CmdResult<()> {
    blocking(move || {
        let bin = code_server(&app)?;
        app.state::<vscode::Vscode>().open_file(&bin, &default_homes().1, Path::new(&path), line, col)
    })
    .await
}
