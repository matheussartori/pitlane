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
    NewQuestion { text: text.into(), options: options.iter().map(|s| s.to_string()).collect(), recommended, context: None }
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

    // Choosing again rewrites the same decision in place; the others stay as they were.
    let again = cards::reanswer(&r, c.id, 1, "1").unwrap();
    assert_eq!(again.decision, "D-01");
    assert_eq!(again.card.questions[0].answer.as_deref(), Some("Per IP"));
    assert!(cards::reanswer(&r, c.id, 1, " ").is_err());
    let decisions = fs::read_to_string(cards::decisions_path(&r)).unwrap();
    assert!(decisions.contains("## D-01: Per IP or per account?\n\nPer IP\n"));
    assert!(decisions.contains("changed from: Per account"));
    assert!(!decisions.contains("\n\nPer account\n"));
    assert_eq!(decisions.matches("## D-01").count(), 1);
    assert!(decisions.contains("## D-02: What window?\n\n1 hour\n"));
    assert!(decisions.contains("## D-03: Log refusals?\n\nYes, at info level\n"));
    // The last entry can change too.
    cards::reanswer(&r, c.id, 3, "No").unwrap();
    let decisions = fs::read_to_string(cards::decisions_path(&r)).unwrap();
    assert!(decisions.contains("## D-03: Log refusals?\n\nNo\n") && decisions.ends_with("changed from: Yes, at info level\n"));

    // Talking a decided question over: your turn, then Claude's, whose options become the
    // question's; the decision stays until you choose again. It survives the card file.
    assert!(cards::say(&r, c.id, 1, "you", "  ", None).is_err());
    cards::say(&r, c.id, 1, "you", "And per API key?", None).unwrap();
    let offer = cards::NewQuestion {
        text: String::new(),
        options: vec!["Per IP".into(), "Per API key".into(), " ".into()],
        recommended: Some(1),
        context: Some("Keys are what the partners share.".into()),
    };
    cards::say(&r, c.id, 1, "claude", "Per key is fairer to partners.", Some(offer)).unwrap();
    let back = cards::get(&r, c.id).unwrap();
    let q1 = &back.questions[0];
    assert_eq!(q1.talk.len(), 2);
    assert_eq!((q1.talk[0].by.as_str(), q1.talk[1].by.as_str()), ("you", "claude"));
    assert_eq!(q1.options, vec!["Per IP", "Per API key"]);
    assert_eq!(q1.recommended, Some(1));
    assert_eq!(q1.context.as_deref(), Some("Keys are what the partners share."));
    assert_eq!(q1.answer.as_deref(), Some("Per IP"));
    assert_eq!(cards::reanswer(&r, c.id, 1, "2").unwrap().card.questions[0].answer.as_deref(), Some("Per API key"));
    let prompt = pit_core::analyze::talk_prompt(&r, &cards::get(&r, c.id).unwrap(), 1).unwrap();
    assert!(prompt.contains("The person already decided: Per API key (D-01)"));
    assert!(prompt.contains("The person: And per API key?") && prompt.contains("You: Per key is fairer"));
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

#[test]
fn delete_and_restore() {
    let (_d, r) = repo();
    let a = cards::add(&r, NewCard { title: "First".into(), ..Default::default() }).unwrap();
    let b = cards::add(&r, NewCard { title: "Second".into(), ..Default::default() }).unwrap();
    cards::mark_analyzed(&r, b.id, vec![a.id], vec!["engine".into()]).unwrap();
    cards::delete(&r, b.id).unwrap();
    assert!(cards::get(&r, b.id).is_err());
    // The number stays taken while the card is in the deleted folder.
    let c = cards::add(&r, NewCard { title: "Third".into(), ..Default::default() }).unwrap();
    assert_eq!(c.id, 3);
    cards::delete(&r, a.id).unwrap();
    let back = cards::restore(&r, b.id).unwrap();
    assert_eq!(back.title, "Second");
    assert!(back.after.is_empty() || back.after == vec![a.id]);
    assert!(cards::restore(&r, 99).is_err());
    // Editing the spec makes it a new card to the analyst.
    let edited = cards::update(&r, b.id, CardPatch { body: Some("changed".into()), ..Default::default() }).unwrap();
    assert!(edited.analyzed.is_none() && edited.areas.is_empty());
    // Forget: Claude's notes and open questions go, yours and your answers stay.
    cards::note(&r, b.id, "claude", "found it in src/x.ts").unwrap();
    cards::note(&r, b.id, "you", "mine").unwrap();
    cards::ask(&r, b.id, vec![q("Open?", &["a", "b"], Some(0)), q("Answered?", &["a", "b"], Some(0))]).unwrap();
    let qid = cards::get(&r, b.id).unwrap().questions[1].id;
    cards::answer(&r, b.id, qid, "a").unwrap();
    cards::mark_analyzed(&r, b.id, vec![], vec!["engine".into()]).unwrap();
    let f = cards::forget(&r, &[b.id]).unwrap();
    assert_eq!((f.notes, f.questions), (1, 1));
    let left = cards::get(&r, b.id).unwrap();
    assert!(left.analyzed.is_none() && left.areas.is_empty());
    assert_eq!(left.notes.len(), 1);
    assert_eq!(left.questions.len(), 1);
}

