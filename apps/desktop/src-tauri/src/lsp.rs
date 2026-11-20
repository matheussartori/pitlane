//! Language servers for the editor: one process per (folder, language), started when a file of
//! that language opens, spoken to over stdio with LSP framing and relayed to the window as events.
//! Pitlane never installs one by itself: a server is found on the machine (the project's
//! node_modules, Pitlane's own folder, the PATH), or the editor offers to install it on a click.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Live servers at most; the least recently used goes first. rust-analyzer alone can take GBs.
const CAP: usize = 4;

pub enum Out {
    Msg(u32, String),
    Exit(u32),
}

pub type Sink = Arc<dyn Fn(Out) + Send + Sync>;

#[derive(Clone, Serialize)]
struct MsgOut {
    id: u32,
    msg: String,
}

pub fn app_sink(app: &AppHandle) -> Sink {
    let app = app.clone();
    Arc::new(move |o| {
        let _ = match o {
            Out::Msg(id, msg) => app.emit("lsp-msg", MsgOut { id, msg }),
            Out::Exit(id) => app.emit("lsp-exit", id),
        };
    })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Started {
    pub id: u32,
    /// The server's name, said in the editor ("vtsls", "rust-analyzer").
    pub name: String,
}

/// Where to get a server that is not on the machine: what the install button runs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Missing {
    pub family: String,
    pub hint: String,
    pub install: Option<Vec<String>>,
}

struct Server {
    root: String,
    family: String,
    child: Child,
    stdin: ChildStdin,
    used: Instant,
}

#[derive(Default)]
pub struct Lsps {
    servers: Arc<Mutex<HashMap<u32, Server>>>,
    next: AtomicU32,
}

/// The language family of a Monaco language id or a file extension.
pub fn family(lang: &str) -> Option<&'static str> {
    match lang {
        "typescript" | "javascript" | "typescriptreact" | "javascriptreact" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => Some("ts"),
        "rust" | "rs" => Some("rust"),
        "python" | "py" | "pyi" => Some("python"),
        _ => None,
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

/// Pitlane's own folder for servers installed from the editor.
pub fn own_dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("lsp")
}

/// The command for `family` in `root`, or what is missing.
pub fn find(root: &Path, family: &str, pitlane_home: &Path) -> std::result::Result<(String, PathBuf, Vec<String>), Missing> {
    let own = own_dir(pitlane_home).join("node_modules/.bin");
    let bins = |names: &[&str]| -> Option<PathBuf> {
        for n in names {
            for dir in [root.join("node_modules/.bin"), own.clone()] {
                let f = dir.join(n);
                if f.is_file() {
                    return Some(f);
                }
            }
            if let Some(f) = on_path(n) {
                return Some(f);
            }
        }
        None
    };
    match family {
        "ts" => match bins(&["vtsls", "typescript-language-server"]) {
            Some(p) => Ok((p.file_name().unwrap().to_string_lossy().into_owned(), p, vec!["--stdio".into()])),
            None => Err(Missing {
                family: "ts".into(),
                hint: "TypeScript needs a language server: vtsls, about 30 MB, installed into ~/.pitlane/lsp.".into(),
                install: Some(vec!["npm".into(), "install".into(), "--prefix".into(), own_dir(pitlane_home).to_string_lossy().into_owned(), "@vtsls/language-server".into()]),
            }),
        },
        "python" => {
            let venv = root.join(".venv/bin");
            for n in ["basedpyright-langserver", "pyright-langserver"] {
                if venv.join(n).is_file() {
                    return Ok((n.into(), venv.join(n), vec!["--stdio".into()]));
                }
            }
            match bins(&["basedpyright-langserver", "pyright-langserver"]) {
                Some(p) => Ok((p.file_name().unwrap().to_string_lossy().into_owned(), p, vec!["--stdio".into()])),
                None => Err(Missing {
                    family: "python".into(),
                    hint: "Python needs a language server: basedpyright, installed into ~/.pitlane/lsp.".into(),
                    install: Some(vec!["npm".into(), "install".into(), "--prefix".into(), own_dir(pitlane_home).to_string_lossy().into_owned(), "basedpyright".into()]),
                }),
            }
        }
        "rust" => {
            // rustup's proxy exists even when the component does not: ask it.
            let ok = on_path("rust-analyzer").filter(|p| Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()));
            match ok {
                Some(p) => Ok(("rust-analyzer".into(), p, vec![])),
                None => Err(Missing {
                    family: "rust".into(),
                    hint: "Rust needs rust-analyzer, a rustup component.".into(),
                    install: on_path("rustup").map(|_| vec!["rustup".into(), "component".into(), "add".into(), "rust-analyzer".into()]),
                }),
            }
        }
        other => Err(Missing { family: other.into(), hint: format!("No language server for {other}."), install: None }),
    }
}

