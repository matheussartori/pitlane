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
    // Whatever it left behind goes with it, in its group (which keeps its id for as long as
    // anything is in it) or out of it.
    let left = reach(group);
    signal_all(&left, libc::SIGTERM);
    if status.is_none() {
        std::thread::sleep(Duration::from_millis(500));
    }
    signal_all(&left, libc::SIGKILL);
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
        signal_all(&reach(*group), libc::SIGKILL);
    }
}

/// What `cmd` prints, when it ends within `limit`; none when it does not, and then it is ended
/// with everything it started. For a quick question to a program that may hang (the agent's
/// CLI asked whether it is signed in, with the network down).
pub fn output_within(cmd: &mut Command, limit: Duration) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::unix::process::CommandExt as _;
    let mut child = cmd.stdout(Stdio::piped()).process_group(0).spawn()?;
    let mut pipe = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(p) = pipe.as_mut() {
            let _ = p.read_to_end(&mut out);
        }
        out
    });
    let t = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(Some(reader.join().unwrap_or_default()));
        }
        if t.elapsed() >= limit {
            end_group(child.id(), Duration::from_millis(500));
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One row of the process table: who started it and which group it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    pub parent: i32,
    pub group: i32,
}

/// The process table as `ps` gives it now. Empty when `ps` cannot be run.
pub fn table() -> Vec<Proc> {
    let out = Command::new("ps").args(["-axo", "pid=,ppid=,pgid="]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace().map(str::parse::<i32>);
            Some(Proc { pid: f.next()?.ok()?, parent: f.next()?.ok()?, group: f.next()?.ok()? })
        })
        .collect()
}

/// What `pid` started that is no longer in its group, in `table`: a server run with `setsid` or
/// `nohup`, a child that made a group of its own. Found from `pid` and from everything still in
/// its group (the agent may be gone while the shell it started lives on), down through their
/// children. Each is given as what to signal: a negative number for a group one of them leads
/// (its own children go with it), the pid itself otherwise. Something that already cut every
/// tie (its parent gone, adopted by the system, in a group of its own) cannot be found this way.
pub fn escaped(table: &[Proc], pid: i32) -> Vec<i32> {
    let mut ours: std::collections::BTreeSet<i32> = table.iter().filter(|p| p.pid == pid || p.group == pid).map(|p| p.pid).collect();
    ours.insert(pid);
    loop {
        let more: Vec<i32> = table.iter().filter(|p| ours.contains(&p.parent) && !ours.contains(&p.pid)).map(|p| p.pid).collect();
        if more.is_empty() {
            break;
        }
        ours.extend(more);
    }
    let mut out = std::collections::BTreeSet::new();
    for p in table.iter().filter(|p| ours.contains(&p.pid) && p.group != pid && p.pid > 1) {
        // Its group, when one of ours leads it; never a group led by a stranger.
        out.insert(if p.group > 1 && ours.contains(&p.group) { -p.group } else { p.pid });
    }
    out.into_iter().collect()
}

/// What to signal to reach everything `pid` started: its group (the process alone when it
/// leads none), then what left the group (`escaped`). Read before anything is signalled: once
/// a parent is gone, its children can no longer be traced to it.
pub fn reach(pid: u32) -> Vec<i32> {
    let Ok(pid) = i32::try_from(pid) else { return Vec::new() };
    if pid <= 1 {
        return Vec::new();
    }
    // SAFETY: getpgid takes no pointers.
    let own = if unsafe { libc::getpgid(pid) } == pid { -pid } else { pid };
    let mut targets = vec![own];
    targets.extend(escaped(&table(), pid));
    targets
}

/// Sends `sig` to each of `targets` (as `reach` gives them).
pub fn signal_all(targets: &[i32], sig: i32) {
    for t in targets.iter().filter(|t| t.abs() > 1) {
        // SAFETY: kill takes no pointers; a negative pid names a group.
        unsafe { libc::kill(*t, sig) };
    }
}

/// Ends `pid` and everything it started, in its group or out of it (`reach`): SIGTERM, then
/// SIGKILL to what is still there after `grace`. Blocks for at most `grace`.
pub fn end_group(pid: u32, grace: Duration) {
    let targets = reach(pid);
    signal_all(&targets, libc::SIGTERM);
    let t = Instant::now();
    while t.elapsed() < grace {
        // Signal 0 only asks whether anything is left to signal.
        // SAFETY: kill takes no pointers.
        if targets.iter().all(|target| unsafe { libc::kill(*target, 0) } != 0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_all(&targets, libc::SIGKILL);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, parent: i32, group: i32) -> Proc {
        Proc { pid, parent, group }
    }

    #[test]
    fn what_left_the_group_is_found_through_what_stayed() {
        let table = [
            p(1, 0, 1),
            p(100, 1, 100),   // the agent, leading its group
            p(101, 100, 100), // a shell it runs, in the group
            p(200, 101, 200), // a server started with setsid: its own group
            p(201, 200, 200), // the server's worker
            p(300, 101, 999), // moved into a stranger's group: signalled alone
            p(999, 1, 999),   // the stranger
            p(400, 1, 400),   // someone else's process
        ];
        assert_eq!(escaped(&table, 100), [-200, 300]);
        // The agent is gone; its shell lives on in the group and still leads to the server.
        let orphaned: Vec<Proc> = table.iter().copied().filter(|x| x.pid != 100).collect();
        assert_eq!(escaped(&orphaned, 100), [-200, 300]);
        // Nothing left the group: nothing beyond the group itself.
        assert!(escaped(&[p(100, 1, 100), p(101, 100, 100)], 100).is_empty());
    }

    #[test]
    fn a_quick_question_gets_its_answer_or_gives_up() {
        let said = output_within(Command::new("sh").args(["-c", "echo yes"]), Duration::from_secs(5)).unwrap();
        assert_eq!(said.as_deref(), Some(&b"yes\n"[..]));
        let t = Instant::now();
        let hung = output_within(Command::new("sh").args(["-c", "sleep 30"]), Duration::from_millis(200)).unwrap();
        assert!(hung.is_none());
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_child_that_left_the_group_ends_with_its_parent() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        // The parent leads a group; its child makes a group of its own and sleeps.
        let script = format!("perl -e 'setpgrp(0,0); open(F,\">{}\"); print F $$; close F; sleep 60' & wait", pidfile.display());
        let mut parent = Command::new("sh").arg("-c").arg(script).process_group(0).spawn().unwrap();
        let t = Instant::now();
        let child: i32 = loop {
            if let Some(n) = std::fs::read_to_string(&pidfile).ok().and_then(|s| s.trim().parse().ok()) {
                break n;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "the child never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        // SAFETY: kill takes no pointers; signal 0 only asks whether the process is there.
        assert_eq!(unsafe { libc::kill(child, 0) }, 0);
        end_group(parent.id(), Duration::from_millis(500));
        let _ = parent.wait();
        let t = Instant::now();
        while unsafe { libc::kill(child, 0) } == 0 {
            assert!(t.elapsed() < Duration::from_secs(3), "the child outlived its parent");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
