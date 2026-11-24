//! The report of a Run all: what each card came to (state, lane, where it landed, what the agent
//! said, the judge's verdict, files changed, cost) and what the closing step did. Saved as JSON in
//! `~/.pitlane/reports/`, never in the repository; the app shows it and exports it as a PDF.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::cards;
use crate::time::now_secs;
use crate::runs::{self, RunMeta, SessionKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardReport {
    pub id: u32,
    pub title: String,
    pub state: String,
    pub lane: Option<String>,
    pub target: Option<String>,
    /// The last thing the working agent said it did (its handoff summary).
    pub summary: Option<String>,
    pub verdict: Option<String>,
    pub runs: usize,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub files: Vec<FileChange>,
    /// The last screenshots its runs took (a browser's, kept in the run's `shots/`), oldest
    /// first. Absent in reports written before they were kept; [`list`] fills them in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shots: Option<Vec<ShotRef>>,
}

/// A screenshot of a run: `runs/<run>/shots/<file>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotRef {
    pub run: String,
    pub file: String,
}

/// Claude's short reading of a report (Sonnet, no tools): one line on what the batch came to,
/// a few points on what was built, and one line per suggestion, in the suggestions' order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Digest {
    pub headline: String,
    #[serde(default)]
    pub points: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

/// Work a run noticed outside its card's spec. The person turns it into a card from the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// The card whose run raised it.
    pub card: u32,
    pub title: String,
    pub body: String,
    /// The card made from it, once made.
    #[serde(default)]
    pub made: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Finish {
    pub run: Option<String>,
    pub ok: bool,
    pub summary: Option<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub repo: PathBuf,
    pub repo_name: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub into: Option<String>,
    pub direct: bool,
    pub instructions: String,
    pub cards: Vec<CardReport>,
    /// Every session of the batch: analyses, runs, judges and the closing step.
    pub sessions: usize,
    pub cost_usd: f64,
    pub finish: Option<Finish>,
    /// What the runs noticed beyond their cards, each ready to become a card.
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    /// Written after the report, when it is first shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
}

pub fn dir(pitlane_home: &Path) -> PathBuf {
    pitlane_home.join("reports")
}

fn all_sessions(pitlane_home: &Path, repo: &Path, since: u64) -> Vec<RunMeta> {
    std::fs::read_dir(runs::runs_dir(pitlane_home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("meta.json")).ok())
        .filter_map(|t| serde_json::from_str::<RunMeta>(&t).ok())
        .filter(|m| m.repo == repo && m.started_at >= since)
        .collect()
}

fn files_of(repo: &Repo, lane: &str, base: Option<&str>) -> Vec<FileChange> {
    // The same base the run and Review count from, unless one is given (a card worked directly
    // in a branch: the commit its run started on).
    let Ok(Some(head)) = repo.git.rev_parse(&format!("refs/heads/{lane}")) else { return Vec::new() };
    let base = match base.filter(|b| !b.is_empty()) {
        Some(b) => b.to_owned(),
        None => match crate::verify::lane_base(repo, lane, &head) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        },
    };
    crate::verify::numstat(repo, &format!("{base}..{head}"))
        .unwrap_or_default()
        .into_iter()
        .map(|f| FileChange { path: f.path, added: f.added.into(), removed: f.removed.into() })
        .collect()
}

/// How many screenshots a card keeps in its report.
const SHOTS_PER_CARD: usize = 3;

/// The last screenshots of the latest of `runs` that took any.
fn shots_of(pitlane_home: &Path, runs: &[&RunMeta]) -> Vec<ShotRef> {
    for m in runs.iter().rev() {
        let dir = runs::runs_dir(pitlane_home).join(&m.id).join("shots");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|f| cards::image_type(f).is_some())
            .collect();
        if files.is_empty() {
            continue;
        }
        files.sort();
        let skip = files.len().saturating_sub(SHOTS_PER_CARD);
        return files.into_iter().skip(skip).map(|file| ShotRef { run: m.id.clone(), file }).collect();
    }
    Vec::new()
}

