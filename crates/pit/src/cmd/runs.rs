//! Runs: starting, following, talking to and stopping agents, and their approvals.

use std::process::ExitCode;

use anyhow::{Context, Result};
use pit_core::Repo;
use pit_core::protocol::StartArgs;
use pit_core::runs::{Client, Decision, RunEvent, RunMeta, RunState};

use super::{daemon, pitlane_home, print_json};
use crate::term::{ACCENT, DONE, FAIL, MUTED, RUN, paint, paint_err, plain};

pub fn approvals() -> Result<ExitCode> {
    let list = daemon()?.approvals()?;
    if list.is_empty() {
        println!("nothing is waiting on you");
    }
    for a in list {
        println!(
            "{}  #{} {}\n  {} {}  {}",
            paint(ACCENT, &a.run),
            a.card,
            plain(&a.card_title),
            plain(&a.approval.tool),
            plain(&a.approval.detail),
            paint(MUTED, &format!("pit allow {} | pit deny {}", a.run, a.run))
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn allow(run: String, id: Option<String>, run_wide: bool, always: bool) -> Result<ExitCode> {
    let (decision, word) = if always { (Decision::Always, "always") } else if run_wide { (Decision::Run, "run") } else { (Decision::Once, "once") };
    let approval = daemon()?.decide(&run, id, decision, None)?;
    println!("{} allowed {approval} ({word})", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}

pub fn deny(run: String, id: Option<String>, message: Option<String>) -> Result<ExitCode> {
    let approval = daemon()?.decide(&run, id, Decision::Deny, message)?;
    println!("{} denied {approval}", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}

pub fn start(repo: &Repo, card: u32, detach: bool, allow_api_key: bool, no_brief: bool, fresh: bool) -> Result<ExitCode> {
    let mut c = daemon()?;
    let m = c
        .start(StartArgs { repo: repo.root.clone(), card, allow_api_key, no_brief, fresh, ..Default::default() })
        .map_err(api_key_hint)?;
    println!("{} run {} for #{} {}", paint(DONE, "✓"), m.id, m.card, m.card_title);
    println!("  {}", paint(MUTED, m.reason.as_deref().unwrap_or_default()));
    follow_or_detach(&m.id, detach)
}

pub fn continue_run(run: String, allow_api_key: bool, detach: bool) -> Result<ExitCode> {
    let m = daemon()?.continue_run(&run, allow_api_key).map_err(api_key_hint)?;
    println!("{} run {} continues {run}", paint(DONE, "✓"), m.id);
    println!("  {}", paint(MUTED, m.reason.as_deref().unwrap_or_default()));
    follow_or_detach(&m.id, detach)
}

pub fn resume(run: String, detach: bool) -> Result<ExitCode> {
    let m = daemon()?.resume(&run, None)?;
    println!("{} run {} resumes {}", paint(DONE, "✓"), m.id, run);
    follow_or_detach(&m.id, detach)
}

pub fn runs(repo: &Repo, json: bool) -> Result<ExitCode> {
    let runs = daemon()?.list(Some(&repo.root))?;
    if json {
        return print_json(&runs);
    }
    if runs.is_empty() {
        println!("no runs yet. Start one: pit run <card>");
    }
    for m in &runs {
        println!(
            "{}  {}  #{} {}  {}  {}",
            paint(MUTED, &m.id),
            state_word(m.state),
            m.card,
            m.card_title,
            paint(MUTED, &m.branch),
            paint(MUTED, &run_figures(m))
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn log(run: String, follow: bool) -> Result<ExitCode> {
    if follow {
        return follow_run(&mut daemon()?, &run);
    }
    // Without -f: what happened so far, read from the run's record, without waiting.
    let dir = pit_core::runs::run_dir(&pitlane_home(), &run)?;
    let meta: RunMeta = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).with_context(|| format!("no run {run}"))?)?;
    let timeline = std::fs::read_to_string(dir.join("timeline.jsonl")).unwrap_or_default();
    for e in timeline.lines().filter_map(|l| serde_json::from_str::<RunEvent>(l).ok()) {
        print_event(&e);
    }
    print_end(&meta);
    Ok(ExitCode::SUCCESS)
}

pub fn say(run: String, text: String) -> Result<ExitCode> {
    daemon()?.say(&run, text)?;
    println!("{} sent; the agent reads it after its current step", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}

pub fn stop(run: String) -> Result<ExitCode> {
    daemon()?.stop(&run)?;
    println!("{} stopping {run}; the agent ends its current step", paint(DONE, "✓"));
    Ok(ExitCode::SUCCESS)
}

/// A start refused because the next option is an API key: say how to say yes.
fn api_key_hint(e: anyhow::Error) -> anyhow::Error {
    let s = format!("{e:#}");
    match s.strip_prefix(pitd::supervisor::NEEDS_API_KEY) {
        Some(reason) => anyhow::anyhow!("the route reached an API key ({reason}). It is paid per token; run again with --allow-api-key if you agree"),
        None => e,
    }
}

pub fn follow_or_detach(run: &str, detach: bool) -> Result<ExitCode> {
    if detach {
        println!("  follow with: pit log {run} -f");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}", paint(MUTED, &format!("  following. Ctrl-C leaves it running; pit stop {run} stops it")));
    follow_run(&mut daemon()?, run)
}

fn follow_run(c: &mut Client, run: &str) -> Result<ExitCode> {
    let end = c.follow(run, 0, |_, e| {
        print_event(&e);
        true
    })?;
    Ok(match end {
        Some(m) => {
            print_end(&m);
            if m.state == RunState::Done { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        // The stream ended without the run's end: the daemon went away mid-run.
        None => {
            eprintln!("{} the daemon stopped answering before the run ended", paint_err(FAIL, "✕"));
            ExitCode::FAILURE
        }
    })
}

pub fn state_word(s: RunState) -> String {
    match s {
        RunState::Starting => paint(RUN, "starting"),
        RunState::Running => paint(RUN, "running "),
        RunState::Done => paint(DONE, "done    "),
        RunState::Failed => paint(FAIL, "failed  "),
        RunState::Stopped => paint(MUTED, "stopped "),
        RunState::Interrupted => paint(ACCENT, "interrupted"),
        RunState::Verifying => paint(RUN, "checking"),
    }
}

fn run_figures(m: &RunMeta) -> String {
    let mut parts = Vec::new();
    if let Some(ms) = m.duration_ms {
        parts.push(format!("{}m{:02}s", ms / 60_000, ms / 1000 % 60));
    }
    if let Some(t) = m.turns {
        parts.push(format!("{t} turns"));
    }
    if let Some(c) = m.cost_usd {
        parts.push(format!("${c:.2}"));
    }
    parts.join(", ")
}

fn print_event(e: &RunEvent) {
    match e {
        RunEvent::Started { model, .. } => println!("{} agent started ({model})", paint(RUN, "▸")),
        RunEvent::Text { text, .. } => println!("{}", plain(text.trim_end())),
        RunEvent::Tool { name, detail, .. } => println!("  {} {}", paint(MUTED, &plain(name)), paint(MUTED, &plain(detail))),
        RunEvent::ToolDone { ok: false, detail, .. } => println!("    {} {}", paint(FAIL, "✕"), paint(MUTED, &plain(detail))),
        RunEvent::ToolDone { .. } | RunEvent::Quota { .. } => {}
        RunEvent::Shot { file, .. } => println!("    {} {}", paint(MUTED, "screenshot"), paint(MUTED, file)),
        RunEvent::Said { text } => println!("{} {}", paint(ACCENT, "you:"), plain(text)),
        RunEvent::Aside { by, text, .. } => match by.as_str() {
            "you" => println!("{} {}", paint(ACCENT, "you, on the side:"), plain(text)),
            "agent" => println!("{} {}", paint(ACCENT, "aside:"), plain(text)),
            _ => println!("{} {}", paint(ACCENT, "aside failed:"), plain(text)),
        },
        RunEvent::Tuned { model, effort } => println!(
            "{}",
            paint(MUTED, &[model.as_ref().map(|m| format!("model {m}")), effort.as_ref().map(|e| format!("effort {e}"))].into_iter().flatten().collect::<Vec<_>>().join(", "))
        ),
        RunEvent::Note { text } => println!("{}", paint(MUTED, &plain(text))),
        RunEvent::Proof { command, ok, duration_ms } => println!(
            "{} {command}  {}",
            if *ok { paint(DONE, "✓") } else { paint(FAIL, "✕") },
            paint(MUTED, &format!("{:.1}s", *duration_ms as f64 / 1000.0))
        ),
        RunEvent::Judged { verdict, summary, findings } => println!(
            "{} judge {}: {} ({findings} finding(s))",
            paint(ACCENT, "?"),
            if *verdict == pit_core::verify::Verdict::Approve { "approves" } else { "asks for changes" },
            plain(summary)
        ),
        RunEvent::Limited { window, .. } => println!("{} hit the {} usage limit", paint(ACCENT, "!"), window.replace('_', "-")),
        RunEvent::Approval { approval } => println!(
            "{} waiting for you: {} {}  {}",
            paint(ACCENT, "?"),
            plain(&approval.tool),
            plain(&approval.detail),
            paint(MUTED, "pit allow <run> [--run-wide|--always] | pit deny <run>")
        ),
        RunEvent::Decided { decision, message, .. } => println!(
            "  {} {}",
            paint(MUTED, &format!("{decision:?}").to_lowercase()),
            paint(MUTED, &plain(message.as_deref().unwrap_or_default()))
        ),
        RunEvent::Finished { ok, handoff, denied, .. } => {
            if let Some(s) = handoff.as_ref().and_then(|h| h.get("summary")).and_then(|s| s.as_str()) {
                println!("\n{} {}", if *ok { paint(DONE, "✓") } else { paint(FAIL, "✕") }, plain(s));
            }
            for d in denied {
                println!("  {} not allowed: {}", paint(ACCENT, "!"), plain(d));
            }
        }
        RunEvent::Exited { .. } => {}
    }
}

fn print_end(m: &RunMeta) {
    println!("{} {}  {}", state_word(m.state), m.id, paint(MUTED, &run_figures(m)));
    if let Some(e) = &m.error {
        println!("  {}", paint(FAIL, e.lines().next().unwrap_or_default()));
    }
}
