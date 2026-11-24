//! What a run leaves on disk: its folder, `meta.json`, the timeline and its screenshots.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use pit_core::Repo;
use pit_core::cards::{self, CardPatch, CardState};
use pit_core::time::now_secs;
use pit_core::runs::{self, RunEvent, RunMeta, RunState, SessionKind};
use serde_json::Value;
use tokio::sync::broadcast;

use super::control::{signal, still_agent};
use super::{Live, Supervisor, home_dir};

/// Events a follower may fall behind by before it misses some.
const EVENTS_BUFFER: usize = 1024;
/// How long an agent a gone daemon left behind gets between SIGTERM and SIGKILL.
const LEFT_BEHIND_GRACE: Duration = Duration::from_millis(300);

impl Supervisor {
    pub(super) fn dir(&self, id: &str) -> PathBuf {
        runs::runs_dir(&self.cfg.home).join(id)
    }

    /// Keeps the images a line's tool results carry (screenshots) in the run's `shots/` folder
    /// and shows each one in the log.
    pub(super) fn keep_shots(&self, id: &str, v: &Value) {
        let shots = runs::tool_images(v);
        if shots.is_empty() {
            return;
        }
        let dir = self.dir(id).join("shots");
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let mut n = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        for s in shots {
            n += 1;
            let file = format!("{n:04}.{}", s.ext);
            if fs::write(dir.join(&file), &s.bytes).is_ok() {
                self.emit(id, RunEvent::Shot { tool: s.tool, file, parent: s.parent });
            }
        }
    }

    /// Runs left open by a daemon that went away are marked interrupted.
    pub fn recover(&self) -> Result<usize> {
        let mut n = 0;
        for mut m in self.list_disk()? {
            if m.state.is_live() {
                // An agent the last daemon left behind (it crashed or was killed) must not keep
                // editing the worktree with nobody watching: end its group, if it is still ours.
                if let Some(pid) = m.pid
                    && still_agent(pid, &m.session_id)
                {
                    signal(pid, libc::SIGTERM);
                    std::thread::sleep(LEFT_BEHIND_GRACE);
                    signal(pid, libc::SIGKILL);
                }
                if m.kind == SessionKind::Run {
                    // The agent had started (it was running, or had finished and its work was
                    // being checked): the next Run resumes its session. Interrupted before that,
                    // the session it was going to resume is still the one (as `settle_card` keeps it).
                    let ran = m.pid.is_some() || m.state == RunState::Verifying;
                    let paused = if ran { Some(m.id.clone()) } else { m.resumed_from.clone() };
                    let _ = Repo::open(&m.repo, &home_dir(), &self.cfg.home).and_then(|r| {
                        cards::update(
                            &r,
                            m.card,
                            CardPatch {
                                // A Run all card waits in the queue for the next Start.
                                state: Some(if m.autopilot { CardState::Queued } else { CardState::Ready }),
                                paused: Some(paused),
                                ..Default::default()
                            },
                        )
                    });
                }
                m.pid = None;
                m.state = RunState::Interrupted;
                m.ended_at.get_or_insert(now_secs());
                m.error
                    .get_or_insert_with(|| "the daemon stopped while this run was open".into());
                self.write_meta(&m)?;
                n += 1;
            }
        }
        Ok(n)
    }

    pub(super) fn list_disk(&self) -> Result<Vec<RunMeta>> {
        let dir = runs::runs_dir(&self.cfg.home);
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&dir) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            if let Ok(t) = fs::read_to_string(e.path().join("meta.json"))
                && let Ok(m) = serde_json::from_str::<RunMeta>(&t)
            {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// Records a new session (its folder and `meta.json`) and holds it as live, with nobody
    /// following it yet.
    pub(super) fn register(&self, meta: RunMeta) -> Result<()> {
        fs::create_dir_all(self.dir(&meta.id))?;
        self.write_meta(&meta)?;
        self.hold(meta, 0);
        Ok(())
    }

    /// Holds an ended session as live again, to pick its Claude Code session back up: its
    /// timeline goes on from where it stopped. False when it is live already (someone else
    /// picked it up first), and then nothing changes.
    pub(super) fn reopen_record(&self, meta: RunMeta) -> Result<bool> {
        let events = fs::read_to_string(self.dir(&meta.id).join("timeline.jsonl"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        if self.live().contains_key(&meta.id) {
            return Ok(false);
        }
        self.write_meta(&meta)?;
        Ok(self.hold(meta, events))
    }

    /// Takes a session in as live, unless it already is.
    fn hold(&self, meta: RunMeta, events: usize) -> bool {
        let timeline = self.open_timeline(&meta.id);
        let (tx, _) = broadcast::channel(EVENTS_BUFFER);
        let mut live = self.live();
        if live.contains_key(&meta.id) {
            return false;
        }
        live.insert(
            meta.id.clone(),
            Live {
                meta,
                stdin: Default::default(),
                turn: false,
                said: false,
                results: 0,
                background: 0,
                stop_requested: false,
                admitted: false,
                events,
                timeline,
                tx,
                rules: Vec::new(),
                limited: false,
                pending: HashMap::new(),
            },
        );
        true
    }

    fn write_meta(&self, m: &RunMeta) -> Result<()> {
        pit_core::fsutil::write_atomic(&self.dir(&m.id).join("meta.json"), serde_json::to_string_pretty(m)?)
    }

    pub(super) fn update(&self, id: &str, f: impl FnOnce(&mut RunMeta)) {
        let _order = self.meta_writes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = {
            let mut live = self.live();
            let Some(l) = live.get_mut(id) else { return };
            f(&mut l.meta);
            l.meta.clone()
        };
        let _ = self.write_meta(&snapshot);
    }

    /// The run's timeline, opened once to append to: every event is written under the live
    /// lock, so the file is not opened again for each.
    fn open_timeline(&self, id: &str) -> Option<fs::File> {
        OpenOptions::new().create(true).append(true).open(self.dir(id).join("timeline.jsonl")).ok()
    }

    /// Appends an event to the run's timeline and tells everyone following it.
    pub(super) fn emit(&self, id: &str, ev: RunEvent) {
        let mut live = self.live();
        let Some(l) = live.get_mut(id) else { return };
        let i = l.events;
        if l.timeline.is_none() {
            l.timeline = self.open_timeline(id);
        }
        if let Some(f) = l.timeline.as_mut()
            && let Ok(line) = serde_json::to_string(&ev)
        {
            let _ = writeln!(f, "{line}");
        }
        l.events += 1;
        let _ = l.tx.send((i, ev));
    }

    /// Like `emit`, and kept in its timeline when the session has ended meanwhile (an aside's
    /// answer to a run that finished while it answered).
    pub(super) fn record(&self, id: &str, ev: RunEvent) {
        if self.live().contains_key(id) {
            return self.emit(id, ev);
        }
        if let Ok(line) = serde_json::to_string(&ev)
            && let Ok(mut f) = fs::OpenOptions::new().append(true).open(self.dir(id).join("timeline.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn read_timeline(&self, id: &str, from: usize) -> Vec<(usize, RunEvent)> {
        fs::read_to_string(self.dir(id).join("timeline.jsonl"))
            .unwrap_or_default()
            .lines()
            .enumerate()
            .skip(from)
            .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|e| (i, e)))
            .collect()
    }

    /// Changes a run that is no longer live, on disk.
    pub(super) fn update_disk(&self, id: &str, f: impl FnOnce(&mut RunMeta)) {
        if let Ok(mut m) = self.get(id) {
            f(&mut m);
            let _ = self.write_meta(&m);
        }
    }
}
