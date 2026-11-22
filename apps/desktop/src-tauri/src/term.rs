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
    Data(u32, String),
    Exit(u32, u32),
}

pub type Sink = Arc<dyn Fn(Out) + Send + Sync>;

/// The app's events.
pub fn app_sink(app: &AppHandle) -> Sink {
    let app = app.clone();
    Arc::new(move |o| {
        let _ = match o {
            Out::Data(id, data) => app.emit("term-data", DataOut { id, data }),
            Out::Exit(id, code) => app.emit("term-exit", ExitOut { id, code }),
        };
    })
}

const KEEP_BYTES: usize = 256 * 1024;

struct Term {
    info: TermInfo,
    master: Box<dyn MasterPty + Send>,
    /// Keystrokes go to a thread of their own: a program that stops reading can fill the PTY,
    /// and that must never block the app (writes keep their order through the channel).
    writer: std::sync::mpsc::Sender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
    buffer: Arc<Mutex<String>>,
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
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Pitlane");
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
        let buffer = Arc::new(Mutex::new(String::new()));

        // Output: bytes into text, an incomplete UTF-8 sequence carried to the next read.
        let (out, buf2) = (Arc::clone(&sink), Arc::clone(&buffer));
        std::thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            let mut carry: Vec<u8> = Vec::new();
            loop {
                let n = match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                carry.extend_from_slice(&chunk[..n]);
                let valid = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => carry.len(), // truly invalid bytes: let the lossy decode replace them
                };
                let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
                carry.drain(..valid);
                if text.is_empty() {
                    continue;
                }
                {
                    let mut b = buf2.lock().unwrap();
                    b.push_str(&text);
                    if b.len() > KEEP_BYTES {
                        let cut = b.len() - KEEP_BYTES;
                        let at = (cut..b.len()).find(|i| b.is_char_boundary(*i)).unwrap_or(b.len());
                        b.drain(..at);
                    }
                }
                out(Out::Data(id, text));
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

    pub fn buffer(&self, id: u32) -> Result<String> {
        let all = self.all.lock().unwrap();
        let t = all.get(&id).context("that terminal is gone")?;
        Ok(t.buffer.lock().unwrap().clone())
    }

    pub fn close_all(&self) {
        let all: Vec<Term> = self.all.lock().unwrap().drain().map(|(_, t)| t).collect();
        end(all);
    }
}

/// Ends shells and everything they started: the shell leads its own session, so a hangup to
/// that session reaches the program in front (a dev server, an agent) and the jobs behind it;
/// what is left a moment later is killed.
fn end(mut terms: Vec<Term>) {
    let sids: Vec<String> = terms.iter().filter_map(|t| t.child.process_id()).map(|p| p.to_string()).collect();
    let pkill = |sig: &str| {
        if !sids.is_empty() {
            let _ = std::process::Command::new("pkill").args([sig, "-s", &sids.join(",")]).status();
        }
    };
    pkill("-HUP");
    std::thread::sleep(std::time::Duration::from_millis(300));
    pkill("-KILL");
    for t in &mut terms {
        let _ = t.child.kill();
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
            Out::Data(_, d) => s2.lock().unwrap().push_str(&d),
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
        assert!(terms.buffer(t.id).unwrap().contains("pit42lane"), "kept for a reload");
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
            if let Out::Data(_, d) = o {
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