fn card_runs(sessions: &[RunMeta], id: u32) -> Vec<&RunMeta> {
    let mut runs: Vec<&RunMeta> = sessions.iter().filter(|m| m.card == id && m.kind == SessionKind::Run).collect();
    runs.sort_by_key(|m| m.started_at);
    runs
}

/// Builds and saves the report of a batch that started at `since` (seconds).
#[allow(clippy::too_many_arguments)]
pub fn create(repo: &Repo, pitlane_home: &Path, ids: &[u32], since: u64, into: Option<String>, direct: bool, instructions: &str, finish: Option<Finish>) -> Result<Report> {
    let sessions = all_sessions(pitlane_home, &repo.root, since);
    let mut out = Vec::new();
    let mut suggestions: Vec<Suggestion> = Vec::new();
    for &id in ids {
        let Ok(c) = cards::get(repo, id) else { continue };
        let runs = card_runs(&sessions, id);
        let summary = runs.iter().rev().find_map(|m| m.handoff.as_ref()?.get("summary")?.as_str().map(str::to_owned));
        let verdict = runs.iter().rev().find_map(|m| m.verdict).map(|v| format!("{v:?}").to_lowercase());
        let base = runs.first().and_then(|m| m.base_sha.as_deref());
        let files = match &c.lane {
            // Worked directly: what its runs added on top of where the first one started.
            Some(l) if c.direct() => files_of(repo, l, base),
            Some(l) => files_of(repo, l, None),
            None => Vec::new(),
        };
        for m in &runs {
            let Some(list) = m.handoff.as_ref().and_then(|h| h.get("suggestions")).and_then(|v| v.as_array()) else { continue };
            for x in list {
                let (Some(title), Some(body)) = (x.get("title").and_then(|v| v.as_str()), x.get("body").and_then(|v| v.as_str())) else { continue };
                let title = title.trim().to_owned();
                if title.is_empty() || suggestions.iter().any(|s: &Suggestion| s.title.eq_ignore_ascii_case(&title)) {
                    continue;
                }
                suggestions.push(Suggestion { card: id, title, body: body.trim().to_owned(), made: None });
            }
        }
        out.push(CardReport {
            id,
            title: c.title.clone(),
            state: c.state.as_str().to_owned(),
            lane: c.lane.clone(),
            target: c.target.clone(),
            summary,
            verdict,
            runs: runs.len(),
            cost_usd: runs.iter().filter_map(|m| m.cost_usd).sum(),
            duration_ms: runs.iter().filter_map(|m| m.duration_ms).sum(),
            files,
            shots: Some(shots_of(pitlane_home, &runs)),
        });
    }
    // The id is the second the report was made; a second report in the same second takes the
    // next free one instead of overwriting the first.
    let now = now_secs();
    let d = dir(pitlane_home);
    let id = (now..).find(|n| !d.join(format!("{n}.json")).exists()).unwrap_or(now);
    let r = Report {
        id: format!("{id}"),
        repo: repo.root.clone(),
        repo_name: repo.name.clone(),
        started_at: since,
        ended_at: now,
        into,
        direct,
        instructions: instructions.trim().to_owned(),
        cards: out,
        sessions: sessions.len(),
        cost_usd: sessions.iter().filter_map(|m| m.cost_usd).sum(),
        finish,
        suggestions,
        digest: None,
    };
    save(pitlane_home, &r)?;
    Ok(r)
}

/// A report's file name, refusing anything that is not one of ours (ids are timestamps).
fn file_of(id: &str) -> Result<String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        anyhow::bail!("no report {id}");
    }
    Ok(format!("{id}.json"))
}

