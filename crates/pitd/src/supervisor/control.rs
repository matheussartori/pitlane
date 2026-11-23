//! Talking to and stopping sessions, and the Performance governor's waits.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pit_core::runs::{self, RunEvent, RunMeta, SessionKind};

use super::launch::EFFORTS;
use super::Supervisor;

/// Stop all's escalation: SIGTERM this many seconds after SIGINT, SIGKILL as many after that.
const ESCALATE_AFTER_SECS: u64 = 4;
/// Shutdown waits for the sessions to go in this many polls of `SHUTDOWN_POLL` (9 s in all).
const SHUTDOWN_POLLS: u32 = 90;
const SHUTDOWN_POLL: Duration = Duration::from_millis(100);
/// How long a stopped agent has to end its turn before its group gets SIGTERM.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long a group gets between SIGTERM and SIGKILL in `end_group`.
const GROUP_KILL_AFTER: Duration = Duration::from_secs(2);

impl Supervisor {
    /// The Performance governor: samples the Mac every 2 s and pauses, resumes and throttles
    /// the agents to fit the mode (see `perf`).
    pub fn start_perf(self: &Arc<Self>) {
        self.perf.enable();
        let me = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let sessions: Vec<crate::perf::LiveSession> = me.live()
                    .values()
                    .map(|l| crate::perf::LiveSession {
                        meta: l.meta.clone(),
                    })
                    .collect();
                let m = Arc::clone(&me);
                let notes = tokio::task::spawn_blocking(move || m.perf.tick(&sessions))
                    .await
                    .unwrap_or_default();
                for (id, text) in notes {
                    me.emit(&id, RunEvent::Note { text });
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// Pauses (`hold`) or resumes a session by hand, from the Performance screen.
    pub fn perf_hold(&self, id: &str, hold: bool) -> Result<bool> {
        let (pid, card, title) = {
            let live = self.live();
            let m = &live
                .get(id)
                .with_context(|| format!("run {id} is not running"))?
                .meta;
            (m.pid, m.card, m.card_title.clone())
        };
        if hold {
            let pid = pid.context("its agent has not started yet")?;
            let done = self.perf.hold(id, pid);
            if done {
                self.perf.log_manual("paused_you", card, &title);
                self.emit(id, RunEvent::Note { text: "Paused by you from Performance. Nothing is lost; Resume continues where it was.".into() });
            }
            Ok(done)
        } else {
            let done = self.perf.release(id, pid);
            if done {
                self.perf.log_manual("resumed_you", card, &title);
                self.emit(
                    id,
                    RunEvent::Note {
                        text: "Resumed by you from Performance.".into(),
                    },
                );
            }
            Ok(done)
        }
    }

    /// A card's run waits here, before its agent starts, while the mode or the memory has no
    /// room for one more. Stop ends the wait.
    pub(super) async fn wait_room(&self, id: &str) {
        if !self.perf.enabled() {
            return;
        }
        let mut said = false;
        loop {
            if self.stop_requested(id) {
                break;
            }
            // Read before taking the list: Performance's own lock is never taken under it.
            let held = self.perf.held_ids();
            let allowed = self.perf.allowed();
            {
                // Counted and claimed under one lock: runs waiting together never all go in.
                let mut live = self.live();
                let working = live
                    .values()
                    .filter(|l| {
                        l.meta.id != id
                            && l.meta.kind == SessionKind::Run
                            && (l.meta.pid.is_some() || l.admitted)
                            && !held.contains(&l.meta.id)
                    })
                    .count() as u32;
                if working < allowed {
                    if let Some(l) = live.get_mut(id) {
                        l.admitted = true;
                    }
                    break;
                }
            }
            if !said {
                said = true;
                self.perf.set_waiting(id, true);
                self.emit(id, RunEvent::Note { text: format!("Waiting for room: Performance lets {allowed} card(s) work at once right now. It starts as soon as one ends or there is room.") });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        self.perf.set_waiting(id, false);
    }

    /// A message to the agent while it works: it reads it after its current step.
    pub async fn say(&self, id: &str, text: &str) -> Result<()> {
        let (stdin, mid_turn) = {
            let mut live = self.live();
            let l = live
                .get_mut(id)
                .with_context(|| format!("run {id} is not running; resume it to continue"))?;
            if !l.stdin.is_open() {
                bail!("the agent already finished this session; resume it to continue");
            }
            // Mid-turn, Claude Code may fold it into the turn or answer it right after.
            let mid_turn = l.turn;
            l.said = l.turn;
            l.turn = true;
            (l.stdin.clone(), mid_turn)
        };
        let line = if mid_turn { mid_turn_message(text) } else { text.to_owned() };
        if !stdin.write(&format!("{}\n", runs::claude_user_line(&line))).await? {
            bail!("the agent already finished this session; resume it to continue");
        }
        self.emit(
            id,
            RunEvent::Said {
                text: text.to_owned(),
            },
        );
        Ok(())
    }

    /// `/model` and `/effort` while the session lives: Claude Code's own control requests on
    /// stdin (`set_model`, `apply_flag_settings`), as its SDK sends them. Not a message: the
    /// turn count that closes stdin does not move.
    pub async fn tune(&self, id: &str, model: Option<&str>, effort: Option<&str>) -> Result<()> {
        if let Some(e) = effort
            && !EFFORTS.contains(&e)
        {
            bail!("effort is one of {}", EFFORTS.join(", "));
        }
        if let Some(m) = model
            && (m.is_empty() || m.chars().any(|c| !(c.is_ascii_alphanumeric() || "-._[]".contains(c))))
        {
            bail!("{m:?} is not a model name");
        }
        let mut lines = String::new();
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        if let Some(m) = model {
            let req = serde_json::json!({ "type": "control_request", "request_id": format!("pit-model-{id}-{n}"),
                "request": { "subtype": "set_model", "model": m } });
            lines.push_str(&format!("{req}\n"));
        }
        if let Some(e) = effort {
            let req = serde_json::json!({ "type": "control_request", "request_id": format!("pit-effort-{id}-{n}"),
                "request": { "subtype": "apply_flag_settings", "settings": { "effortLevel": e } } });
            lines.push_str(&format!("{req}\n"));
        }
        if lines.is_empty() {
            return Ok(());
        }
        let stdin = {
            let live = self.live();
            let l = live.get(id).with_context(|| format!("run {id} is not running"))?;
            l.stdin.clone()
        };
        if !stdin.write(&lines).await? {
            bail!("the agent already finished this session");
        }
        // The model shows once Claude Code answers with the one it resolved (`opus` is
        // claude-opus-5-5); the effort has nothing to resolve, so it shows now.
        if let Some(e) = effort.map(str::to_owned) {
            self.emit(id, RunEvent::Tuned { model: None, effort: Some(e.clone()) });
            self.update(id, move |meta| meta.effort = Some(e));
        }
        Ok(())
    }

    pub(super) fn stop_requested(&self, id: &str) -> bool {
        self.live()
            .get(id)
            .is_some_and(|l| l.stop_requested)
    }

    /// Stops every session (runs, analyses, judges) and every analysis still waiting for a slot.
    /// SIGINT first, SIGTERM after 4 s, SIGKILL after 8 s, always to the whole process group.
    pub fn stop_all(self: &Arc<Self>) -> usize {
        self.stop_where(|_| true)
    }

    /// Stops the sessions that match (and the analyses still waiting for a slot).
    pub fn stop_where(self: &Arc<Self>, which: impl Fn(&RunMeta) -> bool) -> usize {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.stop_matching(which)
    }

    /// The process groups of the sessions on now: each agent leads its own group.
    pub fn session_groups(&self) -> std::collections::HashSet<i32> {
        let mut groups: std::collections::HashSet<i32> = self.live().values().filter_map(|l| l.meta.pid).map(|p| p as i32).collect();
        groups.extend(self.one_shots.lock().unwrap().iter().map(|p| *p as i32));
        groups
    }

    /// Stops the live sessions that match; what waits for a slot keeps waiting.
    pub(super) fn stop_matching(self: &Arc<Self>, which: impl Fn(&RunMeta) -> bool) -> usize {
        let pids: Vec<(String, u32)> = {
            let mut live = self.live();
            live.iter_mut()
                .filter(|(_, l)| which(&l.meta))
                .filter_map(|(id, l)| {
                    l.stop_requested = true;
                    l.meta.pid.map(|p| (id.clone(), p))
                })
                .collect()
        };
        let n = pids.len();
        for (id, pid) in &pids {
            // A paused session hears nothing until it runs again.
            self.perf.release(id, Some(*pid));
            signal(*pid, libc::SIGINT);
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            for (after, sig) in [(ESCALATE_AFTER_SECS, libc::SIGTERM), (ESCALATE_AFTER_SECS, libc::SIGKILL)] {
                tokio::time::sleep(Duration::from_secs(after)).await;
                // Only a session still on the same process: a run that moved on to its checks has
                // no agent any more, and its old group id may belong to someone else by now.
                let still: Vec<(String, u32)> = {
                    let live = me.live();
                    pids.iter().filter(|(id, pid)| live.get(id).is_some_and(|l| l.meta.pid == Some(*pid))).cloned().collect()
                };
                for (_, pid) in &still {
                    signal(*pid, sig);
                }
            }
        });
        n
    }

    /// Stop all, wait for the sessions to go (at most 9 s), then end the daemon. Nothing it
    /// started outlives it.
    pub async fn shutdown(self: &Arc<Self>) {
        // Work left in the merge queue waits for a person to resume it.
        if self.queue.lock().unwrap().iter().any(|j| j.state.is_open()) {
            let _ = pit_core::queue::set_paused(&self.cfg.home, true);
        }
        self.stop_all();
        for _ in 0..SHUTDOWN_POLLS {
            if self.live().is_empty() {
                break;
            }
            tokio::time::sleep(SHUTDOWN_POLL).await;
        }
        for l in self.live().values() {
            if let Some(pid) = l.meta.pid {
                signal(pid, libc::SIGKILL);
            }
        }
        // Exiting skips the drops that would end the one-shot calls still in flight.
        for pid in self.one_shots.lock().unwrap().drain() {
            signal(pid, libc::SIGKILL);
        }
        self.perf.release_all();
        // The app's code-server, when the app went without closing it (a crash, a force quit).
        let home = self.cfg.home.clone();
        let _ = tokio::task::spawn_blocking(move || pit_core::procs::end_code_server(&home)).await;
        let _ = fs::remove_file(runs::socket_path(&self.cfg.home));
        std::process::exit(0)
    }

    /// SIGINT ends the current turn cleanly; if the agent is still there after 10 s, SIGTERM.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let pid = {
            let mut live = self.live();
            let l = live
                .get_mut(id)
                .with_context(|| format!("run {id} is not running"))?;
            l.stop_requested = true;
            l.meta.pid
        };
        // Sessions it opened (its judge) stop with it.
        let children: Vec<String> = self.live()
            .values()
            .filter(|l| l.meta.parent.as_deref() == Some(id))
            .map(|l| l.meta.id.clone())
            .collect();
        for c in children {
            let _ = self.stop(&c);
        }
        // Still preparing its worktree, or being checked: `drive` and `verify` see the request.
        let Some(pid) = pid else { return Ok(()) };
        self.perf.release(id, Some(pid));
        signal(pid, libc::SIGINT);
        let me = Arc::clone(self);
        let run = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(STOP_GRACE).await;
            let same = me.live()
                .get(&run)
                .is_some_and(|l| l.meta.pid == Some(pid));
            if same {
                end_group(pid);
            }
        });
        Ok(())
    }
}

