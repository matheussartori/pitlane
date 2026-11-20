//! The daemon with a stand-in agent: a script that speaks Claude Code's stream-json.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use pit_core::Repo;
use pit_core::cards::{self, CardPatch, CardState, NewCard};
use pit_core::runs::{Client, RunEvent, RunState};
use pitd::{Config, Supervisor};
use tempfile::TempDir;

/// One result per user message, like the real CLI; exits when stdin closes.
/// FAKE_MODE=slow waits on SIGINT before answering; FAKE_MODE=ask ends blocked with a question.
const FAKE: &str = r#"#!/bin/bash
trap 'echo "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"num_turns\":1,\"duration_ms\":5,\"total_cost_usd\":0.01,\"usage\":{}}"; exit 130' INT
n=0
while IFS= read -r line; do
  n=$((n+1))
  if [ $n -eq 1 ]; then
    echo "$line" > "$PWD/.briefing-seen"
    echo '{"type":"system","subtype":"init","session_id":"sess-1","model":"fake-model"}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the card."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git status"}}]}}'
    echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"clean"}]}}'
    echo '{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.2,"resetsAt":99}}}}'
    if [ "$FAKE_MODE" = "slow" ]; then sleep 30 & wait $!; fi
    sleep 0.5
    if [ "$FAKE_MODE" = "ask" ]; then
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"duration_ms":100,"total_cost_usd":0.02,"usage":{"input_tokens":5,"output_tokens":7},"structured_output":{"status":"blocked","summary":"Need a limit.","questions":[{"text":"Which limit?","options":["256 KB","1 MB"],"recommended":0}]}}'
    else
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":1200,"total_cost_usd":0.05,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":3},"structured_output":{"status":"done","summary":"Did it."}}'
    fi
  else
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Noted."}]}}'
    echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10,"total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"structured_output":{"status":"done","summary":"Did it, with the note."}}'
  fi
done
"#;

