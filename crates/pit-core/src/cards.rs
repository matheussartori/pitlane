//! Cards: one markdown file each in `.pitlane/cards/`, a TOML frontmatter between `+++` lines
//! and the spec as the body. Answers to a card's questions become decisions in
//! `.pitlane/decisions.md`. CLI, MCP server and app all write here, so every write is atomic
//! and anything that allocates an id holds `.pitlane/.lock`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::lanes::now_secs;

pub const CARDS_DIR: &str = ".pitlane/cards";
pub const DECISIONS: &str = ".pitlane/decisions.md";
const LOCK: &str = ".pitlane/.lock";

/// What the card file says. The board derives more (open questions, the lane's state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardState {
    /// Captured, not triaged yet.
    #[default]
    Inbox,
    /// Specced and answered: can run.
    Ready,
    Running,
    /// The run finished; proofs and the judge are checking it.
    Verifying,
    Review,
    InDev,
    Done,
    Blocked,
}

impl CardState {
    pub const ALL: [CardState; 8] =
        [Self::Inbox, Self::Ready, Self::Running, Self::Verifying, Self::Review, Self::InDev, Self::Done, Self::Blocked];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Review => "review",
            Self::InDev => "in-dev",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// The decision the answer was recorded as, e.g. `D-07`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.answer.is_none()
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
    pub created: String,
    pub updated: String,
    /// Kept last: TOML wants tables after plain values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
    /// The spec, in markdown.
    #[serde(skip)]
    pub body: String,
    #[serde(skip)]
    pub path: PathBuf,
}

impl Card {
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
        let end = rest.find("\n+++").with_context(|| format!("{}: frontmatter never closes", path.display()))?;
        let mut card: Card =
            toml::from_str(&rest[..end + 1]).with_context(|| format!("{}: invalid frontmatter", path.display()))?;
        let after = &rest[end + 4..];
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
#[derive(Debug, Clone, Default)]
pub struct CardPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<CardState>,
    pub lane: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Answered {
    pub card: Card,
    pub decision: String,
}

/// The cards folder of this repository (in the main worktree).
pub fn dir(repo: &Repo) -> PathBuf {
    repo.root.join(CARDS_DIR)
}

/// Every card, by id. Files that fail to parse are reported, never dropped silently.
pub fn list(repo: &Repo) -> Result<(Vec<Card>, Vec<String>)> {
    let dir = dir(repo);
    let mut cards = Vec::new();
    let mut problems = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((cards, problems)),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", dir.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") || !name.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        match fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|t| Card::parse(&t, &path)) {
            Ok(c) => cards.push(c),
            Err(e) => problems.push(format!("{e:#}")),
        }
    }
    cards.sort_by_key(|c| c.id);
    Ok((cards, problems))
}

pub fn get(repo: &Repo, id: u32) -> Result<Card> {
    let prefix = format!("{id:04}-");
    let dir = dir(repo);
    let entry = fs::read_dir(&dir)
        .ok()
        .and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().starts_with(&prefix)))
        .with_context(|| format!("no card #{id}"))?;
    let path = entry.path();
    Card::parse(&fs::read_to_string(&path)?, &path)
}

pub fn add(repo: &Repo, new: NewCard) -> Result<Card> {
    let title = new.title.trim();
    if title.is_empty() {
        bail!("a card needs a title");
    }
    let _lock = Lock::take(repo)?;
    let (cards, _) = list(repo)?;
    let id = cards.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let now = timestamp(now_secs());
    let mut card = Card {
        id,
        title: title.to_owned(),
        state: CardState::Inbox,
        lane: None,
        tags: new.tags,
        created: now.clone(),
        updated: now.clone(),
        questions: Vec::new(),
        body: new.body,
        path: dir(repo).join(format!("{id:04}-{}.md", slug(title))),
    };
    push_questions(&mut card, new.questions)?;
    write(&card)?;
    Ok(card)
}

