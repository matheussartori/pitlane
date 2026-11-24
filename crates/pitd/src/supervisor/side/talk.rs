//! Talking a card's question through with an agent before answering it.

use super::*;

impl Supervisor {
    /// Talks one question over: what the person said goes on the question, a read-only Claude
    /// Code session answers it with the options as they stand now, and that answer goes on the
    /// question too (its options become the question's). Without a message it answers the last
    /// one again (after a failure). Waits for the answer; returns the card.
    pub async fn talk(self: &Arc<Self>, repo: PathBuf, id: u32, question: u32, message: Option<String>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = match message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => cards::say(&r, id, question, "you", m, None)?,
            None => cards::get(&r, id)?,
        };
        let last = card.questions.iter().find(|q| q.id == question).and_then(|q| q.talk.last());
        if last.is_none_or(|t| t.by != "you") {
            anyhow::bail!("there is nothing to answer on question {question} of card #{id}");
        }
        let prompt = pit_core::analyze::talk_prompt(&r, &card, question)?;
        // Like the analysis: never the API key without a yes.
        let choice = self.side_choice(&r.root)?;
        let model = r.flow.run.model.clone();
        let mut args: Vec<String> = read_only_args(&mode_for(&r.root));
        args.extend([
            "--allowedTools".into(),
            READ_ONLY_TOOLS.into(),
            "--max-turns".into(),
            TALK_MAX_TURNS.into(),
        ]);
        if let Some(parent) = siblings_dir(&r.root) {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            pit_core::analyze::TALK_RULES.into(),
            "--json-schema".into(),
            pit_core::analyze::talk_schema().to_string(),
        ]);
        if let Some(m) = &model {
            args.extend(["--model".into(), m.clone()]);
        }
        let side = Side {
            // An analysis to everything that counts sessions: it reads, it is no run.
            kind: SessionKind::Analysis,
            repo: r.root.clone(),
            cwd: r.root.clone(),
            card: id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir),
            model,
            args,
            prompt,
            limit: TALK_LIMIT,
            board: false,
            images: cards::image_bytes(&r, &card),
            resume: false,
            fork: None,
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the answer {e:#}"))?;
        let (reply, cost) = pit_core::analyze::read_talk(&stdout)?;
        let offer = cards::NewQuestion {
            text: String::new(),
            options: reply.options,
            recommended: reply.recommended,
            context: reply.context,
        };
        let card = cards::say(&r, id, question, "claude", &reply.reply, Some(offer))?;
        Ok(serde_json::json!({ "card": cards::CardOut::from(card), "cost_usd": cost }))
    }

    /// Stops the conversation going on a card's questions (it is an analysis session of that
    /// card), and nothing else: Run all's analyses waiting for a slot keep their place.
    pub fn stop_talk(self: &Arc<Self>, repo: &Path, id: u32) -> usize {
        self.stop_matching(|m| m.kind == SessionKind::Analysis && m.card == id && m.repo == repo)
    }
}
