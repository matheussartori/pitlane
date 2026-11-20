//! Pitlane core: branch flow, lanes and the actions on them. The `pit` CLI and, later,
//! the `pitd` daemon are thin layers over this crate.

pub mod actions;
pub mod flow;
pub mod git;
pub mod lanes;
pub mod trash;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::flow::FlowFile;
use crate::git::Git;

/// One repository as Pitlane sees it.
#[derive(Debug, Clone)]
pub struct Repo {
    /// Git run from the main worktree.
    pub git: Git,
    /// Main worktree root.
    pub root: PathBuf,
    /// Worktree the command was run from.
    pub cwd_root: PathBuf,
    /// Short name, the main worktree's folder name.
    pub name: String,
    pub flow: FlowFile,
    /// `~/.pitlane` (or `$PITLANE_HOME`): trash, worktrees by default.
    pub pitlane_home: PathBuf,
    /// The user's home, for `~` in paths.
    pub home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub is_main: bool,
    pub prunable: bool,
}

impl Repo {
    /// Opens with the user's home and `$PITLANE_HOME` (default `~/.pitlane`).
    pub fn open_default(dir: &Path) -> Result<Self> {
        let (home, pitlane_home) = default_homes();
        Self::open(dir, &home, &pitlane_home)
    }

    pub fn open(dir: &Path, home: &Path, pitlane_home: &Path) -> Result<Self> {
        let here = Git::new(dir);
        let cwd_root = PathBuf::from(
            here.run(&["rev-parse", "--show-toplevel"])
                .with_context(|| format!("{} is not inside a git repository", dir.display()))?,
        );
        let worktrees = list_worktrees(&here)?;
        let root = worktrees
            .iter()
            .find(|w| w.is_main)
            .map(|w| w.path.clone())
            .unwrap_or_else(|| cwd_root.clone());
        let flow = if cwd_root.join(flow::FLOW_PATH).exists() {
            FlowFile::load(&cwd_root)?
        } else {
            FlowFile::load(&root)?
        };
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        Ok(Self {
            git: Git::new(&root),
            root,
            cwd_root,
            name,
            flow,
            pitlane_home: pitlane_home.to_path_buf(),
            home: home.to_path_buf(),
        })
    }

    pub fn remote_ref(&self, branch: &str) -> String {
        format!("refs/remotes/{}/{}", self.flow.flow.remote, branch)
    }

    /// `origin/<branch>` as a full sha, if the remote branch exists.
    pub fn remote_sha(&self, branch: &str) -> Result<Option<String>> {
        self.git.rev_parse(&self.remote_ref(branch))
    }

    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        list_worktrees(&self.git)
    }

    pub fn fetch(&self) -> Result<()> {
        self.git
            .run(&["fetch", "--prune", "--quiet", &self.flow.flow.remote])
            .map(drop)
    }
}

/// `(home, pitlane_home)`: `$HOME` and `$PITLANE_HOME` or `~/.pitlane`.
pub fn default_homes() -> (PathBuf, PathBuf) {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let pitlane_home = std::env::var_os("PITLANE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pitlane"));
    (home, pitlane_home)
}

pub fn list_worktrees(git: &Git) -> Result<Vec<Worktree>> {
    let text = git.run(&["worktree", "list", "--porcelain"])?;
    let mut out = Vec::new();
    for (i, block) in text.split("\n\n").filter(|b| !b.trim().is_empty()).enumerate() {
        let mut wt = Worktree {
            path: PathBuf::new(),
            head: String::new(),
            branch: None,
            is_main: i == 0,
            prunable: false,
        };
        let mut bare = false;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                wt.path = PathBuf::from(p);
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                wt.head = h.to_owned();
            } else if let Some(b) = line.strip_prefix("branch ") {
                wt.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
            } else if line.starts_with("prunable") {
                wt.prunable = true;
            } else if line == "bare" {
                bare = true;
            }
        }
        if !bare {
            out.push(wt);
        }
    }
    Ok(out)
}
