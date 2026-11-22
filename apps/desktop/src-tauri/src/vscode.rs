//! VS Code in the Code tab: code-server (open source, Coder, MIT), bundled with Pitlane, run as one
//! local server for the app, shown in a webview of its own over the Code tab. Settings and
//! extensions live in ~/.pitlane/code-server. The server listens on 127.0.0.1 only, behind a password made fresh
//! at each start that the webview types in for you; telemetry and update checks are off.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Where code-server listens when it can (then the next nine ports, then any).
const STABLE_PORT: u16 = 47615;

#[derive(Default)]
pub struct Vscode {
    running: Mutex<Option<Running>>,
}


#[derive(Clone)]
pub struct Running {
    pub port: u16,
    pub password: String,
    pid: u32,
}

pub fn dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("code-server")
}

fn data(pitlane_home: &Path) -> PathBuf {
    dir(pitlane_home).join("data")
}

/// The code-server binary. The build carries it as one archive (`code-server.tar.gz` and its
/// `code-server.version` among the app's resources); the first use unpacks it into
/// ~/.pitlane/code-server/<version>, and older versions there go. Without the archive (a dev
/// checkout that did not fetch it), one on the PATH.
pub fn ensure(resources: Option<&Path>, pitlane_home: &Path) -> Result<PathBuf> {
    let bundled = resources.and_then(|r| {
        let version = std::fs::read_to_string(r.join("code-server.version")).ok()?.trim().to_owned();
        let archive = r.join("code-server.tar.gz");
        (!version.is_empty() && archive.is_file()).then_some((version, archive))
    });
    let Some((version, archive)) = bundled else {
        return std::env::var_os("PATH")
            .and_then(|p| std::env::split_paths(&p).map(|d| d.join("code-server")).find(|f| f.is_file()))
            .context("code-server is missing from this build");
    };
    let root = dir(pitlane_home);
    let target = root.join(&version);
    let bin = target.join("bin/code-server");
    if bin.is_file() {
        return Ok(bin);
    }
    std::fs::create_dir_all(&root)?;
    let part = root.join(format!("{version}.part"));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part)?;
    let ok = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&part).arg("--strip-components=1").status().context("could not run tar")?;
    if !ok.success() || !part.join("bin/code-server").is_file() {
        let _ = std::fs::remove_dir_all(&part);
        bail!("the bundled code-server did not unpack");
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&part, &target)?;
    // Older versions go; settings and extensions stay.
    for e in std::fs::read_dir(&root)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name != version && name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(bin)
}

/// Pitlane's colours, under VS Code's own dark and light themes, merged into the settings. A
/// settings file Pitlane cannot read (comments, a typo) is left alone.
pub fn write_theme(pitlane_home: &Path, dark: serde_json::Map<String, serde_json::Value>, light: serde_json::Map<String, serde_json::Value>, mode: &str) -> Result<()> {
    first_settings(pitlane_home)?;
    let file = data(pitlane_home).join("User/settings.json");
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&file)?) else { return Ok(()) };
    let Some(obj) = v.as_object_mut() else { return Ok(()) };
    look(obj);
    // Pitlane's appearance: VS Code follows the system, or holds the scheme Pitlane holds.
    match mode {
        "light" | "dark" => {
            obj.insert("window.autoDetectColorScheme".into(), false.into());
            obj.insert("workbench.colorTheme".into(), (if mode == "light" { "Light Modern" } else { "Dark Modern" }).into());
        }
        _ => {
            obj.insert("window.autoDetectColorScheme".into(), true.into());
        }
    }
    let custom = obj.entry("workbench.colorCustomizations").or_insert_with(|| serde_json::json!({}));
    if let Some(c) = custom.as_object_mut() {
        c.remove("[Default Dark Modern]");
        c.remove("[Default Light Modern]");
        c.insert("[Dark Modern]".into(), serde_json::Value::Object(dark));
        c.insert("[Light Modern]".into(), serde_json::Value::Object(light));
    }
    std::fs::write(&file, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

fn password() -> String {
    let mut b = [0u8; 24];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Settings Pitlane starts VS Code with: no telemetry, no tour, no AI. Written once; yours after.
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
        "window.commandCenter": true,
        "workbench.secondarySideBar.defaultVisibility": "hidden",
        "workbench.editor.enablePreview": false,
        "workbench.welcomePage.walkthroughs.openOnInstall": false,
        "git.openRepositoryInParentFolders": "always",
        "editor.lineHeight": 20,
        "editor.minimap.enabled": false,
        "window.autoDetectColorScheme": true,
        "security.workspace.trust.enabled": false,
        "chat.disableAIFeatures": true,
        "extensions.ignoreRecommendations": true,
        "update.mode": "none"
    });
    std::fs::File::create(file)?.write_all(serde_json::to_string_pretty(&s)?.as_bytes())?;
    Ok(())
}

