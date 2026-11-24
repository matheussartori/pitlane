//! Hand-offs to macOS: the Finder, the browser and the print dialog.

use crate::CmdResult;

/// Shows a file or folder in the Finder.
#[tauri::command]
pub(crate) async fn reveal(path: String) -> CmdResult<()> {
    open_with(vec!["-R".into(), path]).await
}

/// Runs macOS's `open` off the main thread (a slow Finder must not freeze the window) and waits
/// for it, so it never stays behind as a zombie.
async fn open_with(args: Vec<String>) -> CmdResult<()> {
    crate::blocking(move || {
        std::process::Command::new("/usr/bin/open").args(&args).status()?;
        Ok(())
    })
    .await
}

/// Opens a web link in the person's browser (links clicked in a terminal). Only http and https.
#[tauri::command]
pub(crate) async fn open_external(url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url} is not a web link"));
    }
    open_with(vec![url]).await
}

/// The system print dialog for the window (the Reports screen prints only the report: Save as PDF).
#[tauri::command]
pub(crate) fn print_page(window: tauri::WebviewWindow) -> CmdResult<()> {
    window.print().map_err(|e| e.to_string())
}
