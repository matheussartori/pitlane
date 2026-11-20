//! VS Code in the Code tab: code-server (open source, Coder), downloaded once on the person's
//! click into ~/.pitlane/code-server, run as one local server for the app, shown in a webview of
//! its own over the Code tab. The server listens on 127.0.0.1 only, behind a password made fresh
//! at each start that the webview types in for you; telemetry and update checks are off.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Default)]
pub struct Vscode {
    running: Mutex<Option<Running>>,
    pub progress: Mutex<Progress>,
}

/// Where an install is: "download" (bytes so far of the total), "unpack", or "" when idle.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub version: String,
}

#[derive(Clone)]
pub struct Running {
    pub port: u16,
    pub password: String,
    pid: u32,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub installed: bool,
    pub version: Option<String>,
    pub running: bool,
}

pub fn dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("code-server")
}

fn data(pitlane_home: &Path) -> PathBuf {
    dir(pitlane_home).join("data")
}

/// The code-server binary: Pitlane's own copy, or one already on the machine.
pub fn find(pitlane_home: &Path) -> Option<PathBuf> {
    let own = dir(pitlane_home).join("current/bin/code-server");
    if own.is_file() {
        return Some(own);
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("code-server")).find(|f| f.is_file()))
}

fn version(pitlane_home: &Path) -> Option<String> {
    std::fs::read_to_string(dir(pitlane_home).join("current/VERSION")).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

pub fn status(v: &Vscode, pitlane_home: &Path) -> Status {
    Status { installed: find(pitlane_home).is_some(), version: version(pitlane_home), running: v.running.lock().unwrap().is_some() }
}

/// The latest release for this Mac, from GitHub: (version, tarball URL, size in bytes).
fn latest() -> Result<(String, String, u64)> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => bail!("no code-server build for {other}"),
    };
    let out = Command::new("curl")
        .args(["-fsSL", "-H", "Accept: application/vnd.github+json", "https://api.github.com/repos/coder/code-server/releases/latest"])
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!("could not reach GitHub: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let tag = v["tag_name"].as_str().context("no release")?.trim_start_matches('v').to_owned();
    let want = format!("macos-{arch}.tar.gz");
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["browser_download_url"].as_str().is_some_and(|u| u.ends_with(&want))))
        .with_context(|| format!("release {tag} has no {want}"))?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default().to_owned();
    Ok((tag, url, asset["size"].as_u64().unwrap_or(0)))
}

/// Downloads and unpacks code-server; the person clicked for it.
/// Progress is the size of the file so far against the release's own size, read as it grows.
pub fn install(pitlane_home: &Path, progress: &Mutex<Progress>) -> Result<String> {
    let set = |phase: &str, done: u64, total: u64, version: &str| *progress.lock().unwrap() = Progress { phase: phase.into(), done, total, version: version.into() };
    set("download", 0, 0, "");
    let r = install_steps(pitlane_home, &set);
    set("", 0, 0, "");
    r
}

fn install_steps(pitlane_home: &Path, set: &dyn Fn(&str, u64, u64, &str)) -> Result<String> {
    let (ver, url, size) = latest()?;
    let root = dir(pitlane_home);
    std::fs::create_dir_all(&root)?;
    let tarball = root.join(format!("code-server-{ver}.tar.gz"));
    set("download", 0, size, &ver);
    let mut curl = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&tarball)
        .arg(&url)
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run curl")?;
    let status = loop {
        if let Some(st) = curl.try_wait()? {
            break st;
        }
        let done = std::fs::metadata(&tarball).map(|m| m.len()).unwrap_or(0);
        set("download", done, size, &ver);
        std::thread::sleep(Duration::from_millis(250));
    };
    if !status.success() {
        let _ = std::fs::remove_file(&tarball);
        bail!("the download of code-server {ver} failed");
    }
    set("unpack", size, size, &ver);
    let unpack = root.join(format!("v{ver}"));
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack)?;
    let ok = Command::new("tar").args(["-xzf"]).arg(&tarball).arg("-C").arg(&unpack).arg("--strip-components=1").status()?;
    let _ = std::fs::remove_file(&tarball);
    if !ok.success() || !unpack.join("bin/code-server").is_file() {
        bail!("code-server {ver} did not unpack");
    }
    std::fs::write(unpack.join("VERSION"), &ver)?;
    let current = root.join("current");
    let _ = std::fs::remove_file(&current);
    let _ = std::fs::remove_dir_all(&current);
    std::os::unix::fs::symlink(&unpack, &current)?;
    Ok(ver)
}