struct Fx {
    _tmp: TempDir,
    home: PathBuf,
    repo: PathBuf,
    fake: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

fn fx() -> Fx {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("pitlane");
    let repo = tmp.path().join("repo");
    git(tmp.path(), &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
        git(&repo, &["config", k, v]);
    }
    fs::create_dir_all(repo.join(".pitlane")).unwrap();
    let wt_root = tmp.path().join("wt/{repo}/{branch}");
    fs::write(
        repo.join(".pitlane/flow.toml"),
        format!("[worktree]\nroot = \"{}\"\n", wt_root.display()),
    )
    .unwrap();
    fs::write(repo.join("README.md"), "demo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    git(&repo, &["branch", "feat/x"]);
    let fake = tmp.path().join("fake-claude");
    fs::write(&fake, FAKE).unwrap();
    Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
    Fx { _tmp: tmp, home, repo, fake }
}

fn card(fx: &Fx, title: &str) -> u32 {
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::add(&r, NewCard { title: title.into(), body: "Do the thing.".into(), ..Default::default() }).unwrap();
    cards::update(&r, c.id, CardPatch { lane: Some(Some("feat/x".into())), ..Default::default() }).unwrap();
    c.id
}

fn card_state(fx: &Fx, id: u32) -> (CardState, usize) {
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::get(&r, id).unwrap();
    (c.state, c.open_questions())
}

fn sup(fx: &Fx) -> Arc<Supervisor> {
    Supervisor::new(Config { home: fx.home.clone(), claude_bin: fx.fake.to_string_lossy().into_owned() })
}

async fn wait_end(s: &Arc<Supervisor>, id: &str) -> pit_core::runs::RunMeta {
    for _ in 0..200 {
        let m = s.get(id).unwrap();
        if !m.state.is_live() {
            return m;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {id} never ended");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_from_start_to_review_with_a_note_on_the_way() {
    let fx = fx();
    let id = card(&fx, "Add a line");
    let s = sup(&fx);
    let meta = s.start(fx.repo.clone(), id, None).unwrap();
    assert_eq!(meta.state, RunState::Starting);

    // Wait for the agent to be at work, then talk to it.
    for _ in 0..200 {
        if s.subscribe(&meta.id, 0).unwrap().replay.iter().any(|(_, e)| matches!(e, RunEvent::Quota { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(card_state(&fx, id).0, CardState::Running);
    s.say(&meta.id, "Also keep it short.").await.unwrap();

    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(end.model.as_deref(), Some("fake-model"));
    assert_eq!(end.session_id, "sess-1");
    assert_eq!(end.turns, Some(4));
    assert!((end.cost_usd.unwrap() - 0.06).abs() < 1e-9);
    assert_eq!(end.tokens.cache_read, 3);
    assert_eq!(end.handoff.as_ref().unwrap()["summary"], "Did it, with the note.");
    assert!(end.worktree.join(".briefing-seen").exists(), "ran in the lane's worktree");
    assert!(fs::read_to_string(end.worktree.join(".briefing-seen")).unwrap().contains("Do the thing."));
    assert_eq!(card_state(&fx, id).0, CardState::Review);

    let tl = s.subscribe(&meta.id, 0).unwrap();
    assert!(tl.rx.is_none());
    let kinds: Vec<&str> = tl
        .replay
        .iter()
        .map(|(_, e)| match e {
            RunEvent::Started { .. } => "started",
            RunEvent::Text { .. } => "text",
            RunEvent::Tool { .. } => "tool",
            RunEvent::ToolDone { .. } => "tool-done",
            RunEvent::Said { .. } => "said",
            RunEvent::Quota { .. } => "quota",
            RunEvent::Finished { .. } => "finished",
            RunEvent::Note { .. } => "note",
            RunEvent::Exited { .. } => "exited",
        })
        .filter(|k| *k != "note")
        .collect();
    assert_eq!(kinds, ["started", "text", "tool", "tool-done", "quota", "said", "finished", "text", "finished", "exited"]);
    assert_eq!(s.quota().len(), 1);
    assert!(s.say(&meta.id, "too late").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_handoff_asks_in_the_inbox() {
    let fx = fx();
    let id = card(&fx, "Limit avatars");
    // SAFETY: tests in this file that need FAKE_MODE run it through the script's env instead.
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: {
            let wrapper = fx.fake.with_file_name("fake-ask");
            fs::write(&wrapper, format!("#!/bin/bash\nFAKE_MODE=ask exec {} \"$@\"\n", fx.fake.display())).unwrap();
            Command::new("chmod").arg("+x").arg(&wrapper).status().unwrap();
            wrapper.to_string_lossy().into_owned()
        },
    });
    let meta = s.start(fx.repo.clone(), id, None).unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done);
    let (state, open) = card_state(&fx, id);
    assert_eq!(state, CardState::Blocked);
    assert_eq!(open, 1, "the agent's question is in the Inbox");
    // And a card with open questions does not start.
    assert!(s.start(fx.repo.clone(), id, None).unwrap_err().to_string().contains("open question"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_the_turn_and_frees_the_card() {
    let fx = fx();
    let id = card(&fx, "Slow one");
    let wrapper = fx.fake.with_file_name("fake-slow");
    fs::write(&wrapper, format!("#!/bin/bash\nFAKE_MODE=slow exec {} \"$@\"\n", fx.fake.display())).unwrap();
    Command::new("chmod").arg("+x").arg(&wrapper).status().unwrap();
    let s = Supervisor::new(Config { home: fx.home.clone(), claude_bin: wrapper.to_string_lossy().into_owned() });
    let meta = s.start(fx.repo.clone(), id, None).unwrap();
    for _ in 0..100 {
        if s.get(&meta.id).unwrap().pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(s.start(fx.repo.clone(), id, None).unwrap_err().to_string().contains("already running"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.stop(&meta.id).unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Stopped);
    assert_eq!(card_state(&fx, id).0, CardState::Ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn over_the_socket_with_follow_and_recovery() {
    let fx = fx();
    let id = card(&fx, "Socket");
    let cfg = Config { home: fx.home.clone(), claude_bin: fx.fake.to_string_lossy().into_owned() };
    let server = tokio::spawn(pitd::serve(cfg));
    let home = fx.home.clone();
    let repo = fx.repo.clone();
    let (events, end) = tokio::task::spawn_blocking(move || {
        let mut c = loop {
            if let Ok(c) = Client::connect(&home, None) {
                break c;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(c.call(serde_json::json!({"op":"ping"})).unwrap()["ok"], true);
        let run = c.call(serde_json::json!({"op":"start","repo": repo, "card": id})).unwrap();
        let run_id = run["run"]["id"].as_str().unwrap().to_owned();
        let mut f = Client::connect(&home, None).unwrap();
        let mut seen = Vec::new();
        let end = f.follow(&run_id, 0, |i, e| {
            seen.push((i, e));
            true
        });
        let listed = c.call(serde_json::json!({"op":"list"})).unwrap();
        assert_eq!(listed["runs"].as_array().unwrap().len(), 1);
        assert!(c.call(serde_json::json!({"op":"nope"})).is_err());
        (seen, end.unwrap().unwrap())
    })
    .await
    .unwrap();
    assert_eq!(end.state, RunState::Done);
    assert!(matches!(events.last().unwrap().1, RunEvent::Exited { .. }));
    assert!(events.windows(2).all(|w| w[1].0 == w[0].0 + 1), "no gaps");
    server.abort();

    // A run left open by a dead daemon is marked interrupted on the next start.
    let s = sup(&fx);
    let mut m = s.get(&end.id).unwrap();
    m.state = RunState::Running;
    fs::write(fx.home.join("runs").join(&m.id).join("meta.json"), serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!(s.recover().unwrap(), 1);
    assert_eq!(s.get(&end.id).unwrap().state, RunState::Interrupted);
}
