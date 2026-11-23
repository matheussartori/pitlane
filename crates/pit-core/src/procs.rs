//! Ending processes Pitlane started, with everything they started in turn.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// SIGTERM to the group `pid` leads (to the process alone when it leads none), then SIGKILL to
/// what is still there after `grace`. Blocks for at most `grace`.
pub fn end_group(pid: u32, grace: Duration) {
    let Ok(pid) = i32::try_from(pid) else { return };
    if pid <= 1 {
        return;
    }
    // SAFETY: kill and getpgid take no pointers; a negative pid names the group `pid` leads.
    let target = if unsafe { libc::getpgid(pid) } == pid { -pid } else { pid };
    unsafe { libc::kill(target, libc::SIGTERM) };
    let t = Instant::now();
    while t.elapsed() < grace {
        // Signal 0 only asks whether anything is left to signal.
        if unsafe { libc::kill(target, 0) } != 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(target, libc::SIGKILL) };
}

/// Where the desktop app keeps its code-server (the VS Code in the Code tab).
pub fn code_server_dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("code-server")
}

/// The pid file the app writes when it starts code-server.
pub fn code_server_pidfile(pitlane_home: &Path) -> PathBuf {
    code_server_dir(pitlane_home).join("server.pid")
}

/// Ends the code-server the app started, if one is still running: when the app did not get to
/// close it (a crash, a force quit), the daemon or the next start does. Only a process that is
/// that code-server (its command names code-server and Pitlane's folder) is touched. True when
/// one was ended.
pub fn end_code_server(pitlane_home: &Path) -> bool {
    let pidfile = code_server_pidfile(pitlane_home);
    let Some(pid) = std::fs::read_to_string(&pidfile).ok().and_then(|p| p.trim().parse::<u32>().ok()) else {
        return false;
    };
    let cmd = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let ours = cmd.contains("code-server") && cmd.contains(code_server_dir(pitlane_home).to_string_lossy().as_ref());
    if ours {
        end_group(pid, Duration::from_secs(1));
    }
    let _ = std::fs::remove_file(&pidfile);
    ours
}
