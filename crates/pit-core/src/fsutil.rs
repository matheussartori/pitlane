//! Writing Pitlane's own files: whole or not at all, and TOML edits that keep the file's
//! comments and are checked before anything is written.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

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
/// it, whole. Edits made in this process go one at a time, so two never drop each other's change.
pub fn edit_toml<T>(
    path: &Path,
    template: &str,
    change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<T>,
    check: impl FnOnce(&str) -> Result<()>,
) -> Result<T> {
    static EDITING: Mutex<()> = Mutex::new(());
    let _one = EDITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = read_optional(path)?.unwrap_or_else(|| template.to_owned());
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("invalid {}", path.display()))?;
    let out = change(&mut doc)?;
    let text = doc.to_string();
    check(&text).with_context(|| format!("{} would no longer be valid", path.display()))?;
    write_atomic(path, text)?;
    Ok(out)
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
}
