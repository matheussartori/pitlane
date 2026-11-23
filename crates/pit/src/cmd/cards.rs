//! Cards, and what Pitlane knows about the code they touch: the plan, the index, briefings.

use std::process::ExitCode;

use anyhow::{Result, bail};
use pit_core::Repo;
use pit_core::cards::{self, CardOut, CardPatch, CardState, NewCard, NewQuestion};

use super::{pitlane_home, print_json};
use crate::cli::CardCmd;
use crate::term::{ACCENT, DONE, FAIL, MUTED, paint, plain};

pub fn card(repo: &Repo, cmd: CardCmd) -> Result<ExitCode> {
    match cmd {
        CardCmd::Add { title, body, body_file, tags } => {
            let body = match body_file {
                Some(p) if p.as_os_str() == "-" => std::io::read_to_string(std::io::stdin())?,
                Some(p) => std::fs::read_to_string(&p)?,
                None => body.unwrap_or_default(),
            };
            let c = cards::add(repo, NewCard { title, body, tags, questions: Vec::new() })?;
            println!("{} #{} {}", paint(DONE, "✓"), c.id, c.title);
            println!("  {}", c.path.display());
        }
        CardCmd::List { state, json } => {
            let state = state.map(|s| CardState::parse(&s)).transpose()?;
            let (all, problems) = cards::list(repo)?;
            let shown: Vec<_> = all.into_iter().filter(|c| state.is_none_or(|s| c.state == s)).collect();
            if json {
                let out: Vec<CardOut> = shown.into_iter().map(CardOut::from).collect();
                return print_json(&out);
            }
            if shown.is_empty() {
                println!("no cards. Add one: pit card add \"<title>\"");
            }
            for c in &shown {
                let open = c.open_questions();
                let asks = if open > 0 { paint(ACCENT, &format!("  {open} to answer")) } else { String::new() };
                let lane = c.lane.as_deref().map(|l| paint(MUTED, &format!("  {}", plain(l)))).unwrap_or_default();
                println!("{}  {:<8}  {}{asks}{lane}", paint(MUTED, &format!("#{:04}", c.id)), c.state.as_str(), plain(&c.title));
            }
            for p in problems {
                eprintln!("{} {p}", paint(FAIL, "skipped:"));
            }
        }
        CardCmd::Show { id, json } => {
            let c = cards::get(repo, id)?;
            if json {
                return print_json(&CardOut::from(c));
            }
            println!("#{} {}  ({}{})", c.id, plain(&c.title), c.state.as_str(), c.lane.as_deref().map(|l| format!(", lane {}", plain(l))).unwrap_or_default());
            if !c.body.is_empty() {
                println!("\n{}", plain(&c.body));
            }
            for q in &c.questions {
                println!("\n{} {}", paint(if q.is_open() { ACCENT } else { MUTED }, &format!("Q{}", q.id)), plain(&q.text));
                for (i, o) in q.options.iter().enumerate() {
                    let rec = if q.recommended == Some(i) { "  (recommended)" } else { "" };
                    println!("   {}. {o}{rec}", i + 1);
                }
                if let Some(a) = &q.answer {
                    println!("   {} {a}  {}", paint(DONE, "→"), paint(MUTED, q.decision.as_deref().unwrap_or("")));
                }
            }
        }
        CardCmd::Move { id, state } => {
            let c = cards::update(repo, id, CardPatch { state: Some(CardState::parse(&state)?), ..Default::default() })?;
            println!("{} #{} is {}", paint(DONE, "✓"), c.id, c.state.as_str());
        }
        CardCmd::Ask { id, question, options, recommend } => {
            let recommended = match recommend {
                Some(0) => bail!("--recommend counts from 1"),
                Some(n) => Some(n - 1),
                None => None,
            };
            let c = cards::ask(repo, id, vec![NewQuestion { text: question, options, recommended, context: None }])?;
            println!("{} #{} has {} open question(s)", paint(DONE, "✓"), c.id, c.open_questions());
        }
        CardCmd::Answer { id, question, answer } => {
            let a = cards::answer(repo, id, question, &answer)?;
            println!("{} recorded as {} in {}", paint(DONE, "✓"), a.decision, cards::decisions_path(repo).display());
        }
        CardCmd::Link { id, branch, unlink } => {
            if branch.is_none() && !unlink {
                bail!("name a branch, or pass --unlink");
            }
            let c = cards::update(repo, id, CardPatch { lane: Some(branch), ..Default::default() })?;
            match &c.lane {
                Some(l) => println!("{} #{} is worked on {l}", paint(DONE, "✓"), c.id),
                None => println!("{} #{} has no lane", paint(DONE, "✓"), c.id),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn plan(repo: &Repo, json: bool) -> Result<ExitCode> {
    let p = pit_core::plan::plan(repo, &pitlane_home())?;
    if json {
        return print_json(&p);
    }
    print_plan(&p);
    Ok(ExitCode::SUCCESS)
}

pub fn index(repo: &Repo) -> Result<ExitCode> {
    let (_, st) = pit_core::index::update(repo, &pitlane_home())?;
    println!(
        "{} {} files, {} symbols ({} parsed, {} unchanged, {} skipped) in {}ms",
        paint(DONE, "✓"),
        st.files,
        st.symbols,
        st.parsed,
        st.reused,
        st.skipped,
        st.ms
    );
    Ok(ExitCode::SUCCESS)
}

pub fn brief(repo: &Repo, card: Option<u32>, text: Option<String>, json: bool, budget: usize) -> Result<ExitCode> {
    let c = match (card, text) {
        (Some(id), None) => cards::get(repo, id)?,
        (None, Some(t)) => cards::Card { title: t, ..Default::default() },
        _ => anyhow::bail!("give a card number or --text"),
    };
    let b = pit_core::brief::compile(repo, &pitlane_home(), &c, budget)?;
    if json {
        return print_json(&b);
    }
    if b.text.is_empty() {
        println!("nothing in the index matches this card ({} files indexed)", b.stats.indexed);
    } else {
        println!("{}", b.text);
        eprintln!(
            "{}",
            paint(MUTED, &format!("{} files, {} chars (~{} tokens), {}ms, index {} files", b.stats.files, b.stats.chars, b.stats.chars / 4, b.stats.ms, b.stats.indexed))
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn print_plan(p: &pit_core::plan::Plan) {
    let title = |id: u32| p.footprints.iter().find(|f| f.card == id).map(|f| f.title.clone()).unwrap_or_default();
    // The file name, or folder/name for an index or mod file, which says nothing alone.
    let short = |files: &[String]| {
        files
            .iter()
            .map(|f| {
                let parts: Vec<&str> = f.rsplit('/').take(2).collect();
                let name = parts[0];
                if parts.len() == 2 && (name.starts_with("index.") || name.starts_with("mod.") || name.starts_with("__init__.")) { format!("{}/{name}", parts[1]) } else { name.to_owned() }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if p.batches.is_empty() {
        println!("no open cards without a lane to plan");
    }
    for (i, b) in p.batches.iter().enumerate() {
        let late = b.iter().all(|id| p.waits.iter().any(|w| w.card == *id));
        let head = if late {
            "after the lanes they share files with land".to_owned()
        } else if i == 0 {
            "can start now, together".to_owned()
        } else {
            format!("then, batch {}", i + 1)
        };
        println!("{}", paint(MUTED, &head));
        for id in b {
            println!("  #{id} {}", title(*id));
        }
    }
    for c in &p.clashes {
        println!("  #{} and #{} would both touch {}", c.card, c.with_card.unwrap_or_default(), short(&c.files));
    }
    for w in &p.waits {
        println!("  #{} would touch {} that {} already changed", w.card, short(&w.files), w.with_lane.as_deref().unwrap_or("a lane"));
    }
    println!("{}", paint(MUTED, "predicted from the index: the files each card's map leads with"));
}
