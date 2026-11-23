//! The PATH agents and git get: the login shell's, plus Homebrew and the usual tool folders.

use std::path::Path;

use pit_core::default_homes;

/// How long the login shell has to say its PATH.
const LOGIN_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
/// How often the login shell is checked for having finished.
const LOGIN_SHELL_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// The PATH your login shell sets up (nvm, asdf, Homebrew…): an app opened from the Finder only
/// gets the system's, and agents need your tools. At most a few seconds; None when it fails.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    use std::os::unix::process::CommandExt as _;
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", "printf '__PITLANE_PATH__%s__END__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // Its own group, so what an rc file starts goes with it on a timeout.
        .process_group(0)
        .spawn()
        .ok()?;
    // Read as it comes: an rc file that prints a lot would otherwise fill the pipe and stall.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).ok().map(|_| out)
    });
    let t = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t.elapsed() < LOGIN_SHELL_TIMEOUT => std::thread::sleep(LOGIN_SHELL_POLL),
            _ => {
                pit_core::procs::end_group(child.id(), std::time::Duration::ZERO);
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = reader.join().ok()??;
    let start = out.find("__PITLANE_PATH__")? + "__PITLANE_PATH__".len();
    let end = out[start..].find("__END__")? + start;
    Some(out[start..end].to_owned()).filter(|p| !p.is_empty())
}

/// Apps opened from Finder get a bare PATH; git, gh and setup commands live in Homebrew.
pub(crate) fn extend_path() {
    let current = login_shell_path().or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
    let mut parts: Vec<String> = current.split(':').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    let cargo = default_homes().0.join(".cargo/bin").to_string_lossy().into_owned();
    let local = default_homes().0.join(".local/bin").to_string_lossy().into_owned();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", local.as_str(), cargo.as_str()] {
        if !parts.iter().any(|p| p == extra) && Path::new(extra).exists() {
            parts.insert(0, extra.to_owned());
        }
    }
    // SAFETY: called first thing in `run`, before Tauri or any other thread starts.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}
