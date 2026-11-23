//! Who is on the other end of the socket. Anything an agent runs (a test, a build script, a git
//! hook) runs as you and can reach the daemon's socket, so a request from inside a session the
//! daemon started may only read and ask: it never decides an approval, starts or reviews work,
//! or ends the daemon. The sandbox the runs get keeps their commands off the socket in the first
//! place; this is the check behind it, for whatever runs outside the sandbox.

use std::collections::HashSet;

/// How far up the process tree to look: deeper than any real chain, short of a loop.
const MAX_DEPTH: usize = 64;

/// What a session's own processes may still ask of the daemon: reading, and asking you.
const AGENT_OPS: &[&str] = &["ping", "ask", "get", "list", "sessions", "quota", "follow", "approvals", "perf"];

pub fn allowed_from_agent(op: &str) -> bool {
    AGENT_OPS.contains(&op)
}

/// Whether `peer` is one of the daemon's sessions or something they started: the daemon is among
/// its ancestors, or it is still in the process group of a session (a command that left the tree
/// by letting its parent end stays in its group unless it makes a new one).
pub fn from_agent(peer: i32, daemon: i32, groups: &HashSet<i32>) -> bool {
    // SAFETY: getpgid reads the group of a pid; it takes no pointers.
    let group = unsafe { libc::getpgid(peer) };
    if group > 1 && groups.contains(&group) {
        return true;
    }
    let mut pid = peer;
    for _ in 0..MAX_DEPTH {
        match parent(pid) {
            Some(p) if p == daemon => return true,
            Some(p) if p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn parent(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`, which we own.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then_some(info.pbi_ppid as i32)
}

#[cfg(not(target_os = "macos"))]
fn parent(pid: i32) -> Option<i32> {
    // /proc/<pid>/stat: "pid (comm) state ppid ...", where comm may hold spaces and parens.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn a_child_of_the_daemon_is_an_agent_and_a_stranger_is_not() {
        let me = std::process::id() as i32;
        let child = Command::new("sleep").arg("5").spawn().unwrap();
        let pid = child.id() as i32;
        assert!(from_agent(pid, me, &HashSet::new()), "a child is one of ours");
        assert!(!from_agent(pid, 999_999, &HashSet::new()), "not a child of someone else");
        assert!(!from_agent(me, me + 1, &HashSet::new()));
        let mut child = child;
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn what_an_agent_may_ask() {
        assert!(allowed_from_agent("ask"));
        assert!(allowed_from_agent("get"));
        for op in ["decide", "start", "review", "shutdown", "hold", "perf_set", "enqueue", "stop_all", "say"] {
            assert!(!allowed_from_agent(op), "{op}");
        }
    }
}