#[test]
fn images_ride_with_the_card() {
    let (_t, r) = repo();
    let c = cards::add(&r, NewCard { title: "Kart menu".into(), body: "Make it pop.".into(), ..Default::default() }).unwrap();
    assert!(cards::attach(&r, c.id, "notes.txt", b"x").is_err(), "only images");
    let c = cards::attach(&r, c.id, "Screen Shot 1.png", b"\x89PNG fake").unwrap();
    let c2 = cards::attach(&r, c.id, "shot.JPG", b"jpeg").unwrap();
    assert_eq!(c2.images, ["1-screen-shot-1.png", "2-shot.jpg"]);
    assert!(cards::images_dir(&r, c.id).join("2-shot.jpg").is_file());
    let got = cards::image_bytes(&r, &c2);
    assert_eq!(got[0].0, "image/png");
    assert_eq!(got[1].1, b"jpeg");

    // The briefing names them, and the first message carries them before the text.
    let b = pit_core::runs::briefing(&r, &c2);
    assert!(b.contains("## Images") && b.contains("2 images") && b.contains("2-shot.jpg"));
    let line: serde_json::Value = serde_json::from_str(&pit_core::runs::claude_user_line_with("hi", &got)).unwrap();
    let content = line["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["source"]["media_type"], "image/png");
    assert_eq!(content[2]["text"], "hi");

    let c3 = cards::detach(&r, c.id, "1-screen-shot-1.png").unwrap();
    assert_eq!(c3.images, ["2-shot.jpg"]);
    assert!(!cards::images_dir(&r, c.id).join("1-screen-shot-1.png").exists());
    // The free number is reused only once its file is gone.
    let c4 = cards::attach(&r, c.id, "again.png", b"p").unwrap();
    assert_eq!(c4.images, ["2-shot.jpg", "1-again.png"]);
    assert_eq!(cards::get(&r, c.id).unwrap().images.len(), 2, "kept in the card file");
}

#[test]
fn a_note_holding_a_diff_keeps_the_card_readable() {
    let (_t, r) = repo();
    let c = cards::add(&r, NewCard { title: "Diff in a note".into(), body: "Spec.".into(), ..Default::default() }).unwrap();
    let diff = "--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1 +1 @@\n-old\n+new";
    cards::note(&r, c.id, "claude", diff).unwrap();
    let back = cards::get(&r, c.id).unwrap();
    assert_eq!(back.body, "Spec.");
    assert!(back.notes.iter().any(|n| n.text == diff));
    let (all, problems) = cards::list(&r).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(all.len(), 1);
}

#[test]
fn a_leftover_temp_file_is_not_the_card() {
    let (_t, r) = repo();
    let c = cards::add(&r, NewCard { title: "Real".into(), ..Default::default() }).unwrap();
    // A write that crashed between writing and renaming leaves this behind.
    fs::write(c.path.with_extension("tmp999"), "+++\nbroken").unwrap();
    for _ in 0..5 {
        assert_eq!(cards::get(&r, c.id).unwrap().path, c.path);
    }
    let u = cards::update(&r, c.id, CardPatch { state: Some(CardState::Ready), ..Default::default() }).unwrap();
    assert_eq!(u.path, c.path);
    assert_eq!(cards::get(&r, c.id).unwrap().state, CardState::Ready);
}

#[test]
fn a_blocked_card_always_says_why_and_forgets_it_when_it_moves_on() {
    let (_t, r) = repo();
    let c = cards::add(&r, NewCard { title: "Stuck".into(), ..Default::default() }).unwrap();
    let b = cards::update(&r, c.id, CardPatch { state: Some(CardState::Blocked), ..Default::default() }).unwrap();
    assert_eq!(b.blocked.as_ref().map(|b| b.reason), Some(cards::BlockReason::Other), "blocked by hand");
    let why = cards::Block::new(cards::BlockReason::Questions, "Which limit?", Some("r1".into()));
    let b = cards::update(&r, c.id, CardPatch { state: Some(CardState::Blocked), blocked: Some(why), ..Default::default() }).unwrap();
    assert!(cards::wants_recheck(&b), "blocked on questions, none open");
    let checking = cards::set_checking(&r, c.id, true).unwrap();
    assert!(!cards::wants_recheck(&checking), "not twice");
    // Written and read back as TOML.
    let again = cards::get(&r, c.id).unwrap();
    assert_eq!(again.blocked.as_ref().map(|b| (b.reason, b.detail.as_str())), Some((cards::BlockReason::Questions, "Which limit?")));
    let ready = cards::update(&r, c.id, CardPatch { state: Some(CardState::Ready), ..Default::default() }).unwrap();
    assert!(ready.blocked.is_none());
    assert!(cards::set_checking(&r, c.id, false).is_err(), "not blocked any more");
}

/// One move by hand, carried out as the daemon does (minus stopping a run).
fn mv(r: &Repo, id: u32, to: cards::MoveTo, live: bool) -> anyhow::Result<(cards::Card, bool)> {
    let c = cards::get(r, id)?;
    let m = cards::move_rule(&c, to, live, Some("waits for the art"))?;
    Ok((cards::update(r, id, m.patch)?, m.stop))
}

#[test]
fn moves_by_hand_follow_the_board_rules() {
    use cards::MoveTo::{Backlog, Hold, Queue};
    let (_t, r) = repo();
    let c = cards::add(&r, NewCard { title: "Siege map".into(), ..Default::default() }).unwrap();

    // Backlog → queue → backlog: never read by Claude, so it is new again.
    assert_eq!(mv(&r, c.id, Queue, false).unwrap().0.state, CardState::Queued);
    assert_eq!(mv(&r, c.id, Backlog, false).unwrap().0.state, CardState::Inbox);

    // Held: stays in the Backlog with why; queueing it lets it go.
    let held = mv(&r, c.id, Hold, false).unwrap().0;
    assert_eq!((held.state, held.held.as_deref()), (CardState::Inbox, Some("waits for the art")));
    let queued = mv(&r, c.id, Queue, false).unwrap().0;
    assert_eq!((queued.state, queued.held), (CardState::Queued, None));
    // Held from the queue: back in the Backlog, held.
    let held = mv(&r, c.id, Hold, false).unwrap().0;
    assert_eq!((held.state, held.held.is_some()), (CardState::Inbox, true));

    // Running: out of In progress stops the run; it cannot be held there, nor moved while checked.
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Running), ..Default::default() }).unwrap();
    assert!(cards::get(&r, c.id).unwrap().held.is_none(), "held only in the Backlog");
    assert!(mv(&r, c.id, Hold, true).is_err());
    let (back, stop) = mv(&r, c.id, Queue, true).unwrap();
    assert_eq!((back.state, stop), (CardState::Queued, true));
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Verifying), ..Default::default() }).unwrap();
    assert!(mv(&r, c.id, Backlog, false).is_err());

    // Review: to the Backlog is Discard (the lane stays); the queue is not a way out of a review.
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Review), lane: Some(Some("feat/siege".into())), ..Default::default() }).unwrap();
    assert!(mv(&r, c.id, Queue, false).is_err());
    let discarded = mv(&r, c.id, Backlog, false).unwrap().0;
    assert_eq!((discarded.state, discarded.lane.as_deref()), (CardState::Ready, Some("feat/siege")));

    // Blocked: to the queue with its retries reset, not while it asks.
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Blocked), retries: Some(2), ..Default::default() }).unwrap();
    let freed = mv(&r, c.id, Queue, false).unwrap().0;
    assert_eq!((freed.state, freed.retries, freed.blocked.is_none()), (CardState::Queued, 0, true));
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Blocked), ..Default::default() }).unwrap();
    cards::ask(&r, c.id, vec![q("Day or night?", &["Day", "Night"], Some(0))]).unwrap();
    assert!(mv(&r, c.id, Queue, false).is_err());

    // Landed: reopened in the Backlog, without its old lane; never straight to the queue.
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Done), ..Default::default() }).unwrap();
    assert!(mv(&r, c.id, Queue, false).is_err());
    let reopened = mv(&r, c.id, Backlog, false).unwrap().0;
    assert_eq!((reopened.state, reopened.lane), (CardState::Ready, None));
}
