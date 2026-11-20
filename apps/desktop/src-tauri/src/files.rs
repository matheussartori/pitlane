//! Files for the editor: list, read and save inside one root (the repository or a lane's
//! worktree). Every path is resolved and must stay under the root; nothing outside is touched.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

const MAX_READ: u64 = 4 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

#[derive(Debug, Serialize, PartialEq)]
pub struct FileText {
    pub path: String,
    pub text: String,
    /// Not shown: binary, or larger than the editor should hold.
    pub refused: Option<String>,
}

/// Tracked and untracked files that git does not ignore, sorted.
pub fn list(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("{} is not a git checkout: {}", root.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut files: Vec<String> = out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
    files.sort();
    files.dedup();
    files.truncate(MAX_FILES);
    Ok(files)
}

/// `rel` under `root`, refusing anything that would leave it (`..`, absolute paths, symlinks out).
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        bail!("{rel} is not a path inside the folder");
    }
    let root = root.canonicalize().with_context(|| format!("no folder {}", root.display()))?;
    let full = root.join(rel_path);
    // The deepest existing ancestor must resolve inside the root (a symlink could point out).
    let mut probe = full.as_path();
    while !probe.exists() {
        probe = probe.parent().context("bad path")?;
    }
    if !probe.canonicalize()?.starts_with(&root) {
        bail!("{rel} points outside the folder");
    }
    Ok(full)
}

pub fn read(root: &Path, rel: &str) -> Result<FileText> {
    let full = inside(root, rel)?;
    let meta = std::fs::metadata(&full).with_context(|| format!("no file {rel}"))?;
    if meta.len() > MAX_READ {
        return Ok(FileText { path: rel.to_owned(), text: String::new(), refused: Some(format!("{} MB is more than the editor holds", meta.len() / 1_048_576)) });
    }
    let bytes = std::fs::read(&full)?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Ok(FileText { path: rel.to_owned(), text: String::new(), refused: Some("a binary file".into()) });
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileText { path: rel.to_owned(), text, refused: None }),
        Err(_) => Ok(FileText { path: rel.to_owned(), text: String::new(), refused: Some("not UTF-8 text".into()) }),
    }
}

/// Saves atomically (a temporary file beside it, then a rename), keeping the file's permissions.
pub fn write(root: &Path, rel: &str, text: &str) -> Result<()> {
    let full = inside(root, rel)?;
    let dir = full.parent().context("bad path")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.pitlane-save", full.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    std::fs::write(&tmp, text)?;
    if let Ok(meta) = std::fs::metadata(&full) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &full).with_context(|| format!("could not save {rel}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pit-files-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        Command::new("git").arg("-C").arg(&d).args(["init", "-q"]).status().unwrap();
        std::fs::write(d.join("src/a.ts"), "export const a = 1;\n").unwrap();
        std::fs::write(d.join(".gitignore"), "dist/\n").unwrap();
        std::fs::create_dir_all(d.join("dist")).unwrap();
        std::fs::write(d.join("dist/out.js"), "x").unwrap();
        std::fs::write(d.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 0, 1]).unwrap();
        d
    }

    #[test]
    fn lists_reads_and_saves_inside_the_root_only() {
        let d = repo();
        assert_eq!(list(&d).unwrap(), [".gitignore", "logo.png", "src/a.ts"]);
        assert_eq!(read(&d, "src/a.ts").unwrap().text, "export const a = 1;\n");
        assert_eq!(read(&d, "logo.png").unwrap().refused.as_deref(), Some("a binary file"));
        write(&d, "src/a.ts", "export const a = 2;\n").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("src/a.ts")).unwrap(), "export const a = 2;\n");
        write(&d, "src/new/b.ts", "b").unwrap();
        assert!(d.join("src/new/b.ts").exists());
        for bad in ["../x", "/etc/passwd", "src/../../x"] {
            assert!(read(&d, bad).is_err(), "{bad}");
            assert!(write(&d, bad, "no").is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), d.join("out")).unwrap();
            assert!(write(&d, "out/escape.txt", "no").is_err(), "a symlink out of the root");
        }
    }
}
