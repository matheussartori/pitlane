//! End-to-end checks on throwaway repositories: a bare `origin` plus a clone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pit_core::Repo;
use pit_core::actions::{self, LocalUpdate, NewOptions};
use pit_core::lanes::{Landed, Lane, Status, Yard, YardOptions, yard};
use tempfile::TempDir;

struct Fx {
    tmp: TempDir,
    work: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

impl Fx {
    /// origin with `main` and `dev` (same commit), cloned into `work`, on main.
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin.git");
        let work = tmp.path().join("work");
        git(tmp.path(), &["init", "--quiet", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(tmp.path(), &["init", "--quiet", "-b", "main", work.to_str().unwrap()]);
        for (k, v) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&work, &["config", k, v]);
        }
        git(&work, &["remote", "add", "origin", origin.to_str().unwrap()]);
        let fx = Self { tmp, work };
        fx.write("a.txt", "a\n");
        fx.write("b.txt", "b\n");
        fx.commit("init");
        git(&fx.work, &["push", "--quiet", "-u", "origin", "main"]);
        git(&fx.work, &["branch", "dev"]);
        git(&fx.work, &["push", "--quiet", "origin", "dev"]);
        fx
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.work.join(file), text).unwrap();
    }

    fn commit(&self, msg: &str) -> String {
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "--quiet", "-m", msg]);
        git(&self.work, &["rev-parse", "HEAD"])
    }

    fn sh(&self, args: &[&str]) -> String {
        git(&self.work, args)
    }

    /// A lane off origin/main with one commit per file.
    fn lane(&self, branch: &str, files: &[&str]) {
        self.sh(&["switch", "--quiet", "-c", branch, "origin/main"]);
        for f in files {
            self.write(f, &format!("{branch} {f}\n"));
            self.commit(&format!("{branch}: {f}"));
        }
        self.sh(&["switch", "--quiet", "main"]);
    }

    /// `git merge --no-ff` of `branch` into `into`, pushed.
    fn merge(&self, branch: &str, into: &str) {
        self.sh(&["switch", "--quiet", into]);
        self.sh(&["pull", "--quiet", "--ff-only", "origin", into]);
        self.sh(&["merge", "--quiet", "--no-ff", "-m", &format!("Merge {branch} into {into}"), branch]);
        self.sh(&["push", "--quiet", "origin", into]);
        self.sh(&["switch", "--quiet", "main"]);
    }

    fn repo(&self) -> Repo {
        let mut r = Repo::open(&self.work, self.tmp.path(), &self.tmp.path().join(".pitlane")).unwrap();
        r.flow.worktree.root = self.tmp.path().join("wt/{repo}/{branch}").to_string_lossy().into_owned();
        r
    }

    fn yard(&self) -> Yard {
        yard(&self.repo(), YardOptions { fetch: true, github: false }).unwrap()
    }
}

fn lane<'a>(y: &'a Yard, branch: &str) -> &'a Lane {
    y.lanes.iter().find(|l| l.branch == branch).unwrap_or_else(|| panic!("no lane {branch}"))
}

#[test]
fn detects_every_way_of_landing() {
    let fx = Fx::new();

    fx.lane("feat/merged", &["m.txt"]);
    fx.merge("feat/merged", "main");

    // Squash of two commits: no single patch-id matches, only the merge simulation can tell.
    fx.lane("feat/squashed", &["s1.txt", "s2.txt"]);
    fx.sh(&["switch", "--quiet", "main"]);
    fx.sh(&["pull", "--quiet", "--ff-only", "origin", "main"]);
    fx.sh(&["merge", "--quiet", "--squash", "feat/squashed"]);
    fx.commit("squash feat/squashed");
    fx.sh(&["push", "--quiet", "origin", "main"]);

    // Rebased onto main: same patches, new shas.
    fx.lane("feat/rebased", &["r.txt"]);
    fx.write("unrelated.txt", "u\n");
    fx.commit("main moves on"); // otherwise the cherry-pick can recreate the exact same sha
    fx.sh(&["cherry-pick", "--quiet", "feat/rebased"]);
    fx.sh(&["push", "--quiet", "origin", "main"]);

    fx.lane("feat/open", &["o.txt"]);
    fx.sh(&["branch", "feat/empty", "origin/main"]);

    let y = fx.yard();
    assert_eq!(lane(&y, "feat/merged").in_production, Some(Landed::Ancestor));
    assert_eq!(lane(&y, "feat/squashed").in_production, Some(Landed::MergeTree));
    assert_eq!(lane(&y, "feat/rebased").in_production, Some(Landed::PatchId));
    for b in ["feat/merged", "feat/squashed", "feat/rebased"] {
        assert_eq!(lane(&y, b).status, Status::CanDelete, "{b}");
    }
    assert_eq!(lane(&y, "feat/open").in_production, None);
    assert_eq!(lane(&y, "feat/open").status, Status::InProgress);
    assert_eq!(lane(&y, "feat/empty").status, Status::New);
    assert!(y.lanes.iter().all(|l| l.branch != "main" && l.branch != "dev"));
}