pub fn update(repo: &Repo, id: u32, patch: CardPatch) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    if let Some(t) = patch.title {
        if t.trim().is_empty() {
            bail!("a card needs a title");
        }
        card.title = t.trim().to_owned();
    }
    if let Some(b) = patch.body {
        card.body = b;
    }
    if let Some(s) = patch.state {
        card.state = s;
    }
    if let Some(l) = patch.lane {
        card.lane = l.filter(|b| !b.trim().is_empty()).map(|b| b.trim().to_owned());
    }
    if let Some(t) = patch.tags {
        card.tags = t;
    }
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// Adds questions in one batch: the interview asks everything at once.
pub fn ask(repo: &Repo, id: u32, questions: Vec<NewQuestion>) -> Result<Card> {
    if questions.is_empty() {
        bail!("no questions to add");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let now = timestamp(now_secs());
    push_questions(&mut card, questions)?;
    card.updated = now;
    write(&card)?;
    Ok(card)
}

/// Records an answer on the card and as a numbered decision. `answer` may be an option's
/// number (1-based), an option's text or free text.
pub fn answer(repo: &Repo, id: u32, question: u32, answer: &str) -> Result<Answered> {
    let answer = answer.trim();
    if answer.is_empty() {
        bail!("the answer is empty");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let q = card
        .questions
        .iter_mut()
        .find(|q| q.id == question)
        .with_context(|| format!("card #{id} has no question {question}"))?;
    if !q.is_open() {
        bail!("question {question} on card #{id} is already answered");
    }
    let text = match answer.parse::<usize>() {
        Ok(n) if n >= 1 && n <= q.options.len() => q.options[n - 1].clone(),
        _ => answer.to_owned(),
    };
    let now_s = now_secs();
    let now = timestamp(now_s);
    let decision = next_decision_id(repo)?;
    append_decision(repo, &decision, &q.text, &text, id, &card.title, &now[..10])?;
    q.answer = Some(text);
    q.answered_at = Some(now.clone());
    q.decision = Some(decision.clone());
    card.updated = now;
    // Every question answered: the card is ready, unless someone already moved it on.
    if card.state == CardState::Inbox && card.open_questions() == 0 {
        card.state = CardState::Ready;
    }
    write(&card)?;
    Ok(Answered { card, decision })
}

fn push_questions(card: &mut Card, new: Vec<NewQuestion>) -> Result<()> {
    let first = card.questions.iter().map(|q| q.id).max().unwrap_or(0) + 1;
    for (next, q) in (first..).zip(new) {
        let text = q.text.trim();
        if text.is_empty() {
            bail!("a question has no text");
        }
        if let Some(r) = q.recommended
            && r >= q.options.len()
        {
            bail!("recommended option {r} is out of range for {:?}", text);
        }
        card.questions.push(Question {
            id: next,
            text: text.to_owned(),
            options: q.options.into_iter().map(|o| o.trim().to_owned()).filter(|o| !o.is_empty()).collect(),
            recommended: q.recommended,
            answer: None,
            answered_at: None,
            decision: None,
        });
    }
    Ok(())
}

fn next_decision_id(repo: &Repo) -> Result<String> {
    let text = fs::read_to_string(repo.root.join(DECISIONS)).unwrap_or_default();
    let max = text
        .lines()
        .filter_map(|l| l.strip_prefix("## D-"))
        .filter_map(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("D-{:02}", max + 1))
}

fn append_decision(repo: &Repo, id: &str, question: &str, answer: &str, card: u32, title: &str, day: &str) -> Result<()> {
    let path = repo.root.join(DECISIONS);
    let mut text = fs::read_to_string(&path).unwrap_or_else(|_| {
        "# Decisions\n\nAnswers given while specifying cards. Agents read these before working.\n".to_owned()
    });
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("\n## {id}: {question}\n\n{answer}\n\nFrom card #{card} ({title}), {day}.\n"));
    atomic_write(&path, &text)
}

fn write(card: &Card) -> Result<()> {
    atomic_write(&card.path, &card.to_file()?)
}

fn atomic_write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, text).with_context(|| format!("could not write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))
}

/// Cross-process lock on `.pitlane/.lock`. A lock older than 10 s is taken as abandoned.
struct Lock(PathBuf);

impl Lock {
    fn take(repo: &Repo) -> Result<Self> {
        let path = repo.root.join(LOCK);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        for _ in 0..250 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(10));
                    if stale {
                        let _ = fs::remove_file(&path);
                    } else {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("could not lock {}", path.display())),
            }
        }
        bail!("{} is held by another pit process; try again", path.display())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// `Rate limit the invites endpoint` → `rate-limit-the-invites-endpoint` (accents folded, 48 chars at most).
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(fold).flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut s: String = out.trim_matches('-').chars().take(48).collect();
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() { "card".into() } else { s }
}

fn fold(c: char) -> Option<char> {
    Some(match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        c => c,
    })
}

/// Unix seconds to `2026-11-20T14:03:00Z` (UTC), without a date crate.
pub fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_795_168_980), "2026-11-20T10:03:00Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Rate limit the invites endpoint"), "rate-limit-the-invites-endpoint");
        assert_eq!(slug("Validar tamanho do avatar (256 KB)"), "validar-tamanho-do-avatar-256-kb");
        assert_eq!(slug("Ação: botão de convite!"), "acao-botao-de-convite");
        assert_eq!(slug("!!!"), "card");
    }

    #[test]
    fn file_round_trip() {
        let card = Card {
            id: 41,
            title: "Limit avatar size".into(),
            state: CardState::Inbox,
            lane: Some("fix/avatar".into()),
            tags: vec!["core".into()],
            created: "2026-11-20T10:00:00Z".into(),
            updated: "2026-11-20T10:00:00Z".into(),
            questions: vec![Question {
                id: 1,
                text: "Which limit?".into(),
                options: vec!["256 KB".into(), "1 MB".into()],
                recommended: Some(0),
                answer: None,
                answered_at: None,
                decision: None,
            }],
            body: "## Goal\n\nRefuse avatars over the limit.".into(),
            path: PathBuf::from("x.md"),
        };
        let text = card.to_file().unwrap();
        assert!(text.starts_with("+++\nid = 41\n"));
        let back = Card::parse(&text, Path::new("x.md")).unwrap();
        assert_eq!(back.title, card.title);
        assert_eq!(back.lane, card.lane);
        assert_eq!(back.questions, card.questions);
        assert_eq!(back.body, card.body);
        assert_eq!(back.open_questions(), 1);
    }
}