impl Lsps {
    /// The server for (root, family), started if it is not running.
    pub fn start(&self, sink: Sink, root: &Path, family: &str, program: &Path, args: &[String], name: &str) -> Result<Started> {
        let root_s = root.to_string_lossy().into_owned();
        {
            let mut all = self.servers.lock().unwrap();
            if let Some((id, s)) = all.iter_mut().find(|(_, s)| s.root == root_s && s.family == family) {
                s.used = Instant::now();
                return Ok(Started { id: *id, name: name.into() });
            }
            while all.len() >= CAP {
                let oldest = *all.iter().min_by_key(|(_, s)| s.used).map(|(id, _)| id).unwrap();
                if let Some(mut s) = all.remove(&oldest) {
                    let _ = s.child.kill();
                }
            }
        }
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("could not start {}", program.display()))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let servers = self.servers.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Ok(Some(body)) = read_message(&mut r) {
                sink(Out::Msg(id, body));
            }
            servers.lock().unwrap().remove(&id);
            sink(Out::Exit(id));
        });
        self.servers.lock().unwrap().insert(id, Server { root: root_s, family: family.into(), child, stdin, used: Instant::now() });
        Ok(Started { id, name: name.into() })
    }

    pub fn send(&self, id: u32, msg: &str) -> Result<()> {
        let mut all = self.servers.lock().unwrap();
        let Some(s) = all.get_mut(&id) else { bail!("the language server stopped") };
        s.used = Instant::now();
        write!(s.stdin, "Content-Length: {}\r\n\r\n{}", msg.len(), msg)?;
        s.stdin.flush()?;
        Ok(())
    }

    pub fn close_all(&self) {
        for (_, mut s) in self.servers.lock().unwrap().drain() {
            let _ = s.child.kill();
        }
    }
}

/// One LSP message: headers, a blank line, then Content-Length bytes. None at the end.
fn read_message(r: &mut impl BufRead) -> Result<Option<String>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok();
        }
    }
    let len = len.context("a message without Content-Length")?;
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages() {
        let raw = "Content-Length: 7\r\n\r\n{\"a\":1}Content-Type: x\r\nContent-Length: 2\r\n\r\n{}";
        let mut r = BufReader::new(raw.as_bytes());
        assert_eq!(read_message(&mut r).unwrap().as_deref(), Some("{\"a\":1}"));
        assert_eq!(read_message(&mut r).unwrap().as_deref(), Some("{}"));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn relays_a_server_both_ways() {
        // A stand-in server: answers each framed message with the same body, framed.
        let script = r#"import sys
while True:
    n = None
    while True:
        l = sys.stdin.buffer.readline()
        if not l: sys.exit(0)
        l = l.strip()
        if not l: break
        if l.lower().startswith(b"content-length:"): n = int(l.split(b":")[1])
    body = sys.stdin.buffer.read(n)
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
"#;
        let dir = std::env::temp_dir();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |o| {
            if let Out::Msg(_, m) = o {
                let _ = tx.lock().unwrap().send(m);
            }
        });
        let l = Lsps::default();
        let s = l.start(sink.clone(), &dir, "ts", Path::new("/usr/bin/python3"), &["-c".into(), script.into()], "echo").unwrap();
        let again = l.start(sink, &dir, "ts", Path::new("/usr/bin/python3"), &["-c".into(), script.into()], "echo").unwrap();
        assert_eq!(s.id, again.id, "one server per folder and language");
        l.send(s.id, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"x":"é"}}"#).unwrap();
        let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(got.contains("\"é\""), "{got}");
        l.close_all();
    }

    #[test]
    fn says_what_is_missing_and_how_to_get_it() {
        let empty = std::env::temp_dir().join("pit-lsp-none");
        let _ = std::fs::create_dir_all(&empty);
        assert_eq!(family("typescriptreact"), Some("ts"));
        assert_eq!(family("markdown"), None);
        if on_path("vtsls").is_none() && on_path("typescript-language-server").is_none() {
            let m = find(&empty, "ts", &empty).unwrap_err();
            assert!(m.install.unwrap().contains(&"@vtsls/language-server".to_string()));
        }
    }
}
