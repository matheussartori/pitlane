//! Checking a run's work before a person reviews it: deterministic proofs (the repository's own
//! commands) and a judge, a separate Claude Code session that sees only the spec and the diff and
//! fixes nothing.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Repo;
use crate::cards::Card;
use crate::lanes::Target;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proof {
    pub command: String,
    pub ok: bool,
    pub code: Option<i32>,
    pub duration_ms: u64,
    /// The end of what it printed, for when it fails.
    pub tail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Approve,
    Changes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    /// blocker, major, minor or nit.
    pub severity: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgement {
    pub verdict: Verdict,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JudgeRun {
    pub model: Option<String>,
    pub judgement: Option<Judgement>,
    pub error: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
}

/// What `verify.json` in a run's folder holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Verification {
    pub run: String,
    pub proofs: Vec<Proof>,
    /// No `[verify] commands` in flow.toml.
    pub no_commands: bool,
    pub judge: Option<JudgeRun>,
    /// Rounds of changes allowed on the card ([verify] max_rounds).
    #[serde(default)]
    pub max_rounds: u32,
    pub started_at: u64,
    pub ended_at: Option<u64>,
}

impl Verification {
    pub fn proofs_ok(&self) -> bool {
        self.proofs.iter().all(|p| p.ok)
    }
}

/// Runs one proof in the worktree, through the shell, like a person would.
pub fn run_proof(worktree: &Path, command: &str) -> Proof {
    let t = Instant::now();
    let out = Command::new("sh")
        .arg("-c")
        // Grouped, so stderr of every part of a compound command lands in the output.
        .arg(format!("{{ {command}\n}} 2>&1"))
        .current_dir(worktree)
        .stdin(Stdio::null())
        .output();
    let (ok, code, text) = match out {
        Ok(o) => (o.status.success(), o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (false, None, format!("could not run it: {e}")),
    };
    Proof { command: command.to_owned(), ok, code, duration_ms: t.elapsed().as_millis() as u64, tail: tail(&text, 40, 4000) }
}

fn tail(text: &str, lines: usize, bytes: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let mut t = all[all.len().saturating_sub(lines)..].join("\n");
    if t.len() > bytes {
        let cut = t.len() - bytes;
        let at = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
        t = format!("…{}", &t[at..]);
    }
    t
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// What a lane changes against production: per-file counts and the patch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaneDiff {
    pub base: String,
    pub head: String,
    pub files: Vec<FileStat>,
    pub patch: String,
    /// The patch was cut at the size limit; the counts are complete.
    pub truncated: bool,
}

/// The lane against production, or against `from` (a commit of the lane) for one round's work.
pub fn lane_diff(repo: &Repo, branch: &str, limit_bytes: usize, from: Option<&str>) -> Result<LaneDiff> {
    let head = repo.git.rev_parse(&format!("refs/heads/{branch}"))?.with_context(|| format!("no local branch {branch}"))?;
    let base = match from {
        Some(f) => repo.git.rev_parse(f)?.with_context(|| format!("no commit {f}"))?,
        None => {
            let prod = Target::resolve(repo, &repo.flow.flow.production)?.context("production branch not found")?;
            repo.git.run(&["merge-base", &prod.sha, &head])?
        }
    };
    let range = format!("{base}..{head}");
    let files = repo
        .git
        .lines(&["diff", "--numstat", &range])?
        .into_iter()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let (a, r, p) = (f.next()?, f.next()?, f.next()?);
            Some(FileStat { path: p.to_owned(), added: a.parse().unwrap_or(0), removed: r.parse().unwrap_or(0) })
        })
        .collect();
    let mut patch = repo.git.run(&["diff", "--no-color", "--no-ext-diff", &range])?;
    let truncated = patch.len() > limit_bytes;
    if truncated {
        let at = (0..=limit_bytes).rev().find(|i| patch.is_char_boundary(*i)).unwrap_or(0);
        patch.truncate(at);
    }
    Ok(LaneDiff { base, head, files, patch, truncated })
}

pub fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["approve", "changes"],
                "description": "approve: the diff does what the spec asks and nothing it should not. changes: it needs work." },
            "summary": { "type": "string", "description": "Two sentences at most, for the person who reviews." },
            "findings": { "type": "array", "items": { "type": "object", "properties": {
                "file": { "type": "string" },
                "line": { "type": "integer" },
                "severity": { "type": "string", "enum": ["blocker", "major", "minor", "nit"] },
                "text": { "type": "string", "description": "What is wrong and why, in one or two sentences." } },
                "required": ["severity", "text"] } }
        },
        "required": ["verdict", "summary", "findings"]
    })
}

