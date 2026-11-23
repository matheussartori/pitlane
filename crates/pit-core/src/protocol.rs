//! What the daemon and its clients say to each other: JSON, one message per line. A request is an
//! object whose `op` names what it asks for; the other keys are that op's. A reply is an object,
//! or `{"error": "..."}` when the daemon could not do it.
//!
//! Every client (the app, `pit`, its MCP server) and the daemon go through the types here, so a
//! mistyped key is caught by the compiler instead of reaching the daemon as a null. The wire
//! stays what it was before these types: keys the daemon does not know are ignored, and an
//! optional one may be left out or sent as null.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::queue::ShipJob;
use crate::runs::{Approval, Decision, RunEvent, RunMeta};

/// One request to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Start(StartArgs),
    /// Run all's analysis: questions and notes on each card not analysed yet.
    Analyze { repo: PathBuf, cards: Vec<u32> },
    /// A card blocked on its questions, all answered: read it again and free it when nothing
    /// else is open. `changed` says whether a reading started.
    Recheck { repo: PathBuf, card: u32 },
    /// What you chose for a blocked card: try again (the way that fits why it stopped), leave it
    /// for the review, or send it back to the Backlog. Replies with the run it started, if any.
    Unblock { repo: PathBuf, card: u32, action: crate::cards::Unblock },
    /// Claude's short reading of a report, written on it.
    ReportDigest { repo: PathBuf, id: String },
    /// Claude's summary of a card, and its title when `title`.
    Describe {
        repo: PathBuf,
        card: u32,
        #[serde(default)]
        title: bool,
    },
    /// Talks a card's question over with Claude; without a message, the last one is answered again.
    Talk { repo: PathBuf, card: u32, question: u32, message: Option<String> },
    StopTalk { repo: PathBuf, card: u32 },
    /// Run all's closing step: what the person asked for when everything is done.
    Finish { repo: PathBuf, cards: Vec<u32>, instructions: String, into: Option<String> },
    /// The Board's assistant: turns what the person says into cards.
    Assist { repo: PathBuf, text: String },
    Resume { run: String },
    /// The runs, of one repository or all.
    List { repo: Option<PathBuf> },
    /// Every agent session, of one repository or all.
    Sessions { repo: Option<PathBuf> },
    Get { run: String },
    Say { run: String, text: String },
    /// `/model` and `/effort` for a live session.
    Tune { run: String, model: Option<String>, effort: Option<String> },
    Stop { run: String },
    StopAll,
    Perf,
    /// Changes Performance's settings: a patch of the keys to change.
    PerfSet { config: Value },
    /// Pauses or resumes one session by hand.
    PerfHold { run: String, hold: bool },
    StopAnalyses,
    Shutdown,
    /// The desktop app holds the connection while it is open; when it closes, everything the
    /// daemon started stops and the daemon ends.
    Hold,
    Quota,
    Review { repo: PathBuf, card: u32, action: ReviewAction, message: Option<String> },
    Enqueue { repo: PathBuf, branch: String, to: Option<String> },
    Queue { repo: Option<PathBuf> },
    QueueResume,
    Dequeue { id: String },
    Continue {
        run: String,
        #[serde(default)]
        allow_api_key: bool,
    },
    /// A tool a run wants to use; the answer waits for a person when no rule allows it.
    Ask {
        run: String,
        tool: String,
        #[serde(default)]
        input: Value,
        tool_use_id: Option<String>,
    },
    /// A person's answer to what a run waits on; without `approval`, the oldest one.
    Decide { run: String, approval: Option<String>, decision: Decision, message: Option<String> },
    Approvals,
    /// Streams a run's events from `from` as `{i, event}` lines, then `{end: <run>}`.
    Follow {
        run: String,
        #[serde(default)]
        from: usize,
    },
}

/// What `start` takes besides the op: the card, and how to run it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartArgs {
    pub repo: PathBuf,
    pub card: u32,
    #[serde(default)]
    pub allow_api_key: bool,
    #[serde(default)]
    pub no_brief: bool,
    pub into: Option<String>,
    #[serde(default)]
    pub direct: bool,
    pub instructions: Option<String>,
    /// Run all's cards run on autopilot: fix rounds, more turns and landing need nobody.
    #[serde(default)]
    pub autopilot: bool,
    /// A card stopped mid-work resumes its session unless the person starts over.
    #[serde(default)]
    pub fresh: bool,
}

/// What a person decides about a card in review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewAction {
    Approve,
    Changes,
    Discard,
}

impl ReviewAction {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "approve" => Self::Approve,
            "changes" => Self::Changes,
            "discard" => Self::Discard,
            a => bail!("unknown review action {a:?}; approve, changes or discard"),
        })
    }
}

