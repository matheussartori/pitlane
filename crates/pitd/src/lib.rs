//! `pit daemon`: owns the agent runs, so closing the app or the terminal never kills one.
//! Speaks JSON, one message per line, on a Unix socket only this user can open.
//!
//! Requests: `ping`, `start {repo, card}`, `resume {run}`, `list {repo?}`, `get {run}`,
//! `say {run, text}`, `stop {run}`, `quota`, `ask {run, tool, input, tool_use_id}` (waits for a
//! person), `decide {run, approval?, decision, message?}`, `approvals`, and `follow {run, from}`, which streams
//! `{i, event}` lines and ends with `{end: <run>}`.

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
    if UnixStream::connect(&sock).await.is_ok() {
        bail!("a Pitlane daemon is already running on {}", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("could not listen on {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let sup = Supervisor::new(cfg);
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

async fn connection(sup: Arc<Supervisor>, stream: UnixStream) -> Result<()> {
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

fn run_arg(req: &Value) -> Result<&str> {
    req.get("run").and_then(Value::as_str).context("run is required")
}

async fn handle(sup: &Arc<Supervisor>, req: &Value) -> Result<Value> {
    let op = req.get("op").and_then(Value::as_str).unwrap_or_default();
    Ok(match op {
        "ping" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() }),
        "start" => {
            let repo = PathBuf::from(req.get("repo").and_then(Value::as_str).context("repo is required")?);
            let card = req.get("card").and_then(Value::as_u64).context("card is required")?;
            json!({ "run": sup.start(repo, u32::try_from(card)?, None)? })
        }
        "resume" => {
            let prev = sup.get(run_arg(req)?)?;
            json!({ "run": sup.start(prev.repo.clone(), prev.card, Some(prev.id))? })
        }
        "list" => {
            let repo = req.get("repo").and_then(Value::as_str).map(PathBuf::from);
            json!({ "runs": sup.list(repo.as_deref())? })
        }
        "get" => json!({ "run": sup.get(run_arg(req)?)? }),
        "say" => {
            let text = req.get("text").and_then(Value::as_str).context("text is required")?;
            sup.say(run_arg(req)?, text).await?;
            json!({ "ok": true })
        }
        "stop" => {
            sup.stop(run_arg(req)?)?;
            json!({ "ok": true })
        }
        "quota" => json!({ "quota": sup.quota() }),
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
