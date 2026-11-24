//! Writing Pitlane's own files: whole or not at all, and TOML edits that keep the file's
//! comments and are checked before anything is written.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Writes `bytes` to `path` whole or not at all: a temp file beside it (its own name for each
/// write, so two writers never rename each other's half-written file), then renamed over the old
/// one. A reader sees the old file or the new one; a process that dies mid-write leaves the old.
/// No fsync: on macOS it is a full flush of the disk's cache, tens of milliseconds a write, and
/// cards are written all the time; these files are Pitlane's state, not its only copy of work.
pub fn write_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let name = path.file_name().with_context(|| format!("{} has no file name", path.display()))?;
    // Hidden and with no known extension: listings of cards, runs or reports never pick it up.
    let tmp = dir.join(format!(
        ".{}.tmp{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes.as_ref())?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("could not write {}", path.display()))
}

/// A key as a folder or file name: letters, digits, `.` and `_` kept, every run of anything else
/// one `-`, none at the ends. Readable, not unique: pair it with `short_hash`.
pub fn flat_name(key: &str) -> String {
    let mut slug = String::new();
    for c in key.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    slug.trim_matches(|c| c == '-' || c == '.').to_owned()
}

/// Eight hex digits of a stable hash (FNV-1a), the same in every build: names on disk use it.
pub fn short_hash(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", h >> 32)
}

/// Reads a whole text file, `None` only when there is none. Any other failure (permissions, bad
/// UTF-8) is an error: treating it as "no file" would have the next write replace what is there.
pub fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("could not read {}", path.display())),
    }
}

/// Edits the TOML file at `path` keeping its comments and layout: reads it (`template` when there
/// is none), lets `change` edit the document, has `check` read the result, and only then writes
/// it, whole. Edits go one at a time, in this process and across them (the app, the daemon and the
/// CLI all edit `flow.toml`), so two never drop each other's change. The lock that says so lives
/// in the temporary folder, named after the file: nothing is written beside a file in a repository.
pub fn edit_toml<T>(
    path: &Path,
    template: &str,
    change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<T>,
    check: impl FnOnce(&str) -> Result<()>,
) -> Result<T> {
    static EDITING: Mutex<()> = Mutex::new(());
    let _one = EDITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _others = FileLock::take(&edit_lock(path), Duration::from_secs(5))?;
    let text = read_optional(path)?.unwrap_or_else(|| template.to_owned());
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("invalid {}", path.display()))?;
    let out = change(&mut doc)?;
    let text = doc.to_string();
    check(&text).with_context(|| format!("{} would no longer be valid", path.display()))?;
    write_atomic(path, text)?;
    Ok(out)
}

/// Where the lock for editing `path` lives: one file per edited file, by a hash of its path.
fn edit_lock(path: &Path) -> std::path::PathBuf {
    let full = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // FNV-1a, spelled out: every pit binary must name the same lock, whatever built it.
    let hash = full.as_os_str().as_encoded_bytes().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    std::env::temp_dir().join(format!("pitlane-edit-{hash:016x}.lock"))
}

/// Cross-process lock: an OS lock (`flock`) on a lock file, held while the value lives. The OS
/// drops it when the holder ends, crash included, so there is no stale lock to guess about and
/// nobody can take a lock that is still held. `flock` locks are per open file, so two holders in
/// one process exclude each other too.
pub struct FileLock(#[allow(dead_code)] std::fs::File);

impl FileLock {
    /// Takes the lock at `path` (creating the file and its folder), waiting up to `wait` for
    /// whoever holds it.
    pub fn take(path: &Path, wait: Duration) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("could not open {}", path.display()))?;
        let until = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
                Err(std::fs::TryLockError::WouldBlock) => bail!("{} is held by another pit process; try again", path.display()),
                Err(std::fs::TryLockError::Error(e)) => return Err(e).with_context(|| format!("could not lock {}", path.display())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_and_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/x.json");
        write_atomic(&p, "one").unwrap();
        write_atomic(&p, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_check_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.toml");
        std::fs::write(&p, "# mine\na = 1\n").unwrap();
        let r = edit_toml(&p, "", |doc| {
            doc["a"] = toml_edit::value("x");
            Ok(())
        }, |_| anyhow::bail!("no"));
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# mine\na = 1\n");
        edit_toml(&p, "", |doc| {
            doc["a"] = toml_edit::value(2);
            Ok(())
        }, |_| Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# mine\na = 2\n");
    }

    #[test]
    fn a_held_lock_keeps_others_out_until_dropped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x/.lock");
        // A wait on the takes that must succeed: a process another test forks holds a copy of
        // the descriptor until it execs, and with it the lock.
        let held = FileLock::take(&p, Duration::from_secs(2)).unwrap();
        assert!(FileLock::take(&p, Duration::from_millis(50)).is_err());
        drop(held);
        FileLock::take(&p, Duration::from_secs(2)).unwrap();
    }
}
