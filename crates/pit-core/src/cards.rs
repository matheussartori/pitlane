//! Cards: one markdown file each in the repository's board folder, `~/.pitlane/boards/<repo>/cards/`,
//! a TOML frontmatter between `+++` lines and the spec as the body. Answers to a card's questions
//! become decisions in `decisions.md` beside them. The board lives outside the repository so it is
//! one source of truth across branches and worktrees, and no checkout, stash or clean touches it.
//! CLI, MCP server and app all write here, so every write is atomic and anything that allocates an
//! id holds the board's `.lock`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::lanes::now_secs;

pub const CARDS_DIR: &str = "cards";
pub const DECISIONS: &str = "decisions.md";
const LOCK: &str = ".lock";
/// Where cards lived before the board moved out of the repository; adopted on open.
const LEGACY_CARDS: &str = ".pitlane/cards";
const LEGACY_DECISIONS: &str = ".pitlane/decisions.md";
const LEGACY_LOCK: &str = ".pitlane/.lock";

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
    pub created: String,
    pub updated: String,
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
#[derive(Debug, Clone, Default)]
pub struct CardPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<CardState>,
    pub lane: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub target: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Answered {
    pub card: Card,
    pub decision: String,
}

/// The cards folder of this repository's board.
pub fn dir(repo: &Repo) -> PathBuf {
    repo.board.join(CARDS_DIR)
}

/// Where a card's images live, beside the cards (not in the repository).
pub fn images_dir(repo: &Repo, id: u32) -> PathBuf {
    repo.board.join("images").join(id.to_string())
}

/// The media type of an image file Pitlane accepts, from its name.
pub fn image_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// The largest image a card takes; the Claude API refuses bigger ones.
pub const IMAGE_MAX: usize = 5 * 1024 * 1024;

/// Attaches an image to the card under a fresh name built from `name`; returns the card.
pub fn attach(repo: &Repo, id: u32, name: &str, bytes: &[u8]) -> Result<Card> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if image_type(name).is_none() {
        bail!("{name} is not a PNG, JPEG, GIF or WebP image");
    }
    if bytes.len() > IMAGE_MAX {
        bail!("{name} is over 5 MB");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let stem = slug(name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name));
    let stem = if stem.is_empty() { "image".to_owned() } else { stem };
    let n = (1..).find(|n| !card.images.iter().any(|i| i.starts_with(&format!("{n}-")))).unwrap_or(1);
    let file = format!("{n}-{stem}.{ext}");
    let dir = images_dir(repo, id);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    fs::write(dir.join(&file), bytes).with_context(|| format!("could not write {file}"))?;
    card.images.push(file);
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// Takes an image off the card and deletes its file.
pub fn detach(repo: &Repo, id: u32, file: &str) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let before = card.images.len();
    card.images.retain(|i| i != file);
    if card.images.len() == before {
        bail!("card #{id} has no image {file}");
    }
    let _ = fs::remove_file(images_dir(repo, id).join(file));
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// The card's images that are still on disk: (media type, bytes).
pub fn image_bytes(repo: &Repo, card: &Card) -> Vec<(&'static str, Vec<u8>)> {
    let dir = images_dir(repo, card.id);
    card.images
        .iter()
        .filter(|f| !f.contains('/') && !f.contains(".."))
        .filter_map(|f| Some((image_type(f)?, fs::read(dir.join(f)).ok()?)))
        .collect()
}

/// The project's decisions file, beside the cards.
pub fn decisions_path(repo: &Repo) -> PathBuf {
    repo.board.join(DECISIONS)
}

/// A repository's board folder: keyed by its remote's URL, so moving or re-cloning the folder keeps
/// the board; by the main worktree's path when there is no remote.
pub fn board_for(pitlane_home: &Path, remote_url: Option<&str>, root: &Path) -> PathBuf {
    let key = match remote_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => normalize_remote(url),
        None => root.to_string_lossy().into_owned(),
    };
    let mut slug = String::new();
    for c in key.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    pitlane_home.join("boards").join(slug.trim_matches(|c| c == '-' || c == '.'))
}

/// `git@github.com:me/x.git`, `https://user@github.com/me/x` and `ssh://git@github.com/me/x.git`
/// all become `github.com/me/x`.
pub fn normalize_remote(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/');
    u = u.strip_suffix(".git").unwrap_or(u);
    let scp_like = !u.contains("://");
    if let Some((_, rest)) = u.split_once("://") {
        u = rest;
    }
    let host_end = u.find('/').unwrap_or(u.len());
    if let Some(at) = u[..host_end].rfind('@') {
        u = &u[at + 1..];
    }
    let mut out = u.to_owned();
    if scp_like && let Some(colon) = out.find(':') && !out[..colon].contains('/') {
        out.replace_range(colon..=colon, "/");
    }
    out.to_lowercase()
}

