//! `pit daemon`: owns the agent runs, so closing the app or the terminal never kills one.
//! Speaks JSON, one message per line, on a Unix socket only this user can open.
//!
//! Requests: `ping`, `start {repo, card, fresh?}`, `resume {run}`, `list {repo?}`, `get {run}`,
//! `say {run, text}`, `tune {run, model?, effort?}`, `stop {run}`, `stop_all`, `perf`, `perf_set {config}`, `perf_hold {run, hold}`, `shutdown`, `hold` (the app; its end stops all
//! and ends the daemon), `quota`, `ask {run, tool, input, tool_use_id}` (waits for a
//! person), `decide {run, approval?, decision, message?}`, `approvals`, and `follow {run, from}`, which streams
//! `{i, event}` lines and ends with `{end: <run>}`. The messages are typed in
//! `pit_core::protocol`.

mod peer;
pub mod perf;
pub mod supervisor;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pit_core::protocol::{
    self, AnswerReply, ApprovalsReply, Changed, Decided, Done, JobReply, JobsReply, Pong, QuotaReply, Request, RunReply, RunsReply, SessionsReply,
    StartArgs, Stopped, Waiting,
};
use pit_core::runs::{self, RunEvent};
use serde::Serialize;
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
        let parsed = Request::from_value(req.clone());
        // The op the check goes by: the request's, or for one that does not read as a request,
        // the name it gives, so an op an agent may not use is refused whether it reads or not.
        let op = match &parsed {
            Ok(r) => r.op(),
            Err(_) => protocol::named_op(&req),
        };
        if agent && !peer::allowed_from_agent(op) {
            send(&mut write, &json!({ "error": format!("{op:?} is not open to a session's own processes: it is yours to do, from Pitlane or pit") })).await?;
            continue;
        }
        let req = match parsed {
            Ok(r) => r,
            Err(e) => {
                send(&mut write, &json!({ "error": format!("{e:#}") })).await?;
                continue;
            }
        };
        if let Request::Hold = req {
            // The desktop app holds this connection while it is open. When it closes (quit,
            // crash, force quit), everything the daemon started stops and the daemon ends.
            send(&mut write, &reply(Done { ok: true })?).await?;
            while let Ok(Some(_)) = lines.next_line().await {}
            sup.shutdown().await;
        }
        if let Request::Follow { run, from } = &req {
            // A follow takes over the connection until the run ends.
            return follow(&sup, run, *from, &mut write).await;
        }
        let answer = match handle(&sup, req).await {
            Ok(v) => v,
            Err(e) => json!({ "error": format!("{e:#}") }),
        };
        send(&mut write, &answer).await?;
    }
    Ok(())
}