/// Every op the daemon knows, as it is written in `op`.
pub const OPS: &[&str] = &[
    "ping", "start", "analyze", "recheck", "unblock", "report_digest", "describe", "talk", "stop_talk", "finish", "assist", "resume", "list", "sessions", "get",
    "say", "tune", "stop", "stop_all", "perf", "perf_set", "perf_hold", "stop_analyses", "shutdown", "hold", "quota", "review", "enqueue",
    "queue", "queue_resume", "dequeue", "continue", "ask", "decide", "approvals", "follow",
];

/// The op a message names, whether or not it reads as a request ("" when it names none).
pub fn named_op(v: &Value) -> &str {
    v.get("op").and_then(Value::as_str).unwrap_or_default()
}

impl Request {
    /// The request in a message, or why it is not one: an op nobody knows, or a key of the op
    /// that is missing or of the wrong kind.
    pub fn from_value(v: Value) -> Result<Self> {
        let op = named_op(&v);
        if !OPS.contains(&op) {
            bail!("unknown op {op:?}");
        }
        Ok(serde_json::from_value(v)?)
    }

    /// The op, as it is written on the wire.
    pub fn op(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Start(_) => "start",
            Self::Analyze { .. } => "analyze",
            Self::Recheck { .. } => "recheck",
            Self::Unblock { .. } => "unblock",
            Self::ReportDigest { .. } => "report_digest",
            Self::Describe { .. } => "describe",
            Self::Talk { .. } => "talk",
            Self::StopTalk { .. } => "stop_talk",
            Self::Finish { .. } => "finish",
            Self::Assist { .. } => "assist",
            Self::Resume { .. } => "resume",
            Self::List { .. } => "list",
            Self::Sessions { .. } => "sessions",
            Self::Get { .. } => "get",
            Self::Say { .. } => "say",
            Self::Tune { .. } => "tune",
            Self::Stop { .. } => "stop",
            Self::StopAll => "stop_all",
            Self::Perf => "perf",
            Self::PerfSet { .. } => "perf_set",
            Self::PerfHold { .. } => "perf_hold",
            Self::StopAnalyses => "stop_analyses",
            Self::Shutdown => "shutdown",
            Self::Hold => "hold",
            Self::Quota => "quota",
            Self::Review { .. } => "review",
            Self::Enqueue { .. } => "enqueue",
            Self::Queue { .. } => "queue",
            Self::QueueResume => "queue_resume",
            Self::Dequeue { .. } => "dequeue",
            Self::Continue { .. } => "continue",
            Self::Ask { .. } => "ask",
            Self::Decide { .. } => "decide",
            Self::Approvals => "approvals",
            Self::Follow { .. } => "follow",
        }
    }
}

// ---------------------------------------------------------------- replies
//
// The ops whose answer the daemon builds itself have one of these. The others (`analyze`,
// `describe`, `talk`, `finish`, `review`, `report_digest`, `perf`, `perf_set`) answer with what
// the step that did the work returned, and stay JSON.

/// `ok`: done. The answer of `say`, `tune`, `stop`, `assist`, `shutdown`, `hold`, `queue_resume`
/// and `dequeue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
}

/// `ping`: the daemon is up, which version, which process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {
    pub ok: bool,
    pub version: String,
    pub pid: u32,
}

/// `start`, `resume`, `continue` and `get`: the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReply {
    pub run: RunMeta,
}

/// `list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunsReply {
    pub runs: Vec<RunMeta>,
}

/// `sessions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsReply {
    pub sessions: Vec<RunMeta>,
}

/// `stop_all`, `stop_analyses` and `stop_talk`: how many sessions were stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stopped {
    pub stopped: usize,
}

/// `perf_hold`: whether the session was paused or resumed (false when it already was).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Changed {
    pub changed: bool,
}

/// `quota`: the last usage events seen, per account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaReply {
    pub quota: HashMap<String, Vec<RunEvent>>,
}

/// `enqueue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobReply {
    pub job: ShipJob,
}

/// `queue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobsReply {
    pub jobs: Vec<ShipJob>,
}

/// `ask`: the answer for Claude Code's permission prompt, as it reads it (`behavior` and the rest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerReply {
    pub answer: Value,
}

/// `decide`: the approval that was answered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decided {
    pub approval: String,
}

/// `approvals`: everything waiting on a person.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalsReply {
    pub approvals: Vec<Waiting>,
}