/// Removes its key from the set when dropped.
pub(super) struct Release<'a>(
    pub(super) &'a Mutex<std::collections::HashSet<(PathBuf, u32)>>,
    pub(super) (PathBuf, u32),
);
impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

/// Is that pid still a Claude Code process (and not a stranger who got the number)?
/// Whether `pid` is still the agent of session `session`: its command line names that session
/// (`--session-id` or `--resume`). After a reboot the number may belong to anything, your own
/// Claude Code included, and its whole group would be ended.
pub(super) fn still_agent(pid: u32, session: &str) -> bool {
    !session.is_empty()
        && std::process::Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(session))
            .unwrap_or(false)
}

/// SIGTERM to the whole group, SIGKILL a moment later, off the async threads.
pub(super) fn end_group(pid: u32) {
    signal(pid, libc::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(GROUP_KILL_AFTER).await;
        signal(pid, libc::SIGKILL);
    });
}

/// Signals the agent's whole process group, like Ctrl-C in a terminal: the agent and the
/// command it is running both hear it.
pub(super) fn signal(pid: u32, sig: i32) {
    // SAFETY: kill(2) on the group of a process we spawned with its own group; a stale id only
    // yields ESRCH.
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

/// What the agent gets for a message sent while its turn runs. Claude Code folds it into the
/// turn beside the next tool result, and the agent tended to answer it in its thinking, which
/// the person never sees, and go on as if nothing was asked. So it is told where to answer.
fn mid_turn_message(text: &str) -> String {
    format!(
        "{text}\n\n(Sent from Pitlane while you work. The person reads only your text replies, \
         never your thinking: answer this in a short text reply now, before your next tool \
         call, then go on with the work.)"
    )
}

#[cfg(test)]
mod tests {
    use super::mid_turn_message;

    #[test]
    fn a_mid_turn_message_asks_for_a_visible_answer() {
        let m = mid_turn_message("e o resumo?");
        assert!(m.starts_with("e o resumo?\n\n"));
        assert!(m.contains("text reply now"));
    }
}
