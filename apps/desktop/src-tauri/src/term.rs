//! Free terminals: the person's own login shell in a real PTY, in the repository or a lane's
//! worktree. Output streams to the app as `term-data` events and is kept (the last 256 KB) so a
//! reloaded window can draw each terminal again. Terminals end with the app.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// A program a terminal starts instead of a bare shell.
pub struct Program {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub session: Option<String>,
}

/// Where a terminal's output and exit go: the app's events, or a test.
pub enum Out {
    /// Output, and its number in the terminal's sequence (see `Kept`).
    Data(u32, String, u64),
    Exit(u32, u32),
}

pub type Sink = Arc<dyn Fn(Out) + Send + Sync>;

/// The app's events.
pub fn app_sink(app: &AppHandle) -> Sink {
    let app = app.clone();
    Arc::new(move |o| {
        let _ = match o {
            Out::Data(id, data, seq) => app.emit("term-data", DataOut { id, data, seq }),
            Out::Exit(id, code) => app.emit("term-exit", ExitOut { id, code }),
        };
    })
}

const KEEP_BYTES: usize = 256 * 1024;
/// Output is handed to the window at most this often, and at most this much at a time: short
/// enough that an echoed key is never felt, long enough that a flood is not an event per read.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(6);
const FLUSH_BYTES: usize = 64 * 1024;

struct Term {
    info: TermInfo,
    master: Box<dyn MasterPty + Send>,
    /// Keystrokes go to a thread of their own: a program that stops reading can fill the PTY,
    /// and that must never block the app (writes keep their order through the channel).
    writer: std::sync::mpsc::Sender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
    buffer: Arc<Mutex<Kept>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TermInfo {
    pub id: u32,
    /// The Claude Code session this terminal resumed (`claude --resume`), if it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub cwd: String,
    /// What the tab says: the folder, or the lane's branch.
    pub label: String,
    pub exited: Option<u32>,
}

#[derive(Default)]
pub struct Terms {
    next: Mutex<u32>,
    all: Arc<Mutex<HashMap<u32, Term>>>,
}

#[derive(Clone, Serialize)]
struct DataOut {
    id: u32,
    data: String,
    seq: u64,
}

/// What a terminal printed lately, kept for a window that reloads, and how many pieces of
/// output it has sent so far. A reload reads both at once: output numbered past `seq` is not
/// in `text` yet, output up to it is, so nothing shows twice or goes missing.
#[derive(Default, Clone, Serialize)]
pub struct Kept {
    pub text: String,
    pub seq: u64,
}

#[derive(Clone, Serialize)]
struct ExitOut {
    id: u32,
    code: u32,
}

impl Terms {
    pub fn open(&self, sink: Sink, cwd: &str, label: &str, cols: u16, rows: u16) -> Result<TermInfo> {
        self.launch(sink, cwd, label, cols, rows, None)
    }

