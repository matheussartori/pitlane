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
    pub run: RunCfg,
    pub verify: VerifyCfg,
    pub project: ProjectCfg,
}

/// A project that spans more than this repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectCfg {
    /// Other git repositories the cards' code lives in, relative to this one (`../amity-games`).
    /// Each run gets the same branch in each of them, and may edit and commit there too.
    pub repos: Vec<String>,
}

/// Takes a related repository off `[project] repos`.
pub fn remove_project_repo(root: &Path, rel: &str) -> Result<()> {
    let path = root.join(FLOW_PATH);
    if !path.exists() {
        return Ok(());
    }
    edit_flow(root, |doc| {
        if let Some(repos) = doc.get_mut("project").and_then(|p| p.get_mut("repos")).and_then(|r| r.as_array_mut()) {
            repos.retain(|v| v.as_str() != Some(rel));
        }
        Ok(())
    })
}

/// Adds a related repository to `[project] repos`, keeping the file's comments.
pub fn add_project_repo(root: &Path, rel: &str) -> Result<()> {
    edit_flow(root, |doc| {
        let project = doc
            .entry("project")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .context("[project] in flow.toml is not a table")?;
        let repos = project
            .entry("repos")
            .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .context("[project] repos in flow.toml is not an array")?;
        if !repos.iter().any(|v| v.as_str() == Some(rel)) {
            repos.push(rel);
        }
        Ok(())
    })
}

/// Checks after a run, before a person reviews.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerifyCfg {
    /// Run in the lane's worktree, one after the other: build, lint, tests.
    pub commands: Vec<String>,
    /// A separate Claude Code session reviews the diff against the spec.
    pub judge: bool,
    pub judge_model: Option<String>,
    /// Rounds of "ask for changes" on one card before Pitlane hands it back to you.
    pub max_rounds: u32,
}

impl Default for VerifyCfg {
    fn default() -> Self {
        Self { commands: Vec::new(), judge: true, judge_model: None, max_rounds: 3 }
    }
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

/// How agents run cards.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunCfg {
    /// Extra tools the agent may use without asking, in Claude Code's syntax, e.g. "Bash(npm test:*)".
    pub allow: Vec<String>,
    pub max_turns: u32,
    /// Claude Code model alias or name; empty: the CLI's default.
    pub model: Option<String>,
    /// Prefix of the branch made for a card that has no lane: `feat/` gives `feat/41-avatar-limit`.
    pub branch_prefix: String,
    /// Add Pitlane's map of where to look (from the repository index) to each run's first message.
    pub brief: bool,
    /// Claude Code's permission mode for every session Pitlane starts. `auto` (the default): a
    /// classifier lets safe actions through and blocks risky ones, so agents rarely stop to ask;
    /// `acceptEdits` or `default` ask more.
    pub permission_mode: String,
    /// Give agents a headless browser (Playwright's MCP server) to open what they change and take
    /// screenshots, shown in the run. Off by default: it starts Chrome and fetches the server with npx.
    pub browser: bool,
    /// Run the agent's commands in Claude Code's sandbox (on by default; not under Run all): they
    /// write only to the worktree, the git repository and the usual package caches, and reach no
    /// local socket, Pitlane's included. Anything else asks you first, as an unsandboxed command.
    pub sandbox: bool,
}

impl Default for RunCfg {
    fn default() -> Self {
        Self { allow: Vec::new(), max_turns: 150, model: None, branch_prefix: "feat/".into(), brief: true, permission_mode: "auto".into(), browser: false, sandbox: true }
    }
}

/// Adds `rule` to `[run] allow` in the repository's `.pitlane/flow.toml`, keeping its comments
/// and layout. Creates the file from the template when there is none. Idempotent.
pub fn add_run_allow(root: &Path, rule: &str) -> Result<()> {
    edit_flow(root, |doc| {
        let run = doc
            .entry("run")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .context("[run] in flow.toml is not a table")?;
        let allow = run
            .entry("allow")
            .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .context("[run] allow in flow.toml is not an array")?;
        if !allow.iter().any(|v| v.as_str() == Some(rule)) {
            allow.push(rule);
        }
        Ok(())
    })
}

