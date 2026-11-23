//! Run all's analysis, a card's description and a report's digest: read-only sessions that answer in JSON.

use super::*;

impl Supervisor {
    /// Run all's analysis: for each card not analysed yet, a read-only Claude Code session in the
    /// repository returns questions and notes, written on the card. Three at a time.
    pub async fn analyze(self: &Arc<Self>, repo: PathBuf, ids: Vec<u32>) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        // Never the API key: analysis is not worth a paid token without a yes.
        let choice = self.side_choice(&r.root)?;
        // The repository's model for agents, else yours (the judge's model is for judging).
        let model = r.flow.run.model.clone();
        let batch: Vec<cards::Card> = ids
            .into_iter()
            .filter_map(|id| cards::get(&r, id).ok())
            .collect();
        let todo: Vec<cards::Card> = batch
            .iter()
            .filter(|c| c.analyzed.is_none() && c.state != CardState::Done)
            .cloned()
            .collect();
        let gate = Arc::clone(&self.analysis_gate);
        let epoch = self.epoch.load(Ordering::SeqCst);
        // A card already being analysed (another Run all) is not analysed twice.
        let todo: Vec<cards::Card> = {
            let mut busy = self.analysing.lock().unwrap();
            todo.into_iter()
                .filter(|c| busy.insert((r.root.clone(), c.id)))
                .collect()
        };
        let mut tasks = Vec::new();
        for card in todo {
            let (gate, me, root, choice, model) = (
                gate.clone(),
                self.clone(),
                r.root.clone(),
                choice.clone(),
                model.clone(),
            );
            let prompt = pit_core::analyze::prompt(&r, &card, &batch);
            tasks.push(tokio::spawn(async move {
                let _busy = Release(&me.analysing, (root.clone(), card.id));
                let _slot = gate.acquire().await;
                if me.epoch.load(Ordering::SeqCst) != epoch {
                    return (card.id, Err(anyhow::anyhow!("stopped before it started")));
                }
                let out = me
                    .analyst(&root, &card, &prompt, pit_core::analyze::RULES, choice.as_ref(), model.as_deref())
                    .await;
                (card.id, out)
            }));
        }
        let mut results = Vec::new();
        // Sibling repositories the cards' code lives in: what the analysts named, and what their
        // notes mention, that exist next to this one and are not configured yet.
        let mut found: Vec<String> = Vec::new();
        for t in tasks {
            let Ok((id, out)) = t.await else { continue };
            results.push(match out.and_then(|(a, cost)| {
                let (q, n) = (a.questions.len(), a.notes.len());
                let text = format!("{}\n{}", a.repos.join(" "), a.notes.join("\n"));
                for rel in pit_core::related::mentioned(&r, &text) {
                    if !found.contains(&rel) {
                        found.push(rel);
                    }
                }
                pit_core::analyze::record(&r, id, a)?;
                Ok((q, n, cost))
            }) {
                Ok((q, n, cost)) => serde_json::json!({ "card": id, "questions": q, "notes": n, "cost_usd": cost }),
                Err(e) => serde_json::json!({ "card": id, "error": format!("{e:#}") }),
            });
        }
        // Added now, shown on the Run sheet, where the person can take any of them off.
        let repos_error = if found.is_empty() {
            None
        } else {
            pit_core::related::add(&r, &found)
                .err()
                .map(|e| format!("{e:#}"))
        };
        Ok(
            serde_json::json!({ "analyzed": results, "repos_added": found, "repos_error": repos_error }),
        )
    }

    /// Claude's title and summary for a card just made (`title`) or whose spec changed (summary
    /// only). Always Sonnet, no tools, about a cent; written on the card when it is back, unless
    /// the spec changed meanwhile.
    pub async fn describe(self: &Arc<Self>, repo: PathBuf, id: u32, title: bool) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let card = cards::get(&r, id)?;
        // Never the API key, like the analysis: a summary is not worth a paid token without a yes.
        let choice = self.side_choice(&r.root)?;
        let language = pit_core::language::agent_language(&self.cfg.home);
        let system = format!("{}{}", pit_core::describe::RULES, pit_core::language::rule(&language));
        let out = self
            .one_shot(
                &r.root,
                choice.as_ref(),
                &system,
                &pit_core::describe::schema(title).to_string(),
                format!(
                    "{}{}",
                    pit_core::describe::prompt(&card, title),
                    pit_core::language::reminder(&language)
                ),
                "the description",
            )
            .await?;
        let d = pit_core::describe::read(&String::from_utf8_lossy(&out.stdout))?;
        let card = cards::describe(
            &r,
            id,
            &card.body,
            d.title.as_deref().filter(|_| title),
            &d.summary,
        )?;
        Ok(serde_json::json!({ "card": card.id, "title": card.title, "summary": card.summary }))
    }

    /// Claude's digest of report `id` (Sonnet, no tools, about a cent): saved on the report.
    pub async fn report_digest(self: &Arc<Self>, repo: PathBuf, id: String) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let report = pit_core::report::get(&self.cfg.home, &id)?;
        // Never the API key, like the card summaries.
        let choice = self.side_choice(&r.root)?;
        let language = pit_core::language::agent_language(&self.cfg.home);
        let system = format!("{}{}", pit_core::report::DIGEST_RULES, pit_core::language::rule(&language));
        let out = self
            .one_shot(
                &r.root,
                choice.as_ref(),
                &system,
                &pit_core::report::digest_schema().to_string(),
                format!("{}{}", pit_core::report::digest_prompt(&report), pit_core::language::reminder(&language)),
                "the digest",
            )
            .await?;
        let d = pit_core::report::read_digest(&String::from_utf8_lossy(&out.stdout))?;
        let report = pit_core::report::set_digest(&self.cfg.home, &id, d)?;
        Ok(serde_json::to_value(report)?)
    }

    pub(super) async fn analyst(
        &self,
        repo: &Path,
        card: &cards::Card,
        prompt: &str,
        rules: &str,
        choice: Option<&Choice>,
        model: Option<&str>,
    ) -> Result<(pit_core::analyze::Analysis, Option<f64>)> {
        let mut args: Vec<String> = read_only_args(&mode_for(repo));
        args.extend([
            "--allowedTools".into(),
            READ_ONLY_TOOLS.into(),
            "--max-turns".into(),
            ANALYSIS_MAX_TURNS.into(),
        ]);
        // Read-only: it may look at sibling repositories, where a project's code often lives.
        if let Some(parent) = siblings_dir(repo) {
            args.extend(["--add-dir".into(), parent.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--append-system-prompt".into(),
            rules.into(),
            "--json-schema".into(),
            pit_core::analyze::schema().to_string(),
        ]);
        if let Some(m) = model {
            args.extend(["--model".into(), m.into()]);
        }
        let side = Side {
            kind: SessionKind::Analysis,
            repo: repo.to_owned(),
            cwd: repo.to_owned(),
            card: card.id,
            card_title: card.title.clone(),
            parent: None,
            account: choice
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.and_then(|c| c.config_dir.clone()),
            model: model.map(str::to_owned),
            args,
            prompt: prompt.to_owned(),
            limit: ANALYSIS_LIMIT,
            board: false,
            images: Repo::open(repo, &home_dir(), &self.cfg.home)
                .map(|r| cards::image_bytes(&r, card))
                .unwrap_or_default(),
        };
        let stdout = self
            .side_session(side)
            .await
            .map_err(|e| anyhow::anyhow!("the analysis {e:#}"))?;
        pit_core::analyze::read(&stdout)
    }
}
