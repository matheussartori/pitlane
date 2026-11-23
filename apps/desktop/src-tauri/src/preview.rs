//! Preview: a lane's dev server, drawn beside Pitlane. A webview of its own over the Preview
//! screen's area (the page's CSP keeps frames out, and a dev server is a site of its own), parked
//! out of sight when another screen shows. Only this machine's addresses load in it.

use tauri::webview::WebviewBuilder;
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewUrl};

use crate::CmdResult;

const LABEL: &str = "preview";
const PARKED: (f64, f64) = (-20_000.0, 0.0);

/// A dev server on this machine: http(s) on localhost, 127.0.0.1, ::1, 0.0.0.0 or *.localhost.
fn is_local(u: &tauri::Url) -> bool {
    matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]" | "::1" | "0.0.0.0") || h.ends_with(".localhost"))
}

/// Shows `url` over the Preview screen's area (logical pixels of the window); loads it when it is
/// a different address from the one shown, keeps the page (and its state) when it is the same.
#[tauri::command]
pub(crate) fn preview_show(app: tauri::AppHandle, url: String, x: f64, y: f64, w: f64, h: f64) -> CmdResult<()> {
    let e = |e: tauri::Error| e.to_string();
    let mut u = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
    if !is_local(&u) {
        return Err("Preview shows only this machine's dev servers (localhost).".into());
    }
    // 0.0.0.0 is where servers listen, not an address a page loads from.
    if u.host_str() == Some("0.0.0.0") {
        let _ = u.set_host(Some("localhost"));
    }
    let (w, h) = (w.max(1.0), h.max(1.0));
    if let Some(wv) = app.get_webview(LABEL) {
        if !wv.url().is_ok_and(|now| now == u) {
            wv.navigate(u).map_err(e)?;
        }
        wv.set_size(LogicalSize::new(w, h)).map_err(e)?;
        wv.set_position(LogicalPosition::new(x, y)).map_err(e)?;
        return Ok(());
    }
    let window = app.get_window("main").ok_or("no main window")?;
    // Links to other sites open in the browser, never in the preview.
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::External(u)).focused(false).on_navigation(|u| {
        if is_local(u) || u.scheme() == "about" {
            return true;
        }
        if matches!(u.scheme(), "http" | "https") {
            let _ = std::process::Command::new("open").arg(u.as_str()).spawn();
        }
        false
    });
    window.add_child(builder, LogicalPosition::new(x, y), LogicalSize::new(w, h)).map_err(e)?;
    Ok(())
}

/// Takes the preview off the screen (another screen, or a sheet over it); the page stays loaded.
#[tauri::command]
pub(crate) fn preview_hide(app: tauri::AppHandle) {
    if let Some(wv) = app.get_webview(LABEL) {
        let _ = wv.set_position(LogicalPosition::new(PARKED.0, PARKED.1));
    }
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_focus();
    }
}

/// Loads the page again (after a change the dev server did not push).
#[tauri::command]
pub(crate) fn preview_reload(app: tauri::AppHandle) {
    if let Some(wv) = app.get_webview(LABEL) {
        let _ = wv.eval("location.reload()");
    }
}

/// Closes the preview for good: its page, its server connection.
#[tauri::command]
pub(crate) fn preview_close(app: tauri::AppHandle) {
    if let Some(wv) = app.get_webview(LABEL) {
        let _ = wv.close();
    }
}
