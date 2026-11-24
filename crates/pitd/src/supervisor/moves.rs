//! A card moved by hand: the Board's drag and the card's "Move to" (and MCP) go through the same
//! rules (`cards::move_rule`). A running card moved out of In progress stops first; its session is
//! kept, so its next run resumes it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use pit_core::cards::{self, Card, MoveTo};
use pit_core::runs::SessionKind;

use super::Supervisor;

impl Supervisor {
    pub fn move_card(self: &Arc<Self>, repo: PathBuf, id: u32, to: MoveTo, reason: Option<String>) -> Result<Card> {
        let r = self.repo(&repo)?;
        let card = cards::get(&r, id)?;
        let run = self.live()
            .values()
            .find(|l| l.meta.kind == SessionKind::Run && l.meta.repo == r.root && l.meta.card == id)
            .map(|l| l.meta.id.clone());
        let moved = cards::move_rule(&card, to, run.is_some(), reason.as_deref())?;
        // The card first: when the run ends stopped, it finds the card already moved and leaves it there.
        let card = cards::update(&r, id, moved.patch)?;
        if moved.stop
            && let Some(run) = run
        {
            self.stop(&run)?;
        }
        Ok(card)
    }
}