    /// A terminal that starts a program (Claude Code resuming a session), then leaves you in your
    /// shell in the same folder when it ends.
    pub fn launch(&self, sink: Sink, cwd: &str, label: &str, cols: u16, rows: u16, program: Option<Program>) -> Result<TermInfo> {
        if !Path::new(cwd).is_dir() {
            bail!("{cwd} is not a folder");
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .context("could not open a terminal")?;
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        let session = program.as_ref().and_then(|p| p.session.clone());
        let mut cmd = match &program {
            None => {
                let mut c = CommandBuilder::new(&shell);
                c.arg("-l");
                c
            }
            // The words go as arguments, never through a shell's parsing: "$@" runs them as given.
            Some(p) => {
                let mut c = CommandBuilder::new("/bin/sh");
                c.args(["-c", "\"$@\"; exec \"${SHELL:-/bin/zsh}\" -l", "pitlane"]);
                c.args(&p.args);
                for (k, v) in &p.env {
                    c.env(k, v);
                }
                c
            }
        };
        cmd.env("SHELL", &shell);
        // Pitlane opened from inside a Claude Code would hand its markers down: a claude typed or
        // resumed here would think it is that one's child (and stop saving its transcript).
        for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_CHILD_SESSION"] {
            cmd.env_remove(k);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Pitlane");
        // zsh marks each command (OSC 133) through our startup files, which source yours first.
        if Path::new(&shell).file_name().is_some_and(|n| n == "zsh")
            && let Some(dir) = zsh_integration()
        {
            let user = std::env::var("ZDOTDIR").ok().filter(|d| !d.is_empty()).or_else(|| std::env::var("HOME").ok()).unwrap_or_default();
            cmd.env("PITLANE_USER_ZDOTDIR", user);
            cmd.env("ZDOTDIR", dir);
        }
        let child = pair.slave.spawn_command(cmd).with_context(|| format!("could not start {shell}"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let mut pty_in = pair.master.take_writer()?;
        let (writer, keys) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for data in keys {
                if pty_in.write_all(&data).and_then(|_| pty_in.flush()).is_err() {
                    break;
                }
            }
        });

        let id = {
            let mut n = self.next.lock().unwrap();
            *n += 1;
            *n
        };
        let info = TermInfo { id, session, cwd: cwd.to_owned(), label: label.to_owned(), exited: None };
        let buffer = Arc::new(Mutex::new(Kept::default()));

        // Output. One thread reads the PTY; another turns what was read into text and hands it
        // on at most every `FLUSH_EVERY`, so a program that floods the terminal (`yes`, a noisy
        // build) is a few large events instead of thousands of tiny ones the window chokes on.
        // The channel between them is bounded: when the window falls behind, the reader waits,
        // the PTY fills and the program slows down, as in any terminal.
        let (bytes_tx, bytes_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if bytes_tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let (out, buf2) = (Arc::clone(&sink), Arc::clone(&buffer));
        std::thread::spawn(move || {
            // An incomplete UTF-8 sequence is carried to the next flush.
            let mut carry: Vec<u8> = Vec::new();
            while let Ok(first) = bytes_rx.recv() {
                carry.extend_from_slice(&first);
                let until = std::time::Instant::now() + FLUSH_EVERY;
                let mut open = true;
                while carry.len() < FLUSH_BYTES {
                    match bytes_rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
                        Ok(more) => carry.extend_from_slice(&more),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            open = false;
                            break;
                        }
                    }
                }
                let valid = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) if e.error_len().is_none() && open => e.valid_up_to(),
                    Err(_) => carry.len(), // truly invalid bytes: let the lossy decode replace them
                };
                let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
                carry.drain(..valid);
                if text.is_empty() {
                    continue;
                }
                let seq = {
                    let mut k = buf2.lock().unwrap();
                    let b = &mut k.text;
                    b.push_str(&text);
                    if b.len() > KEEP_BYTES {
                        let cut = b.len() - KEEP_BYTES;
                        let at = (cut..b.len()).find(|i| b.is_char_boundary(*i)).unwrap_or(b.len());
                        b.drain(..at);
                    }
                    k.seq += 1;
                    k.seq
                };
                out(Out::Data(id, text, seq));
            }
        });

        self.all.lock().unwrap().insert(id, Term { info: info.clone(), master: pair.master, writer, child, buffer });

