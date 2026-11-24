//! Reading and changing cards on disk, under the board's lock.

use super::*;

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

/// The numbers held by the card files in `dir`, read from the names: a card whose file no
/// longer parses still holds its number.
pub(super) fn file_ids(dir: &Path) -> Vec<u32> {
    fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| file_id(&e.file_name())).collect()
}

pub(super) fn deleted_ids(repo: &Repo) -> Vec<u32> {
    file_ids(&trash_dir(repo))
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
    let from = card_file(&trash_dir(repo), id).with_context(|| format!("no deleted card #{id}"))?;
    if card_file(&dir(repo), id).is_some() {
        bail!("there is already a card #{id}");
    }
    let to = dir(repo).join(from.file_name().context("a card file has a name")?);
    fs::rename(&from, &to)?;
    Card::parse(&fs::read_to_string(&to)?, &to)
}

/// The card number a file in a board folder holds: `0007-title.md` is 7. Anything else (a
/// write's temp file, a stray file) holds none.
pub(super) fn file_id(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?;
    let (digits, _) = name.strip_suffix(".md")?.split_once('-')?;
    digits.parse().ok()
}

/// The file of card `id` in `dir`, if there is one.
pub(super) fn card_file(dir: &Path, id: u32) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().find(|e| file_id(&e.file_name()) == Some(id)).map(|e| e.path())
}

pub fn get(repo: &Repo, id: u32) -> Result<Card> {
    let path = card_file(&dir(repo), id).with_context(|| format!("no card #{id}"))?;
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
    // Counted from the file names, so a card that fails to parse never has its number reused.
    let id = cards.iter().map(|c| c.id).chain(file_ids(&dir(repo))).chain(deleted_ids(repo)).max().unwrap_or(0) + 1;
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
        summary: None,
        paused: None,
        blocked: None,
        retries: 0,
        held: None,
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
        // The summary told the old spec: the board shows the spec until a new one is written.
        if b.trim() != card.body.trim() {
            card.summary = None;
        }
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
    if let Some(p) = patch.paused {
        card.paused = p;
    }
    if let Some(b) = patch.blocked {
        card.blocked = Some(b);
    }
    if let Some(n) = patch.retries {
        card.retries = n;
    }
    if let Some(h) = patch.held {
        card.held = h.map(|w| w.trim().to_owned());
    }
    // Held only in the Backlog.
    if !card.state.in_backlog() {
        card.held = None;
    }
    // Got through, or sent on by you: a later block starts with its retries again.
    if matches!(card.state, CardState::Ready | CardState::Review | CardState::InDev | CardState::Done) {
        card.retries = 0;
    }
    // A reason only while blocked, and always one then.
    if card.state != CardState::Blocked {
        card.blocked = None;
    } else if card.blocked.is_none() {
        card.blocked = Some(Block::new(BlockReason::Other, "", None));
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

/// Writes what Claude made of a new or changed card: the summary the board shows and, when
/// given, the title. Nothing changes when the spec moved on since Claude read it (`body` is the
/// spec it read): a newer description will come for it.
pub fn describe(repo: &Repo, id: u32, body: &str, title: Option<&str>, summary: &str) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    if card.body.trim() != body.trim() {
        return Ok(card);
    }
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        card.title = t.to_owned();
    }
    let summary = summary.trim();
    card.summary = (!summary.is_empty()).then(|| summary.to_owned());
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

/// A question a run asks while it works (Presence): one question, tied to the run, never
/// moving the card. Returns its id on the card.
pub fn ask_live(repo: &Repo, id: u32, question: NewQuestion, run: &str) -> Result<u32> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    push_questions(&mut card, vec![question])?;
    let q = card.questions.last_mut().context("no question added")?;
    let now = timestamp(now_secs());
    q.run = Some(run.to_owned());
    q.asked_at = Some(now.clone());
    let qid = q.id;
    card.updated = now;
    write(&card)?;
    Ok(qid)
}

/// Blocked cards Pitlane tries again by itself, due by `now` (Unix seconds).
pub fn due(repo: &Repo, now: u64) -> Result<Vec<Card>> {
    Ok(list(repo)?.0
        .into_iter()
        .filter(|c| c.state == CardState::Blocked && c.blocked.as_ref().is_some_and(|b| b.checking.is_none() && b.retry_at.is_some_and(|t| t <= now)))
        .collect())
}

/// Whether the card waits on a recheck: its agent stopped to ask, and every question is answered
/// now. Pitlane then reads the card again with the answers, and puts it back in the Backlog when
/// nothing else is open.
pub fn wants_recheck(card: &Card) -> bool {
    card.state == CardState::Blocked
        && card.blocked.as_ref().is_some_and(|b| matches!(b.reason, BlockReason::Questions | BlockReason::Check) && b.checking.is_none())
        && card.open_questions() == 0
}

/// Marks a blocked card as being read again (`true`) or not any more. Fails when it is not
/// blocked on questions, or stuck, any more (someone moved it meanwhile).
pub fn set_checking(repo: &Repo, id: u32, on: bool) -> Result<Card> {
    edit_block(repo, id, |b| {
        if !matches!(b.reason, BlockReason::Questions | BlockReason::Stuck) {
            bail!("card #{id} is not waiting on a reading");
        }
        b.checking = on.then(|| timestamp(now_secs()));
        Ok(())
    })
}

/// Changes why a card is blocked, in place. Fails when it is not blocked any more.
pub fn edit_block(repo: &Repo, id: u32, change: impl FnOnce(&mut Block) -> Result<()>) -> Result<Card> {
    let _lock = Lock::take(repo)?;
    let mut card = get(repo, id)?;
    let Some(b) = card.blocked.as_mut().filter(|_| card.state == CardState::Blocked) else {
        bail!("card #{id} is not blocked any more");
    };
    change(b)?;
    card.updated = timestamp(now_secs());
    write(&card)?;
    Ok(card)
}

pub(super) fn push_questions(card: &mut Card, new: Vec<NewQuestion>) -> Result<()> {
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
        // Blank options go, and the recommendation keeps pointing at the same option (none when
        // it was a blank one).
        let recommended = q.recommended.filter(|&r| !q.options[r].trim().is_empty()).map(|r| {
            q.options[..r].iter().filter(|o| !o.trim().is_empty()).count()
        });
        card.questions.push(Question {
            id: next,
            text: text.to_owned(),
            options: q.options.into_iter().map(|o| o.trim().to_owned()).filter(|o| !o.is_empty()).collect(),
            recommended,
            context: q.context.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()),
            answer: None,
            answered_at: None,
            decision: None,
            run: None,
            asked_at: None,
            auto: false,
            talk: Vec::new(),
        });
    }
    Ok(())
}

pub(super) fn write(card: &Card) -> Result<()> {
    atomic_write(&card.path, &card.to_file()?)
}

pub(super) fn atomic_write(path: &Path, text: &str) -> Result<()> {
    crate::fsutil::write_atomic(path, text)
}

/// The board's cross-process lock, held while the value lives: anything that allocates an id or
/// rewrites a card holds it.
pub(super) struct Lock(#[allow(dead_code)] crate::fsutil::FileLock);

impl Lock {
    pub(super) fn take(repo: &Repo) -> Result<Self> {
        crate::fsutil::FileLock::take(&repo.board.join(LOCK), LOCK_WAIT).map(Self)
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

pub(super) fn fold(c: char) -> Option<char> {
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