/// Moves cards and decisions left in the repository's `.pitlane/` (the old place) into the board.
/// A file the board already has with other content stays where it is, so nothing is lost.
pub fn adopt_legacy(repo: &Repo) -> Result<usize> {
    let legacy_cards = repo.root.join(LEGACY_CARDS);
    let legacy_decisions = repo.root.join(LEGACY_DECISIONS);
    if !legacy_cards.exists() && !legacy_decisions.exists() {
        return Ok(0);
    }
    let _lock = Lock::take(repo)?;
    let mut moved = 0;
    let mut adopt = |src: &Path, dst: &Path| -> Result<()> {
        let text = fs::read_to_string(src).with_context(|| format!("could not read {}", src.display()))?;
        match fs::read_to_string(dst) {
            Ok(existing) if existing != text => return Ok(()),
            Ok(_) => {}
            Err(_) => atomic_write(dst, &text)?,
        }
        fs::remove_file(src).with_context(|| format!("could not remove {}", src.display()))?;
        moved += 1;
        Ok(())
    };
    if let Ok(entries) = fs::read_dir(&legacy_cards) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if entry.path().extension().is_some_and(|e| e == "md") {
                adopt(&entry.path(), &dir(repo).join(&name))?;
            }
        }
        let _ = fs::remove_dir(&legacy_cards);
    }
    if legacy_decisions.exists() {
        adopt(&legacy_decisions, &decisions_path(repo))?;
    }
    let _ = fs::remove_file(repo.root.join(LEGACY_LOCK));
    Ok(moved)
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
    cards.sort_by_key(Card::order);
    Ok((cards, problems))
}

/// Where deleted cards go, beside the cards: a delete is undone by moving the file back.
pub fn trash_dir(repo: &Repo) -> PathBuf {
    repo.board.join("deleted")
}

fn deleted_ids(repo: &Repo) -> Vec<u32> {
    fs::read_dir(trash_dir(repo))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().split('-').next()?.parse().ok())
        .collect()
}

/// Deletes a card: its file moves to the board's `deleted/` folder (undo with `restore`), and
/// other cards stop waiting on it. Its lane and worktree are left alone.
pub fn delete(repo: &Repo, id: u32) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let card = get(repo, id)?;
    let to = trash_dir(repo);
    fs::create_dir_all(&to)?;
    let name = card.path.file_name().context("a card without a file name")?;
    fs::rename(&card.path, to.join(name)).with_context(|| format!("could not delete #{id}"))?;
    let (others, _) = list(repo)?;
    for mut c in others.into_iter().filter(|c| c.after.contains(&id)) {
        c.after.retain(|a| *a != id);
        write(&c)?;
    }
    Ok(card)
}

/// What `forget` took off a card.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Forgot {
    pub notes: usize,
    pub questions: usize,
}

/// Forgets what Claude learned on these cards, to analyse them again from scratch: its notes and
/// the judge's, the questions nobody answered yet, the analysis mark and the order it worked out.
/// Your own notes and your answers (decisions) stay.
pub fn forget(repo: &Repo, ids: &[u32]) -> Result<Forgot> {
    let _lock = Lock::take(repo)?;
    let mut out = Forgot::default();
    for &id in ids {
        let mut card = get(repo, id)?;
        let (n, q) = (card.notes.len(), card.questions.len());
        card.notes.retain(|x| x.by == "you");
        card.questions.retain(|x| !x.is_open());
        out.notes += n - card.notes.len();
        out.questions += q - card.questions.len();
        card.analyzed = None;
        card.after.clear();
        card.areas.clear();
        card.updated = timestamp(now_secs());
        write(&card)?;
    }
    Ok(out)
}