fn password() -> String {
    let mut b = [0u8; 24];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Settings Pitlane starts VS Code with: calm, no telemetry, no tour. Written once; yours after.
fn first_settings(pitlane_home: &Path) -> Result<()> {
    let user = data(pitlane_home).join("User");
    let file = user.join("settings.json");
    if file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&user)?;
    let s = serde_json::json!({
        "telemetry.telemetryLevel": "off",
        "workbench.startupEditor": "none",
        "workbench.tips.enabled": false,
        "workbench.layoutControl.enabled": false,
        "window.commandCenter": false,
        "editor.minimap.enabled": false,
        "workbench.colorTheme": "Default Dark Modern",
        "window.autoDetectColorScheme": true,
        "workbench.preferredDarkColorTheme": "Default Dark Modern",
        "workbench.preferredLightColorTheme": "Default Light Modern",
        "security.workspace.trust.enabled": false,
        "chat.disableAIFeatures": true,
        "extensions.ignoreRecommendations": true,
        "update.mode": "none"
    });
    std::fs::File::create(file)?.write_all(serde_json::to_string_pretty(&s)?.as_bytes())?;
    Ok(())
}

impl Vscode {
    /// The server, started if it is not running.
    pub fn start(&self, pitlane_home: &Path) -> Result<Running> {
        let mut slot = self.running.lock().unwrap();
        if let Some(r) = slot.as_ref() {
            if TcpStream::connect(("127.0.0.1", r.port)).is_ok() {
                return Ok(r.clone());
            }
        }
        let bin = find(pitlane_home).context("code-server is not installed")?;
        first_settings(pitlane_home)?;
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let pw = password();
        let log = std::fs::File::create(dir(pitlane_home).join("server.log"))?;
        let child: Child = Command::new(&bin)
            .arg("--bind-addr")
            .arg(format!("127.0.0.1:{port}"))
            .args(["--auth", "password", "--disable-telemetry", "--disable-update-check", "--disable-workspace-trust", "--disable-getting-started-override"])
            .arg("--user-data-dir")
            .arg(data(pitlane_home))
            .arg("--extensions-dir")
            .arg(dir(pitlane_home).join("extensions"))
            .env("PASSWORD", &pw)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .with_context(|| format!("could not start {}", bin.display()))?;
        let pid = child.id();
        // Reap it when it ends, so it never lingers as a zombie.
        std::thread::spawn(move || {
            let mut c = child;
            let _ = c.wait();
        });
        let t = Instant::now();
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if t.elapsed() > Duration::from_secs(30) {
                bail!("code-server did not start; see {}", dir(pitlane_home).join("server.log").display());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let r = Running { port, password: pw, pid };
        *slot = Some(r.clone());
        Ok(r)
    }

    /// Opens a file at a line in the running VS Code (code-server's own "open in this window").
    pub fn open_file(&self, pitlane_home: &Path, file: &Path, line: Option<u32>, col: Option<u32>) -> Result<()> {
        let bin = find(pitlane_home).context("code-server is not installed")?;
        let target = match (line, col) {
            (Some(l), Some(c)) => format!("{}:{l}:{c}", file.display()),
            (Some(l), None) => format!("{}:{l}", file.display()),
            _ => file.display().to_string(),
        };
        let out = Command::new(bin).arg("--user-data-dir").arg(data(pitlane_home)).args(["--reuse-window", "--goto"]).arg(target).output()?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    pub fn current(&self) -> Option<Running> {
        self.running.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        if let Some(r) = self.running.lock().unwrap().take() {
            let _ = Command::new("kill").arg(r.pid.to_string()).status();
        }
    }
}

/// The page that types the password into code-server's login form, once, in its own webview.
pub fn login_script(password: &str) -> String {
    format!(
        r#"(function(){{var p=document.querySelector('input[type=password]');if(!p)return;p.value={pw};var f=p.form||document.querySelector('form');if(f)f.submit();}})();"#,
        pw = serde_json::to_string(password).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_it_is_missing_and_never_runs_without_it() {
        let home = std::env::temp_dir().join(format!("pit-vscode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        if find(&home).is_none() {
            let v = Vscode::default();
            assert!(!status(&v, &home).installed);
            assert!(v.start(&home).is_err());
        }
        let s = login_script("ab\"c");
        assert!(s.contains(r#""ab\"c""#), "{s}");
        assert_eq!(password().len(), 48);
    }
}
