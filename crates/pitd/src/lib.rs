//! `pit daemon`: owns the agent runs, so closing the app or the terminal never kills one.
//! Speaks JSON, one message per line, on a Unix socket only this user can open.
//!
//! Requests: `ping`, `start {repo, card, fresh?}`, `resume {run}`, `list {repo?}`, `get {run}`,
//! `say {run, text}`, `tune {run, model?, effort?}`, `stop {run}`, `stop_all`, `perf`, `perf_set {config}`, `perf_hold {run, hold}`, `shutdown`, `hold` (the app; its end stops all
//! and ends the daemon), `quota`, `ask {run, tool, input, tool_use_id}` (waits for a
//! person), `decide {run, approval?, decision, message?}`, `approvals`, and `follow {run, from}`, which streams
//! `{i, event}` lines and ends with `{end: <run>}`.

mod peer;
pub mod perf;
pub mod supervisor;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pit_core::runs::{self, RunEvent};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;

pub use supervisor::{Config, Supervisor};

/// Binds the socket and serves until the process ends. Refuses to start twice.
pub async fn serve(cfg: Config) -> Result<()> {
    let sock = runs::socket_path(&cfg.home);
    std::fs::create_dir_all(&cfg.home)?;
    // One daemon per home, decided by a lock the OS drops when the process ends: two daemons
    // started at once (the app and the CLI) would otherwise both bind, and the second's
    // recovery would end the first one's agents. Held for as long as `serve` runs.
    let _single = single_instance(&cfg.home)?;
    if UnixStream::connect(&sock).await.is_ok() {
        bail!("a Pitlane daemon is already running on {}", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("could not listen on {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let sup = Supervisor::new(cfg);
    sup.start_queue();
    sup.start_perf();
    let recovered = sup.recover()?;
    eprintln!("pitd: listening on {} ({recovered} interrupted run(s) recovered)", sock.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let sup = Arc::clone(&sup);
        tokio::spawn(async move {
            if let Err(e) = connection(sup, stream).await {
                eprintln!("pitd: connection: {e:#}");
            }
        });
    }
}

/// Takes `pitd.lock` in `home` for good, or fails when another daemon holds it.
fn single_instance(home: &std::path::Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let path = home.join("pitd.lock");
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).with_context(|| format!("could not open {}", path.display()))?;
    // SAFETY: flock on a descriptor this function owns; it takes no pointers.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("a Pitlane daemon is already running for {}", home.display());
    }
    Ok(f)
}

async fn connection(sup: Arc<Supervisor>, stream: UnixStream) -> Result<()> {
    // Asked once per connection: a process does not leave a session while it holds a socket.
    // A peer whose pid cannot be read is treated as one of the sessions'.
    let agent = match stream.peer_cred().ok().and_then(|c| c.pid()) {
        Some(pid) => peer::from_agent(pid, std::process::id() as i32, &sup.session_groups()),
        None => true,
    };
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(&mut write, &json!({ "error": format!("not JSON: {e}") })).await?;
                continue;
            }
        };
        let op = req.get("op").and_then(Value::as_str).unwrap_or("");
        if agent && !peer::allowed_from_agent(op) {
            send(&mut write, &json!({ "error": format!("{op:?} is not open to a session's own processes: it is yours to do, from Pitlane or pit") })).await?;
            continue;
        }
        if req.get("op").and_then(Value::as_str) == Some("hold") {
            // The desktop app holds this connection while it is open. When it closes (quit,
            // crash, force quit), everything the daemon started stops and the daemon ends.
            send(&mut write, &json!({ "ok": true })).await?;
            while let Ok(Some(_)) = lines.next_line().await {}
            sup.shutdown().await;
        }
        if req.get("op").and_then(Value::as_str) == Some("follow") {
            // A follow takes over the connection until the run ends.
            return follow(&sup, &req, &mut write).await;
        }
        let reply = match handle(&sup, &req).await {
            Ok(v) => v,
            Err(e) => json!({ "error": format!("{e:#}") }),
        };
        send(&mut write, &reply).await?;
    }
    Ok(())
}