/// Pitlane's layout for VS Code, in the spirit of Cursor's: no title bar (the activity bar on the
/// side, no menu), roomy tab chips, thin icons (Fluent for the workbench, Symbols for files), Geist
/// Mono. A key is set when it is missing or still holds an earlier Pitlane default; a value you
/// chose stays.
fn look(obj: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::{Value, json};
    let mono = "'Geist Mono', Menlo, monospace";
    let keys: Vec<(&str, Value, Vec<Value>)> = vec![
        // Cursor's layout: the views as a row of icons atop the side bar, one title bar with search.
        ("workbench.activityBar.location", json!("top"), vec![json!("default")]),
        ("window.commandCenter", json!(true), vec![json!(false)]),
        ("window.menuBarVisibility", json!("hidden"), vec![json!("compact")]),
        ("window.title", json!("${activeEditorShort}"), vec![]),
        ("window.density.editorTabHeight", json!("default"), vec![json!("compact")]), // 35px bar: the tab chips get room to read
        ("workbench.activity.showAccounts", json!(true), vec![json!(false)]),
        // Flat, so Pitlane's own shapes apply (the modern UI floats rounded panels on a backdrop).
        ("workbench.experimental.modernUI", json!(false), vec![]),
        ("workbench.productIconTheme", json!("fluent-icons"), vec![]),
        ("workbench.iconTheme", json!("symbols"), vec![]),
        ("workbench.tree.indent", json!(12), vec![]),
        ("workbench.tree.renderIndentGuides", json!("none"), vec![]),
        ("editor.fontFamily", json!(mono), vec![json!("'SF Mono', Menlo, monospace")]),
        ("terminal.integrated.fontFamily", json!(mono), vec![json!("'SF Mono', Menlo, monospace")]),
        ("editor.fontSize", json!(12.5), vec![json!(13)]),
        ("editor.overviewRulerBorder", json!(false), vec![]),
        ("editor.hideCursorInOverviewRuler", json!(true), vec![]),
        ("editor.guides.bracketPairs", json!(false), vec![]),
        ("editor.bracketPairColorization.enabled", json!(false), vec![]),
        ("workbench.colorTheme", json!("Dark Modern"), vec![json!("Default Dark Modern")]),
        ("workbench.preferredDarkColorTheme", json!("Dark Modern"), vec![json!("Default Dark Modern")]),
        ("workbench.preferredLightColorTheme", json!("Light Modern"), vec![json!("Default Light Modern")]),
    ];
    for (k, v, earlier) in keys {
        match obj.get(k) {
            Some(cur) if !earlier.contains(cur) => {}
            _ => {
                obj.insert(k.to_owned(), v);
            }
        }
    }
}

