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

/// One result per user message; exits when stdin closes.
/// FAKE_MODE=slow waits on SIGINT before answering; FAKE_MODE=ask ends blocked with a question;
/// FAKE_MODE=ask-done finishes and asks anyway; FAKE_MODE=related commits in the related repo.
/// FAKE_MODE=fold takes a message that arrives mid-turn into that turn, as Claude Code does: it
/// gets no result of its own. FAKE_MODE=server hands off with a background task still running.
/// FAKE_MODE=linger hands off and then never leaves, whatever stdin does.
/// FAKE_MODE=partial hands off one stage of a larger card, with what is left.
const FAKE: &str = r#"#!/bin/bash
# The judge, a side session: one prompt line on stdin, one result. FAKE_VERDICT picks the verdict.
if [[ " $* " == *" --permission-prompts none "* ]]; then
  IFS= read -r line; echo "$line" > "$PWD/.judge-prompt"
  echo "{\"type\":\"result\",\"subtype\":\"success\",\"num_turns\":1,\"usage\":{},\"is_error\":false,\"total_cost_usd\":0.01,\"duration_ms\":40,\"structured_output\":{\"verdict\":\"${FAKE_VERDICT:-changes}\",\"summary\":\"Needs the SVG case.\",\"findings\":[{\"file\":\"a.ts\",\"line\":3,\"severity\":\"major\",\"text\":\"SVG is not refused.\"}]}}"
  exit 0
fi
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
    elif [ "$FAKE_MODE" = "related" ]; then
      echo "$*" > "$PWD/.args"
      p=$(grep -o -- '- games: `[^`]*`' "$PWD/.briefing-seen" | head -1 | sed 's/.*`\(.*\)`/\1/')
      echo "fixed" > "$p/fix.txt" && git -C "$p" add fix.txt && git -C "$p" -c user.name=T -c user.email=t@example.com commit -qm "fix in games"
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":100,"total_cost_usd":0.01,"usage":{},"structured_output":{"status":"done","summary":"Fixed it in games."}}'
    elif [ "$FAKE_MODE" = "ask-done" ]; then
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"duration_ms":100,"total_cost_usd":0.02,"usage":{"input_tokens":5,"output_tokens":7},"structured_output":{"status":"done","summary":"Did it.","questions":[{"text":"Also the studio?","options":["No","Yes"],"recommended":0}]}}'
    elif [ "$FAKE_MODE" = "partial" ]; then
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":100,"total_cost_usd":0.01,"usage":{},"structured_output":{"status":"partial","summary":"Stage 1 of 3: measured.","left":["Stage 2: the walls","Stage 3: the props"]}}'
    elif [ "$FAKE_MODE" = "server" ]; then
      echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"id":"b1","command":"npm run dev"}]}'
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":100,"total_cost_usd":0.01,"usage":{},"structured_output":{"status":"done","summary":"Did it, server left up."}}'
    elif [ "$FAKE_MODE" = "linger" ]; then
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":100,"total_cost_usd":0.01,"usage":{},"structured_output":{"status":"done","summary":"Did it, and stayed."}}'
      sleep 600
    elif [ "$FAKE_MODE" = "self-turn" ]; then
      # Out of turns while a background task was due; Claude Code then starts a turn by itself.
      echo '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":3,"duration_ms":100,"total_cost_usd":0.02,"usage":{}}'
      sleep 0.3
      echo '{"type":"system","subtype":"init","session_id":"sess-1","model":"fake-model"}'
      echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The suite finished."}]}}'
      sleep 1
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10,"total_cost_usd":0.01,"usage":{},"structured_output":{"status":"done","summary":"Did it."}}'
    else
      echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":1200,"total_cost_usd":0.05,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":3},"structured_output":{"status":"done","summary":"Did it."}}'
    fi
  elif [ "$FAKE_MODE" = "fold" ]; then
    continue
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
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

