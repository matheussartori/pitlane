//! Ending processes Pitlane started, with everything they started in turn.

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Asked while a command runs: true once whoever started it wants it gone.
pub type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;

thread_local! {
    static WATCH: RefCell<Option<Cancel>> = const { RefCell::new(None) };
}

/// The groups of the commands `run_shell` has running, so closing can end them all.
static SHELLS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Runs `f` with the flow.toml commands it starts on this thread (a worktree's setup, a ship's
/// check) watched: each in a group of its own, under a time limit, ended when `cancelled` says
/// so and when the daemon or the app closes. Without it they run as in a terminal, which is what
/// the CLI wants: output on the screen and Ctrl+C to stop.
pub fn watched<T>(cancelled: Cancel, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Cancel>);
    impl Drop for Restore {
        fn drop(&mut self) {
            WATCH.with(|w| *w.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(WATCH.with(|w| w.replace(Some(cancelled))));
    f()
}

/// What `watched` set for this thread, if anything.
pub fn watch() -> Option<Cancel> {
    WATCH.with(|w| w.borrow().clone())
}

/// How a command run by `run_shell` ended.
pub struct Ran {
    /// Its exit status, or none when it was killed first.
    pub status: Option<ExitStatus>,
    /// Why it was killed (stopped, took too long), when it was.
    pub why: Option<String>,
    /// The end of what it printed (stdout and stderr together), at most `OUTPUT_KEPT` bytes.
    pub output: String,
}

/// How much of a command's output is read back: enough for the tail a person reads.
const OUTPUT_KEPT: u64 = 64 * 1024;

/// Runs `command` in `dir` through the shell, like a person would, in a process group of its own
/// that is killed whole when the command ends, when it takes longer than `limit`, or when
/// `cancelled` says so: a watcher or server it starts never lingers. Blocks until then.
pub fn run_shell(dir: &Path, command: &str, limit: Duration, cancelled: &dyn Fn() -> bool) -> std::io::Result<Ran> {
    use std::os::unix::process::CommandExt;
    let t = Instant::now();
    // Output to a file, not a pipe: a child that keeps the pipe open cannot hold the caller.
    let mut out = tempfile::tempfile()?;
    let mut child = Command::new("sh")
        .arg("-c")
        // Grouped, so stderr of every part of a compound command lands in the output.
        .arg(format!("{{ {command}\n}} 2>&1"))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .process_group(0)
        .spawn()?;
    let group = child.id();
    SHELLS.lock().unwrap().push(group);
    let (status, why) = loop {
        match child.try_wait() {
            Ok(Some(st)) => break (Some(st), None),
            Ok(None) if cancelled() => break (None, Some("stopped".to_owned())),
            Ok(None) if t.elapsed() > limit => break (None, Some(format!("took longer than {} minutes; killed", limit.as_secs() / 60))),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => break (None, Some(format!("could not wait for it: {e}"))),
        }
    };
    // Whatever it left behind goes with it: the group keeps its id for as long as anything is in it.
    signal_group(group, libc::SIGTERM);
    if status.is_none() {
        std::thread::sleep(Duration::from_millis(500));
    }
    signal_group(group, libc::SIGKILL);
    let _ = child.wait();
    SHELLS.lock().unwrap().retain(|g| *g != group);
    let len = out.metadata().map(|m| m.len()).unwrap_or(0);
    let mut bytes = Vec::new();
    if out.seek(SeekFrom::Start(len.saturating_sub(OUTPUT_KEPT))).is_ok() {
        let _ = out.read_to_end(&mut bytes);
    }
    // Lossy: one stray byte (Latin-1, a binary dump) must not empty what a failure printed.
    Ok(Ran { status, why, output: String::from_utf8_lossy(&bytes).into_owned() })
}

/// Ends every command still running through `run_shell`. For the daemon and the app as they
/// close: nothing they started stays behind.
pub fn end_shells() {
    for group in SHELLS.lock().unwrap().iter() {
        signal_group(*group, libc::SIGKILL);
    }
}

fn signal_group(group: u32, sig: i32) {
    let Ok(g) = i32::try_from(group) else { return };
    if g > 1 {
        // SAFETY: kill takes no pointers; a negative pid names the group.
        unsafe { libc::kill(-g, sig) };
    }
}

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