impl Vscode {
    /// The server, started if it is not running.
    pub fn start(&self, bin: &Path, resources: Option<&Path>, pitlane_home: &Path) -> Result<Running> {
        let mut slot = self.running.lock().unwrap();
        if let Some(r) = slot.as_ref() {
            if TcpStream::connect(("127.0.0.1", r.port)).is_ok() {
                return Ok(r.clone());
            }
        }
        first_settings(pitlane_home)?;
        std::fs::create_dir_all(dir(pitlane_home))?;
        install_bundled(bin, resources, pitlane_home);
        // A server left by an app that did not get to close it: it goes.
        let pidfile = dir(pitlane_home).join("server.pid");
        if let Some(old) = std::fs::read_to_string(&pidfile).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
            let cmd = Command::new("ps").args(["-o", "command=", "-p", &old.to_string()]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            if cmd.contains("code-server") && cmd.contains(&dir(pitlane_home).to_string_lossy().into_owned()) {
                let _ = Command::new("kill").arg(old.to_string()).status();
                // Let it let go of its port, so this one takes the same.
                let t = Instant::now();
                while TcpListener::bind(("127.0.0.1", STABLE_PORT)).is_err() && t.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // The same port each time when it is free: the page keeps one origin, so VS Code's own cache
        // (its theme, among others) survives a restart and the first frame is already right.
        let port = (STABLE_PORT..STABLE_PORT + 10)
            .find(|p| TcpListener::bind(("127.0.0.1", *p)).is_ok())
            .map_or_else(|| TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()), Ok)?;
        let pw = password();
        let log = std::fs::File::create(dir(pitlane_home).join("server.log"))?;
        let child: Child = Command::new(bin)
            // Its config file lives with Pitlane's, not in ~/.config.
            .arg("--config")
            .arg(dir(pitlane_home).join("config.yaml"))
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
        let _ = std::fs::write(&pidfile, pid.to_string());
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
    pub fn open_file(&self, bin: &Path, pitlane_home: &Path, file: &Path, line: Option<u32>, col: Option<u32>) -> Result<()> {
        let target = match (line, col) {
            (Some(l), Some(c)) => format!("{}:{l}:{c}", file.display()),
            (Some(l), None) => format!("{}:{l}", file.display()),
            _ => file.display().to_string(),
        };
        // code-server has no --goto: `path:line:col` with --reuse-window opens at that line.
        let out = Command::new(bin).arg("--config").arg(dir(pitlane_home).join("config.yaml")).arg("--user-data-dir").arg(data(pitlane_home)).arg("--reuse-window").arg(target).output()?;
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

/// The extensions that come with Pitlane (its icons), installed offline the first time.
fn install_bundled(bin: &Path, resources: Option<&Path>, pitlane_home: &Path) {
    let Some(dir_in) = resources.map(|r| r.join("vscode-extensions")) else { return };
    let Ok(entries) = std::fs::read_dir(&dir_in) else { return };
    let exts = dir(pitlane_home).join("extensions");
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".vsix") else { continue };
        if exts.join(id).is_dir() {
            continue;
        }
        let _ = Command::new(bin)
            .arg("--config")
            .arg(dir(pitlane_home).join("config.yaml"))
            .arg("--user-data-dir")
            .arg(data(pitlane_home))
            .arg("--extensions-dir")
            .arg(&exts)
            .arg("--install-extension")
            .arg(e.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

const LOOK_CSS: &str = include_str!("vscode-look.css");
const GEIST: &[u8] = include_bytes!("../../node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2");
const GEIST_MONO: &[u8] = include_bytes!("../../node_modules/@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2");

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

const LOOK_JS: &str = include_str!("vscode-look.js");
const PLACE_JS: &str = include_str!("vscode-place.js");

/// Pitlane's type and shapes on VS Code's page (vscode-look.css, Geist and Geist Mono), and the
/// folder picker in its side bar, given the places it offers. Run before VS Code draws.
pub fn look_script(places: &serde_json::Value) -> String {
    let fonts = serde_json::json!([["Geist", base64(GEIST)], ["Geist Mono", base64(GEIST_MONO)]]);
    let look = LOOK_JS.replace("__CSS__", &serde_json::to_string(LOOK_CSS).unwrap_or_default()).replace("__FONTS__", &fonts.to_string());
    format!("window.__pitlanePlaces={};\n{look}\n{PLACE_JS}", places)
}

/// Updates the places the picker offers, in a page already loaded.
pub fn places_script(places: &serde_json::Value) -> String {
    format!("window.__pitlanePlaces={places};if(window.__pitlaneSetPlaces)window.__pitlaneSetPlaces(window.__pitlanePlaces);")
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
    fn unpacks_the_bundled_archive_once() {
        let base = std::env::temp_dir().join(format!("pit-vscode-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/code-server-9.9.9/bin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("code-server"), "#!/bin/sh\n").unwrap();
        let res = base.join("res");
        std::fs::create_dir_all(&res).unwrap();
        assert!(Command::new("tar").arg("-czf").arg(res.join("code-server.tar.gz")).arg("-C").arg(base.join("src")).arg("code-server-9.9.9").status().unwrap().success());
        std::fs::write(res.join("code-server.version"), "9.9.9-macos-arm64\n").unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(dir(&home).join("1.0.0-macos-arm64")).unwrap();
        std::fs::create_dir_all(data(&home)).unwrap();
        let bin = ensure(Some(&res), &home).unwrap();
        assert!(bin.ends_with("9.9.9-macos-arm64/bin/code-server"));
        assert!(!dir(&home).join("1.0.0-macos-arm64").exists(), "the older version goes");
        assert!(data(&home).exists(), "settings stay");
        assert_eq!(ensure(Some(&res), &home).unwrap(), bin);
    }

    #[test]
    fn writes_the_colours_without_losing_the_person_s_settings() {
        let home = std::env::temp_dir().join(format!("pit-vscode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut dark = serde_json::Map::new();
        dark.insert("editor.background".into(), "#111111".into());
        write_theme(&home, dark.clone(), serde_json::Map::new(), "system").unwrap();
        let file = data(&home).join("User/settings.json");
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["telemetry.telemetryLevel"], "off");
        v["editor.fontSize"] = 16.into();
        std::fs::write(&file, v.to_string()).unwrap();
        dark.insert("editor.background".into(), "#222222".into());
        write_theme(&home, dark, serde_json::Map::new(), "system").unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["editor.fontSize"], 16, "your own settings stay");
        assert_eq!(v["workbench.colorCustomizations"]["[Dark Modern]"]["editor.background"], "#222222");
        assert_eq!(v["workbench.iconTheme"], "symbols");
        std::fs::write(&file, "// a comment\n{}").unwrap();
        write_theme(&home, serde_json::Map::new(), serde_json::Map::new(), "system").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("// a comment"), "a file it cannot read is left alone");
    }

    #[test]
    fn encodes_base64_and_keeps_the_person_s_choices() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let mut o = serde_json::Map::new();
        o.insert("workbench.iconTheme".into(), "material-icon-theme".into());
        o.insert("editor.fontFamily".into(), "'SF Mono', Menlo, monospace".into());
        look(&mut o);
        assert_eq!(o["workbench.iconTheme"], "material-icon-theme", "a choice stays");
        assert_eq!(o["editor.fontFamily"], "'Geist Mono', Menlo, monospace", "an earlier Pitlane default moves on");
        assert!(look_script(&serde_json::json!([])).contains("adoptedStyleSheets"));
    }

    #[test]
    fn types_the_password_safely() {
        let s = login_script("ab\"c");
        assert!(s.contains(r#""ab\"c""#), "{s}");
        assert_eq!(password().len(), 48);
    }
}
