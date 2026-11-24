//! Closing the daemon, in a test binary of its own: closing ends every command the process
//! started, which would end the other tests' too.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use pit_core::Repo;
use pit_core::cards::{self, CardPatch, NewCard};
use pitd::{Config, Supervisor};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_stops_what_runs_and_lets_nothing_new_start() {
    let tmp = TempDir::new().unwrap();
    let (home, repo) = (tmp.path().join("pitlane"), tmp.path().join("repo"));
    git(tmp.path(), &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
        git(&repo, &["config", k, v]);
    }
    fs::create_dir_all(repo.join(".pitlane")).unwrap();
    let wt_root = tmp.path().join("wt/{repo}/{branch}");
    fs::write(repo.join(".pitlane/flow.toml"), format!("[worktree]\nroot = \"{}\"\n\n[verify]\njudge = false\n", wt_root.display())).unwrap();
    fs::write(repo.join("README.md"), "demo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    git(&repo, &["branch", "feat/x"]);
    git(&repo, &["branch", "feat/y"]);
    // An agent that says it is signed in and then never answers: it only ends when it is stopped.
    let fake = tmp.path().join("fake-claude");
    fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = auth ]; then echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\"}'; exit 0; fi\nexec sleep 60\n").unwrap();
    Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
    let card = |title: &str, lane: &str| {
        let r = Repo::open(&repo, tmp.path(), &home).unwrap();
        let c = cards::add(&r, NewCard { title: title.into(), body: "Do the thing.".into(), ..Default::default() }).unwrap();
        cards::update(&r, c.id, CardPatch { lane: Some(Some(lane.into())), ..Default::default() }).unwrap();
        c.id
    };
    let (first, second) = (card("First", "feat/x"), card("Second", "feat/y"));

    let s = Supervisor::new(Config { home: home.clone(), claude_bin: fake.to_string_lossy().into_owned(), pit_bin: None });
    let run = s.start(repo.clone(), first, None, Default::default()).unwrap();
    let t = Instant::now();
    while s.get(&run.id).unwrap().pid.is_none() {
        assert!(t.elapsed() < Duration::from_secs(20), "the agent never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let t = Instant::now();
    s.close().await;
    assert!(t.elapsed() <= pit_core::runs::DAEMON_CLOSES_WITHIN, "closing took {:?}", t.elapsed());
    let ended = s.get(&run.id).unwrap();
    assert!(!ended.state.is_live(), "the run is still {:?}", ended.state);
    assert!(ended.pid.is_none());

    let refused = s.start(repo.clone(), second, None, Default::default());
    let why = format!("{:#}", refused.expect_err("a run started while closing"));
    assert!(why.contains("closing"), "{why}");
}
