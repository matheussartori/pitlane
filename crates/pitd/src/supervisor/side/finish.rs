//! Run all's last step: what the person asked for once the whole batch is done.

use super::*;

impl Supervisor {
    /// Run all's closing step: one Claude Code session in the repository (or the target branch's
    /// worktree) that does what the person asked for when everything is done. It edits files and
    /// asks through Pitlane for anything no rule allows. Waits for it; returns its summary.
    pub async fn finish(
        self: &Arc<Self>,
        repo: PathBuf,
        ids: Vec<u32>,
        instructions: String,
        into: Option<String>,
    ) -> Result<Value> {
        let r = Repo::open(&repo, &home_dir(), &self.cfg.home)?;
        let cwd = match into.as_deref() {
            Some(t) if !r.flow.flow.is_trunk(t) => {
                actions::ensure_worktree(&r, t).unwrap_or_else(|_| r.root.clone())
            }
            _ => r.root.clone(),
        };
        let choice = self.side_choice(&r.root)?;
        let mut tools: Vec<String> = RUN_BASE_TOOLS.iter().map(|s| s.to_string()).collect();
        tools.extend(r.flow.run.allow.iter().cloned());
        let args: Vec<String> = vec![
            "--setting-sources".into(),
            "project,local".into(),
            "--permission-mode".into(),
            mode_for(&r.root),
            "--allowedTools".into(),
            tools.join(","),
            "--disallowedTools".into(),
            "AskUserQuestion".into(),
            "--max-turns".into(),
            FINISH_MAX_TURNS.into(),
        ];
        let side = Side {
            kind: SessionKind::Finish,
            repo: r.root.clone(),
            cwd,
            card: 0,
            card_title: "When it is all done".into(),
            parent: None,
            account: choice
                .as_ref()
                .map(|c| c.account.clone())
                .unwrap_or_else(|| accounts::DEFAULT_ACCOUNT.into()),
            config_dir: choice.as_ref().and_then(|c| c.config_dir.clone()),
            model: None,
            args,
            prompt: pit_core::report::finish_prompt(&r, &ids, &instructions, into.as_deref()),
            limit: FINISH_LIMIT,
            board: false,
            images: Vec::new(),
            resume: false,
            fork: None,
        };
        let id_hint = now_secs();
        let out = self.side_session(side).await;
        // The session's id: the newest finish session of this repository.
        let run = self
            .sessions(Some(&r.root))?
            .into_iter()
            .find(|m| m.kind == SessionKind::Finish && m.started_at >= id_hint)
            .map(|m| m.id);
        Ok(match out {
            Ok(line) => {
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                serde_json::json!({
                    "run": run,
                    "ok": !v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    "summary": v.get("result").and_then(Value::as_str),
                    "cost_usd": v.get("total_cost_usd").and_then(Value::as_f64),
                })
            }
            Err(e) => serde_json::json!({ "run": run, "ok": false, "summary": format!("{e:#}") }),
        })
    }
}