fn fx() -> Fx {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("pitlane");
    let repo = tmp.path().join("repo");
    git(
        tmp.path(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    for (k, v) in [
        ("user.name", "T"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&repo, &["config", k, v]);
    }
    fs::create_dir_all(repo.join(".pitlane")).unwrap();
    let wt_root = tmp.path().join("wt/{repo}/{branch}");
    fs::write(
        repo.join(".pitlane/flow.toml"),
        format!(
            "[worktree]\nroot = \"{}\"\n\n[verify]\njudge = false\n",
            wt_root.display()
        ),
    )
    .unwrap();
    fs::write(repo.join("README.md"), "demo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    git(&repo, &["branch", "feat/x"]);
    let fake = tmp.path().join("fake-claude");
    fs::write(&fake, FAKE).unwrap();
    Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
    Fx {
        _tmp: tmp,
        home,
        repo,
        fake,
    }
}

fn card(fx: &Fx, title: &str) -> u32 {
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::add(
        &r,
        NewCard {
            title: title.into(),
            body: "Do the thing.".into(),
            ..Default::default()
        },
    )
    .unwrap();
    cards::update(
        &r,
        c.id,
        CardPatch {
            lane: Some(Some("feat/x".into())),
            ..Default::default()
        },
    )
    .unwrap();
    c.id
}

fn card_state(fx: &Fx, id: u32) -> (CardState, usize) {
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::get(&r, id).unwrap();
    (c.state, c.open_questions())
}

fn sup(fx: &Fx) -> Arc<Supervisor> {
    Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: fx.fake.to_string_lossy().into_owned(),
        pit_bin: None,
    })
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
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    assert_eq!(meta.state, RunState::Starting);

    // Wait for the agent to be at work, then talk to it.
    for _ in 0..200 {
        if s.subscribe(&meta.id, 0)
            .unwrap()
            .replay
            .iter()
            .any(|(_, e)| matches!(e, RunEvent::Quota { .. }))
        {
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
    assert_eq!(
        end.handoff.as_ref().unwrap()["summary"],
        "Did it, with the note."
    );
    assert!(
        end.worktree.join(".briefing-seen").exists(),
        "ran in the lane's worktree"
    );
    assert!(
        fs::read_to_string(end.worktree.join(".briefing-seen"))
            .unwrap()
            .contains("Do the thing.")
    );
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
            RunEvent::Shot { .. } => "shot",
            RunEvent::Tuned { .. } => "tuned",
            RunEvent::Quota { .. } => "quota",
            RunEvent::Finished { .. } => "finished",
            RunEvent::Note { .. } => "note",
            RunEvent::Approval { .. } => "approval",
            RunEvent::Limited { .. } => "limited",
            RunEvent::Proof { .. } => "proof",
            RunEvent::Judged { .. } => "judged",
            RunEvent::Decided { .. } => "decided",
            RunEvent::Exited { .. } => "exited",
        })
        .filter(|k| *k != "note")
        .collect();
    assert_eq!(
        kinds,
        [
            "started",
            "text",
            "tool",
            "tool-done",
            "quota",
            "said",
            "finished",
            "text",
            "finished",
            "exited"
        ]
    );
    assert_eq!(s.quota().len(), 1);
    assert!(s.say(&meta.id, "too late").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_agent_starts_by_itself_still_hears_you() {
    let fx = fx();
    let id = card(&fx, "Add a line");
    let wrapper = fx.home.join("self-turn.sh");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/bash\nFAKE_MODE=self-turn exec {} \"$@\"\n",
            fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&wrapper)
        .status()
        .unwrap();
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    });
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();

    // Wait for the second turn, the one nobody sent, then talk to it.
    for _ in 0..300 {
        let started = s
            .subscribe(&meta.id, 0)
            .unwrap()
            .replay
            .iter()
            .filter(|(_, e)| matches!(e, RunEvent::Started { .. }))
            .count();
        if started == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    s.say(&meta.id, "What is the progress?").await.unwrap();

    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(
        end.handoff.as_ref().unwrap()["summary"],
        "Did it, with the note."
    );
}

/// The stand-in agent in one of its modes.
fn fake_in(fx: &Fx, mode: &str) -> Arc<Supervisor> {
    let wrapper = fx.fake.with_file_name(format!("fake-{mode}"));
    fs::write(
        &wrapper,
        format!(
            "#!/bin/bash\nFAKE_MODE={mode} exec {} \"$@\"\n",
            fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&wrapper)
        .status()
        .unwrap();
    Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    })
}

