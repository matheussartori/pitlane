//! The files of a folder (the repository or a lane's worktree), so a path printed in a terminal
//! can be told apart from any other text.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const MAX_FILES: usize = 50_000;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_what_git_would_not_ignore() {
        let d = std::env::temp_dir().join(format!("pit-files-{}", std::process::id()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("dist")).unwrap();
        Command::new("git").arg("-C").arg(&d).args(["init", "-q"]).status().unwrap();
        std::fs::write(d.join("src/a.ts"), "export const a = 1;\n").unwrap();
        std::fs::write(d.join(".gitignore"), "dist/\n").unwrap();
        std::fs::write(d.join("dist/out.js"), "x").unwrap();
        assert_eq!(list(&d).unwrap(), [".gitignore", "src/a.ts"]);
    }
}