/// Brings a deleted card back.
pub fn restore(repo: &Repo, id: u32) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let prefix = format!("{id:04}-");
    let from = fs::read_dir(trash_dir(repo))
        .ok()
        .and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().starts_with(&prefix)))
        .with_context(|| format!("no deleted card #{id}"))?;
    if get(repo, id).is_ok() {
        bail!("there is already a card #{id}");
    }
    let to = dir(repo).join(from.file_name());
    fs::rename(from.path(), &to)?;
    Card::parse(&fs::read_to_string(&to)?, &to)
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
    // Deleted cards keep their numbers, so an undo never collides with a newer card.
    let id = cards.iter().map(|c| c.id).chain(deleted_ids(repo)).max().unwrap_or(0) + 1;
    // New cards go to the bottom of the board.
    let rank = cards.iter().map(|c| c.order().0).max().map_or(id as i64, |r| (r + 1).max(id as i64));
    let now = timestamp(now_secs());
    let mut card = Card {
        id,
        title: title.to_owned(),
        state: CardState::Inbox,
        lane: None,
        tags: new.tags,
        rank: Some(rank),
        analyzed: None,
        target: None,
        after: Vec::new(),
        areas: Vec::new(),
        images: Vec::new(),
        created: now.clone(),
        updated: now.clone(),
        questions: Vec::new(),
        notes: Vec::new(),
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
    let (title_before, body_before) = (card.title.clone(), card.body.clone());
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
    if let Some(t) = patch.target {
        card.target = t.filter(|b| !b.trim().is_empty()).map(|b| b.trim().to_owned());
    }
    // A changed spec is a new card to the analyst: read it again before the next Run all.
    if (title_before != card.title || body_before != card.body) && card.analyzed.is_some() {
        card.analyzed = None;
        card.after.clear();
        card.areas.clear();
    }
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

/// Puts the cards in `ids` in that order on the board; the others keep their place after them.
pub fn reorder(repo: &Repo, ids: &[u32]) -> Result<()> {
    let _lock = Lock::take(repo)?;
    let (cards, _) = list(repo)?;
    let mut order: Vec<u32> = ids.iter().copied().filter(|i| cards.iter().any(|c| c.id == *i)).collect();
    order.extend(cards.iter().map(|c| c.id).filter(|i| !ids.contains(i)));
    for (i, id) in order.into_iter().enumerate() {
        let mut card = cards.iter().find(|c| c.id == id).cloned().expect("listed above");
        let rank = i as i64 + 1;
        if card.rank != Some(rank) {
            card.rank = Some(rank);
            write(&card)?;
        }
    }
    Ok(())
}

/// Marks the card as analysed by Run all.
pub fn mark_analyzed(repo: &Repo, id: u32, after: Vec<u32>, areas: Vec<String>) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    card.analyzed = Some(timestamp(now_secs()));
    card.after = after.into_iter().filter(|a| *a != id).collect();
    card.areas = areas.into_iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    write(&card)?;
    Ok(card)
}

/// Adds a note to the card: context for whoever works it next.
pub fn note(repo: &Repo, id: u32, by: &str, text: &str) -> Result<Card> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the note is empty");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let now = timestamp(now_secs());
    card.notes.push(Note { at: now.clone(), by: by.trim().to_owned(), text: text.to_owned() });
    card.updated = now;
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