#[test]
fn integration_stage_and_contamination() {
    let fx = Fx::new();
    fx.lane("feat/other", &["other.txt"]);
    fx.merge("feat/other", "dev");

    fx.lane("feat/tested", &["t.txt"]);
    fx.merge("feat/tested", "dev");

    // Started from dev: brings feat/other along.
    fx.sh(&["switch", "--quiet", "-c", "feat/from-dev", "origin/dev"]);
    fx.write("fd.txt", "x\n");
    fx.commit("from dev");
    fx.sh(&["switch", "--quiet", "main"]);

    // Merged dev into itself.
    fx.lane("feat/pulled-dev", &["pd.txt"]);
    fx.sh(&["switch", "--quiet", "feat/pulled-dev"]);
    fx.sh(&["merge", "--quiet", "--no-ff", "-m", "merge dev", "origin/dev"]);
    fx.sh(&["switch", "--quiet", "main"]);

    let y = fx.yard();
    let tested = lane(&y, "feat/tested");
    assert_eq!(tested.in_integration, Some(Landed::Ancestor));
    assert_eq!(tested.in_production, None);
    assert_eq!(tested.status, Status::NeedsProduction);
    assert!(tested.integration_commits.is_empty());
    assert_eq!(lane(&y, "feat/from-dev").status, Status::Contaminated);
    assert_eq!(lane(&y, "feat/pulled-dev").status, Status::Contaminated);
    assert!(y.only_in_integration.is_empty());

    let err = actions::plan_ship(&fx.repo(), "feat/from-dev", Some("main"), true).unwrap_err();
    assert!(format!("{err:#}").contains("exist only in dev"), "{err:#}");
}

#[test]
fn only_in_dev_lists_direct_commits() {
    let fx = Fx::new();
    fx.sh(&["switch", "--quiet", "dev"]);
    fx.write("hotfix.txt", "h\n");
    let direct = fx.commit("direct on dev");
    fx.write("picked.txt", "p\n");
    let picked = fx.commit("also picked to main");
    fx.sh(&["push", "--quiet", "origin", "dev"]);
    fx.sh(&["switch", "--quiet", "main"]);
    fx.sh(&["cherry-pick", "--quiet", &picked]);
    fx.sh(&["push", "--quiet", "origin", "main"]);

    let y = fx.yard();
    let shas: Vec<&str> = y.only_in_integration.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, vec![direct.as_str()]);
}

#[test]
fn ship_to_dev_then_main_then_clean_and_undo() {
    let fx = Fx::new();
    fx.lane("feat/x", &["x.txt"]);
    fx.sh(&["push", "--quiet", "origin", "feat/x"]);
    let repo = fx.repo();

    let p = actions::plan_ship(&repo, "feat/x", None, true).unwrap();
    assert_eq!(p.target, "dev");
    assert!(p.conflicts.is_empty() && p.tree.is_some());
    let r = actions::execute_ship(&repo, &p).unwrap();
    assert!(matches!(r.local, LocalUpdate::Updated));
    assert_eq!(fx.sh(&["rev-parse", "dev"]), r.merge_sha);
    assert_eq!(fx.sh(&["log", "-1", "--format=%s", "origin/dev"]), "Merge feat/x into dev");
    assert_eq!(lane(&fx.yard(), "feat/x").status, Status::NeedsProduction);

    let p = actions::plan_ship(&repo, "feat/x", None, true).unwrap();
    assert_eq!(p.target, "main");
    let r = actions::execute_ship(&repo, &p).unwrap();
    // main is checked out and clean: fast-forwarded in place.
    assert!(matches!(r.local, LocalUpdate::Updated));
    assert_eq!(fx.sh(&["rev-parse", "HEAD"]), r.merge_sha);
    assert_eq!(lane(&fx.yard(), "feat/x").status, Status::CanDelete);

    let again = actions::plan_ship(&repo, "feat/x", None, true).unwrap();
    assert!(again.already.is_some());

    let y = fx.yard();
    let plan = actions::plan_clean(&repo, &y).unwrap();
    assert_eq!(plan.items.len(), 1);
    assert!(plan.items[0].delete_remote);
    let out = actions::execute_clean(&repo, &plan).unwrap();
    assert!(out.iter().all(|o| o.error.is_none()), "{:?}", out.iter().map(|o| &o.error).collect::<Vec<_>>());
    fx.sh(&["fetch", "--quiet", "--prune", "origin"]);
    assert!(fx.sh(&["branch", "--list", "feat/x"]).is_empty());
    assert!(fx.sh(&["branch", "-r", "--list", "origin/feat/x"]).is_empty());

    let u = actions::undo_clean(&repo, "feat/x").unwrap();
    assert!(u.remote_restored);
    fx.sh(&["fetch", "--quiet", "origin"]);
    assert_eq!(fx.sh(&["rev-parse", "feat/x"]), fx.sh(&["rev-parse", "origin/feat/x"]));
}

