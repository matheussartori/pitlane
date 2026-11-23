//! Cards: one markdown file each in the repository's board folder, `~/.pitlane/boards/<repo>/cards/`,
//! a TOML frontmatter between `+++` lines and the spec as the body. Answers to a card's questions
//! become decisions in `decisions.md` beside them. The board lives outside the repository so it is
//! one source of truth across branches and worktrees, and no checkout, stash or clean touches it.
//! CLI, MCP server and app all write here, so every write is atomic and anything that allocates an
//! id holds the board's lock.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::time::{is_zero, local_day, now_secs, timestamp};

pub const CARDS_DIR: &str = "cards";
pub const DECISIONS: &str = "decisions.md";
pub const CONTEXT: &str = "context.md";
/// The board's lock file. Not `.lock`: older builds took that one by creating and deleting
/// it, and the two schemes must not mistake each other's file for their own.
const LOCK: &str = ".board.lock";
/// How long a board write waits for another one to finish.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// Where cards lived before the board moved out of the repository; adopted on open.
const LEGACY_CARDS: &str = ".pitlane/cards";
const LEGACY_DECISIONS: &str = ".pitlane/decisions.md";
const LEGACY_LOCK: &str = ".pitlane/.lock";

mod model;
mod place;
mod images;
mod store;
mod decisions;

pub use model::*;
pub use place::*;
pub use images::*;
pub use store::*;
pub use decisions::*;

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
            summary: None,
            paused: None,
            blocked: None,
            retries: 0,
            held: None,
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
                run: None,
                asked_at: None,
                auto: false,
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