/// The run ends by itself within `secs`, or the test fails: nobody stops it.
async fn ends_within(s: &Arc<Supervisor>, id: &str, secs: u64) -> pit_core::runs::RunMeta {
    for _ in 0..secs * 20 {
        let m = s.get(id).unwrap();
        if !m.state.is_live() {
            return m;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {id} was still open after {secs} s with its work handed off");
}

/// What happened on the night of the stuck board: a message sent while the agent worked was
/// taken into that turn, so the session had two messages and one result, and waited for a second
/// result for ever. The run stayed "running" for eight hours with its work long handed off.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_taken_into_the_turn_does_not_keep_the_run_open() {
    let fx = fx();
    let id = card(&fx, "Add a line");
    let s = fake_in(&fx, "fold");
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    for _ in 0..200 {
        if s.subscribe(&meta.id, 0)
            .unwrap()
            .replay
            .iter()
            .any(|(_, e)| matches!(e, RunEvent::Quota { .. }))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    s.say(&meta.id, "Just move it back and stop.").await.unwrap();

    let end = ends_within(&s, &meta.id, 20).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(end.handoff.as_ref().unwrap()["summary"], "Did it.");
    assert_eq!(card_state(&fx, id).0, CardState::Review);
    assert!(s.list(None).unwrap().iter().all(|m| !m.state.is_live()));
}

/// The handoff is the end of the work: what the agent left running (a dev server) holds the
/// session for a moment, not for a quarter of an hour, and goes with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_ends_the_run_whatever_was_left_running() {
    let fx = fx();
    let id = card(&fx, "Add a line");
    let s = fake_in(&fx, "server");
    s.set_patience(Duration::from_secs(30), Duration::from_millis(500));
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    let end = ends_within(&s, &meta.id, 15).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(card_state(&fx, id).0, CardState::Review);
}

/// Closing the session is a request. An agent that stays anyway is ended by the daemon, and
/// the card moves on with the work it handed off.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_does_not_leave_is_ended() {
    let fx = fx();
    let id = card(&fx, "Add a line");
    let s = fake_in(&fx, "linger");
    s.set_patience(Duration::from_millis(400), Duration::from_secs(120));
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    let end = ends_within(&s, &meta.id, 15).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(
        end.handoff.as_ref().unwrap()["summary"],
        "Did it, and stayed."
    );
    assert_eq!(card_state(&fx, id).0, CardState::Review);
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
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/bash\nFAKE_MODE=ask exec {} \"$@\"\n",
                    fx.fake.display()
                ),
            )
            .unwrap();
            Command::new("chmod")
                .arg("+x")
                .arg(&wrapper)
                .status()
                .unwrap();
            wrapper.to_string_lossy().into_owned()
        },
        pit_bin: None,
    });
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done);
    let (state, open) = card_state(&fx, id);
    assert_eq!(state, CardState::Blocked);
    assert_eq!(open, 1, "the agent's question is in the Inbox");
    // It says why, and keeps the run to resume.
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::get(&r, id).unwrap();
    let b = c.blocked.clone().expect("a blocked card says why");
    assert_eq!((b.reason, b.run.as_deref()), (cards::BlockReason::Questions, Some(meta.id.as_str())));
    assert_eq!(c.paused.as_deref(), Some(meta.id.as_str()), "the next run picks up this session");
    assert!(!cards::wants_recheck(&c), "not while a question is open");
    // And a card with open questions does not start.
    assert!(
        s.start(fx.repo.clone(), id, None, Default::default())
            .unwrap_err()
            .to_string()
            .contains("open question")
    );
    // Answered: it waits on a recheck; moved on by hand, it forgets why it was blocked.
    let q = c.questions[0].id;
    let a = cards::answer(&r, id, q, "1").unwrap();
    assert!(cards::wants_recheck(&a.card), "every question answered: read it again");
    let moved = cards::update(&r, id, CardPatch { state: Some(CardState::Ready), ..Default::default() }).unwrap();
    assert!(moved.blocked.is_none());
    assert_eq!(moved.paused.as_deref(), Some(meta.id.as_str()), "still the session to resume (see the stopped-card test)");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_run_does_not_ask() {
    let fx = fx();
    let id = card(&fx, "Loading screen");
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: {
            let wrapper = fx.fake.with_file_name("fake-ask-done");
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/bash\nFAKE_MODE=ask-done exec {} \"$@\"\n",
                    fx.fake.display()
                ),
            )
            .unwrap();
            Command::new("chmod")
                .arg("+x")
                .arg(&wrapper)
                .status()
                .unwrap();
            wrapper.to_string_lossy().into_owned()
        },
        pit_bin: None,
    });
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done);
    let (state, open) = card_state(&fx, id);
    assert_eq!(state, CardState::Review);
    assert_eq!(open, 0, "nothing lands in the Inbox after Run");
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::get(&r, id).unwrap();
    assert!(c.notes.iter().any(|n| {
        n.text
            .contains("Decided without asking: Also the studio? Went with: No")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_the_turn_and_frees_the_card() {
    let fx = fx();
    let id = card(&fx, "Slow one");
    let wrapper = fx.fake.with_file_name("fake-slow");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/bash\nFAKE_MODE=slow exec {} \"$@\"\n",
            fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&wrapper)
        .status()
        .unwrap();
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    });
    let meta = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    for _ in 0..100 {
        if s.get(&meta.id).unwrap().pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        s.start(fx.repo.clone(), id, None, Default::default())
            .unwrap_err()
            .to_string()
            .contains("already running")
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.stop(&meta.id).unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Stopped);
    assert_eq!(card_state(&fx, id).0, CardState::Ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_card_resumes_its_session_with_what_it_got_meanwhile() {
    let fx = fx();
    let id = card(&fx, "Slow one");
    let wrapper = fx.fake.with_file_name("fake-slow-args");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/bash\n[[ \" $* \" == *\" -p \"* ]] && echo \"$*\" > \"$PWD/.args\"\nFAKE_MODE=slow exec {} \"$@\"\n",
            fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&wrapper).status().unwrap();
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    });
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let run_and_stop = async |opts: pitd::supervisor::StartOpts| {
        let meta = s.start(fx.repo.clone(), id, None, opts).unwrap();
        // Up to ten seconds: under a loaded machine the agent can take a while to be on.
        for _ in 0..500 {
            if s.get(&meta.id).unwrap().pid.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(s.get(&meta.id).unwrap().pid.is_some(), "the agent never started");
        tokio::time::sleep(Duration::from_millis(300)).await;
        s.stop(&meta.id).unwrap();
        let end = wait_end(&s, &meta.id).await;
        assert_eq!(end.state, RunState::Stopped);
        end
    };

    // Stopped mid-work: the card keeps the run, to resume it.
    let first = run_and_stop(Default::default()).await;
    let c = cards::get(&r, id).unwrap();
    assert_eq!((c.state, c.paused.as_deref()), (CardState::Ready, Some(first.id.as_str())));

    // Claude Code saved the session; a note arrives while the agent is away.
    let claude = fx.home.with_file_name("claude-config");
    fs::create_dir_all(claude.join("projects/-wt")).unwrap();
    fs::write(claude.join("projects/-wt/sess-1.jsonl"), "{}\n").unwrap();
    // SAFETY: only this test reads it, and the stand-in agent ignores it.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &claude) };
    cards::note(&r, id, "you", "Use 1 MB.").unwrap();

    // Run again: the same session goes on, told what it missed; the card lets go of the old run.
    let second = run_and_stop(Default::default()).await;
    assert_eq!(second.resumed_from.as_deref(), Some(first.id.as_str()));
    assert_eq!(second.session_id, "sess-1");
    assert!(fs::read_to_string(second.worktree.join(".args")).unwrap().contains("--resume sess-1"));
    let said = fs::read_to_string(second.worktree.join(".briefing-seen")).unwrap();
    assert!(said.contains("same session") && said.contains("Use 1 MB."), "{said}");
    assert_eq!(cards::get(&r, id).unwrap().paused.as_deref(), Some(second.id.as_str()));

    // Start over: a new session from the card.
    let third = run_and_stop(pitd::supervisor::StartOpts { fresh: true, ..Default::default() }).await;
    assert_eq!(third.resumed_from, None);
    assert!(!fs::read_to_string(third.worktree.join(".args")).unwrap().contains("--resume"));
}

#[tokio::test(flavor = "multi_thread")]
async fn over_the_socket_with_follow_and_recovery() {
    let fx = fx();
    let id = card(&fx, "Socket");
    let cfg = Config {
        home: fx.home.clone(),
        claude_bin: fx.fake.to_string_lossy().into_owned(),
        pit_bin: None,
    };
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
        assert!(c.ping().unwrap().ok);
        let run_id = c
            .start(pit_core::protocol::StartArgs { repo, card: id, ..Default::default() })
            .unwrap()
            .id;
        let mut f = Client::connect(&home, None).unwrap();
        let mut seen = Vec::new();
        let end = f.follow(&run_id, 0, |i, e| {
            seen.push((i, e));
            true
        });
        assert_eq!(c.list(None).unwrap().len(), 1);
        // As an older client wrote it, by hand: optional keys left out, one nobody knows.
        let got = c.call(serde_json::json!({"op":"get","run": run_id, "extra": 1})).unwrap();
        assert_eq!(got["run"]["id"], run_id.as_str());
        let e = c.call(serde_json::json!({"op":"nope"})).unwrap_err();
        assert_eq!(e.to_string(), "unknown op \"nope\"");
        assert!(c.call(serde_json::json!({"op":"say","run": run_id})).is_err(), "text is required");
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
    fs::write(
        fx.home.join("runs").join(&m.id).join("meta.json"),
        serde_json::to_string(&m).unwrap(),
    )
    .unwrap();
    assert_eq!(s.recover().unwrap(), 1);
    assert_eq!(s.get(&end.id).unwrap().state, RunState::Interrupted);
}

#[tokio::test(flavor = "multi_thread")]
async fn approvals_wait_for_a_person_and_remember_rules() {
    use pit_core::runs::Decision;
    use serde_json::json;
    let fx = fx();
    let id = card(&fx, "Needs commands");
    let wrapper = fx.fake.with_file_name("fake-slow-2");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/bash\nFAKE_MODE=slow exec {} \"$@\"\n",
            fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&wrapper)
        .status()
        .unwrap();
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    });
    let run = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap()
        .id;
    for _ in 0..100 {
        if s.get(&run).unwrap().pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Asked: it waits, everyone sees it, a person answers "for this run".
    let ask = |cmd: &str, use_id: &str| {
        let (s, run, cmd, use_id) = (
            Arc::clone(&s),
            run.clone(),
            cmd.to_owned(),
            use_id.to_owned(),
        );
        tokio::spawn(async move {
            s.ask(&run, "Bash", json!({ "command": cmd }), Some(use_id))
                .await
        })
    };
    let first = ask("npm test -- drift", "t1");
    for _ in 0..100 {
        if !s.approvals().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let waiting = s.approvals();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].1.rule, "Bash(npm test:*)");
    assert_eq!(s.get(&run).unwrap().waiting.len(), 1);
    assert_eq!(s.decide(&run, None, Decision::Run, None).unwrap(), "t1");
    assert_eq!(first.await.unwrap()["behavior"], "allow");
    assert!(s.get(&run).unwrap().waiting.is_empty());

    // The same command again: allowed by the run's rule, nobody asked.
    let again = s
        .ask(
            &run,
            "Bash",
            json!({ "command": "npm test -- kart" }),
            Some("t2".into()),
        )
        .await;
    assert_eq!(again["behavior"], "allow");
    assert!(s.approvals().is_empty());

    // Always: saved to flow.toml, which keeps its own content.
    let py = ask("python3 scripts/check.py", "t3");
    while s.approvals().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    s.decide(&run, Some("t3"), Decision::Always, None).unwrap();
    assert_eq!(py.await.unwrap()["behavior"], "allow");
    let flow = fs::read_to_string(fx.repo.join(".pitlane/flow.toml")).unwrap();
    assert!(flow.contains("Bash(python3:*)"), "{flow}");
    assert!(flow.contains("[worktree]"), "the rest of the file is kept");

    // Deny with a reason the agent reads.
    let rm = ask("rm -rf build", "t4");
    while s.approvals().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    s.decide(
        &run,
        None,
        Decision::Deny,
        Some("Use npm run clean.".into()),
    )
    .unwrap();
    let a = rm.await.unwrap();
    assert_eq!(
        (a["behavior"].as_str(), a["message"].as_str()),
        (Some("deny"), Some("Use npm run clean."))
    );

    // Still waiting when the run ends: the agent hears no.
    let late = ask("curl example.com", "t5");
    while s.approvals().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    s.stop(&run).unwrap();
    assert_eq!(late.await.unwrap()["behavior"], "deny");
    wait_end(&s, &run).await;
    assert_eq!(
        s.ask(&run, "Bash", json!({ "command": "ls" }), None).await["behavior"],
        "deny"
    );

    let tl = s.subscribe(&run, 0).unwrap().replay;
    assert_eq!(
        tl.iter()
            .filter(|(_, e)| matches!(e, RunEvent::Approval { .. }))
            .count(),
        4
    );
    assert_eq!(
        tl.iter()
            .filter(|(_, e)| matches!(e, RunEvent::Decided { .. }))
            .count(),
        3
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limit_hands_the_card_to_the_next_option_with_your_yes() {
    let fx = fx();
    let id = card(&fx, "Long one");
    let dir = fx.fake.parent().unwrap().to_path_buf();
    let flag = dir.join("limit-once");
    let args_log = dir.join("args.log");
    fs::write(&flag, "").unwrap();
    // The first launch hits the 5-hour limit after writing a file; later ones work normally.
    let wrapper = dir.join("fake-limit");
    fs::write(
        &wrapper,
        format!(
            r#"#!/bin/bash
echo "ARGS $*" >> {log}
echo "CCD=$CLAUDE_CONFIG_DIR" >> {log}
if [ "$1" = "-p" ] && [ -f {flag} ]; then
  rm {flag}
  IFS= read -r line
  echo '{{"type":"system","subtype":"init","session_id":"s-lim","model":"fake-model"}}'
  echo partial > partial.txt
  echo '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour","resetsAt":99,"unifiedWindows":{{"five_hour":{{"utilization":1.0,"resetsAt":99}}}}}}}}'
  echo '{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude AI usage limit reached|99","num_turns":2,"duration_ms":50,"total_cost_usd":0.03,"usage":{{}}}}'
  cat > /dev/null
  exit 1
fi
exec {fake} "$@"
"#,
            log = args_log.display(),
            flag = flag.display(),
            fake = fx.fake.display()
        ),
    )
    .unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&wrapper)
        .status()
        .unwrap();
    fs::create_dir_all(&fx.home).unwrap();
    fs::write(
        fx.home.join("routing.toml"),
        "[[account]]\nid = \"api\"\nkind = \"api-key\"\napi_key_helper = \"echo not-a-real-key\"\n\n[route.run]\nprefer = [\"@repo\", \"haiku@api\"]\n",
    )
    .unwrap();
    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: wrapper.to_string_lossy().into_owned(),
        pit_bin: None,
    });

    let first = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    assert_eq!(first.account, "default");
    assert!(
        first
            .reason
            .as_deref()
            .unwrap()
            .contains("first in the route")
    );
    let end = wait_end(&s, &first.id).await;
    for _ in 0..100 {
        if s.get(&first.id).unwrap().next.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let end = s.get(&end.id).unwrap();
    assert_eq!(end.state, RunState::Failed);
    assert!(
        end.error
            .as_deref()
            .unwrap()
            .contains("Hit the usage limit on default"),
        "{:?}",
        end.error
    );
    assert_eq!(end.next.as_deref(), Some("haiku on api"));
    assert_eq!(end.resets_at, Some(99));
    assert_eq!(
        card_state(&fx, id).0,
        CardState::Blocked,
        "waits for your yes to the API key"
    );
    let log = Command::new("git")
        .arg("-C")
        .arg(&end.worktree)
        .args(["log", "-1", "--format=%s"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&log.stdout).contains("checkpoint of card"),
        "the partial work was committed"
    );
    assert!(
        s.start(fx.repo.clone(), id, None, Default::default())
            .is_err(),
        "a plain start still needs the yes"
    );

    // Your yes: the continuation runs on the API key account, in its own profile, with the helper.
    let next = s.continue_run(&first.id, true).unwrap();
    assert_eq!(next.account, "api");
    assert_eq!(next.model.as_deref(), Some("haiku"));
    assert_eq!(next.handoff_from.as_deref(), Some(first.id.as_str()));
    assert_eq!(next.exhausted, vec!["default".to_owned()]);
    let done = wait_end(&s, &next.id).await;
    assert_eq!(done.state, RunState::Done, "{:?}", done.error);
    let seen = fs::read_to_string(done.worktree.join(".briefing-seen")).unwrap();
    assert!(seen.contains("You are continuing run"), "{seen}");
    assert!(
        seen.contains("checkpoint of card"),
        "commits so far are in the briefing"
    );
    let args = fs::read_to_string(&args_log).unwrap();
    assert!(args.contains("--model haiku"), "{args}");
    assert!(args.contains("apiKeyHelper"), "{args}");
    assert!(
        args.contains(&format!("CCD={}", fx.home.join("profiles/api").display())),
        "{args}"
    );
    assert!(
        args.lines().any(|l| l == "CCD="),
        "the first run used your normal login"
    );
    assert!(s.get(&first.id).unwrap().next.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn proofs_and_a_judge_then_your_review() {
    use pitd::supervisor::ReviewAction;
    let fx = fx();
    // A real origin with main and dev, so approving can ship the lane into dev.
    let origin = fx.home.parent().unwrap().join("origin.git");
    git(
        fx.home.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    git(&fx.repo, &["push", "-q", "origin", "main:dev"]);
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let flow = fs::read_to_string(fx.repo.join(".pitlane/flow.toml")).unwrap().replace(
        "[verify]\njudge = false\n",
        "[verify]\ncommands = [\"test -f README.md\", \"echo two failing >&2; false\"]\njudge = true\njudge_model = \"sonnet\"\nmax_rounds = 1\n",
    );
    fs::write(fx.repo.join(".pitlane/flow.toml"), flow).unwrap();
    let id = card(&fx, "Refuse SVG avatars");
    // The lane has work of its own, so there is a diff to judge and something to ship.
    git(&fx.repo, &["switch", "-q", "feat/x"]);
    fs::write(
        fx.repo.join("validate.ts"),
        "export const refuseSvg = true;\n",
    )
    .unwrap();
    git(&fx.repo, &["add", "validate.ts"]);
    git(&fx.repo, &["commit", "-qm", "refuse svg"]);
    git(&fx.repo, &["switch", "-q", "main"]);
    let s = sup(&fx);

    let run = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap()
        .id;
    let end = wait_end(&s, &run).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    assert_eq!(end.proofs_ok, Some(false));
    assert_eq!(end.verdict, Some(pit_core::verify::Verdict::Changes));
    assert_eq!(card_state(&fx, id).0, CardState::Review);
    let v: pit_core::verify::Verification = serde_json::from_str(
        &fs::read_to_string(fx.home.join("runs").join(&run).join("verify.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v.proofs.len(), 2);
    assert!(v.proofs[0].ok && !v.proofs[1].ok);
    assert!(
        v.proofs[1].tail.contains("two failing"),
        "{}",
        v.proofs[1].tail
    );
    assert_eq!(
        v.judge
            .as_ref()
            .unwrap()
            .judgement
            .as_ref()
            .unwrap()
            .findings
            .len(),
        1
    );
    // The judge saw the spec and the diff, never what the working agent said.
    let prompt = fs::read_to_string(end.worktree.join(".judge-prompt")).unwrap();
    assert!(
        prompt.contains("Do the thing.") && prompt.contains("## The diff"),
        "{prompt}"
    );
    assert!(
        prompt.contains("validate.ts") && prompt.contains("refuseSvg"),
        "the diff is in the prompt"
    );
    assert!(
        !prompt.contains("Reading the card."),
        "the agent's words stay out of the judge's context"
    );
    let kinds: Vec<bool> = s
        .subscribe(&run, 0)
        .unwrap()
        .replay
        .iter()
        .map(|(_, e)| matches!(e, RunEvent::Proof { .. } | RunEvent::Judged { .. }))
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k).count(), 3);
    // The judge ran as a session of its own, watchable beside the run, never listed as a run.
    let judge = s
        .sessions(None)
        .unwrap()
        .into_iter()
        .find(|m| m.kind == pit_core::runs::SessionKind::Judge)
        .expect("a judge session");
    assert_eq!(judge.parent.as_deref(), Some(run.as_str()));
    assert_eq!(judge.state, RunState::Done);
    assert!(
        s.list(None)
            .unwrap()
            .iter()
            .all(|m| m.kind == pit_core::runs::SessionKind::Run)
    );

    // Ask for changes: the same session continues with the comments, one round allowed.
    let again = s
        .review(
            fx.repo.clone(),
            id,
            ReviewAction::Changes,
            Some("Also refuse SVG.".into()),
        )
        .await
        .unwrap();
    let next = again["run"]["id"].as_str().unwrap().to_owned();
    let next_end = wait_end(&s, &next).await;
    assert_eq!(next_end.round, 1);
    assert_eq!(next_end.resumed_from.as_deref(), Some(run.as_str()));
    assert!(
        fs::read_to_string(next_end.worktree.join(".briefing-seen"))
            .unwrap()
            .contains("Also refuse SVG.")
    );
    assert_eq!(card_state(&fx, id).0, CardState::Review);
    let over = s
        .review(
            fx.repo.clone(),
            id,
            ReviewAction::Changes,
            Some("More.".into()),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{over:#}").contains("rounds of changes"),
        "{over:#}"
    );

    // Approve: the lane joins the merge queue, ships into dev, and the card follows.
    git(&fx.repo, &["fetch", "-q", "origin"]);
    s.start_queue();
    let ok = s
        .review(fx.repo.clone(), id, ReviewAction::Approve, None)
        .await
        .unwrap();
    assert_eq!(ok["shipped_to"], "dev");
    let job = ok["job"]["id"].as_str().unwrap().to_owned();
    wait_job(&s, &job).await;
    assert_eq!(card_state(&fx, id).0, CardState::InDev);
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let merged = Command::new("git")
        .arg("-C")
        .arg(&fx.repo)
        .args(["merge-base", "--is-ancestor", "feat/x", "origin/dev"])
        .status()
        .unwrap();
    assert!(merged.success(), "feat/x is in origin/dev");
    assert!(
        s.review(fx.repo.clone(), id, ReviewAction::Discard, None)
            .await
            .is_err(),
        "no longer in review"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_all_fixes_and_lands_on_its_own() {
    let fx = fx();
    let origin = fx.home.parent().unwrap().join("origin.git");
    git(
        fx.home.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    git(&fx.repo, &["push", "-q", "origin", "main:dev"]);
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let flow = fs::read_to_string(fx.repo.join(".pitlane/flow.toml"))
        .unwrap()
        .replace(
            "[verify]\njudge = false\n",
            "[verify]\ncommands = [\"test -f README.md\"]\njudge = true\nmax_rounds = 1\n",
        );
    fs::write(fx.repo.join(".pitlane/flow.toml"), flow).unwrap();
    let id = card(&fx, "Refuse SVG avatars");
    git(&fx.repo, &["switch", "-q", "feat/x"]);
    fs::write(
        fx.repo.join("validate.ts"),
        "export const refuseSvg = true;\n",
    )
    .unwrap();
    git(&fx.repo, &["add", "validate.ts"]);
    git(&fx.repo, &["commit", "-qm", "refuse svg"]);
    git(&fx.repo, &["switch", "-q", "main"]);
    let s = sup(&fx);
    s.start_queue();

    // The judge asks for changes every time: one round runs on its own, then the card lands anyway.
    let opts = pitd::supervisor::StartOpts {
        autopilot: true,
        ..Default::default()
    };
    s.start(fx.repo.clone(), id, None, opts).unwrap();
    let mut state = CardState::Running;
    for _ in 0..600 {
        state = card_state(&fx, id).0;
        if state == CardState::InDev || state == CardState::Blocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(state, CardState::InDev, "landed without anyone approving");
    let runs = s.list(None).unwrap();
    let rounds: Vec<u32> = runs
        .iter()
        .filter(|m| m.card == id)
        .map(|m| m.round)
        .collect();
    assert!(
        rounds.contains(&0) && rounds.contains(&1),
        "a fix round ran by itself: {rounds:?}"
    );
    let fix = runs.iter().find(|m| m.card == id && m.round == 1).unwrap();
    assert!(fix.autopilot);
    assert!(
        fs::read_to_string(fix.worktree.join(".briefing-seen"))
            .unwrap()
            .contains("SVG is not refused."),
        "the findings went to the fix round"
    );
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    assert!(
        cards::get(&r, id)
            .unwrap()
            .notes
            .iter()
            .any(|n| n.text.contains("Landed after 2 rounds"))
    );
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let merged = Command::new("git")
        .arg("-C")
        .arg(&fx.repo)
        .args(["merge-base", "--is-ancestor", "feat/x", "origin/dev"])
        .status()
        .unwrap();
    assert!(merged.success(), "feat/x is in origin/dev");
}

/// A large card whose run closed one stage: what it did lands, but the card is not done. On the
/// night of the stuck board a six-stage card closed as done after its first stage.
#[tokio::test(flavor = "multi_thread")]
async fn run_all_does_not_close_a_card_that_says_more_is_needed() {
    let fx = fx();
    let origin = fx.home.parent().unwrap().join("origin.git");
    git(
        fx.home.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    git(&fx.repo, &["push", "-q", "origin", "main:dev"]);
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let id = card(&fx, "Maps and props, in stages");
    git(&fx.repo, &["switch", "-q", "feat/x"]);
    fs::write(fx.repo.join("stage-1.md"), "measured\n").unwrap();
    git(&fx.repo, &["add", "stage-1.md"]);
    git(&fx.repo, &["commit", "-qm", "stage 1"]);
    git(&fx.repo, &["switch", "-q", "main"]);
    let s = fake_in(&fx, "partial");
    s.start_queue();

    let opts = pitd::supervisor::StartOpts {
        autopilot: true,
        ..Default::default()
    };
    s.start(fx.repo.clone(), id, None, opts).unwrap();
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let mut reopened = false;
    for _ in 0..600 {
        reopened = cards::get(&r, id)
            .unwrap()
            .notes
            .iter()
            .any(|n| n.text.contains("said more is needed"));
        if reopened || card_state(&fx, id).0 == CardState::Blocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reopened, "the card was told it is not done");
    assert_eq!(
        card_state(&fx, id).0,
        CardState::Ready,
        "back in the backlog, not done"
    );
    let c = cards::get(&r, id).unwrap();
    assert!(
        c.notes
            .iter()
            .any(|n| n.text.contains("Stage 2: the walls")),
        "what is left stays on the card"
    );
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let merged = Command::new("git")
        .arg("-C")
        .arg(&fx.repo)
        .args(["merge-base", "--is-ancestor", "feat/x", "origin/dev"])
        .status()
        .unwrap();
    assert!(merged.success(), "the stage it finished landed");
    assert!(s.list(None).unwrap().iter().all(|m| !m.state.is_live()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_related_repository_gets_the_branch_and_lands_too() {
    let fx = fx();
    let top = fx.home.parent().unwrap().to_path_buf();
    // A sibling repository with a remote, where the card's code really lives.
    let games = top.join("games");
    let origin = top.join("games-origin.git");
    git(
        &top,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(&top, &["init", "-q", "-b", "main", games.to_str().unwrap()]);
    for (k, v) in [
        ("user.name", "T"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&games, &["config", k, v]);
    }
    fs::write(games.join("engine.ts"), "export {};\n").unwrap();
    // Its worktrees stay in the test's folder, never in the real ~/.pitlane.
    fs::create_dir_all(games.join(".pitlane")).unwrap();
    fs::write(
        games.join(".pitlane/flow.toml"),
        format!(
            "[worktree]\nroot = \"{}\"\n",
            top.join("wt/{repo}/{branch}").display()
        ),
    )
    .unwrap();
    git(&games, &["add", "-A"]);
    git(&games, &["commit", "-qm", "init"]);
    git(
        &games,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&games, &["push", "-q", "origin", "main"]);
    let flow = fs::read_to_string(fx.repo.join(".pitlane/flow.toml")).unwrap()
        + "\n[project]\nrepos = [\"../games\"]\n";
    fs::write(fx.repo.join(".pitlane/flow.toml"), flow).unwrap();
    git(&fx.repo, &["branch", "feat/day"]);
    let id = card(&fx, "Fix the engine");
    let r = Repo::open(&fx.repo, &top, &fx.home).unwrap();
    cards::update(
        &r,
        id,
        CardPatch {
            target: Some(Some("feat/day".into())),
            ..Default::default()
        },
    )
    .unwrap();

    let s = Supervisor::new(Config {
        home: fx.home.clone(),
        claude_bin: {
            let wrapper = fx.fake.with_file_name("fake-related");
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/bash\nFAKE_MODE=related exec {} \"$@\"\n",
                    fx.fake.display()
                ),
            )
            .unwrap();
            Command::new("chmod")
                .arg("+x")
                .arg(&wrapper)
                .status()
                .unwrap();
            wrapper.to_string_lossy().into_owned()
        },
        pit_bin: None,
    });
    s.start_queue();
    let meta = s
        .start(
            fx.repo.clone(),
            id,
            None,
            pitd::supervisor::StartOpts {
                autopilot: true,
                ..Default::default()
            },
        )
        .unwrap();
    let end = wait_end(&s, &meta.id).await;
    assert_eq!(end.state, RunState::Done, "{:?}", end.error);
    let notes: Vec<String> = s
        .subscribe(&meta.id, 0)
        .unwrap()
        .replay
        .into_iter()
        .filter_map(|(_, e)| match e {
            RunEvent::Note { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(end.related.len(), 1, "{notes:#?}");
    let lane = &end.related[0];
    assert_eq!(
        (lane.name.as_str(), lane.branch.as_str()),
        ("games", "feat/x")
    );
    let args = fs::read_to_string(end.worktree.join(".args")).unwrap();
    assert!(
        args.contains("--add-dir") && args.contains(&lane.worktree.to_string_lossy().into_owned()),
        "{args}"
    );
    assert!(
        args.contains("--permission-mode bypassPermissions"),
        "Run all never stops for a permission: {args}"
    );
    assert!(
        fs::read_to_string(end.worktree.join(".briefing-seen"))
            .unwrap()
            .contains("Related repositories")
    );

    // It lands in feat/day in both repositories, made in games from a fresh origin/main.
    let mut state = CardState::Running;
    for _ in 0..400 {
        state = card_state(&fx, id).0;
        if state == CardState::Done || state == CardState::InDev || state == CardState::Blocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        matches!(state, CardState::Done | CardState::InDev),
        "{state:?} {:?}",
        s.queue_list(None)
    );
    let has = Command::new("git")
        .arg("-C")
        .arg(&games)
        .args(["cat-file", "-e", "feat/day:fix.txt"])
        .status()
        .unwrap();
    assert!(
        has.success(),
        "the fix made in games landed in games' feat/day"
    );
}

async fn wait_job(s: &Arc<Supervisor>, id: &str) -> pit_core::queue::ShipJob {
    for _ in 0..300 {
        if let Some(j) = s.queue_list(None).into_iter().find(|j| j.id == id)
            && !j.state.is_open()
        {
            return j;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("job {id} never finished");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_merge_queue_ships_one_lane_at_a_time() {
    use pit_core::queue::JobState;
    let fx = fx();
    let origin = fx.home.parent().unwrap().join("origin.git");
    git(
        fx.home.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main", "main:dev"]);
    // Three lanes off main: two touch different files, the third clashes with the first.
    for (branch, file, text) in [
        ("feat/a", "a.txt", "a\n"),
        ("feat/b", "b.txt", "b\n"),
        ("feat/clash", "a.txt", "other\n"),
    ] {
        git(&fx.repo, &["switch", "-q", "-c", branch, "main"]);
        fs::write(fx.repo.join(file), text).unwrap();
        git(&fx.repo, &["add", "-A"]);
        git(&fx.repo, &["commit", "-qm", branch]);
    }
    git(&fx.repo, &["switch", "-q", "main"]);
    git(&fx.repo, &["fetch", "-q", "origin"]);
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::add(
        &r,
        NewCard {
            title: "B".into(),
            ..Default::default()
        },
    )
    .unwrap();
    cards::update(
        &r,
        c.id,
        CardPatch {
            lane: Some(Some("feat/b".into())),
            state: Some(CardState::Review),
            ..Default::default()
        },
    )
    .unwrap();

    let s = sup(&fx);
    let a = s.enqueue(&fx.repo, "feat/a", None).unwrap();
    let b = s.enqueue(&fx.repo, "feat/b", None).unwrap();
    let x = s.enqueue(&fx.repo, "feat/clash", None).unwrap();
    assert_eq!(
        a.target, "dev",
        "the next stage of a fresh lane is integration"
    );
    assert!(
        s.enqueue(&fx.repo, "feat/a", None).is_err(),
        "one open job per lane"
    );
    assert!(s.enqueue(&fx.repo, "main", None).is_err());
    assert_eq!(s.queue_list(Some(&r.root)).len(), 3);

    // The queue survives a restart before it ships anything.
    let s2 = sup(&fx);
    assert_eq!(
        s2.queue_list(None)
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .count(),
        3
    );
    drop(s2);

    s.start_queue();
    let (ja, jb, jx) = (
        wait_job(&s, &a.id).await,
        wait_job(&s, &b.id).await,
        wait_job(&s, &x.id).await,
    );
    assert_eq!((ja.state, jb.state), (JobState::Done, JobState::Done));
    assert_eq!(jx.state, JobState::Conflict, "{:?}", jx.error);
    assert_eq!(jx.conflicts, vec!["a.txt".to_owned()]);
    assert!(ja.finished_at <= jb.finished_at, "in order");
    git(&fx.repo, &["fetch", "-q", "origin"]);
    for b in ["feat/a", "feat/b"] {
        let ok = Command::new("git")
            .arg("-C")
            .arg(&fx.repo)
            .args(["merge-base", "--is-ancestor", b, "origin/dev"])
            .status()
            .unwrap();
        assert!(ok.success(), "{b} is in origin/dev");
    }
    assert_eq!(
        card_state(&fx, c.id).0,
        CardState::InDev,
        "the card followed its lane"
    );

    // The clash can leave the line; a lane already in dev goes on to main next.
    s.dequeue(&jx.id).unwrap();
    let main_job = s.enqueue(&fx.repo, "feat/b", None).unwrap();
    assert_eq!(main_job.target, "main");
    assert_eq!(wait_job(&s, &main_job.id).await.state, JobState::Done);
    assert_eq!(card_state(&fx, c.id).0, CardState::Done);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_message_carries_a_map_of_where_to_look() {
    let fx = fx();
    fs::create_dir_all(fx.repo.join("src")).unwrap();
    fs::write(
        fx.repo.join("src/kart.ts"),
        "export function driftAngle(tier: number): number {\n  return tier * 3;\n}\n",
    )
    .unwrap();
    git(&fx.repo, &["add", "-A"]);
    git(&fx.repo, &["commit", "-qm", "kart"]);
    let s = sup(&fx);

    let id = card(&fx, "Raise the drift angle for tier 3");
    let run = s
        .start(fx.repo.clone(), id, None, Default::default())
        .unwrap();
    let end = wait_end(&s, &run.id).await;
    let b = end.brief.expect("briefed");
    assert_eq!(b.files, 1);
    let notes: Vec<String> = s
        .read_timeline(&run.id, 0)
        .into_iter()
        .filter_map(|(_, e)| match e {
            RunEvent::Note { text } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Briefed with 1 file from")),
        "{notes:?}"
    );

    let other = card(&fx, "Drift angle again, without the map");
    let opts = pit_supervisor_opts_no_brief();
    let run = s.start(fx.repo.clone(), other, None, opts).unwrap();
    assert!(wait_end(&s, &run.id).await.brief.is_none());
}

fn pit_supervisor_opts_no_brief() -> pitd::supervisor::StartOpts {
    pitd::supervisor::StartOpts {
        no_brief: true,
        ..Default::default()
    }
}

/// With the real Claude Code (signed in): a card blocked on a question you answered is read
/// again and goes back to the Backlog. `cargo test -p pitd --test runs -- --ignored rechecks`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude"]
async fn rechecks_an_answered_card_with_the_real_claude() {
    let fx = fx();
    let r = Repo::open(&fx.repo, fx.home.parent().unwrap(), &fx.home).unwrap();
    let c = cards::add(&r, NewCard { title: "Add a doubling helper".into(), body: "Add a small function to README.md's example section that doubles a number.".into(), ..Default::default() }).unwrap();
    cards::ask(&r, c.id, vec![cards::NewQuestion { text: "What should the function be called?".into(), options: vec!["double".into(), "twice".into()], recommended: Some(0), context: Some("Only the name changes.".into()) }]).unwrap();
    let q = cards::get(&r, c.id).unwrap().questions[0].id;
    cards::update(&r, c.id, CardPatch { state: Some(CardState::Blocked), blocked: Some(cards::Block::new(cards::BlockReason::Questions, "Needs the function's name.", None)), ..Default::default() }).unwrap();
    cards::answer(&r, c.id, q, "1").unwrap();
    let s = Supervisor::new(Config { home: fx.home.clone(), claude_bin: "claude".into(), pit_bin: None });
    assert!(s.recheck(fx.repo.clone(), c.id).unwrap(), "a reading starts");
    for _ in 0..360 {
        let now = cards::get(&r, c.id).unwrap();
        if now.blocked.as_ref().is_none_or(|b| b.checking.is_none()) {
            eprintln!("state {:?}, open {}, notes {:?}", now.state, now.open_questions(), now.notes.iter().map(|n| &n.text).collect::<Vec<_>>());
            assert!(now.state == CardState::Ready || now.open_questions() > 0);
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the reading never ended");
}