/// Adds a turn to a question's conversation. Claude's turn, when it brings options, becomes the
/// question's options, recommendation and context: the next answer chooses among them.
pub fn say(repo: &Repo, id: u32, question: u32, by: &str, text: &str, offer: Option<NewQuestion>) -> Result<Card> {
    let text = text.trim();
    if text.is_empty() {
        bail!("nothing to say");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let q = card
        .questions
        .iter_mut()
        .find(|q| q.id == question)
        .with_context(|| format!("card #{id} has no question {question}"))?;
    let now = timestamp(now_secs());
    let mut turn = Turn { at: now.clone(), by: by.trim().to_owned(), text: text.to_owned(), options: Vec::new(), recommended: None };
    if let Some(o) = offer {
        let options: Vec<String> = o.options.into_iter().map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect();
        if !options.is_empty() {
            let rec = o.recommended.filter(|r| *r < options.len());
            q.options = options.clone();
            q.recommended = rec;
            turn.options = options;
            turn.recommended = rec;
        }
        if let Some(c) = o.context.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()) {
            q.context = Some(c);
        }
    }
    q.talk.push(turn);
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

/// Chooses again on an answered question: the card and its decision (same `D-xx`) take the new
/// answer, and the decisions file keeps a line saying what it was before.
pub fn reanswer(repo: &Repo, id: u32, question: u32, answer: &str) -> Result<Answered> {
    let answer = answer.trim();
    if answer.is_empty() {
        bail!("the answer is empty");
    }
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let title = card.title.clone();
    let q = card
        .questions
        .iter_mut()
        .find(|q| q.id == question)
        .with_context(|| format!("card #{id} has no question {question}"))?;
    let Some(old) = q.answer.clone() else { bail!("question {question} on card #{id} is not answered yet") };
    let text = match answer.parse::<usize>() {
        Ok(n) if n >= 1 && n <= q.options.len() => q.options[n - 1].clone(),
        _ => answer.to_owned(),
    };
    let now = timestamp(now_secs());
    if text == old {
        return Ok(Answered { decision: q.decision.clone().unwrap_or_default(), card });
    }
    let decision = match q.decision.clone() {
        Some(d) if rewrite_decision(repo, &d, &q.text, &text, &old, id, &title, &now[..10])? => d,
        _ => {
            let d = next_decision_id(repo)?;
            append_decision(repo, &d, &q.text, &text, id, &title, &now[..10])?;
            d
        }
    };
    q.answer = Some(text);
    q.answered_at = Some(now.clone());
    q.decision = Some(decision.clone());
    card.updated = now;
    write(&card)?;
    Ok(Answered { card, decision })
}

/// Replaces the body of `## {id}: …` in the decisions file. False when the entry is not there.
#[allow(clippy::too_many_arguments)]
fn rewrite_decision(repo: &Repo, id: &str, question: &str, answer: &str, old: &str, card: u32, title: &str, day: &str) -> Result<bool> {
    let path = decisions_path(repo);
    let Ok(text) = fs::read_to_string(&path) else { return Ok(false) };
    let head = format!("## {id}:");
    let Some(start) = text.lines().scan(0usize, |at, l| {
        let here = *at;
        *at += l.len() + 1;
        Some((here, l))
    })
    .find(|(_, l)| l.starts_with(&head))
    .map(|(at, _)| at) else {
        return Ok(false);
    };
    let rest = &text[start..];
    let end = rest
        .match_indices("\n## ")
        .next()
        .map(|(i, _)| start + i + 1)
        .unwrap_or(text.len());
    let entry = format!("## {id}: {question}\n\n{answer}\n\nFrom card #{card} ({title}), {day}; changed from: {old}\n");
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str(&text[..start]);
    out.push_str(&entry);
    if end < text.len() {
        out.push('\n');
        out.push_str(&text[end..]);
    }
    atomic_write(&path, &out)?;
    Ok(true)
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
            context: q.context.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()),
            answer: None,
            answered_at: None,
            decision: None,
            talk: Vec::new(),
        });
    }
    Ok(())
}

fn next_decision_id(repo: &Repo) -> Result<String> {
    let text = fs::read_to_string(decisions_path(repo)).unwrap_or_default();
    let max = text
        .lines()
        .filter_map(|l| l.strip_prefix("## D-"))
        .filter_map(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("D-{:02}", max + 1))
}

fn append_decision(repo: &Repo, id: &str, question: &str, answer: &str, card: u32, title: &str, day: &str) -> Result<()> {
    let path = decisions_path(repo);
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

/// Cross-process lock on the board's `.lock`. A lock older than 10 s is taken as abandoned.
struct Lock(PathBuf);

impl Lock {
    fn take(repo: &Repo) -> Result<Self> {
        let path = repo.board.join(LOCK);
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
            target: None,
            after: Vec::new(),
            areas: Vec::new(),
            images: Vec::new(),
            created: "2026-11-20T10:00:00Z".into(),
            updated: "2026-11-20T10:00:00Z".into(),
            questions: vec![Question {
                id: 1,
                text: "Which limit?".into(),
                options: vec!["256 KB".into(), "1 MB".into()],
                recommended: Some(0),
                context: None,
                answer: None,
                answered_at: None,
                decision: None,
                talk: Vec::new(),
            }],
            rank: Some(3),
            analyzed: None,
            notes: vec![Note { at: "2026-11-20T11:00:00Z".into(), by: "claude".into(), text: "Avatars go through `upload.ts`.\n\nTwo lines.".into() }],
            body: "## Goal\n\nRefuse avatars over the limit.".into(),
            path: PathBuf::from("x.md"),
        };
        let text = card.to_file().unwrap();
        assert!(text.starts_with("+++\nid = 41\n"));
        let back = Card::parse(&text, Path::new("x.md")).unwrap();
        assert_eq!(back.title, card.title);
        assert_eq!(back.lane, card.lane);
        assert_eq!(back.questions, card.questions);
        assert_eq!(back.notes, card.notes);
        assert_eq!(back.rank, Some(3));
        assert_eq!(back.body, card.body);
        assert_eq!(back.open_questions(), 1);
    }
}
