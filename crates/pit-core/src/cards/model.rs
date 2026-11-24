//! What a card is: its file's fields, its questions, blocks and the moves between states.

use super::*;

/// What the card file says. The board derives more (open questions, the lane's state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardState {
    /// Captured, not triaged yet.
    #[default]
    Inbox,
    /// Specced and answered: can run.
    Ready,
    /// In Run all's queue: the batch takes it as soon as it can. New ideas stay in the Backlog,
    /// out of the batch, until someone moves them here.
    Queued,
    Running,
    /// The run finished; proofs and the judge are checking it.
    Verifying,
    Review,
    InDev,
    Done,
    Blocked,
}

impl CardState {
    pub const ALL: [CardState; 9] =
        [Self::Inbox, Self::Ready, Self::Queued, Self::Running, Self::Verifying, Self::Review, Self::InDev, Self::Done, Self::Blocked];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Review => "review",
            Self::InDev => "in-dev",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
    }

    /// New or ready: in the Backlog, out of any batch.
    pub fn in_backlog(self) -> bool {
        matches!(self, Self::Inbox | Self::Ready)
    }

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|st| st.as_str() == s)
            .with_context(|| format!("unknown state {s:?}; one of {}", Self::ALL.map(|s| s.as_str()).join(", ")))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Index into `options` of the answer the asker recommends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
    /// What the question is about and what each option changes, so it can be answered without
    /// reading the whole card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// The decision the answer was recorded as, e.g. `D-07`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// The run that asked it while working (Presence: Here), and waits on the answer while it lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// When the run asked it (only for a run's question): how long its agent has waited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<String>,
    /// Answered with the agent's recommendation because nobody was there to answer (Presence:
    /// Away, or no answer in time). It can be answered again from the card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// Talking it over with Claude: what the person asked or proposed, and what Claude answered
    /// (with the options as they stand after it). Last, so the TOML tables come after the values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub talk: Vec<Turn>,
}

/// One turn of a question's conversation. Claude's turns carry the options it now offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub at: String,
    /// `you` or `claude`.
    pub by: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
}

/// One piece of context on a card: what someone learned that the next one to work it needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub at: String,
    /// Who wrote it: `claude`, `judge` or `you`.
    pub by: String,
    pub text: String,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.answer.is_none()
    }
}

/// Why a card is Blocked, so Pitlane knows what would free it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockReason {
    /// The agent stopped to ask: answering frees it (after a recheck).
    Questions,
    /// The agent handed off part of the card and asked you to look at what it made before the
    /// next stage builds on it: answering frees it at once, and the next run hears the answer.
    Check,
    /// The agent ended blocked without a question: something it cannot do or decide.
    Stuck,
    /// The run failed (the agent crashed, or could not start).
    Failed,
    /// No account or model in the route could take the run.
    NoRoute,
    /// The route's next option is an API key, paid per token: it waits for your yes.
    ApiKey,
    /// Its branch conflicts with where it lands.
    Conflict,
    /// The checks still fail after the fix rounds.
    Checks,
    /// Set by hand, or by something that did not say why.
    #[default]
    Other,
}

/// What blocked a card, and since when.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub reason: BlockReason,
    /// In a few words, for the board and the card.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub since: String,
    /// The run that ended blocked, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Pitlane is reading the card again (with your answers, or to turn what stopped the agent
    /// into questions), since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checking: Option<String>,
    /// When Pitlane tries the card again by itself (Unix seconds); none when it waits for you.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<u64>,
}

impl Block {
    pub fn new(reason: BlockReason, detail: impl Into<String>, run: Option<String>) -> Self {
        Self { reason, detail: detail.into(), since: timestamp(now_secs()), run, checking: None, retry_at: None }
    }

    /// The same, tried again by Pitlane at `at`.
    pub fn retry_at(mut self, at: Option<u64>) -> Self {
        self.retry_at = at;
        self
    }
}

/// What can be done about a blocked card (the Unblock action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unblock {
    /// Try again, the way that fits why it stopped: read it again, run it again, another fix
    /// round, or bring the target in.
    Retry,
    /// Leave it for the review as it is (its checks failed).
    Review,
    /// Back to the Backlog, to be run when you choose.
    Backlog,
}

impl Unblock {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "retry" => Self::Retry,
            "review" => Self::Review,
            "backlog" => Self::Backlog,
            _ => bail!("unknown action {s:?} (retry, review or backlog)"),
        })
    }
}