        // Exit: wait on the shell off the lock, by polling, so close() can still kill it.
        let (out, all) = (sink, Arc::clone(&self.all));
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let mut all = all.lock().unwrap();
                let Some(t) = all.get_mut(&id) else { return };
                if let Ok(Some(status)) = t.child.try_wait() {
                    t.info.exited = Some(status.exit_code());
                    out(Out::Exit(id, status.exit_code()));
                    return;
                }
            }
        });
        Ok(info)
    }

    pub fn write(&self, id: u32, data: &str) -> Result<()> {
        let all = self.all.lock().unwrap();
        let t = all.get(&id).context("that terminal is gone")?;
        t.writer.send(data.as_bytes().to_vec()).map_err(|_| anyhow::anyhow!("that terminal is gone"))?;
        Ok(())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let all = self.all.lock().unwrap();
        let t = all.get(&id).context("that terminal is gone")?;
        t.master.resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })?;
        Ok(())
    }

    pub fn close(&self, id: u32) -> Result<()> {
        if let Some(t) = self.all.lock().unwrap().remove(&id) {
            // Off the calling thread: the hangup waits a moment before the kill.
            std::thread::spawn(move || end(vec![t]));
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<TermInfo> {
        let mut v: Vec<TermInfo> = self.all.lock().unwrap().values().map(|t| t.info.clone()).collect();
        v.sort_by_key(|t| t.id);
        v
    }

    pub fn buffer(&self, id: u32) -> Result<Kept> {
        let all = self.all.lock().unwrap();
        let t = all.get(&id).context("that terminal is gone")?;
        Ok(t.buffer.lock().unwrap().clone())
    }

    pub fn close_all(&self) {
        let all: Vec<Term> = self.all.lock().unwrap().drain().map(|(_, t)| t).collect();
        end(all);
    }
}

/// The folder of zsh startup files that add command marks, written once per app run.
fn zsh_integration() -> Option<&'static str> {
    static DIR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        // In Pitlane's own folder, not a temp folder another user could write first: every shell
        // sources these.
        let dir = pit_core::default_homes().1.join("zsh");
        std::fs::create_dir_all(&dir).ok()?;
        for (name, body) in [
            (".zshenv", include_str!("shell/zshenv")),
            (".zprofile", include_str!("shell/zprofile")),
            (".zshrc", include_str!("shell/zshrc")),
            (".zlogin", include_str!("shell/zlogin")),
        ] {
            std::fs::write(dir.join(name), body).ok()?;
        }
        dir.to_str().map(str::to_owned)
    })
    .as_deref()
}

/// Ends shells and everything they started: the shell leads its own session, so a hangup to
/// that session reaches the program in front (a dev server, an agent) and the jobs behind it;
/// what is left a moment later is killed.
fn end(mut terms: Vec<Term>) {
    // A shell that already exited was reaped: its pid may be another process's by now, and
    // there is nothing of it left to end.
    terms.retain(|t| t.info.exited.is_none());
    if terms.is_empty() {
        return;
    }
    let sids: Vec<i32> = terms.iter().filter_map(|t| t.child.process_id()).map(|p| p as i32).collect();
    signal_sessions(&sids, libc::SIGHUP);
    std::thread::sleep(std::time::Duration::from_millis(300));
    signal_sessions(&sids, libc::SIGKILL);
    for t in &mut terms {
        let _ = t.child.kill();
    }
}

