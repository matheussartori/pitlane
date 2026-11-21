//! Cards on a throwaway repository: files, ids, the interview and decisions.

use std::fs;
use std::process::Command;
use std::sync::Arc;

use pit_core::Repo;
use pit_core::cards::{self, CardPatch, CardState, NewCard, NewQuestion};
use tempfile::TempDir;

fn repo() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    let work = tmp.path().join("work");
    let ok = Command::new("git").args(["init", "-q", "-b", "main"]).arg(&work).status().unwrap();
    assert!(ok.success());
    let r = Repo::open(&work, tmp.path(), &tmp.path().join(".pitlane")).unwrap();
    (tmp, r)
}

fn q(text: &str, options: &[&str], recommended: Option<usize>) -> NewQuestion {
    NewQuestion { text: text.into(), options: options.iter().map(|s| s.to_string()).collect(), recommended }
}

#[test]
fn add_list_get_update() {
    let (_t, r) = repo();
    let a = cards::add(&r, NewCard { title: "Limit avatar size".into(), body: "## Goal\n\nRefuse big avatars.".into(), ..Default::default() }).unwrap();
    let b = cards::add(&r, NewCard { title: "Copiar link do convite".into(), ..Default::default() }).unwrap();
    assert_eq!((a.id, b.id), (1, 2));
    assert!(a.path.ends_with("cards/0001-limit-avatar-size.md"));
    assert!(a.path.starts_with(&r.board) && !a.path.starts_with(&r.root), "the board lives outside the repository");
    assert!(b.path.ends_with("cards/0002-copiar-link-do-convite.md"));

    let (all, problems) = cards::list(&r).unwrap();
    assert_eq!(all.len(), 2);
    assert!(problems.is_empty());
    assert_eq!(cards::get(&r, 1).unwrap().body, "## Goal\n\nRefuse big avatars.");

    let u = cards::update(
        &r,
        2,
        CardPatch { state: Some(CardState::Ready), lane: Some(Some("feat/invite-link".into())), ..Default::default() },
    )
    .unwrap();
    assert_eq!(u.state, CardState::Ready);
    assert_eq!(cards::get(&r, 2).unwrap().lane.as_deref(), Some("feat/invite-link"));
    let u = cards::update(&r, 2, CardPatch { lane: Some(None), ..Default::default() }).unwrap();
    assert!(u.lane.is_none());

    assert!(cards::get(&r, 9).is_err());
    assert!(cards::add(&r, NewCard { title: "  ".into(), ..Default::default() }).is_err());
}

#[test]
fn a_broken_file_is_reported_not_dropped() {
    let (_t, r) = repo();
    cards::add(&r, NewCard { title: "Fine".into(), ..Default::default() }).unwrap();
    fs::write(cards::dir(&r).join("0007-broken.md"), "no frontmatter here").unwrap();
    let (all, problems) = cards::list(&r).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("0007-broken.md"));
}

#[test]
fn concurrent_adds_never_share_an_id() {
    let (_t, r) = repo();
    let r = Arc::new(r);
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let r = Arc::clone(&r);
            std::thread::spawn(move || cards::add(&r, NewCard { title: format!("Card {i}"), ..Default::default() }).unwrap().id)
        })
        .collect();
    let mut ids: Vec<u32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, (1..=8).collect::<Vec<_>>());
    assert!(!r.board.join(".lock").exists());
}

