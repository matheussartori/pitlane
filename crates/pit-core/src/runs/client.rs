//! A small blocking client for the daemon's socket, starting the daemon when none answers.

use super::*;

/// A blocking connection to the daemon. Starts it when nobody answers.
pub struct Client {
    stream: BufReader<UnixStream>,
}

impl Client {
    /// `pit_bin` is the `pit` executable to start `pit daemon` with, if it is not running.
    pub fn connect(pitlane_home: &Path, pit_bin: Option<&Path>) -> Result<Self> {
        let sock = socket_path(pitlane_home);
        if let Ok(s) = UnixStream::connect(&sock) {
            return Ok(Self { stream: BufReader::new(s) });
        }
        let Some(bin) = pit_bin else { bail!("the Pitlane daemon is not running (start it with: pit daemon)") };
        spawn_daemon(bin, pitlane_home)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(s) = UnixStream::connect(&sock) {
                return Ok(Self { stream: BufReader::new(s) });
            }
            if Instant::now() > deadline {
                bail!("started the daemon but it does not answer on {}; see {}", sock.display(), pitlane_home.join("pitd.log").display());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// How long a call may wait for the daemon's answer (none: as long as it takes). For a
    /// caller that must not hang on a daemon that stopped answering, such as the app quitting.
    pub fn with_timeout(self, limit: Option<Duration>) -> Result<Self> {
        self.stream.get_ref().set_read_timeout(limit)?;
        self.stream.get_ref().set_write_timeout(limit)?;
        Ok(self)
    }

    fn send(&mut self, req: &impl Serialize) -> Result<()> {
        let s = self.stream.get_mut();
        writeln!(s, "{}", serde_json::to_string(req)?)?;
        s.flush()?;
        Ok(())
    }

    fn read(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        if self.stream.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line).context("the daemon sent something that is not JSON")?))
    }

    /// One request, one answer. A daemon-side error becomes an `Err`.
    /// The connection itself, for another thread to hang it up (`shutdown`) while this one waits
    /// on an answer: the daemon then drops what it was waiting on (an approval, a question).
    pub fn hangup_handle(&self) -> std::io::Result<UnixStream> {
        self.stream.get_ref().try_clone()
    }

    pub fn request(&mut self, req: &Request) -> Result<Value> {
        self.exchange(req)
    }

    /// A request as raw JSON, for what no `Request` can say: an op this build does not know, or
    /// a message as an older client wrote it. Everything else goes through `request`.
    pub fn call(&mut self, req: Value) -> Result<Value> {
        self.exchange(&req)
    }

    fn exchange(&mut self, req: &impl Serialize) -> Result<Value> {
        self.send(req)?;
        let v = self.read()?.context("the daemon closed the connection")?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            bail!("{e}");
        }
        Ok(v)
    }

    /// `request`, with the answer read as a `T`.
    fn request_as<T: DeserializeOwned>(&mut self, req: &Request) -> Result<T> {
        let v = self.request(req)?;
        serde_json::from_value(v).with_context(|| format!("the daemon's answer to {} is not what it should be", req.op()))
    }

    pub fn ping(&mut self) -> Result<Pong> {
        self.request_as(&Request::Ping)
    }

    /// Starts a run on a card.
    pub fn start(&mut self, args: StartArgs) -> Result<RunMeta> {
        Ok(self.request_as::<RunReply>(&Request::Start(args))?.run)
    }

    /// Starts the run again where `run` stopped.
    pub fn resume(&mut self, run: &str) -> Result<RunMeta> {
        Ok(self.request_as::<RunReply>(&Request::Resume { run: run.into() })?.run)
    }

    /// A new run that carries on from `run`.
    pub fn continue_run(&mut self, run: &str, allow_api_key: bool) -> Result<RunMeta> {
        Ok(self.request_as::<RunReply>(&Request::Continue { run: run.into(), allow_api_key })?.run)
    }

    pub fn get(&mut self, run: &str) -> Result<RunMeta> {
        Ok(self.request_as::<RunReply>(&Request::Get { run: run.into() })?.run)
    }

    /// The runs of `repo`, or of every repository.
    pub fn list(&mut self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        Ok(self.request_as::<RunsReply>(&Request::List { repo: repo.map(Path::to_owned) })?.runs)
    }

    /// Every agent session of `repo`, or of every repository.
    pub fn sessions(&mut self, repo: Option<&Path>) -> Result<Vec<RunMeta>> {
        Ok(self.request_as::<SessionsReply>(&Request::Sessions { repo: repo.map(Path::to_owned) })?.sessions)
    }

    /// Run all's analysis of `cards`. The answer is the analysis step's own.
    pub fn analyze(&mut self, repo: &Path, cards: Vec<u32>) -> Result<Value> {
        self.request(&Request::Analyze { repo: repo.to_owned(), cards })
    }

    /// Reads a card again once its questions are answered (see `cards::wants_recheck`); whether a
    /// reading started.
    pub fn recheck(&mut self, repo: &Path, card: u32) -> Result<bool> {
        Ok(self.request_as::<crate::protocol::Changed>(&Request::Recheck { repo: repo.to_owned(), card })?.changed)
    }

    /// Carries out what you chose for a blocked card; the run it started, if any.
    pub fn unblock(&mut self, repo: &Path, card: u32, action: crate::cards::Unblock) -> Result<Option<RunMeta>> {
        let v = self.request(&Request::Unblock { repo: repo.to_owned(), card, action })?;
        Ok(serde_json::from_value(v.get("run").cloned().unwrap_or(Value::Null))?)
    }

    /// Moves a card by hand (see `cards::move_rule`); the card as it is now (a `CardOut`).
    pub fn move_card(&mut self, repo: &Path, card: u32, to: crate::cards::MoveTo, reason: Option<String>) -> Result<Value> {
        self.request(&Request::Move { repo: repo.to_owned(), card, to, reason })
    }

    /// Claude's short reading of a report, written on it; the report as it is now.
    pub fn report_digest(&mut self, repo: &Path, id: &str) -> Result<crate::report::Report> {
        self.request_as(&Request::ReportDigest { repo: repo.to_owned(), id: id.into() })
    }

    /// Asks for Claude's summary of a card, and its title when `title`. The daemon answers at
    /// once and writes them on the card when they are ready.
    pub fn describe(&mut self, repo: &Path, card: u32, title: bool) -> Result<()> {
        self.request_as::<Done>(&Request::Describe { repo: repo.to_owned(), card, title }).map(drop)
    }

    /// Asks a daemon that is already up for the summary of a card made or rewritten outside the
    /// app (`pit card add`, the MCP's `add_card`). Never starts a daemon for it, never waits long
    /// and never fails: with no daemon the card simply has no summary until the app describes it.
    pub fn describe_if_up(pitlane_home: &Path, repo: &Path, card: u32) {
        if let Ok(mut c) = Self::connect(pitlane_home, None).and_then(|c| c.with_timeout(Some(Duration::from_secs(2)))) {
            let _ = c.describe(repo, card, false);
        }
    }

    /// Talks a card's question over with Claude.
    pub fn talk(&mut self, repo: &Path, card: u32, question: u32, message: Option<String>) -> Result<Value> {
        self.request(&Request::Talk { repo: repo.to_owned(), card, question, message })
    }

    /// Passes a message on to a card's agent: the run that got it, or none when it became a
    /// note on the card.
    pub fn relay(&mut self, repo: &Path, card: u32, text: &str) -> Result<Option<String>> {
        Ok(self.request_as::<crate::protocol::Relayed>(&Request::Relay { repo: repo.to_owned(), card, text: text.to_owned() })?.run)
    }

    /// Stops the conversation on a card's questions; how many sessions stopped.
    pub fn stop_talk(&mut self, repo: &Path, card: u32) -> Result<usize> {
        Ok(self.request_as::<Stopped>(&Request::StopTalk { repo: repo.to_owned(), card })?.stopped)
    }

    /// Run all's closing step. The answer is that step's own (`run`, `ok`, `summary`, `cost_usd`).
    pub fn finish(&mut self, repo: &Path, cards: Vec<u32>, instructions: String, into: Option<String>) -> Result<Value> {
        self.request(&Request::Finish { repo: repo.to_owned(), cards, instructions, into })
    }

    /// The Board's assistant: turns `text` into cards, in the background.
    pub fn assist(&mut self, repo: &Path, text: String) -> Result<()> {
        self.request_as::<Done>(&Request::Assist { repo: repo.to_owned(), text }).map(drop)
    }

    pub fn say(&mut self, run: &str, text: String) -> Result<()> {
        self.request_as::<Done>(&Request::Say { run: run.into(), text }).map(drop)
    }

    /// Asks a card's run something on the side; returns the aside session's id.
    pub fn aside(&mut self, run: &str, text: String) -> Result<String> {
        self.request_as::<RunReply>(&Request::Aside { run: run.into(), text }).map(|r| r.run.id)
    }

    /// `/model` and `/effort` for a live session.
    pub fn tune(&mut self, run: &str, model: Option<String>, effort: Option<String>) -> Result<()> {
        self.request_as::<Done>(&Request::Tune { run: run.into(), model, effort }).map(drop)
    }

    pub fn stop(&mut self, run: &str) -> Result<()> {
        self.request_as::<Done>(&Request::Stop { run: run.into() }).map(drop)
    }

    /// Stops every session the daemon runs; how many.
    pub fn stop_all(&mut self) -> Result<usize> {
        Ok(self.request_as::<Stopped>(&Request::StopAll)?.stopped)
    }

    /// Stops Run all's analyses; how many.
    pub fn stop_analyses(&mut self) -> Result<usize> {
        Ok(self.request_as::<Stopped>(&Request::StopAnalyses)?.stopped)
    }

    /// Presence: its settings and the mode in force (`mode`).
    pub fn presence(&mut self) -> Result<Value> {
        self.request(&Request::Presence)
    }

    /// Changes Presence's settings; the answer is Presence as it is now.
    pub fn presence_set(&mut self, config: Value) -> Result<Value> {
        self.request(&Request::PresenceSet { config })
    }

    /// Asks the person a call for run `run` (its `ask` tool); what the agent should read back.
    pub fn consult(&mut self, run: &str, question: crate::cards::NewQuestion, costly: bool) -> Result<String> {
        let v = self.request(&Request::Consult { run: run.into(), question, costly })?;
        Ok(v.get("text").and_then(Value::as_str).unwrap_or_default().to_owned())
    }

    /// Performance as the daemon's governor sees it.
    pub fn perf(&mut self) -> Result<Value> {
        self.request(&Request::Perf)
    }

    /// Changes Performance's settings; the answer holds them as they are now (`config`).
    pub fn perf_set(&mut self, config: Value) -> Result<Value> {
        self.request(&Request::PerfSet { config })
    }

    /// Pauses (`hold`) or resumes one session by hand.
    pub fn perf_hold(&mut self, run: &str, hold: bool) -> Result<Changed> {
        self.request_as(&Request::PerfHold { run: run.into(), hold })
    }

    /// Asks the daemon to stop every session and end.
    pub fn shutdown(&mut self) -> Result<()> {
        self.request_as::<Done>(&Request::Shutdown).map(drop)
    }

    /// The last usage events seen, per account.
    pub fn quota(&mut self) -> Result<HashMap<String, Vec<RunEvent>>> {
        Ok(self.request_as::<QuotaReply>(&Request::Quota)?.quota)
    }

    /// Your decision on a card in review. The answer is the review's own.
    pub fn review(&mut self, repo: &Path, card: u32, action: ReviewAction, message: Option<String>) -> Result<Value> {
        self.request(&Request::Review { repo: repo.to_owned(), card, action, message })
    }

    /// Puts a branch in the merge queue.
    pub fn enqueue(&mut self, repo: &Path, branch: String, to: Option<String>) -> Result<ShipJob> {
        Ok(self.request_as::<JobReply>(&Request::Enqueue { repo: repo.to_owned(), branch, to })?.job)
    }

    /// The merge queue of `repo`, or of every repository.
    pub fn queue(&mut self, repo: Option<&Path>) -> Result<Vec<ShipJob>> {
        Ok(self.request_as::<JobsReply>(&Request::Queue { repo: repo.map(Path::to_owned) })?.jobs)
    }

    pub fn queue_resume(&mut self) -> Result<()> {
        self.request_as::<Done>(&Request::QueueResume).map(drop)
    }

    pub fn dequeue(&mut self, id: &str) -> Result<()> {
        self.request_as::<Done>(&Request::Dequeue { id: id.into() }).map(drop)
    }

    /// Asks for a tool a run wants to use; waits for a person when no rule allows it. The answer
    /// is the one Claude Code's permission prompt reads.
    pub fn ask(&mut self, run: &str, tool: String, input: Value, tool_use_id: Option<String>) -> Result<Value> {
        Ok(self.request_as::<AnswerReply>(&Request::Ask { run: run.into(), tool, input, tool_use_id })?.answer)
    }

    /// Answers what a run waits on (`approval`, or the oldest); the approval that was answered.
    pub fn decide(&mut self, run: &str, approval: Option<String>, decision: Decision, message: Option<String>) -> Result<String> {
        Ok(self.request_as::<Decided>(&Request::Decide { run: run.into(), approval, decision, message })?.approval)
    }

    /// Everything waiting on a person.
    pub fn approvals(&mut self) -> Result<Vec<Waiting>> {
        Ok(self.request_as::<ApprovalsReply>(&Request::Approvals)?.approvals)
    }

    /// Holds the daemon to this process: blocks while both live. When this connection closes,
    /// the daemon stops every session it started and ends. Returns when the daemon goes away.
    pub fn hold(mut self) -> Result<()> {
        self.request(&Request::Hold)?;
        while self.read().ok().flatten().is_some() {}
        Ok(())
    }

    /// Replays a run's events from `from` and then follows it live, until it ends or `each`
    /// returns false. Returns the run as it stands at the end.
    pub fn follow(&mut self, run: &str, from: usize, mut each: impl FnMut(usize, RunEvent) -> bool) -> Result<Option<RunMeta>> {
        self.send(&Request::Follow { run: run.into(), from })?;
        while let Some(v) = self.read()? {
            if let Some(e) = v.get("error").and_then(Value::as_str) {
                bail!("{e}");
            }
            if let Some(end) = v.get("end") {
                return Ok(serde_json::from_value(end.clone()).ok());
            }
            if let (Some(i), Some(ev)) = (v.get("i").and_then(Value::as_u64), v.get("event"))
                && let Ok(ev) = serde_json::from_value::<RunEvent>(ev.clone())
                && !each(i as usize, ev)
            {
                return Ok(None);
            }
        }
        Ok(None)
    }
}

pub(super) fn spawn_daemon(bin: &Path, pitlane_home: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(pitlane_home)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(pitlane_home.join("pitd.log"))?;
    std::process::Command::new(bin)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0) // its own group: a Ctrl-C in the terminal that started it does not reach it
        .spawn()
        .with_context(|| format!("could not start {} daemon", bin.display()))?;
    Ok(())
}