async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, v: &Value) -> Result<()> {
    w.write_all(format!("{v}\n").as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

/// The run a request names; only a plain name, so it never reaches outside `runs/`.
fn run_arg(req: &Value) -> Result<&str> {
    let run = req.get("run").and_then(Value::as_str).context("run is required")?;
    if !pit_core::is_plain_name(run) {
        bail!("{run:?} is not a run");
    }
    Ok(run)
}

async fn handle(sup: &Arc<Supervisor>, req: &Value) -> Result<Value> {
    let op = req.get("op").and_then(Value::as_str).unwrap_or_default();
    Ok(match op {
        "ping" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() }),
        "start" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = req.get("card").and_then(Value::as_u64).context("card is required")?;
            let allow_api_key = req.get("allow_api_key").and_then(Value::as_bool).unwrap_or(false);
            let no_brief = req.get("no_brief").and_then(Value::as_bool).unwrap_or(false);
            let into = req.get("into").and_then(Value::as_str).map(str::to_owned);
            let direct = req.get("direct").and_then(Value::as_bool).unwrap_or(false);
            let instructions = req.get("instructions").and_then(Value::as_str).map(str::to_owned);
            // Run all's cards run on autopilot: fix rounds, more turns and landing need nobody.
            let autopilot = req.get("autopilot").and_then(Value::as_bool).unwrap_or(false);
            // A card stopped mid-work resumes its session unless the person starts over.
            let fresh = req.get("fresh").and_then(Value::as_bool).unwrap_or(false);
            json!({ "run": sup.start(repo, u32::try_from(card)?, None, supervisor::StartOpts { allow_api_key, no_brief, into, direct, instructions, autopilot, fresh, ..Default::default() })? })
        }
        "analyze" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let ids: Vec<u32> = serde_json::from_value(req.get("cards").cloned().context("cards is required")?)?;
            sup.analyze(repo, ids).await?
        }
        "report_digest" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let id = req.get("id").and_then(Value::as_str).context("id is required")?.to_owned();
            sup.report_digest(repo, id).await?
        }
        "describe" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = req.get("card").and_then(Value::as_u64).context("card is required")?;
            let title = req.get("title").and_then(Value::as_bool).unwrap_or(false);
            sup.describe(repo, u32::try_from(card)?, title).await?
        }
        "talk" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = req.get("card").and_then(Value::as_u64).context("card is required")?;
            let question = req.get("question").and_then(Value::as_u64).context("question is required")?;
            let message = req.get("message").and_then(Value::as_str).map(str::to_owned);
            sup.talk(repo, u32::try_from(card)?, u32::try_from(question)?, message).await?
        }
        "stop_talk" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = req.get("card").and_then(Value::as_u64).context("card is required")?;
            json!({ "stopped": sup.stop_talk(&repo, u32::try_from(card)?) })
        }
        "finish" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let ids: Vec<u32> = serde_json::from_value(req.get("cards").cloned().context("cards is required")?)?;
            let instructions = req.get("instructions").and_then(Value::as_str).context("instructions is required")?.to_owned();
            let into = req.get("into").and_then(Value::as_str).map(str::to_owned);
            sup.finish(repo, ids, instructions, into).await?
        }
        "assist" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let text = req.get("text").and_then(Value::as_str).context("text is required")?.trim().to_owned();
            if text.is_empty() {
                bail!("say what you want on the board");
            }
            sup.assist(repo, text)?;
            json!({ "ok": true })
        }
        "resume" => {
            let prev = sup.get(run_arg(req)?)?;
            json!({ "run": sup.start(prev.repo.clone(), prev.card, Some(prev.id), Default::default())? })
        }
        "list" => {
            let repo = req.get("repo").and_then(Value::as_str).map(PathBuf::from);
            json!({ "runs": sup.list(repo.as_deref())? })
        }
        "sessions" => {
            let repo = req.get("repo").and_then(Value::as_str).map(PathBuf::from);
            json!({ "sessions": sup.sessions(repo.as_deref())? })
        }
        "get" => json!({ "run": sup.get(run_arg(req)?)? }),
        "say" => {
            let text = req.get("text").and_then(Value::as_str).context("text is required")?;
            sup.say(run_arg(req)?, text).await?;
            json!({ "ok": true })
        }
        "tune" => {
            let model = req.get("model").and_then(Value::as_str);
            let effort = req.get("effort").and_then(Value::as_str);
            sup.tune(run_arg(req)?, model, effort).await?;
            json!({ "ok": true })
        }
        "stop" => {
            sup.stop(run_arg(req)?)?;
            json!({ "ok": true })
        }
        "stop_all" => json!({ "stopped": sup.stop_all() }),
        "perf" => sup.perf.snapshot(),
        "perf_set" => json!({ "config": sup.perf.set(req.get("config").context("config is required")?)? }),
        "perf_hold" => {
            let hold = req.get("hold").and_then(Value::as_bool).context("hold is required")?;
            json!({ "changed": sup.perf_hold(run_arg(req)?, hold)? })
        }
        "stop_analyses" => json!({ "stopped": sup.stop_where(|m| m.kind == pit_core::runs::SessionKind::Analysis) }),
        "shutdown" => {
            let sup = Arc::clone(sup);
            tokio::spawn(async move { sup.shutdown().await });
            json!({ "ok": true })
        }
        "quota" => json!({ "quota": sup.quota() }),
        "review" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = u32::try_from(req.get("card").and_then(Value::as_u64).context("card is required")?)?;
            let action = match req.get("action").and_then(Value::as_str).unwrap_or_default() {
                "approve" => supervisor::ReviewAction::Approve,
                "changes" => supervisor::ReviewAction::Changes,
                "discard" => supervisor::ReviewAction::Discard,
                a => bail!("unknown review action {a:?}; approve, changes or discard"),
            };
            let message = req.get("message").and_then(Value::as_str).map(str::to_owned);
            sup.review(repo, card, action, message).await?
        }
        "enqueue" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let branch = req.get("branch").and_then(Value::as_str).context("branch is required")?;
            let to = req.get("to").and_then(Value::as_str);
            json!({ "job": sup.enqueue(&repo, branch, to)? })
        }
        "queue" => {
            let repo = req.get("repo").and_then(Value::as_str).map(PathBuf::from);
            json!({ "jobs": sup.queue_list(repo.as_deref()) })
        }
        "queue_resume" => {
            sup.resume_queue()?;
            json!({ "ok": true })
        }
        "dequeue" => {
            sup.dequeue(req.get("id").and_then(Value::as_str).context("id is required")?)?;
            json!({ "ok": true })
        }
        "continue" => {
            let allow_api_key = req.get("allow_api_key").and_then(Value::as_bool).unwrap_or(false);
            json!({ "run": sup.continue_run(run_arg(req)?, allow_api_key)? })
        }
        "ask" => {
            let tool = req.get("tool").and_then(Value::as_str).context("tool is required")?;
            let input = req.get("input").cloned().unwrap_or(Value::Null);
            let use_id = req.get("tool_use_id").and_then(Value::as_str).map(str::to_owned);
            json!({ "answer": sup.ask(run_arg(req)?, tool, input, use_id).await })
        }
        "decide" => {
            let decision = pit_core::runs::Decision::parse(req.get("decision").and_then(Value::as_str).context("decision is required")?)?;
            let approval = req.get("approval").and_then(Value::as_str);
            let message = req.get("message").and_then(Value::as_str).map(str::to_owned);
            json!({ "approval": sup.decide(run_arg(req)?, approval, decision, message)? })
        }
        "approvals" => {
            let list: Vec<Value> = sup.approvals().into_iter().map(|(m, a)| json!({ "run": m.id, "card": m.card, "card_title": m.card_title, "approval": a })).collect();
            json!({ "approvals": list })
        }
        _ => bail!("unknown op {op:?}"),
    })
}