/// One approval a run waits on, with the run and its card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Waiting {
    pub run: String,
    pub card: u32,
    pub card_title: String,
    pub approval: Approval,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Each request as it goes on the wire, against what the clients wrote by hand before.
    #[test]
    fn requests_are_what_the_clients_sent() {
        let repo = PathBuf::from("/r");
        let cases: Vec<(Request, Value)> = vec![
            (Request::Ping, json!({ "op": "ping" })),
            (
                Request::Start(StartArgs {
                    repo: repo.clone(),
                    card: 3,
                    allow_api_key: false,
                    no_brief: false,
                    into: Some("dev".into()),
                    direct: true,
                    instructions: None,
                    autopilot: true,
                    fresh: false,
                }),
                json!({ "op": "start", "repo": "/r", "card": 3, "allow_api_key": false, "no_brief": false, "into": "dev", "direct": true, "instructions": null, "autopilot": true, "fresh": false }),
            ),
            (Request::List { repo: Some(repo.clone()) }, json!({ "op": "list", "repo": "/r" })),
            (Request::Tune { run: "r1".into(), model: Some("opus".into()), effort: None }, json!({ "op": "tune", "run": "r1", "model": "opus", "effort": null })),
            (Request::PerfSet { config: json!({ "mode": "light" }) }, json!({ "op": "perf_set", "config": { "mode": "light" } })),
            (Request::StopAnalyses, json!({ "op": "stop_analyses" })),
            (
                Request::Review { repo: repo.clone(), card: 2, action: ReviewAction::Changes, message: Some("again".into()) },
                json!({ "op": "review", "repo": "/r", "card": 2, "action": "changes", "message": "again" }),
            ),
            (
                Request::Decide { run: "r1".into(), approval: None, decision: Decision::Always, message: None },
                json!({ "op": "decide", "run": "r1", "approval": null, "decision": "always", "message": null }),
            ),
            (
                Request::Ask { run: "r1".into(), tool: "Bash".into(), input: json!({ "command": "ls" }), tool_use_id: Some("t1".into()) },
                json!({ "op": "ask", "run": "r1", "tool": "Bash", "input": { "command": "ls" }, "tool_use_id": "t1" }),
            ),
            (Request::Continue { run: "r1".into(), allow_api_key: true }, json!({ "op": "continue", "run": "r1", "allow_api_key": true })),
            (Request::Follow { run: "r1".into(), from: 7 }, json!({ "op": "follow", "run": "r1", "from": 7 })),
            (Request::Hold, json!({ "op": "hold" })),
        ];
        for (req, wire) in cases {
            let sent = serde_json::to_value(&req).unwrap();
            assert_eq!(sent, wire, "{}", req.op());
            assert_eq!(sent["op"], req.op());
            let back = Request::from_value(wire).unwrap();
            assert_eq!(serde_json::to_value(&back).unwrap(), sent, "{}", req.op());
        }
    }

    /// What an older client sent still reads: optional keys left out, extra keys, nulls.
    #[test]
    fn older_clients_still_read() {
        let r = Request::from_value(json!({ "op": "start", "repo": "/r", "card": 4, "allow_api_key": true, "no_brief": true, "fresh": false })).unwrap();
        let Request::Start(a) = r else { panic!("{r:?}") };
        assert_eq!((a.card, a.allow_api_key, a.no_brief, a.direct, a.autopilot), (4, true, true, false, false));
        assert!(a.into.is_none() && a.instructions.is_none());
        assert!(matches!(Request::from_value(json!({ "op": "list" })).unwrap(), Request::List { repo: None }));
        assert!(matches!(Request::from_value(json!({ "op": "ping", "from": "someone" })).unwrap(), Request::Ping));
        let r = Request::from_value(json!({ "op": "ask", "run": "r1", "tool": "Bash", "tool_use_id": null })).unwrap();
        assert!(matches!(r, Request::Ask { input: Value::Null, tool_use_id: None, .. }), "{r:?}");
        assert!(matches!(Request::from_value(json!({ "op": "follow", "run": "r1" })).unwrap(), Request::Follow { from: 0, .. }));
    }

    #[test]
    fn what_is_not_a_request() {
        assert_eq!(Request::from_value(json!({ "op": "nope" })).unwrap_err().to_string(), "unknown op \"nope\"");
        assert_eq!(Request::from_value(json!({ "run": "r1" })).unwrap_err().to_string(), "unknown op \"\"");
        assert!(Request::from_value(json!({ "op": "get" })).is_err(), "run is required");
        assert!(Request::from_value(json!({ "op": "review", "repo": "/r", "card": 1, "action": "ship" })).is_err());
        assert!(Request::from_value(json!({ "op": "decide", "run": "r1", "decision": "maybe" })).is_err());
        // Every op in OPS is one `Request` reads: none of them is unknown to serde.
        for op in OPS {
            if let Err(e) = Request::from_value(json!({ "op": op })) {
                assert!(!e.to_string().contains("unknown variant"), "{op}: {e}");
            }
        }
    }
}
