//! `.pitlane/flow.toml`: the repository's branch flow, versioned with the code.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FLOW_PATH: &str = ".pitlane/flow.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlowFile {
    pub flow: Flow,
    pub worktree: WorktreeCfg,
    pub ship: ShipCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Flow {
    /// Every new branch starts from `<remote>/<base>`, freshly fetched.
    pub base: String,
    /// Where work is tried before production. Empty string: no integration stage.
    pub integration: String,
    pub production: String,
    pub remote: String,
    /// Allowed branch name patterns (`*` matches anything). Empty: any name.
    pub naming: Vec<String>,
    pub forbid: Vec<Forbid>,
    pub cleanup: Cleanup,
    /// A clean lane with no commit for this many days shows as stale.
    pub stale_days: u32,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            base: "main".into(),
            integration: "dev".into(),
            production: "main".into(),
            remote: "origin".into(),
            naming: Vec::new(),
            forbid: vec![Forbid { merge: "dev".into(), into: "main".into() }],
            cleanup: Cleanup::AfterMain,
            stale_days: 14,
        }
    }
}

impl Flow {
    pub fn integration(&self) -> Option<&str> {
        (!self.integration.is_empty() && self.integration != self.production)
            .then_some(self.integration.as_str())
    }

    /// Branches that are never lanes: they are the flow itself.
    pub fn is_trunk(&self, branch: &str) -> bool {
        branch == self.base || branch == self.production || branch == self.integration
    }

    pub fn name_allowed(&self, branch: &str) -> bool {
        self.naming.is_empty() || self.naming.iter().any(|p| glob_match(p, branch))
    }

    pub fn forbids(&self, merge: &str, into: &str) -> bool {
        self.forbid.iter().any(|f| glob_match(&f.merge, merge) && glob_match(&f.into, into))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forbid {
    pub merge: String,
    pub into: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cleanup {
    /// A lane can be deleted once it is in production.
    AfterMain,
    /// `pit clean` never offers anything.
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorktreeCfg {
    /// `~` is the home directory; `{repo}` and `{branch}` (slashes become dashes) are filled in.
    pub root: String,
    /// Untracked files each worktree needs, copied from the main worktree.
    pub copy: Vec<String>,
    /// Directories cloned copy-on-write (APFS clonefile) instead of reinstalled.
    /// A `*` segment matches one directory level, e.g. `packages/*/node_modules`.
    pub clone_deps: Vec<String>,
    /// Shell command run inside a new worktree.
    pub setup: Option<String>,
}

impl Default for WorktreeCfg {
    fn default() -> Self {
        Self {
            root: "~/.pitlane/wt/{repo}/{branch}".into(),
            copy: Vec::new(),
            clone_deps: Vec::new(),
            setup: None,
        }
    }
}

impl WorktreeCfg {
    pub fn path_for(&self, home: &Path, repo: &str, branch: &str) -> PathBuf {
        let filled = self
            .root
            .replace("{repo}", repo)
            .replace("{branch}", &branch.replace('/', "-"));
        match filled.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(filled),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShipCfg {
    /// Shell command that must pass in the lane's worktree before it ships to production.
    pub check: Option<String>,
}

impl FlowFile {
    /// Reads `.pitlane/flow.toml` under `root`; defaults when the file does not exist.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FLOW_PATH);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("could not read {}", path.display())),
        }
    }
}

/// The file `pit init` writes.
pub const TEMPLATE: &str = r#"# Pitlane branch flow. Versioned with the repo.

[flow]
base        = "main"        # every new branch starts here, always from a freshly fetched origin/main
integration = "dev"         # where work is tried; "" if there is no such stage
production  = "main"
remote      = "origin"
naming      = ["feat/*", "fix/*", "chore/*"]
forbid      = [{ merge = "dev", into = "main" }]
cleanup     = "after-main"  # a lane can be deleted once it is in main
stale_days  = 14

[worktree]
root       = "~/.pitlane/wt/{repo}/{branch}"
copy       = []             # untracked files each worktree needs, e.g. [".env"]
clone_deps = []             # copy-on-write clones, e.g. ["node_modules"]
# setup    = "npm install --prefer-offline"

[ship]
# check = "npm test"        # must pass before a lane ships to production
"#;

/// `*` matches any run of characters (slashes included); everything else is literal.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("feat/*", "feat/x"));
        assert!(glob_match("feat/*", "feat/a/b"));
        assert!(!glob_match("feat/*", "fix/x"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYc"));
        assert!(!glob_match("a*b", "aXXc"));
        assert!(glob_match("dev", "dev"));
    }

    #[test]
    fn template_parses_and_defaults_match() {
        let f: FlowFile = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(f.flow.integration(), Some("dev"));
        assert!(f.flow.forbids("dev", "main"));
        assert!(!f.flow.name_allowed("random"));
        let d = FlowFile::default();
        assert!(d.flow.name_allowed("random"));
    }

    #[test]
    fn worktree_path() {
        let w = WorktreeCfg::default();
        assert_eq!(
            w.path_for(Path::new("/h"), "amity", "fix/avatar-svg"),
            PathBuf::from("/h/.pitlane/wt/amity/fix-avatar-svg")
        );
    }
}