async fn follow(sup: &Arc<Supervisor>, req: &Value, w: &mut tokio::net::unix::OwnedWriteHalf) -> Result<()> {
    let id = match run_arg(req) {
        Ok(id) => id.to_owned(),
        Err(e) => return send(w, &json!({ "error": e.to_string() })).await,
    };
    let from = req.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
    let sub = match sup.subscribe(&id, from) {
        Ok(s) => s,
        Err(e) => return send(w, &json!({ "error": format!("{e:#}") })).await,
    };
    let mut next = from;
    let mut ended = false;
    for (i, ev) in sub.replay {
        ended |= matches!(ev, RunEvent::Exited { .. });
        send(w, &json!({ "i": i, "event": ev })).await?;
        next = i + 1;
    }
    if let (Some(mut rx), false) = (sub.rx, ended) {
        loop {
            match rx.recv().await {
                Ok((i, ev)) if i >= next => {
                    let done = matches!(ev, RunEvent::Exited { .. });
                    send(w, &json!({ "i": i, "event": ev })).await?;
                    next = i + 1;
                    if done {
                        break;
                    }
                }
                Ok(_) => {}
                // Fell behind: catch up from the timeline on disk, then keep listening.
                Err(RecvError::Lagged(_)) => {
                    let sub = sup.subscribe(&id, next)?;
                    for (i, ev) in sub.replay {
                        send(w, &json!({ "i": i, "event": ev })).await?;
                        next = i + 1;
                    }
                }
                Err(RecvError::Closed) => break,
            }
        }
    }
    send(w, &json!({ "end": sup.get(&id)? })).await
}