#[test]
fn ship_reports_conflicts_and_changes_nothing() {
    let fx = Fx::new();
    fx.lane("feat/a", &["a.txt"]);
    fx.sh(&["switch", "--quiet", "dev"]);
    fx.write("a.txt", "dev edit\n");
    fx.commit("dev edits a");
    fx.sh(&["push", "--quiet", "origin", "dev"]);
    fx.sh(&["switch", "--quiet", "main"]);
    let before = fx.sh(&["rev-parse", "origin/dev"]);

    let p = actions::plan_ship(&fx.repo(), "feat/a", Some("dev"), true).unwrap();
    assert_eq!(p.conflicts, vec!["a.txt".to_owned()]);
    assert!(p.tree.is_none());
    assert!(actions::execute_ship(&fx.repo(), &p).is_err());
    fx.sh(&["fetch", "--quiet", "origin"]);
    assert_eq!(fx.sh(&["rev-parse", "origin/dev"]), before);
}

#[test]
fn new_lane_gets_a_worktree_from_origin_main() {
    let fx = Fx::new();
    fx.write(".env", "SECRET=1\n");
    fx.sh(&["config", "core.excludesFile", "/dev/null"]);
    fs::write(fx.work.join(".git/info/exclude"), ".env\nnode_modules\n").unwrap();
    fs::create_dir_all(fx.work.join("node_modules/pkg")).unwrap();
    fs::write(fx.work.join("node_modules/pkg/index.js"), "x").unwrap();
    // Local main moves ahead of origin/main: the lane must ignore it.
    fx.write("local-only.txt", "l\n");
    fx.commit("local only");

    let mut repo = fx.repo();
    repo.flow.flow.naming = vec!["feat/*".into()];
    repo.flow.worktree.copy = vec![".env".into(), "missing.env".into()];
    repo.flow.worktree.clone_deps = vec!["node_modules".into()];
    assert!(actions::new_lane(&repo, "bad-name", NewOptions { fetch: true, setup: false }).is_err());

    let r = actions::new_lane(&repo, "feat/new", NewOptions { fetch: true, setup: false }).unwrap();
    assert_eq!(r.base_sha, fx.sh(&["rev-parse", "origin/main"]));
    assert_eq!(r.path, fx.tmp.path().join("wt/work/feat-new"));
    assert_eq!(fs::read_to_string(r.path.join(".env")).unwrap(), "SECRET=1\n");
    assert!(r.path.join("node_modules/pkg/index.js").exists());
    assert!(!r.path.join("local-only.txt").exists());
    assert_eq!(r.missing, vec!["missing.env".to_owned()]);

    let y = yard(&repo, YardOptions { fetch: false, github: false }).unwrap();
    let l = lane(&y, "feat/new");
    assert_eq!(l.status, Status::New);
    assert_eq!(l.worktree.as_ref().unwrap().dirty, 0);
}

