//! Hand-offs to macOS: the Finder, the browser and the print dialog.

use crate::CmdResult;

/// Shows a file or folder in the Finder.
#[tauri::command]
pub(crate) fn reveal(path: String) -> CmdResult<()> {
    std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens a web link in the person's browser (links clicked in a terminal). Only http and https.
#[tauri::command]
pub(crate) fn open_external(url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url} is not a web link"));
    }
    std::process::Command::new("/usr/bin/open").arg(&url).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// The system print dialog for the window (the Reports screen prints only the report: Save as PDF).
#[tauri::command]
pub(crate) fn print_page(window: tauri::WebviewWindow) -> CmdResult<()> {
    window.print().map_err(|e| e.to_string())
}