async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, v: &Value) -> Result<()> {
    w.write_all(format!("{v}\n").as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

/// The run a request names; only a plain name, so it never reaches outside `runs/`.
fn run_arg(run: &str) -> Result<&str> {
    if !pit_core::is_plain_name(run) {
        bail!("{run:?} is not a run");
    }
    Ok(run)
}

/// A reply as it goes on the wire.
fn reply(r: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(r)?)
}

async fn handle(sup: &Arc<Supervisor>, req: Request) -> Result<Value> {
    let ok = || reply(Done { ok: true });
    match req {
        Request::Ping => reply(Pong { ok: true, version: env!("CARGO_PKG_VERSION").into(), pid: std::process::id() }),
        Request::Start(a) => {
            // Run all's cards run on autopilot (fix rounds, more turns and landing need nobody), and
            // a card stopped mid-work resumes its session unless the person starts over (`fresh`).
            let StartArgs { repo, card, allow_api_key, no_brief, into, direct, instructions, autopilot, fresh } = a;
            let opts = supervisor::StartOpts { allow_api_key, no_brief, into, direct, instructions, autopilot, fresh, ..Default::default() };
            reply(RunReply { run: sup.start(repo, card, None, opts)? })
        }
        Request::Analyze { repo, cards } => sup.analyze(repo, cards).await,
        Request::ReportDigest { repo, id } => sup.report_digest(repo, id).await,
        Request::Describe { repo, card, title } => sup.describe(repo, card, title).await,
        Request::Talk { repo, card, question, message } => sup.talk(repo, card, question, message).await,
        Request::StopTalk { repo, card } => reply(Stopped { stopped: sup.stop_talk(&repo, card) }),
        Request::Finish { repo, cards, instructions, into } => sup.finish(repo, cards, instructions, into).await,
        Request::Assist { repo, text } => {
            let text = text.trim().to_owned();
            if text.is_empty() {
                bail!("say what you want on the board");
            }
            sup.assist(repo, text)?;
            ok()
        }
        Request::Resume { run } => {
            let prev = sup.get(run_arg(&run)?)?;
            reply(RunReply { run: sup.start(prev.repo.clone(), prev.card, Some(prev.id), Default::default())? })
        }
        Request::List { repo } => reply(RunsReply { runs: sup.list(repo.as_deref())? }),
        Request::Sessions { repo } => reply(SessionsReply { sessions: sup.sessions(repo.as_deref())? }),
        Request::Get { run } => reply(RunReply { run: sup.get(run_arg(&run)?)? }),
        Request::Say { run, text } => {
            sup.say(run_arg(&run)?, &text).await?;
            ok()
        }
        Request::Tune { run, model, effort } => {
            sup.tune(run_arg(&run)?, model.as_deref(), effort.as_deref()).await?;
            ok()
        }
        Request::Stop { run } => {
            sup.stop(run_arg(&run)?)?;
            ok()
        }
        Request::StopAll => reply(Stopped { stopped: sup.stop_all() }),
        Request::Perf => Ok(sup.perf.snapshot()),
        Request::PerfSet { config } => Ok(json!({ "config": sup.perf.set(&config)? })),
        Request::PerfHold { run, hold } => reply(Changed { changed: sup.perf_hold(run_arg(&run)?, hold)? }),
        Request::StopAnalyses => reply(Stopped { stopped: sup.stop_where(|m| m.kind == pit_core::runs::SessionKind::Analysis) }),
        Request::Shutdown => {
            let sup = Arc::clone(sup);
            tokio::spawn(async move { sup.shutdown().await });
            ok()
        }
        Request::Quota => reply(QuotaReply { quota: sup.quota() }),
        Request::Review { repo, card, action, message } => sup.review(repo, card, action, message).await,
        Request::Enqueue { repo, branch, to } => reply(JobReply { job: sup.enqueue(&repo, &branch, to.as_deref())? }),
        Request::Queue { repo } => reply(JobsReply { jobs: sup.queue_list(repo.as_deref()) }),
        Request::QueueResume => {
            sup.resume_queue()?;
            ok()
        }
        Request::Dequeue { id } => {
            sup.dequeue(&id)?;
            ok()
        }
        Request::Continue { run, allow_api_key } => reply(RunReply { run: sup.continue_run(run_arg(&run)?, allow_api_key)? }),
        Request::Ask { run, tool, input, tool_use_id } => reply(AnswerReply { answer: sup.ask(run_arg(&run)?, &tool, input, tool_use_id).await }),
        Request::Decide { run, approval, decision, message } => reply(Decided { approval: sup.decide(run_arg(&run)?, approval.as_deref(), decision, message)? }),
        Request::Approvals => {
            let approvals = sup.approvals().into_iter().map(|(m, a)| Waiting { run: m.id, card: m.card, card_title: m.card_title, approval: a }).collect();
            reply(ApprovalsReply { approvals })
        }
        // `connection` takes these over before they get here.
        Request::Hold | Request::Follow { .. } => bail!("unknown op {:?}", req.op()),
    }
}

async fn follow(sup: &Arc<Supervisor>, run: &str, from: usize, w: &mut tokio::net::unix::OwnedWriteHalf) -> Result<()> {
    let id = match run_arg(run) {
        Ok(id) => id.to_owned(),
        Err(e) => return send(w, &json!({ "error": e.to_string() })).await,
    };
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
                    let mut done = false;
                    for (i, ev) in sub.replay {
                        done |= matches!(ev, RunEvent::Exited { .. });
                        send(w, &json!({ "i": i, "event": ev })).await?;
                        next = i + 1;
                    }
                    // The run ended in what was caught up on: nothing more will come.
                    if done {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            }
        }
    }
    send(w, &json!({ "end": sup.get(&id)? })).await
}