#[test]
fn sync_with_the_remote() {
    use pit_core::lanes::Sync;
    let fx = Fx::new();
    let origin = fx.tmp.path().join("origin.git");

    fx.lane("feat/local", &["l.txt"]);

    fx.lane("feat/synced", &["s.txt"]);
    fx.sh(&["push", "--quiet", "-u", "origin", "feat/synced"]);

    fx.lane("feat/ahead", &["a.txt"]);
    fx.sh(&["push", "--quiet", "-u", "origin", "feat/ahead"]);
    fx.sh(&["switch", "--quiet", "feat/ahead"]);
    fx.write("a2.txt", "x\n");
    fx.commit("one more");
    fx.sh(&["switch", "--quiet", "main"]);

    // Someone else pushed to feat/behind: a second clone.
    fx.lane("feat/behind", &["b1.txt"]);
    fx.sh(&["push", "--quiet", "-u", "origin", "feat/behind"]);
    let other = fx.tmp.path().join("other");
    git(fx.tmp.path(), &["clone", "--quiet", origin.to_str().unwrap(), other.to_str().unwrap()]);
    for (k, v) in [("user.name", "Other"), ("user.email", "o@example.com"), ("commit.gpgsign", "false")] {
        git(&other, &["config", k, v]);
    }
    git(&other, &["switch", "--quiet", "feat/behind"]);
    fs::write(other.join("b2.txt"), "y\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "--quiet", "-m", "from the other clone"]);
    git(&other, &["push", "--quiet"]);

    // Pushed once, then deleted on the remote.
    fx.lane("feat/gone", &["g.txt"]);
    fx.sh(&["push", "--quiet", "-u", "origin", "feat/gone"]);
    git(&other, &["push", "--quiet", "origin", "--delete", "feat/gone"]);

    // Only on the remote.
    git(&other, &["switch", "--quiet", "-c", "feat/theirs", "origin/main"]);
    fs::write(other.join("t.txt"), "t\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "--quiet", "-m", "theirs"]);
    git(&other, &["push", "--quiet", "-u", "origin", "feat/theirs"]);

    // Tracks a branch with another name.
    fx.sh(&["switch", "--quiet", "-c", "feat/twin", "--track", "origin/feat/synced"]);
    fx.sh(&["switch", "--quiet", "main"]);

    let y = fx.yard();
    assert_eq!(lane(&y, "feat/local").sync, Sync::LocalOnly);
    assert_eq!(lane(&y, "feat/local").unpushed, 1);
    assert_eq!(lane(&y, "feat/synced").sync, Sync::Synced);
    assert!(lane(&y, "feat/synced").last_push.is_some());
    assert_eq!(lane(&y, "feat/ahead").sync, Sync::Ahead { ahead: 1 });
    assert_eq!(lane(&y, "feat/behind").sync, Sync::Behind { behind: 1 });
    assert_eq!(lane(&y, "feat/gone").sync, Sync::Gone);
    let theirs = lane(&y, "feat/theirs");
    assert_eq!(theirs.sync, Sync::RemoteOnly);
    assert!(!theirs.local && theirs.worktree.is_none());
    assert_eq!(theirs.status, Status::InProgress);
    assert_eq!(lane(&y, "feat/twin").tracks_other.as_deref(), Some("origin/feat/synced"));
    assert_eq!(lane(&y, "feat/twin").sync, Sync::LocalOnly);

    // Ship warns about remote commits the local branch lacks.
    let p = actions::plan_ship(&fx.repo(), "feat/behind", Some("dev"), false).unwrap();
    assert_eq!((p.remote_ahead, p.remote_behind), (0, 1));

    // `pit new` on a branch that exists only remotely checks it out, tracking it.
    let r = actions::new_lane(&fx.repo(), "feat/theirs", NewOptions { fetch: false, setup: false }).unwrap();
    assert_eq!(r.base, "origin/feat/theirs");
    assert_eq!(fx.sh(&["rev-parse", "feat/theirs"]), fx.sh(&["rev-parse", "origin/feat/theirs"]));
    assert_eq!(lane(&fx.yard(), "feat/theirs").sync, Sync::Synced);
}

#[test]
fn clean_and_undo_a_remote_only_lane() {
    let fx = Fx::new();
    fx.lane("feat/done", &["d.txt"]);
    fx.sh(&["push", "--quiet", "origin", "feat/done"]);
    fx.merge("feat/done", "main");
    fx.sh(&["branch", "-D", "feat/done"]);

    let repo = fx.repo();
    let y = fx.yard();
    let l = lane(&y, "feat/done");
    assert!(!l.local);
    assert_eq!(l.status, Status::CanDelete);

    let plan = actions::plan_clean(&repo, &y).unwrap();
    assert_eq!(plan.items.len(), 1);
    assert!(!plan.items[0].local && plan.items[0].delete_remote);
    let out = actions::execute_clean(&repo, &plan).unwrap();
    assert!(out[0].error.is_none(), "{:?}", out[0].error);
    fx.sh(&["fetch", "--quiet", "--prune", "origin"]);
    assert!(fx.sh(&["branch", "-r", "--list", "origin/feat/done"]).is_empty());

    let u = actions::undo_clean(&repo, "feat/done").unwrap();
    assert!(u.remote_restored && !u.local_restored);
    assert!(fx.sh(&["branch", "--list", "feat/done"]).is_empty());
}
