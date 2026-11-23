//! Answers to a card's questions, and the decisions they become in `decisions.md`.

use super::*;

/// Records an answer on the card and as a numbered decision. `answer` may be an option's
/// number (1-based), an option's text or free text.
pub fn answer(repo: &Repo, id: u32, question: u32, answer: &str) -> Result<Answered> {
    answer_as(repo, id, question, answer, false)
}

/// `answer`, saying whether it is the agent's recommendation taken because nobody answered
/// (`auto`): the decision then says so, and the card can still be answered again.
pub fn answer_as(repo: &Repo, id: u32, question: u32, answer: &str, auto: bool) -> Result<Answered> {
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
    let question_text = q.text.clone();
    q.answer = Some(text.clone());
    q.answered_at = Some(now.clone());
    q.decision = Some(decision.clone());
    q.auto = auto;
    let text = if auto { format!("{text} (the agent's recommendation; nobody was there to answer)") } else { text };
    card.updated = now;
    // Every question answered: the card is ready, unless someone already moved it on.
    if card.state == CardState::Inbox && card.open_questions() == 0 {
        card.state = CardState::Ready;
    }
    // The card first, then the decision: a card that could not be written leaves no decision
    // behind (a retry would add a second one), and a decision that could not be written puts
    // the card back as it was, still asking.
    let before = get(repo, id)?;
    write(&card)?;
    if let Err(e) = append_decision(repo, &decision, &question_text, &text, id, &card.title, &local_day(now_s)) {
        let _ = write(&before);
        return Err(e);
    }
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
    let now_s = now_secs();
    let now = timestamp(now_s);
    if text == old {
        // The same answer on one the agent took for you: now it is yours.
        if q.auto {
            q.auto = false;
            card.updated = now;
            write(&card)?;
        }
        let decision = card.questions.iter().find(|q| q.id == question).and_then(|q| q.decision.clone()).unwrap_or_default();
        return Ok(Answered { decision, card });
    }
    let decision = match q.decision.clone() {
        Some(d) if rewrite_decision(repo, &d, &q.text, &text, &old, id, &title, &local_day(now_s))? => d,
        _ => {
            let d = next_decision_id(repo)?;
            append_decision(repo, &d, &q.text, &text, id, &title, &local_day(now_s))?;
            d
        }
    };
    q.answer = Some(text);
    q.answered_at = Some(now.clone());
    q.decision = Some(decision.clone());
    q.auto = false;
    card.updated = now;
    write(&card)?;
    Ok(Answered { card, decision })
}

/// Replaces the body of `## {id}: …` in the decisions file. False when the entry is not there.
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_decision(repo: &Repo, id: &str, question: &str, answer: &str, old: &str, card: u32, title: &str, day: &str) -> Result<bool> {
    let path = decisions_path(repo);
    let Some(text) = crate::fsutil::read_optional(&path)? else { return Ok(false) };
    let head = format!("## {id}:");
    // Offsets from the lines as they are (`\n` or `\r\n`), so the splice lands on the heading.
    let Some(start) = text
        .split_inclusive('\n')
        .scan(0usize, |at, l| {
            let here = *at;
            *at += l.len();
            Some((here, l))
        })
        .find(|(_, l)| l.starts_with(&head))
        .map(|(at, _)| at)
    else {
        return Ok(false);
    };
    let rest = &text[start..];
    let end = rest
        .match_indices("\n## ")
        .next()
        .map(|(i, _)| start + i + 1)
        .unwrap_or(text.len());
    let (question, answer, old, title) = (one_line(question), entry_body(answer), one_line(old), one_line(title));
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

pub(super) fn next_decision_id(repo: &Repo) -> Result<String> {
    let text = crate::fsutil::read_optional(&decisions_path(repo))?.unwrap_or_default();
    let max = text
        .lines()
        .filter_map(|l| l.strip_prefix("## D-"))
        .filter_map(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("D-{:02}", max + 1))
}

pub(super) fn append_decision(repo: &Repo, id: &str, question: &str, answer: &str, card: u32, title: &str, day: &str) -> Result<()> {
    let path = decisions_path(repo);
    let mut text = crate::fsutil::read_optional(&path)?.unwrap_or_else(|| {
        "# Decisions\n\nAnswers given while specifying cards. Agents read these before working.\n".to_owned()
    });
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let (question, answer, title) = (one_line(question), entry_body(answer), one_line(title));
    text.push_str(&format!("\n## {id}: {question}\n\n{answer}\n\nFrom card #{card} ({title}), {day}.\n"));
    atomic_write(&path, &text)
}

/// Text that goes on an entry's one line (its heading, the card's title): a question or title
/// with line breaks in it could otherwise start an entry of its own.
pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An entry's answer: kept as written, except that a line starting with `#` is escaped, so the
/// answer never opens a heading (a forged `## D-xx` entry) in the decisions file.
pub(super) fn entry_body(text: &str) -> String {
    text.trim()
        .lines()
        .map(|l| if l.trim_start().starts_with('#') { format!("\\{}", l.trim_start()) } else { l.to_owned() })
        .collect::<Vec<_>>()
        .join("\n")
}
