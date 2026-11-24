//! The Board's assistant: what the person types or dictates, turned into cards.

use super::*;

impl Supervisor {
    /// The Board's assistant: what the person typed or said ("make cards from this list") goes to
    /// a Claude Code session in the repository that reads the code and writes cards with
    /// Pitlane's board tools. It runs in the background, in view in the Agents dock; people can
    /// keep talking to it while it works.
    /// One per repository: what you ask next goes to the repository's assistant (it keeps what
    /// was said before), instead of a new Claude Code each time. Once it has answered it ends
    /// like any session, and the next message picks its session back up. Stopped by the
    /// person, or failed, the next one starts afresh.
    pub async fn assist(self: &Arc<Self>, repo: PathBuf, text: String) -> Result<()> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let last = self
            .sessions(Some(&r.root))?
            .into_iter()
            .find(|m| m.kind == SessionKind::Assist);
        if let Some(m) = last {
            let stopping = self.live().get(&m.id).map(|l| l.stop_requested);
            let goes_on = match stopping {
                Some(stopped) => !stopped,
                None => m.state == RunState::Done,
            };
            if goes_on && self.resume_assist(&m.id, &text).await.is_ok() {
                return Ok(());
            }
        }
        if self.cfg.pit_bin.is_none() {
            bail!(NEEDS_PIT);
        }
        let choice = self.side_choice(&r.root)?;
        let account = choice
            .as_ref()
            .map(|c| c.account.clone())
            .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into());
        let side = assist_side(&r, account, choice.and_then(|c| c.config_dir), text, false);
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let _ = me.side_session(side).await;
        });
        Ok(())
    }

    /// What the person writes to a session: said to it while it is open; a Board assistant that
    /// has ended picks its session back up to answer it.
    pub async fn message(self: &Arc<Self>, id: &str, text: &str) -> Result<()> {
        match self.say(id, text).await {
            Err(_) if self.get(id).is_ok_and(|m| m.kind == SessionKind::Assist) => self.resume_assist(id, text).await,
            said => said,
        }
    }

    /// A message for a Board assistant. Open, it is said to it. Ended, its Claude Code session
    /// is picked back up (`--resume`) in the same run, so its row and log go on where they
    /// stopped and it remembers what was said. Ending right now (it answered a moment ago), it
    /// is waited for first.
    async fn resume_assist(self: &Arc<Self>, id: &str, text: &str) -> Result<()> {
        let until = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            let open = self.live().get(id).map(|l| (l.stop_requested, l.stdin.is_open()));
            match open {
                None => break,
                Some((true, _)) => bail!("the assistant is stopping; write again once it has stopped"),
                Some((false, true)) => return self.say(id, text).await,
                Some((false, false)) if tokio::time::Instant::now() >= until => {
                    bail!("the assistant is still ending; write again in a moment")
                }
                Some(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        if self.cfg.pit_bin.is_none() {
            bail!(NEEDS_PIT);
        }
        let m = self.get(id)?;
        if m.kind != SessionKind::Assist {
            bail!("the agent already finished this session; resume it to continue");
        }
        let r = Repo::open(&m.repo, &home_dir(), &self.cfg.home)?;
        // Its account: Claude Code saved the session in that account's directory.
        let config_dir = accounts::profile_dir(&m.account, &self.cfg.home);
        let side = assist_side(&r, m.account.clone(), config_dir, text.to_owned(), true);
        let meta = RunMeta {
            state: RunState::Running,
            ended_at: None,
            error: None,
            pid: None,
            waiting: Vec::new(),
            asking: Vec::new(),
            ..m
        };
        if !self.reopen_record(meta)? {
            // Someone picked it up a moment before: it is open again.
            return self.say(id, text).await;
        }
        self.emit(id, RunEvent::Said { text: text.to_owned() });
        let me = Arc::clone(self);
        let id = id.to_owned();
        tokio::spawn(async move {
            let _ = me.drive_side(id, side).await;
        });
        Ok(())
    }

    /// Passes a message on to a card's agent (`relay`): the card's live run reads it after its
    /// current step, labeled as passed on, in its log like anything you say to it. With no run
    /// working the card it becomes a note on the card, which the next run reads. `assistant` is
    /// the repository of the Board assistant that sent it, when a session did; an assistant only
    /// reaches its own repository's cards.
    pub async fn relay(&self, repo: PathBuf, card: u32, text: &str, assistant: Option<&Path>) -> Result<Option<String>> {
        let text = text.trim();
        if text.is_empty() {
            bail!("say what to pass on");
        }
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        if let Some(root) = assistant
            && root != r.root
        {
            bail!("the Board assistant passes messages on only to its own repository's cards");
        }
        cards::get(&r, card)?;
        let run = self.live()
            .values()
            .filter(|l| l.meta.kind == SessionKind::Run && l.meta.repo == r.root && l.meta.card == card && l.stdin.is_open() && !l.stop_requested)
            .max_by_key(|l| l.meta.started_at)
            .map(|l| l.meta.id.clone());
        let from = if assistant.is_some() { "the Board assistant" } else { "Pitlane's board tools" };
        if let Some(id) = run
            && self.say(&id, &format!("Passed on by {from}, from the person: {text}")).await.is_ok()
        {
            return Ok(Some(id));
        }
        cards::note(&r, card, if assistant.is_some() { "board assistant" } else { "claude" }, text)?;
        Ok(None)
    }

    /// The repository of the Board assistant `peer` belongs to, if it is one's (its Claude Code,
    /// its board tools, or anything they started).
    pub fn assistant_of(&self, peer: i32) -> Option<PathBuf> {
        let assistants: std::collections::HashMap<i32, PathBuf> = self.live()
            .values()
            .filter(|l| l.meta.kind == SessionKind::Assist)
            .filter_map(|l| Some((l.meta.pid? as i32, l.meta.repo.clone())))
            .collect();
        let pids = assistants.keys().copied().collect();
        crate::peer::session_of(peer, &pids).and_then(|pid| assistants.get(&pid).cloned())
    }
}

const NEEDS_PIT: &str = "the board assistant needs the pit executable for its board tools";

/// The Board assistant's Claude Code: reads the code, writes cards with the board tools, never
/// edits. `resume` picks its session back up instead of starting it.
fn assist_side(r: &Repo, account: String, config_dir: Option<PathBuf>, text: String, resume: bool) -> Side {
    let tools = format!("{READ_ONLY_TOOLS},mcp__pitlane__add_card,mcp__pitlane__list_cards,mcp__pitlane__get_card,mcp__pitlane__update_card,mcp__pitlane__delete_card,mcp__pitlane__add_note,mcp__pitlane__brief,mcp__pitlane__message_run");
    let args: Vec<String> = vec![
        "--setting-sources".into(),
        "project,local".into(),
        "--permission-mode".into(),
        mode_for(&r.root),
        // Claude Code's own messages to other sessions never reach a run (a `-p` session
        // cannot approve them) and Pitlane would not see them: it passes messages on with
        // `message_run`.
        "--disallowedTools".into(),
        "Edit,Write,NotebookEdit,AskUserQuestion,SendMessage,ListAgents".into(),
        "--allowedTools".into(),
        tools,
        "--append-system-prompt".into(),
        ASSIST_RULES.into(),
    ];
    Side {
        kind: SessionKind::Assist,
        repo: r.root.clone(),
        cwd: r.root.clone(),
        card: 0,
        card_title: "Board assistant".into(),
        parent: None,
        account,
        config_dir,
        model: None,
        args,
        prompt: text,
        // No time limit: it ends once it has answered, or when the person stops it.
        limit: Duration::MAX,
        board: true,
        images: Vec::new(),
        resume,
    }
}

const ASSIST_RULES: &str = "You are Pitlane's board assistant. The person tells you, typed or dictated (so expect \
spoken phrasing and transcription slips), what work they want on this repository's board. You turn it into cards with \
the pitlane tools: one card per piece of work someone could finish on its own, a short imperative title, and a spec in \
markdown that says what and why, with acceptance in a line or two. Read the code when it helps you write a precise spec \
(paths, names), but never change files and never do the work itself. Before adding, list the cards to avoid duplicates; \
update an existing card instead when it is the same work. When they ask to change, annotate or delete cards, do that. When \
they ask you to tell a card's agent something (run a test first, check a page, change course), pass it on with message_run: \
the card's working run reads it after its current step, or it becomes a note on the card when none is working; say which \
happened. Never use SendMessage for this. End with a one-line summary per card you added or changed (#id and title).";
