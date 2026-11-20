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

/// What git ignores (with .gitignore and the global excludes), folders collapsed to one entry with
/// a trailing slash: shown greyed in the tree, opened on demand.
pub fn ignored(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-o", "-i", "--exclude-standard", "--directory", "-z"])
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("{} is not a git checkout", root.display());
    }
    let mut v: Vec<String> = out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
    v.sort();
    v.truncate(5_000);
    Ok(v)
}

/// One folder's entries (names, folders with a trailing slash), for folders git does not list.
pub fn dir(root: &Path, rel: &str) -> Result<Vec<String>> {
    let full = inside(root, rel)?;
    let mut v = Vec::new();
    for e in std::fs::read_dir(&full).with_context(|| format!("no folder {rel}"))?.flatten().take(5_000) {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        v.push(if is_dir { format!("{name}/") } else { name });
    }
    v.sort_by(|a, b| (!a.ends_with('/'), a.to_lowercase()).cmp(&(!b.ends_with('/'), b.to_lowercase())));
    Ok(v)
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

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git").arg("-C").arg(root).args(args).output().context("could not run git")
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Hit {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub text: String,
}

/// Text across the folder's files (tracked and untracked, not ignored), case-insensitive unless
/// the query has a capital. At most `limit` hits.
pub fn search(root: &Path, query: &str, limit: usize) -> Result<Vec<Hit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["grep", "-n", "--column", "-I", "--untracked", "--exclude-standard", "-F", "--no-color", "-z"];
    if !query.chars().any(char::is_uppercase) {
        args.push("-i");
    }
    args.extend(["-e", query]);
    let out = git(root, &args)?;
    // 1 is "nothing found"; anything above is an error.
    if out.status.code().unwrap_or(2) > 1 {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut hits = Vec::new();
    for l in text.lines() {
        // path\0line\0col\0text with -z
        let mut parts = l.splitn(4, '\0');
        let (Some(path), Some(line), Some(col), Some(t)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(line), Ok(col)) = (line.parse(), col.parse()) else { continue };
        hits.push(Hit { path: path.to_owned(), line, col, text: t.chars().take(240).collect() });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

#[derive(Debug, Serialize, PartialEq, Default)]
pub struct Changes {
    /// The commit compared against, short.
    pub base: Option<String>,
    /// The file as it was there; None when the file is new since.
    pub original: Option<String>,
    /// Lines of the file on disk, 1-based: added, changed, and where lines were removed.
    pub added: Vec<(u32, u32)>,
    pub changed: Vec<(u32, u32)>,
    pub removed: Vec<u32>,
}

/// How a file differs from `base` (a commit), for the gutter and the side-by-side view.
pub fn changes(root: &Path, rel: &str, base: &str) -> Result<Changes> {
    inside(root, rel)?;
    let original = git(root, &["show", &format!("{base}:{rel}")])?;
    let original = original.status.success().then(|| String::from_utf8_lossy(&original.stdout).into_owned());
    let out = git(root, &["diff", "-U0", "--no-color", "--no-ext-diff", base, "--", rel])?;
    let mut c = Changes { base: Some(base.chars().take(8).collect()), original, ..Default::default() };
    let num = |s: &str| -> (u32, u32) {
        let mut it = s.split(',');
        let start = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let len = it.next().map_or(1, |x| x.parse().unwrap_or(1));
        (start, len)
    };
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(rest) = l.strip_prefix("@@ -") else { continue };
        let mut it = rest.split_whitespace();
        let (Some(old), Some(new)) = (it.next(), it.next().and_then(|n| n.strip_prefix('+'))) else { continue };
        let (_, olen) = num(old);
        let (nstart, nlen) = num(new);
        if nlen == 0 {
            c.removed.push(nstart.max(1));
        } else if olen == 0 {
            c.added.push((nstart, nstart + nlen - 1));
        } else {
            c.changed.push((nstart, nstart + nlen - 1));
        }
    }
    if c.original.is_none() && c.added.is_empty() {
        // New and untracked: every line is new.
        if let Ok(text) = std::fs::read_to_string(root.join(rel)) {
            let n = text.lines().count() as u32;
            if n > 0 {
                c.added.push((1, n));
            }
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("pit-files-{}-{n}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
        assert_eq!(ignored(&d).unwrap(), ["dist/"]);
        assert_eq!(dir(&d, "dist").unwrap(), ["out.js"]);
        assert!(dir(&d, "../").is_err());
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

    #[test]
    fn finds_text_and_says_what_changed_since_a_commit() {
        let d = repo();
        let g = |a: &[&str]| assert!(Command::new("git").arg("-C").arg(&d).args(a).status().unwrap().success());
        std::fs::write(d.join("src/b.ts"), "one\ntwo\nthree\nfour\n").unwrap();
        g(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
        g(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
        std::fs::write(d.join("src/b.ts"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
        std::fs::write(d.join("src/c.ts"), "export const two = 2;\n").unwrap();

        let hits = search(&d, "two", 50).unwrap();
        assert_eq!(hits.iter().map(|h| (h.path.as_str(), h.line)).collect::<Vec<_>>(), [("src/b.ts", 2), ("src/c.ts", 1)]);
        assert_eq!(search(&d, "TWO", 50).unwrap().len(), 1, "a capital makes it case-sensitive");

        let c = changes(&d, "src/b.ts", "HEAD").unwrap();
        assert_eq!((c.changed, c.added, c.removed), (vec![(2, 2)], vec![(5, 5)], vec![]));
        assert_eq!(c.original.as_deref(), Some("one\ntwo\nthree\nfour\n"));
        let n = changes(&d, "src/c.ts", "HEAD").unwrap();
        assert_eq!((n.original, n.added), (None, vec![(1, 1)]));
    }
}
