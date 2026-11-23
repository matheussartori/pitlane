//! Where a repository's board lives, and moving boards left where older builds put them.

use super::*;

/// The cards folder of this repository's board.
pub fn dir(repo: &Repo) -> PathBuf {
    repo.board.join(CARDS_DIR)
}

/// The project's decisions file, beside the cards.
pub fn decisions_path(repo: &Repo) -> PathBuf {
    repo.board.join(DECISIONS)
}

/// What the person wrote about the project for every session (repositories, how branches land,
/// what may be pushed or deployed), beside the cards: never in the repository.
pub fn context_path(repo: &Repo) -> PathBuf {
    repo.board.join(CONTEXT)
}

/// The project context, trimmed; empty when there is none.
pub fn project_context(repo: &Repo) -> String {
    fs::read_to_string(context_path(repo)).map(|t| t.trim().to_owned()).unwrap_or_default()
}

/// Replaces the project context; an empty text removes the file.
pub fn set_project_context(repo: &Repo, text: &str) -> Result<()> {
    let path = context_path(repo);
    let text = text.trim();
    if text.is_empty() {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| format!("could not remove {}", path.display())),
            _ => return Ok(()),
        }
    }
    crate::fsutil::write_atomic(&path, format!("{text}\n"))
}

/// A repository's board folder: keyed by its remote's URL, so moving or re-cloning the folder keeps
/// the board; by the main worktree's path when there is no remote. The readable part flattens
/// separators, so a hash of the real key follows it: `me/a-b` and `me-a/b` never share a board.
pub fn board_for(pitlane_home: &Path, remote_url: Option<&str>, root: &Path) -> PathBuf {
    let key = board_key(remote_url, root);
    pitlane_home.join("boards").join(format!("{}-{}", crate::fsutil::flat_name(&key), crate::fsutil::short_hash(&key)))
}

pub(super) fn board_key(remote_url: Option<&str>, root: &Path) -> String {
    match remote_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => normalize_remote(url),
        None => root.to_string_lossy().into_owned(),
    }
}

/// Moves a board kept under the old key (no hash) to its new folder, and leaves the old name as a
/// link to it, so an older `pit` (the MCP Claude Code runs) still finds the same cards. Does
/// nothing once moved, or when the new folder already exists.
pub fn adopt_flat_board(pitlane_home: &Path, remote_url: Option<&str>, root: &Path) -> Result<()> {
    let key = board_key(remote_url, root);
    let old = pitlane_home.join("boards").join(crate::fsutil::flat_name(&key));
    let new = board_for(pitlane_home, remote_url, root);
    let is_dir = fs::symlink_metadata(&old).is_ok_and(|m| m.is_dir());
    if !is_dir || new.exists() {
        return Ok(());
    }
    fs::rename(&old, &new).with_context(|| format!("could not move {} to {}", old.display(), new.display()))?;
    std::os::unix::fs::symlink(&new, &old).with_context(|| format!("could not link {}", old.display()))?;
    Ok(())
}

/// `git@github.com:me/x.git`, `https://user@github.com/me/x` and `ssh://git@github.com/me/x.git`
/// all become `github.com/me/x`.
pub fn normalize_remote(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/');
    u = u.strip_suffix(".git").unwrap_or(u);
    let scp_like = !u.contains("://");
    if let Some((_, rest)) = u.split_once("://") {
        u = rest;
    }
    let host_end = u.find('/').unwrap_or(u.len());
    if let Some(at) = u[..host_end].rfind('@') {
        u = &u[at + 1..];
    }
    let mut out = u.to_owned();
    if scp_like && let Some(colon) = out.find(':') && !out[..colon].contains('/') {
        out.replace_range(colon..=colon, "/");
    }
    out.to_lowercase()
}

/// Moves cards and decisions left in the repository's `.pitlane/` (the old place) into the board.
/// A file the board already has with other content stays where it is, so nothing is lost.
pub fn adopt_legacy(repo: &Repo) -> Result<usize> {
    let legacy_cards = repo.root.join(LEGACY_CARDS);
    let legacy_decisions = repo.root.join(LEGACY_DECISIONS);
    if !legacy_cards.exists() && !legacy_decisions.exists() {
        return Ok(0);
    }
    let _lock = Lock::take(repo)?;
    let mut moved = 0;
    let mut adopt = |src: &Path, dst: &Path| -> Result<()> {
        let text = fs::read_to_string(src).with_context(|| format!("could not read {}", src.display()))?;
        match fs::read_to_string(dst) {
            Ok(existing) if existing != text => return Ok(()),
            Ok(_) => {}
            Err(_) => atomic_write(dst, &text)?,
        }
        fs::remove_file(src).with_context(|| format!("could not remove {}", src.display()))?;
        moved += 1;
        Ok(())
    };
    if let Ok(entries) = fs::read_dir(&legacy_cards) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if entry.path().extension().is_some_and(|e| e == "md") {
                adopt(&entry.path(), &dir(repo).join(&name))?;
            }
        }
        let _ = fs::remove_dir(&legacy_cards);
    }
    if legacy_decisions.exists() {
        adopt(&legacy_decisions, &decisions_path(repo))?;
    }
    let _ = fs::remove_file(repo.root.join(LEGACY_LOCK));
    Ok(moved)
}