/// Turns suggestion `index` of report `id` into a card in the backlog of `repo`, and records it.
pub fn suggest_card(repo: &Repo, pitlane_home: &Path, id: &str, index: usize) -> Result<cards::Card> {
    let path = dir(pitlane_home).join(file_of(id)?);
    let mut r: Report = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let s = r.suggestions.get(index).ok_or_else(|| anyhow::anyhow!("report {id} has no suggestion {index}"))?;
    if let Some(made) = s.made {
        anyhow::bail!("this suggestion is card #{made} already");
    }
    let body = format!("{}\n\n_Suggested by the run on #{} (Run all of {})._", s.body, s.card, r.into.as_deref().unwrap_or(&r.repo_name));
    let card = cards::add(repo, cards::NewCard { title: s.title.clone(), body, ..Default::default() })?;
    r.suggestions[index].made = Some(card.id);
    crate::fsutil::write_atomic(&path, serde_json::to_string_pretty(&r)?)?;
    Ok(card)
}

/// Deletes a report: its file waits in `reports/deleted/` so the app can undo.
pub fn delete(pitlane_home: &Path, id: &str) -> Result<()> {
    let name = file_of(id)?;
    let d = dir(pitlane_home);
    std::fs::create_dir_all(d.join("deleted"))?;
    std::fs::rename(d.join(&name), d.join("deleted").join(&name))?;
    Ok(())
}

/// Brings back a report deleted with [`delete`].
pub fn restore(pitlane_home: &Path, id: &str) -> Result<()> {
    let name = file_of(id)?;
    let d = dir(pitlane_home);
    std::fs::rename(d.join("deleted").join(&name), d.join(&name))?;
    Ok(())
}

/// A repository's reports, newest first.
pub fn list(pitlane_home: &Path, repo: &Path) -> Vec<Report> {
    let mut all: Vec<Report> = std::fs::read_dir(dir(pitlane_home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Report>(&t).ok())
        .filter(|r| r.repo == repo)
        .collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.ended_at));
    // Reports written before screenshots were kept: found once, saved with the report.
    if let Some(since) = all.iter().filter(|r| r.cards.iter().any(|c| c.shots.is_none())).map(|r| r.started_at).min() {
        let sessions = all_sessions(pitlane_home, repo, since);
        for r in all.iter_mut().filter(|r| r.cards.iter().any(|c| c.shots.is_none())) {
            for c in r.cards.iter_mut().filter(|c| c.shots.is_none()) {
                let runs: Vec<&RunMeta> = card_runs(&sessions, c.id).into_iter().filter(|m| m.started_at <= r.ended_at && m.started_at >= r.started_at).collect();
                c.shots = Some(shots_of(pitlane_home, &runs));
            }
            let _ = save(pitlane_home, r);
        }
    }
    all
}

fn save(pitlane_home: &Path, r: &Report) -> Result<()> {
    crate::fsutil::write_atomic(&dir(pitlane_home).join(file_of(&r.id)?), serde_json::to_string_pretty(r)?)
}

/// Reads report `id`.
pub fn get(pitlane_home: &Path, id: &str) -> Result<Report> {
    Ok(serde_json::from_str(&std::fs::read_to_string(dir(pitlane_home).join(file_of(id)?))?)?)
}

/// Writes Claude's digest on report `id`.
pub fn set_digest(pitlane_home: &Path, id: &str, d: Digest) -> Result<Report> {
    let mut r = get(pitlane_home, id)?;
    r.digest = Some(d);
    save(pitlane_home, &r)?;
    Ok(r)
}

/// The rules of the digest: short, plain, for someone who did not watch the run.
pub const DIGEST_RULES: &str = "You are Pitlane's report writer. You get the report of a batch of coding cards an \
agent worked through. You do not read files or run anything. Write for the person who asked for the work and did not \
watch it: plain words, no jargon they did not use, no file paths, no costs, no markdown. headline: one sentence (at \
most 140 characters) saying what the batch came to. points: two to five short lines (at most 110 characters each), \
one per thing that was built, fixed or left undone, most important first; name what the person can now see or do. \
suggestions: exactly one line per suggestion given, in the same order (at most 100 characters each), saying what it \
would do and why it matters. Never invent work the report does not show.";

pub fn digest_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "headline": { "type": "string" },
            "points": { "type": "array", "items": { "type": "string" } },
            "suggestions": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["headline", "points", "suggestions"]
    })
}