/// Where a person moves a card: the Board's drag and the card's "Move to" both ask for one of
/// these, and only these. Running, checking, review and landing are the orchestrator's to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MoveTo {
    /// Ideas, out of any batch.
    Backlog,
    /// Run all's queue: the batch takes it.
    Queue,
    /// Held in the Backlog, with why: Analyse and Start leave it out.
    Hold,
}

impl MoveTo {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "backlog" => Self::Backlog,
            "queue" | "queued" => Self::Queue,
            "hold" => Self::Hold,
            _ => bail!("unknown destination {s:?} (backlog, queue or hold)"),
        })
    }
}

/// What a move does to a card.
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    pub patch: CardPatch,
    /// Its run is live and stops first (the session is kept: the next run resumes it).
    pub stop: bool,
}

/// The rules for moving a card by hand, the same for the drag, the card and MCP. `live`: a run
/// of the card is going now. An error says why not, in words for the person who tried.
pub fn move_rule(card: &Card, to: MoveTo, live: bool, reason: Option<&str>) -> Result<Moved> {
    use CardState::*;
    let id = card.id;
    let patch = |state: CardState| CardPatch { state: Some(state), held: Some(None), ..Default::default() };
    // Back in the Backlog: new if Claude never read it, else ready.
    let backlog = if card.analyzed.is_none() && card.questions.is_empty() && card.state != Ready { Inbox } else { Ready };
    let moved = |p: CardPatch| Ok(Moved { patch: p, stop: false });
    match (card.state, to) {
        (Verifying, _) => bail!("card #{id} is being checked; wait for its checks to end"),
        (Running, MoveTo::Hold) => bail!("card #{id} is running; move it to the Backlog first, then hold it"),
        (Running, MoveTo::Backlog) => Ok(Moved { patch: patch(Ready), stop: live }),
        (Running, MoveTo::Queue) => Ok(Moved { patch: patch(Queued), stop: live }),
        (Inbox | Ready, MoveTo::Backlog) => moved(CardPatch { held: Some(None), ..Default::default() }),
        (Inbox | Ready | Queued, MoveTo::Queue) => moved(patch(Queued)),
        (Queued, MoveTo::Backlog) => moved(patch(backlog)),
        (Inbox | Ready, MoveTo::Hold) => moved(CardPatch { held: Some(Some(reason.unwrap_or_default().to_owned())), ..Default::default() }),
        (Queued, MoveTo::Hold) => moved(CardPatch { state: Some(backlog), held: Some(Some(reason.unwrap_or_default().to_owned())), ..Default::default() }),
        (Blocked, MoveTo::Hold) | (Review, MoveTo::Hold) | (Done | InDev, MoveTo::Hold) => {
            bail!("only a Backlog card can be held; move card #{id} to the Backlog first")
        }
        (Blocked, MoveTo::Backlog) => moved(CardPatch { retries: Some(0), ..patch(Ready) }),
        (Blocked, MoveTo::Queue) => {
            if card.blocked.as_ref().is_some_and(|b| b.checking.is_some()) {
                bail!("card #{id} is being read again; wait for it, or send it to the Backlog");
            }
            if card.open_questions() > 0 {
                bail!("card #{id} has {} open question(s); answer them first", card.open_questions());
            }
            moved(CardPatch { retries: Some(0), ..patch(Queued) })
        }
        // The review's Discard: back to the Backlog with its lane, for the next run to go on in.
        (Review, MoveTo::Backlog) => moved(patch(Ready)),
        (Review, MoveTo::Queue) => bail!("card #{id} waits for your review: approve it, ask for changes, or send it to the Backlog"),
        // Reopened: its work landed, so the next run starts a lane of its own.
        (Done | InDev, MoveTo::Backlog) => moved(CardPatch { lane: Some(None), paused: Some(None), ..patch(Ready) }),
        (Done | InDev, MoveTo::Queue) => bail!("card #{id} already landed; move it to the Backlog to open it again"),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Card {
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub state: CardState,
    /// The branch this card is worked on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Place on the board, lowest first; cards without one sort by id. Run all follows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    /// When Run all's analysis read the card; it is not analysed twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed: Option<String>,
    /// Where the card's work lands (Run all's "Into"): its lane merges into this branch. When it
    /// equals `lane`, the card was worked directly in that branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Cards whose work must land before this one starts (from the analysis).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<u32>,
    /// Shared parts of the code this card changes (an engine, a schema): two cards with an area
    /// in common never run at the same time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
    /// Images attached to the card (screenshots), by file name in its images folder. Runs and the
    /// analysis get them with the spec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// A few lines on what the card is about, written by Claude (Sonnet) when the card is made
    /// or its spec changes; the board shows it instead of the spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The run that was stopped mid-work (by a person, Stop all or Pitlane closing). The next
    /// Run of the card resumes its Claude Code session, with everything it did and said, instead
    /// of starting over; a run that starts clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
    pub created: String,
    pub updated: String,
    /// Times Pitlane tried the card again by itself since it last got through (to the review or
    /// further) or you sent it on; it gives up after a few and waits for you.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Held by you in the Backlog, with why (maybe empty): Analyse and Start leave it out.
    /// Only in the Backlog; any other state forgets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
    /// Why it is Blocked; none in any other state. A table, so after the plain values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Block>,
    /// Kept last: TOML wants tables after plain values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
    /// Context gathered while working the card (by Claude, the judge or you), oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The spec, in markdown.
    #[serde(skip)]
    pub body: String,
    #[serde(skip)]
    pub path: PathBuf,
}

impl Card {
    /// Where the card sits on the board: its rank, or its id for cards never moved.
    pub fn order(&self) -> (i64, u32) {
        (self.rank.unwrap_or(self.id as i64), self.id)
    }

    /// Worked directly in its target branch, without a lane of its own.
    pub fn direct(&self) -> bool {
        self.target.is_some() && self.target == self.lane
    }

    pub fn open_questions(&self) -> usize {
        self.questions.iter().filter(|q| q.is_open()).count()
    }

    pub fn to_file(&self) -> Result<String> {
        let front = toml::to_string(self).context("could not serialise the card")?;
        let body = self.body.trim();
        Ok(if body.is_empty() { format!("+++\n{front}+++\n") } else { format!("+++\n{front}+++\n\n{body}\n") })
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        let rest = text
            .strip_prefix("+++\n")
            .or_else(|| text.strip_prefix("+++\r\n"))
            .with_context(|| format!("{} does not start with a +++ frontmatter", path.display()))?;
        // The frontmatter ends at the first `+++` line that closes valid TOML: a note quoting a
        // diff holds `+++ b/file` lines inside a multi-line string, and cutting there leaves the
        // string open, so that candidate never parses and the search moves on.
        let mut first_error = None;
        let mut found = None;
        for (end, _) in rest.match_indices("\n+++") {
            let after = &rest[end + 4..];
            if !(after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n")) {
                continue;
            }
            match toml::from_str::<Card>(&rest[..end + 1]) {
                Ok(card) => {
                    found = Some((card, after));
                    break;
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        let (mut card, after) = match (found, first_error) {
            (Some(f), _) => f,
            (None, Some(e)) => return Err(e).with_context(|| format!("{}: invalid frontmatter", path.display())),
            (None, None) => bail!("{}: frontmatter never closes", path.display()),
        };
        card.body = after.trim_start_matches(['\r', '\n']).trim_end().to_owned();
        card.path = path.to_path_buf();
        Ok(card)
    }
}

/// A card as the CLI, the MCP server and the app hand it out: the file's fields, the spec,
/// how many questions are still open and the file name.
#[derive(Debug, Clone, Serialize)]
pub struct CardOut {
    #[serde(flatten)]
    pub card: Card,
    pub body: String,
    pub open_questions: usize,
    pub file: String,
}

impl From<Card> for CardOut {
    fn from(card: Card) -> Self {
        Self {
            body: card.body.clone(),
            open_questions: card.open_questions(),
            file: card.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            card,
        }
    }
}

/// A question as the asker writes it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NewQuestion {
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub recommended: Option<usize>,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NewCard {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub questions: Vec<NewQuestion>,
}

/// Only the fields that are `Some` change. `lane: Some(None)` unlinks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<CardState>,
    pub lane: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub target: Option<Option<String>>,
    /// `Some(None)` forgets the stopped run: the next Run starts over.
    pub paused: Option<Option<String>>,
    /// Why the card is blocked, with `state: Blocked`. Leaving Blocked forgets it.
    pub blocked: Option<Block>,
    pub retries: Option<u32>,
    /// `Some(Some(why))` holds the card in the Backlog, `Some(None)` lets it go.
    pub held: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Answered {
    pub card: Card,
    pub decision: String,
}
