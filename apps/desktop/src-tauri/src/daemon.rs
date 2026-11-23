//! The daemon (`pitd`): finding it, or the `pit` that starts it, calling it, and holding it
//! while the app is open.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use pit_core::default_homes;
use pit_core::runs::Client;

/// How long the hold waits before it looks for a daemon again.
const HOLD_RETRY: Duration = Duration::from_millis(500);

/// The `pit` executable that starts the daemon when nobody answers on its socket.
fn pit_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PITLANE_PIT_BIN") {
        return Some(PathBuf::from(p));
    }
    // The one bundled beside the app's own binary (Pitlane.app/Contents/MacOS/pit), then the PATH.
    let bundled = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("pit"))).filter(|p| p.is_file());
    bundled.or_else(|| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("pit")).find(|c| c.is_file())))
}

/// The daemon, started if nobody answers.
pub(crate) fn daemon() -> Result<Client> {
    Client::connect(&default_homes().1, pit_bin().as_deref())
}

/// The daemon if it is up, without starting one.
pub(crate) fn running() -> Option<Client> {
    Client::connect(&default_homes().1, None).ok()
}

/// Set when the app is quitting, so the hold below does not wait for a new daemon.
pub(crate) static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While the app is open it holds any daemon that is up; when the app ends, however it ends
/// (quit, crash, force quit), the connection drops and the daemon stops everything and exits.
pub(crate) fn hold_daemon() {
    std::thread::spawn(|| {
        use std::sync::atomic::Ordering;
        while !QUITTING.load(Ordering::SeqCst) {
            if let Some(c) = running() {
                let _ = c.hold();
            }
            std::thread::sleep(HOLD_RETRY);
        }
    });
}
