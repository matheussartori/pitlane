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

/// The bytes of a `data:` URL (what the image viewer holds).
fn data_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine as _;
    let (_, b64) = url.split_once(";base64,").ok_or_else(|| anyhow::anyhow!("not a base64 data URL"))?;
    Ok(base64::engine::general_purpose::STANDARD.decode(b64)?)
}

/// The image viewer's Download: the save panel, then the image where the person put it.
/// False when the panel was cancelled.
#[tauri::command]
pub(crate) async fn save_image(app: tauri::AppHandle, data: String, name: String) -> CmdResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    crate::blocking(move || {
        let bytes = data_bytes(&data)?;
        let Some(path) = app.dialog().file().set_file_name(&name).blocking_save_file() else {
            return Ok(false);
        };
        let path = path.into_path().map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(&path, bytes)?;
        Ok(true)
    })
    .await
}

/// The image viewer's Copy, when the webview's clipboard refuses an image: a PNG put on the
/// pasteboard through AppleScript, from a temporary file removed right after.
#[tauri::command]
pub(crate) async fn copy_image(png: String) -> CmdResult<()> {
    crate::blocking(move || {
        let bytes = data_bytes(&png)?;
        let file = std::env::temp_dir().join(format!("pitlane-copy-{}.png", std::process::id()));
        std::fs::write(&file, bytes)?;
        let script = format!("set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)", file.display());
        let ok = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).status().map(|s| s.success());
        let _ = std::fs::remove_file(&file);
        if !ok? {
            anyhow::bail!("the clipboard did not take the image");
        }
        Ok(())
    })
    .await
}