#[test]
fn the_interview_becomes_decisions() {
    let (_t, r) = repo();
    let c = cards::add(
        &r,
        NewCard {
            title: "Rate limit the invites endpoint".into(),
            questions: vec![
                q("Per IP or per account?", &["Per IP", "Per account"], Some(1)),
                q("What window?", &["1 minute", "1 hour"], None),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.open_questions(), 2);
    assert_eq!(c.questions[1].id, 2);

    // A later batch keeps numbering.
    let c = cards::ask(&r, c.id, vec![q("Log refusals?", &[], None)]).unwrap();
    assert_eq!(c.questions[2].id, 3);
    assert!(cards::ask(&r, c.id, vec![q("Bad", &["a"], Some(4))]).is_err());

    // By option number, by free text.
    let a = cards::answer(&r, c.id, 1, "2").unwrap();
    assert_eq!(a.decision, "D-01");
    assert_eq!(a.card.questions[0].answer.as_deref(), Some("Per account"));
    assert_eq!(a.card.state, CardState::Inbox);
    assert!(cards::answer(&r, c.id, 1, "1").is_err(), "answered twice");
    cards::answer(&r, c.id, 2, "1 hour").unwrap();
    let last = cards::answer(&r, c.id, 3, "Yes, at info level").unwrap();
    assert_eq!(last.decision, "D-03");
    assert_eq!(last.card.open_questions(), 0);
    assert_eq!(last.card.state, CardState::Ready, "every question answered");

    let decisions = fs::read_to_string(cards::decisions_path(&r)).unwrap();
    assert!(decisions.contains("## D-01: Per IP or per account?\n\nPer account\n"));
    assert!(decisions.contains("From card #1 (Rate limit the invites endpoint)"));
    assert!(decisions.contains("## D-03: Log refusals?"));
}

#[test]
fn remotes_key_the_same_board() {
    for url in [
        "git@github.com:Me/amity.git",
        "https://github.com/me/amity",
        "ssh://git@github.com/me/amity.git",
        "https://token@github.com/me/amity.git/",
    ] {
        assert_eq!(cards::normalize_remote(url), "github.com/me/amity", "{url}");
    }
    let home = std::path::Path::new("/h/.pitlane");
    assert_eq!(
        cards::board_for(home, Some("git@github.com:me/amity.git"), std::path::Path::new("/x")),
        home.join("boards/github.com-me-amity")
    );
}

#[test]
fn legacy_cards_in_the_repository_move_to_the_board() {
    let tmp = TempDir::new().unwrap();
    let work = tmp.path().join("work");
    assert!(Command::new("git").args(["init", "-q", "-b", "main"]).arg(&work).status().unwrap().success());
    // Write a card the real way under another home, then put it where older versions kept it.
    let old = Repo::open(&work, tmp.path(), &tmp.path().join("old-home")).unwrap();
    let c = cards::add(&old, NewCard { title: "Old card".into(), ..Default::default() }).unwrap();
    fs::create_dir_all(work.join(".pitlane/cards")).unwrap();
    fs::rename(&c.path, work.join(".pitlane/cards").join(c.path.file_name().unwrap())).unwrap();
    fs::write(work.join(".pitlane/decisions.md"), "# Decisions\n\n## D-01: Q?\n\nA\n").unwrap();

    let r = Repo::open(&work, tmp.path(), &tmp.path().join(".pitlane")).unwrap();
    assert!(!work.join(".pitlane/cards").exists(), "moved out of the repository");
    assert!(!work.join(".pitlane/decisions.md").exists());
    let (all, problems) = cards::list(&r).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(all.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Old card"]);
    let next = cards::add(&r, NewCard { title: "New".into(), ..Default::default() }).unwrap();
    assert_eq!(next.id, 2);
    let decisions = fs::read_to_string(cards::decisions_path(&r)).unwrap();
    assert!(decisions.contains("## D-01: Q?"));
}

#[test]
fn every_worktree_sees_one_board() {
    let (tmp, r) = repo();
    let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&r.root).args(args).status().unwrap().success());
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = tmp.path().join("wt");
    git(&["worktree", "add", "-q", "-b", "feat/x", wt.to_str().unwrap()]);
    cards::add(&r, NewCard { title: "From main".into(), ..Default::default() }).unwrap();
    let in_wt = Repo::open(&wt, tmp.path(), &tmp.path().join(".pitlane")).unwrap();
    assert_eq!(in_wt.board, r.board);
    assert_eq!(cards::list(&in_wt).unwrap().0.len(), 1);
}
