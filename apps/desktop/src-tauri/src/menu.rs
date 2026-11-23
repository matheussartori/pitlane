//! The app menu, the menu-bar icon, and bringing the window back.

use tauri::Manager;

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The menu-bar icon: a click opens Pitlane, its menu opens or quits it.
pub(crate) fn tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;
    let open = MenuItem::with_id(app, "tray-open", "Open Pitlane", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit Pitlane", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Pitlane")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "tray-open" => show_main(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// The app menu. Like the default one, except ⌘W: it belongs to the screen (a terminal pane, a
/// file), so Close Window moves to ⇧⌘W.
pub(crate) fn menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
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
