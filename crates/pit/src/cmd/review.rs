//! Reviews and the merge queue.

use std::process::ExitCode;

use anyhow::{Result, bail};
use pit_core::Repo;
use pit_core::runs::RunMeta;

use super::runs::{follow_or_detach, state_word};
use super::{daemon, pitlane_home};
use crate::term::{ACCENT, DONE, FAIL, MUTED, RUN, paint};

pub fn review(repo: &Repo, card: u32) -> Result<ExitCode> {
    let v = daemon()?.call(serde_json::json!({ "op": "list", "repo": repo.root }))?;
    let runs: Vec<RunMeta> = serde_json::from_value(v["runs"].clone())?;
    let Some(m) = runs.iter().filter(|m| m.card == card).max_by_key(|m| (m.round, m.started_at)) else {
        bail!("card #{card} has no runs");
    };
    println!("#{} {}  {}  {}", m.card, m.card_title, state_word(m.state), paint(MUTED, &m.branch));
    if let Some(s) = m.handoff.as_ref().and_then(|h| h.get("summary")).and_then(|s| s.as_str()) {
        println!("\n{s}");
    }
    match std::fs::read_to_string(pit_core::runs::runs_dir(&pitlane_home()).join(&m.id).join("verify.json")) {
        Ok(t) => {
            let v: pit_core::verify::Verification = serde_json::from_str(&t)?;
            println!();
            if v.no_commands {
                println!("{}", paint(MUTED, "no checks configured ([verify] commands in .pitlane/flow.toml)"));
            }
            for p in &v.proofs {
                println!("{} {}  {}", if p.ok { paint(DONE, "✓") } else { paint(FAIL, "✕") }, p.command, paint(MUTED, &format!("{:.1}s", p.duration_ms as f64 / 1000.0)));
                if !p.ok {
                    for l in p.tail.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
                        println!("    {}", paint(MUTED, l));
                    }
                }
            }
            if let Some(j) = v.judge.as_ref().and_then(|j| j.judgement.as_ref()) {
                let word = match j.verdict {
                    pit_core::verify::Verdict::Approve => paint(DONE, "approves"),
                    pit_core::verify::Verdict::Changes => paint(ACCENT, "asks for changes"),
                };
                println!("\njudge {word}: {}", j.summary);
                for f in &j.findings {
                    let at = match (&f.file, f.line) {
                        (Some(file), Some(l)) => format!("{file}:{l} "),
                        (Some(file), None) => format!("{file} "),
                        _ => String::new(),
                    };
                    println!("  {} {at}{}", paint(MUTED, &f.severity), f.text);
                }
            } else if let Some(e) = v.judge.as_ref().and_then(|j| j.error.as_ref()) {
                println!("\njudge could not finish: {e}");
            }
        }
        Err(_) => println!("\n{}", paint(MUTED, "no checks ran for this run")),
    }
    println!("\n{}", paint(MUTED, &format!("pit approve {card} | pit changes {card} -m \"…\" | pit discard {card}")));
    Ok(ExitCode::SUCCESS)
}

pub fn approve(repo: &Repo, card: u32) -> Result<ExitCode> {
    let v = daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "approve" }))?;
    println!("{} #{card} is in the merge queue for {} (pit queue)", paint(DONE, "✓"), v["shipped_to"].as_str().unwrap_or("its next stage"));
    Ok(ExitCode::SUCCESS)
}

pub fn changes(repo: &Repo, card: u32, message: String) -> Result<ExitCode> {
    let v = daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "changes", "message": message }))?;
    let run = v["run"]["id"].as_str().unwrap_or_default().to_owned();
    println!("{} #{card} goes back to its agent as run {run}", paint(DONE, "✓"));
    follow_or_detach(&run, false)
}

pub fn discard(repo: &Repo, card: u32) -> Result<ExitCode> {
    daemon()?.call(serde_json::json!({ "op": "review", "repo": repo.root, "card": card, "action": "discard" }))?;
    println!("{} #{card} is ready again; its lane stays", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}

pub fn queue(repo: &Repo) -> Result<ExitCode> {
    let v = daemon()?.call(serde_json::json!({ "op": "queue", "repo": repo.root }))?;
    let jobs: Vec<pit_core::queue::ShipJob> = serde_json::from_value(v["jobs"].clone())?;
    if jobs.is_empty() {
        println!("the queue is empty. Add a lane: pit enqueue <branch>");
    }
    for j in jobs {
        let state = match j.state {
            pit_core::queue::JobState::Queued => paint(MUTED, "queued  "),
            pit_core::queue::JobState::Shipping => paint(RUN, "shipping"),
            pit_core::queue::JobState::Done => paint(DONE, "done    "),
            pit_core::queue::JobState::Conflict => paint(FAIL, "conflict"),
            pit_core::queue::JobState::Failed => paint(FAIL, "failed  "),
            pit_core::queue::JobState::Removed => paint(MUTED, "removed "),
        };
        let extra = if !j.conflicts.is_empty() {
            format!("  {}", j.conflicts.join(", "))
        } else {
            j.error.clone().map(|e| format!("  {}", e.lines().next().unwrap_or_default())).unwrap_or_default()
        };
        println!("{state}  {} → {}  {}{}", j.branch, j.target, paint(MUTED, &j.id), paint(MUTED, &extra));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn enqueue(repo: &Repo, branch: String, to: Option<String>) -> Result<ExitCode> {
    let v = daemon()?.call(serde_json::json!({ "op": "enqueue", "repo": repo.root, "branch": branch, "to": to }))?;
    println!("{} {branch} is in line for {} ({})", paint(DONE, "✓"), v["job"]["target"].as_str().unwrap_or_default(), v["job"]["id"].as_str().unwrap_or_default());
    Ok(ExitCode::SUCCESS)
}

pub fn dequeue(id: String) -> Result<ExitCode> {
    daemon()?.call(serde_json::json!({ "op": "dequeue", "id": id }))?;
    println!("{} {id} left the queue", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}