/// Every process in these sessions (macOS's pkill has no -s): each process's session id, by getsid.
/// A program that started a session of its own (a daemon) has left on purpose and is not followed.
fn signal_sessions(sids: &[i32], sig: i32) {
    if sids.is_empty() {
        return;
    }
    let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid="]).output() else { return };
    let me = std::process::id() as i32;
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace().filter_map(|p| p.parse::<i32>().ok()) {
        if pid <= 1 || pid == me {
            continue;
        }
        // SAFETY: getsid and kill only read or signal a pid; a pid that is gone answers -1.
        let sid = unsafe { libc::getsid(pid) };
        if sid > 0 && sids.contains(&sid) {
            unsafe { libc::kill(pid, sig) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_shell_runs_and_answers() {
        let seen = Arc::new(Mutex::new(String::new()));
        let exited = Arc::new(Mutex::new(None));
        let (s2, e2) = (Arc::clone(&seen), Arc::clone(&exited));
        let sink: Sink = Arc::new(move |o| match o {
            Out::Data(_, d, _) => s2.lock().unwrap().push_str(&d),
            Out::Exit(_, c) => *e2.lock().unwrap() = Some(c),
        });
        let dir = std::env::temp_dir();
        let terms = Terms::default();
        let t = terms.open(sink, dir.to_str().unwrap(), "tmp", 80, 24).unwrap();
        terms.resize(t.id, 120, 40).unwrap();
        terms.write(t.id, "echo pit$((40+2))lane\n").unwrap();
        let until = |f: &dyn Fn() -> bool| {
            let t0 = Instant::now();
            while !f() && t0.elapsed() < Duration::from_secs(15) {
                std::thread::sleep(Duration::from_millis(50));
            }
            f()
        };
        assert!(until(&|| seen.lock().unwrap().contains("pit42lane")), "{}", seen.lock().unwrap());
        assert!(terms.buffer(t.id).unwrap().text.contains("pit42lane"), "kept for a reload");
        if std::env::var("SHELL").is_ok_and(|s| s.ends_with("/zsh")) {
            // Command marks around each command, and the shell ends up with your own ZDOTDIR.
            assert!(until(&|| seen.lock().unwrap().contains("\x1b]133;D;0\x07")), "{:?}", seen.lock().unwrap());
            assert!(seen.lock().unwrap().contains("\x1b]633;E;echo pit$((40+2))lane\x07"));
            terms.write(t.id, "echo zd=${ZDOTDIR}=\n").unwrap();
            assert!(until(&|| seen.lock().unwrap().contains("zd=/")));
            assert!(!seen.lock().unwrap().contains("zd=/private"), "ZDOTDIR given back");
        }
        // A job left running dies with its terminal, even in a process group of its own.
        let sleeping = || std::process::Command::new("pgrep").args(["-f", "sleep 4242"]).output().is_ok_and(|o| !o.stdout.is_empty());
        let other = terms.open(Arc::new(|_| {}), dir.to_str().unwrap(), "tmp", 80, 24).unwrap();
        terms.write(other.id, "sleep 4242 &\n").unwrap();
        assert!(until(&sleeping), "the job started");
        terms.close(other.id).unwrap();
        assert!(until(&|| !sleeping()), "closing a terminal ends what it started");
        terms.write(t.id, "exit 3\n").unwrap();
        assert!(until(&|| exited.lock().unwrap().is_some()));
        assert_eq!(*exited.lock().unwrap(), Some(3));
        assert_eq!(terms.list()[0].exited, Some(3));
        terms.close(t.id).unwrap();
        assert!(terms.list().is_empty());
        assert!(terms.open(Arc::new(|_| {}), "/no/such/folder", "x", 80, 24).is_err());
    }

    #[test]
    fn a_program_gets_its_words_as_given_then_your_shell() {
        let seen = Arc::new(Mutex::new(String::new()));
        let s2 = Arc::clone(&seen);
        let sink: Sink = Arc::new(move |o| {
            if let Out::Data(_, d, _) = o {
                s2.lock().unwrap().push_str(&d)
            }
        });
        let terms = Terms::default();
        let program = Program {
            args: vec!["printf".into(), "[%s]".into(), "a b; echo HACKED".into(), "$PIT_X".into()],
            env: vec![("PIT_X".into(), "set".into())],
            session: Some("s-1".into()),
        };
        let t = terms.launch(sink, std::env::temp_dir().to_str().unwrap(), "p", 80, 24, Some(program)).unwrap();
        assert_eq!(t.session.as_deref(), Some("s-1"));
        let until = |f: &dyn Fn() -> bool| {
            let t0 = Instant::now();
            while !f() && t0.elapsed() < Duration::from_secs(15) {
                std::thread::sleep(Duration::from_millis(50));
            }
            f()
        };
        // Words reach the program untouched: no splitting, no `;`, no expansion.
        assert!(until(&|| seen.lock().unwrap().contains("[a b; echo HACKED][$PIT_X]")), "{}", seen.lock().unwrap());
        // Then the shell takes over, with the environment it was given.
        terms.write(t.id, "echo pit$PIT_X\n").unwrap();
        assert!(until(&|| seen.lock().unwrap().contains("pitset")), "{}", seen.lock().unwrap());
        assert!(!seen.lock().unwrap().contains("\nHACKED"));
        terms.close(t.id).unwrap();
    }
}