/// What the digest reads: the ask, each card's outcome and the agent's words, the suggestions.
pub fn digest_prompt(r: &Report) -> String {
    let clip = |s: &str, n: usize| s.trim().chars().take(n).collect::<String>();
    let mut p = String::new();
    if !r.instructions.trim().is_empty() {
        p.push_str(&format!("## What the person asked when it was all done\n\n{}\n\n", clip(&r.instructions, 2000)));
    }
    p.push_str("## Cards\n\n");
    for c in &r.cards {
        let added: u64 = c.files.iter().map(|f| f.added).sum();
        let removed: u64 = c.files.iter().map(|f| f.removed).sum();
        p.push_str(&format!("### #{} {} [{}] ({} files, +{added} -{removed})\n\n", c.id, c.title, c.state, c.files.len()));
        if let Some(s) = &c.summary {
            p.push_str(&clip(s, 3000));
            p.push_str("\n\n");
        }
    }
    if let Some(f) = &r.finish {
        p.push_str(&format!("## Closing step ({})\n\n{}\n\n", if f.ok { "done" } else { "failed" }, clip(f.summary.as_deref().unwrap_or(""), 2000)));
    }
    p.push_str(&format!("## Suggestions ({})\n\n", r.suggestions.len()));
    for (i, s) in r.suggestions.iter().enumerate() {
        p.push_str(&format!("{}. {}: {}\n", i + 1, s.title, clip(&s.body, 600).replace('\n', " ")));
    }
    p
}

/// Reads `claude -p --output-format json` for a digest.
pub fn read_digest(stdout: &str) -> Result<Digest> {
    let mut d: Digest = crate::runs::answer::structured(stdout, "digest")?.output;
    let tidy = |s: &str, n: usize| s.trim().trim_start_matches(['-', '•', '*', ' ']).chars().take(n).collect::<String>();
    d.headline = tidy(&d.headline, 240);
    d.points = d.points.iter().map(|s| tidy(s, 200)).filter(|s| !s.is_empty()).take(6).collect();
    d.suggestions = d.suggestions.iter().map(|s| tidy(s, 200)).collect();
    if d.headline.is_empty() {
        anyhow::bail!("the digest is empty");
    }
    Ok(d)
}

/// The prompt of the closing step: what the person asked for, and what the batch did.
pub fn finish_prompt(repo: &Repo, ids: &[u32], instructions: &str, into: Option<&str>) -> String {
    let mut p = String::from("Every card of this batch has been worked. Now do what the person asked for when it is all done.\n\n## What they asked\n\n");
    p.push_str(instructions.trim());
    let ctx = cards::project_context(repo);
    if !ctx.is_empty() {
        p.push_str("\n\n## Project context\n\nWritten by the person for every session of this project (how its repositories land, what may be pushed or deployed):\n\n");
        p.push_str(&ctx);
    }
    p.push_str("\n\n## The batch\n\n");
    if let Some(t) = into {
        p.push_str(&format!("The work was aimed at the branch {t}.\n\n"));
    }
    for &id in ids {
        if let Ok(c) = cards::get(repo, id) {
            p.push_str(&format!("- #{} {} [{}]{}\n", c.id, c.title, c.state.as_str(), c.lane.as_deref().map(|l| format!(", lane {l}")).unwrap_or_default()));
        }
    }
    p.push_str("\nDo only what they asked. If something cannot be done, say why. End with a short summary of what you did.\n");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_digest() {
        let out = r#"{"type":"result","is_error":false,"structured_output":{"headline":" Login novo no ar. ","points":["- Tela de login refeita",""],"suggestions":["Cachear avatares"]}}"#;
        let d = read_digest(out).unwrap();
        assert_eq!(d.headline, "Login novo no ar.");
        assert_eq!(d.points, vec!["Tela de login refeita"]);
        assert_eq!(d.suggestions.len(), 1);
        assert!(read_digest(r#"{"is_error":true,"result":"x"}"#).is_err());
    }
}