/// Fixed rules for the judge: same bytes every time.
pub const JUDGE_RULES: &str = "You are the reviewer in Pitlane. Another agent made the change below. You did not write it and \
you do not fix it: you read the spec, the decisions and the diff, open files to check context when you need to, and give a \
verdict. Judge only against the spec and the decisions: does the change do what was asked, completely, without breaking or \
adding anything it should not? Ignore style the project does not ask for. Every finding names the file and line when it can.";

/// The judge's prompt: the spec, the decisions and the diff. Nothing the working agent said.
pub fn judge_prompt(card: &Card, project_decisions: &str, diff: &LaneDiff, proofs: &[Proof]) -> String {
    let mut p = format!("# Card #{}: {}\n\n{}\n", card.id, card.title, if card.body.trim().is_empty() { "(No spec beyond the title.)" } else { card.body.trim() });
    let answered: Vec<_> = card.questions.iter().filter(|q| q.answer.is_some()).collect();
    if !answered.is_empty() {
        p.push_str("\n## Decisions on this card\n\n");
        for q in answered {
            p.push_str(&format!("- {}: {}\n", q.text, q.answer.as_deref().unwrap_or_default()));
        }
    }
    if !project_decisions.trim().is_empty() {
        p.push_str(&format!("\n## Project decisions\n\n{}\n", project_decisions.trim()));
    }
    if !proofs.is_empty() {
        p.push_str("\n## Checks already run\n\n");
        for pr in proofs {
            p.push_str(&format!("- `{}`: {}\n", pr.command, if pr.ok { "passed" } else { "failed" }));
        }
    }
    p.push_str(&format!("\n## The diff ({}..{})\n\n", &diff.base[..diff.base.len().min(8)], &diff.head[..diff.head.len().min(8)]));
    for f in &diff.files {
        p.push_str(&format!("- {} (+{} -{})\n", f.path, f.added, f.removed));
    }
    p.push_str(&format!("\n```diff\n{}\n```\n", diff.patch));
    if diff.truncated {
        p.push_str("\nThe patch was cut for size; read the files it lists to see the rest.\n");
    }
    p.push_str("\nGive your verdict.\n");
    p
}

/// Reads the judge's `--output-format json` answer.
pub fn read_judge(stdout: &str) -> Result<(Judgement, Option<f64>, Option<u64>)> {
    let v: Value = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l).ok().filter(|v| v.get("type").and_then(Value::as_str) == Some("result")))
        .or_else(|| serde_json::from_str(stdout).ok())
        .context("the judge gave no result")?;
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!("the judge stopped with an error: {}", v.get("result").map(|r| r.to_string()).unwrap_or_default());
    }
    let j: Judgement = serde_json::from_value(v.get("structured_output").cloned().context("the judge gave no verdict")?)
        .context("the judge's verdict does not match the schema")?;
    Ok((j, v.get("total_cost_usd").and_then(Value::as_f64), v.get("duration_ms").and_then(Value::as_u64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_verdict() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.04,"duration_ms":900,
            "structured_output":{"verdict":"changes","summary":"Misses the SVG case.","findings":[{"file":"a.ts","line":3,"severity":"major","text":"SVG is not refused."}]}}"#;
        let (j, cost, _) = read_judge(&out.replace('\n', " ")).unwrap();
        assert_eq!(j.verdict, Verdict::Changes);
        assert_eq!(j.findings[0].line, Some(3));
        assert_eq!(cost, Some(0.04));
        assert!(read_judge(r#"{"type":"result","is_error":true,"result":"boom"}"#).is_err());
    }

    #[test]
    fn proofs_keep_the_tail() {
        let dir = std::env::temp_dir();
        let ok = run_proof(&dir, "echo fine");
        assert!(ok.ok && ok.tail.contains("fine"));
        let bad = run_proof(&dir, "echo broken >&2; exit 3");
        assert!(!bad.ok && bad.code == Some(3) && bad.tail.contains("broken"));
        assert_eq!(tail(&"x\n".repeat(100), 3, 1000), "x\nx\nx");
    }
}