/// Sets `[table] key = value` in the repository's `.pitlane/flow.toml`, keeping its comments and
/// layout (the file is made from the template when there is none). The flow must still read after
/// the change, or nothing is written.
pub fn set_value(root: &Path, table: &str, key: &str, value: toml_edit::Value) -> Result<()> {
    edit_flow(root, |doc| {
        let t = doc
            .entry(table)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .with_context(|| format!("[{table}] in flow.toml is not a table"))?;
        match t.get_mut(key).and_then(|i| i.as_value_mut()) {
            // Keep the comment after the value.
            Some(old) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            None => {
                t.insert(key, toml_edit::Item::Value(value));
            }
        }
        Ok(())
    })
}

/// Edits the repository's flow.toml (made from the template when there is none), keeping its
/// comments; the result must still read as a flow, or nothing is written.
fn edit_flow(root: &Path, change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>) -> Result<()> {
    crate::fsutil::edit_toml(&root.join(FLOW_PATH), TEMPLATE, change, |text| {
        toml::from_str::<FlowFile>(text).map(drop).map_err(Into::into)
    })
}

/// `set_value` from JSON (true, 12, "sonnet", ["npm test"]): what the app's Settings sends.
pub fn set_json(root: &Path, table: &str, key: &str, value: &serde_json::Value) -> Result<()> {
    let v: toml_edit::Value = match value {
        serde_json::Value::Bool(b) => (*b).into(),
        serde_json::Value::Number(n) if n.is_i64() => n.as_i64().unwrap_or_default().into(),
        serde_json::Value::String(s) => s.as_str().into(),
        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<toml_edit::Array>().into(),
        other => anyhow::bail!("{other} cannot go in flow.toml"),
    };
    set_value(root, table, key, v)
}

/// Tools every run may use: read and edit in its worktree, and git without leaving the branch.
pub const RUN_BASE_TOOLS: [&str; 16] = [
    "Read", "Edit", "Write", "MultiEdit", "Glob", "Grep", "TodoWrite", "NotebookEdit",
    "Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)", "Bash(git show:*)",
    "Bash(git add:*)", "Bash(git commit:*)", "Bash(ls:*)", "Bash(pwd)",
];

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

[verify]
commands    = []            # checks after each run, in the lane's worktree, e.g. ["npm test", "npm run lint"]
judge       = true          # a separate Claude Code session reviews the diff against the spec
# judge_model = "sonnet"   # empty: the model you use in Claude Code
max_rounds  = 3             # "ask for changes" rounds before the card comes back to you

[run]
allow       = []            # extra tools for agents, e.g. ["Bash(npm test:*)", "Bash(npm run lint:*)"]
max_turns   = 150
branch_prefix = "feat/"     # a card without a lane runs on feat/<id>-<slug>
brief = true                # tell the agent where to look, from Pitlane's index of the repository
permission_mode = "auto"    # Claude Code's mode for every session: auto (asks rarely), acceptEdits or default
browser = false             # a headless browser for agents: they open what they change and screenshot it
sandbox = true              # agents' commands write only to the worktree, the repository and package caches
# model     = "sonnet"
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
    fn sets_a_value_keeping_comments_and_refuses_a_bad_one() {
        let root = std::env::temp_dir().join(format!("pit-flow-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        set_value(&root, "run", "max_turns", 12.into()).unwrap();
        let text = std::fs::read_to_string(root.join(FLOW_PATH)).unwrap();
        assert!(text.contains("# every new branch starts here"), "the template's comments stay");
        assert_eq!(FlowFile::load(&root).unwrap().run.max_turns, 12);
        set_value(&root, "flow", "stale_days", 30.into()).unwrap();
        assert!(std::fs::read_to_string(root.join(FLOW_PATH)).unwrap().contains("stale_days  = 30 # a clean lane") || FlowFile::load(&root).unwrap().flow.stale_days == 30);
        assert!(set_value(&root, "run", "max_turns", "lots".into()).is_err());
        assert_eq!(FlowFile::load(&root).unwrap().run.max_turns, 12, "a bad value leaves the file as it was");
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
