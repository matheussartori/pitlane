//! Performance: how much of this Mac the agents may take, and keeping to it.
//!
//! Two modes. **Full**: you are away, the agents take the machine (up to "Agents at once", the
//! Mac kept awake). **Light**: you are using it, so fewer agents work at once and they run at
//! background priority (efficiency cores, throttled disk), leaving the Mac responsive.
//! "Switch by itself" picks Light while you use the Mac and Full once you have been away a while.
//!
//! Every 2 s the governor samples the Mac (memory pressure, free memory, swap, CPU, macOS's
//! thermal pressure) and each agent's process tree (memory as Activity Monitor counts it,
//! `phys_footprint`). When more cards work than the mode, the memory or the heat allows, it
//! pauses the newest (for memory the largest, for heat the busiest) with SIGSTOP on its whole
//! tree and resumes it with SIGCONT when there is room. CPU alone never pauses a card: a busy
//! Mac is the point of Full, and Light already keeps agents off the performance cores.
//! A pause loses nothing: the process, its conversation, its context and the command it was
//! running are frozen in place, and Claude Code's stream watchdog retries a request that went
//! quiet meanwhile (`CLAUDE_STREAM_IDLE_TIMEOUT_MS`, raised for runs). Stop on a paused run
//! resumes it first so it can end cleanly.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use pit_core::runs::{RunMeta, SessionKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How long a paused request may stay quiet before Claude Code retries it (15 min).
pub const STREAM_IDLE_MS: &str = "900000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Full,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfConfig {
    pub mode: Mode,
    /// Light while you use the Mac, Full after `away_minutes` without a key or the mouse.
    pub auto: bool,
    pub away_minutes: u32,
    /// Cards working at once in Light (Full uses "Agents at once" from Settings).
    pub light_agents: u32,
    /// In Light, agents run at background priority (efficiency cores, throttled disk).
    pub light_background: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Full,
            auto: false,
            away_minutes: 10,
            light_agents: 1,
            light_background: true,
        }
    }
}

fn config_path(home: &Path) -> PathBuf {
    home.join("performance.json")
}

pub fn load_config(home: &Path) -> PerfConfig {
    std::fs::read_to_string(config_path(home))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// "Agents at once" and "fewer while the Mac is loaded", from the desktop's Settings.
fn desktop_limits(home: &Path) -> (u32, bool) {
    let prefs = pit_core::prefs::Prefs::load(home);
    (prefs.max_agents.clamp(1, 16), prefs.adaptive_agents)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    You,
    Mode,
    Memory,
    /// macOS's thermal pressure reached serious or critical.
    Heat,
}

impl Why {
    fn word(self) -> &'static str {
        match self {
            Why::You => "you",
            Why::Mode => "mode",
            Why::Memory => "memory",
            Why::Heat => "heat",
        }
    }
}

struct Hold {
    why: Why,
    since: u64,
    pids: Vec<i32>,
}

#[derive(Clone, Serialize)]
struct Sample {
    t: u64,
    cpu: f64,
    mem_used: u64,
    agents_mem: u64,
    agents_cpu: f64,
}

/// What the governor did, for the screen to say in the person's language.
#[derive(Clone, Serialize, Default)]
struct Event {
    t: u64,
    /// `full`, `light`, `away` (auto went Full), `back` (auto went Light), `paused_mode`,
    /// `paused_memory`, `resumed`.
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    card: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Cards allowed, % free, seconds paused or minutes away, by kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u64>,
}

#[derive(Default)]
struct State {
    cfg: PerfConfig,
    effective: Mode,
    /// Cards that may work at once right now.
    allowed: u32,
    holds: HashMap<String, Hold>,
    /// Runs waiting for room before their agent starts.
    waiting: HashSet<String>,
    throttled: HashSet<i32>,
    tight_streak: u32,
    calm_streak: u32,
    /// Heat cut the cards since they were last all back: a warm Mac waits longer to resume.
    cooling: bool,
    last_cap: u32,
    history: VecDeque<Sample>,
    events: VecDeque<Event>,
    snapshot: Value,
    caffeinate: Option<std::process::Child>,
}

pub struct Perf {
    home: PathBuf,
    /// The governor runs (the daemon proper; tests build supervisors without it).
    enabled: AtomicBool,
    state: Mutex<State>,
}

/// One process, from `ps`.
struct Proc {
    ppid: i32,
    cpu: f64,
    rss: u64,
    name: String,
    app: String,
}

/// What a governor tick needs from the supervisor.
pub struct LiveSession {
    pub meta: RunMeta,
}

impl Perf {
    pub fn new(home: &Path) -> Self {
        let cfg = load_config(home);
        let state = State {
            effective: cfg.mode,
            cfg,
            allowed: desktop_limits(home).0,
            snapshot: json!(null),
            ..Default::default()
        };
        Self {
            home: home.to_owned(),
            enabled: AtomicBool::new(false),
            state: Mutex::new(state),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn config(&self) -> PerfConfig {
        self.state.lock().unwrap().cfg.clone()
    }

    /// Changes the settings: a mode picked by hand turns "Switch by itself" off.
    pub fn set(&self, patch: &Value) -> anyhow::Result<PerfConfig> {
        let mut s = self.state.lock().unwrap();
        let mut cfg = s.cfg.clone();
        if let Some(m) = patch.get("mode").cloned() {
            cfg.mode = serde_json::from_value(m)?;
            if patch.get("auto").is_none() {
                cfg.auto = false;
            }
        }
        if let Some(a) = patch.get("auto").and_then(Value::as_bool) {
            cfg.auto = a;
        }
        if let Some(n) = patch.get("away_minutes").and_then(Value::as_u64) {
            cfg.away_minutes = (n as u32).clamp(1, 120);
        }
        if let Some(n) = patch.get("light_agents").and_then(Value::as_u64) {
            cfg.light_agents = (n as u32).clamp(1, 16);
        }
        if let Some(b) = patch.get("light_background").and_then(Value::as_bool) {
            cfg.light_background = b;
        }
        std::fs::create_dir_all(&self.home)?;
        // Whole or not at all: a half-written file would read as "no settings" on the next start.
        pit_core::fsutil::write_atomic(&config_path(&self.home), serde_json::to_string_pretty(&cfg)?)?;
        if !cfg.auto && cfg.mode != s.effective {
            let kind = match cfg.mode {
                Mode::Full => "full",
                Mode::Light => "light",
            };
            push_event(
                &mut s,
                Event {
                    kind,
                    ..Default::default()
                },
            );
            s.effective = cfg.mode;
        }
        s.cfg = cfg.clone();
        Ok(cfg)
    }

    /// The last sample, the settings and what the governor did.
    pub fn snapshot(&self) -> Value {
        let s = self.state.lock().unwrap();
        let mut v = s.snapshot.clone();
        if !v.is_object() {
            v = json!({});
        }
        v["config"] = serde_json::to_value(&s.cfg).unwrap_or_default();
        v["mode"] = serde_json::to_value(s.effective).unwrap_or_default();
        v["allowed"] = json!(s.allowed);
        v["history"] = serde_json::to_value(&s.history).unwrap_or_default();
        v["events"] =
            serde_json::to_value(s.events.iter().rev().collect::<Vec<_>>()).unwrap_or_default();
        v["governed"] = json!(self.enabled());
        v
    }

    /// Cards that may work at once now.
    pub fn allowed(&self) -> u32 {
        self.state.lock().unwrap().allowed.max(1)
    }

    pub fn is_held(&self, id: &str) -> bool {
        self.state.lock().unwrap().holds.contains_key(id)
    }

    /// The sessions held right now, read in one go (for callers that then take other locks).
    pub fn held_ids(&self) -> std::collections::HashSet<String> {
        self.state.lock().unwrap().holds.keys().cloned().collect()
    }

    pub fn set_waiting(&self, id: &str, on: bool) {
        let mut s = self.state.lock().unwrap();
        if on {
            s.waiting.insert(id.to_owned());
        } else {
            s.waiting.remove(id);
        }
    }

    /// Pauses a session by hand; the governor never resumes it by itself.
    pub fn hold(&self, id: &str, pid: u32) -> bool {
        let pids = tree(pid as i32, &processes());
        let mut s = self.state.lock().unwrap();
        if let Some(h) = s.holds.get_mut(id) {
            h.why = Why::You;
            return false;
        }
        freeze(&pids);
        s.holds.insert(
            id.to_owned(),
            Hold {
                why: Why::You,
                since: now(),
                pids,
            },
        );
        true
    }

    /// Resumes a paused session (by hand, or before a stop so it can end cleanly).
    pub fn release(&self, id: &str, pid: Option<u32>) -> bool {
        let h = self.state.lock().unwrap().holds.remove(id);
        let Some(h) = h else { return false };
        let mut pids = h.pids;
        if let Some(p) = pid {
            pids.extend(tree(p as i32, &processes()));
        }
        thaw(&pids);
        true
    }

    /// Logs a pause or resume done by hand (`paused_you`, `resumed_you`).
    pub fn log_manual(&self, kind: &'static str, card: u32, title: &str) {
        push_event(
            &mut self.state.lock().unwrap(),
            Event {
                kind,
                card: Some(card),
                title: Some(title.to_owned()),
                ..Default::default()
            },
        );
    }

    /// Resumes everything (the daemon is ending).
    pub fn release_all(&self) {
        let holds: Vec<Hold> = self
            .state
            .lock()
            .unwrap()
            .holds
            .drain()
            .map(|(_, h)| h)
            .collect();
        for h in holds {
            thaw(&h.pids);
        }
        if let Some(mut c) = self.state.lock().unwrap().caffeinate.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// One governor step. Returns notes for the runs it paused or resumed.
    pub fn tick(&self, sessions: &[LiveSession]) -> Vec<(String, String)> {
        let procs = processes();
        let sys = system(&procs);
        let temps = crate::temps::read();
        let (max, adaptive) = desktop_limits(&self.home);
        let idle = idle_seconds();
        let mut notes = Vec::new();
        let mut s = self.state.lock().unwrap();

        // Which mode.
        let mode = if s.cfg.auto {
            if idle >= f64::from(s.cfg.away_minutes) * 60.0 {
                Mode::Full
            } else {
                Mode::Light
            }
        } else {
            s.cfg.mode
        };
        if mode != s.effective {
            let ev = match mode {
                Mode::Full => Event {
                    kind: "away",
                    n: Some(u64::from(s.cfg.away_minutes)),
                    ..Default::default()
                },
                Mode::Light => Event {
                    kind: "back",
                    ..Default::default()
                },
            };
            push_event(&mut s, ev);
            s.effective = mode;
        }

        // How many cards may work at once.
        let cap = match mode {
            Mode::Full => max,
            Mode::Light => s.cfg.light_agents.min(max),
        }
        .max(1);
        let (tight_free, critical_free) = match mode {
            Mode::Full => (10.0, 5.0),
            Mode::Light => (25.0, 12.0),
        };
        let mem_critical = adaptive && (sys.pressure >= 4 || sys.free_pct < critical_free);
        let mem_tight = mem_critical || (adaptive && (sys.pressure >= 2 || sys.free_pct < tight_free));
        // Heat goes by macOS's own thermal pressure, not by a sensor: Apple silicon runs its dies
        // near 100 °C under load by design and slows itself down; serious is where Apple asks
        // apps to cut work, critical where the Mac must cool down. Fair asks nothing yet.
        let hot_critical = adaptive && temps.thermal == Some("critical");
        let hot = hot_critical || (adaptive && temps.thermal == Some("serious"));
        let critical = mem_critical || hot_critical;
        let tight = mem_tight || hot;
        if hot {
            s.cooling = true;
        }
        if tight {
            s.tight_streak += 1;
            s.calm_streak = 0;
        } else {
            s.calm_streak += 1;
            s.tight_streak = 0;
        }
        let target = if critical {
            1
        } else if tight {
            cap.saturating_sub(1).max(1)
        } else {
            cap
        };
        // A new cap (a mode or the setting changed) applies at once, unless memory says otherwise.
        if s.allowed == 0 || s.allowed > cap || (cap != s.last_cap && !tight) {
            s.allowed = cap;
        }
        s.last_cap = cap;
        // A card back every 16 s once memory eases, every 60 s after heat: a Mac cools slower
        // than it frees memory, and bringing the work back at once heats it right up again.
        let calm_needed = if s.cooling { 30 } else { 8 };
        if target < s.allowed && s.tight_streak >= 2 {
            s.allowed = target;
        } else if target > s.allowed && (s.calm_streak >= calm_needed || !adaptive) {
            s.allowed += 1;
            s.calm_streak = 1;
        }
        if s.allowed >= cap {
            s.cooling = false;
        }
        let allowed = s.allowed.max(1);

        // Each session's process tree.
        let live_ids: HashSet<&str> = sessions.iter().map(|l| l.meta.id.as_str()).collect();
        s.holds.retain(|id, _| live_ids.contains(id.as_str()));
        s.waiting.retain(|id| live_ids.contains(id.as_str()));
        let mut agents = Vec::new();
        let mut all_pids = HashSet::new();
        for l in sessions {
            let Some(pid) = l.meta.pid else {
                agents.push((l, Vec::new(), 0u64, 0f64));
                continue;
            };
            let pids = tree(pid as i32, &procs);
            let mem: u64 = pids
                .iter()
                .map(|p| footprint(*p).unwrap_or_else(|| procs.get(p).map_or(0, |x| x.rss)))
                .sum();
            let cpu: f64 = pids
                .iter()
                .filter_map(|p| procs.get(p))
                .map(|x| x.cpu)
                .sum();
            all_pids.extend(pids.iter().copied());
            agents.push((l, pids, mem, cpu));
        }

        // Pause what does not fit; resume what does.
        let cards: Vec<&(&LiveSession, Vec<i32>, u64, f64)> = agents
            .iter()
            .filter(|a| a.0.meta.kind == SessionKind::Run && a.0.meta.pid.is_some())
            .collect();
        let working: Vec<_> = cards
            .iter()
            .filter(|a| !s.holds.contains_key(&a.0.meta.id))
            .collect();
        if self.enabled() && working.len() as u32 > allowed {
            let over = working.len() - allowed as usize;
            let by_mode = working.len() as u32 > cap;
            // Memory first: running out of it gets processes killed, heat only slows the Mac.
            let why = if by_mode {
                Why::Mode
            } else if mem_tight {
                Why::Memory
            } else {
                Why::Heat
            };
            let mut pick: Vec<_> = working.clone();
            match why {
                Why::Mode => pick.sort_by_key(|a| std::cmp::Reverse(a.0.meta.started_at)),
                Why::Heat => pick.sort_by(|a, b| b.3.total_cmp(&a.3)),
                _ => pick.sort_by_key(|a| std::cmp::Reverse(a.2)),
            }
            let take = if by_mode { over } else { 1 };
            for a in pick.into_iter().take(take) {
                freeze(&a.1);
                s.holds.insert(
                    a.0.meta.id.clone(),
                    Hold {
                        why,
                        since: now(),
                        pids: a.1.clone(),
                    },
                );
                let reason = match why {
                    Why::Mode => format!(
                        "{} lets {} at once",
                        mode_name(mode),
                        plural(allowed, "card")
                    ),
                    Why::Heat => "the Mac is too warm and macOS is slowing it down".to_string(),
                    _ => format!("memory is tight ({}% free)", sys.free_pct.round()),
                };
                let (kind, n) = match why {
                    Why::Mode => ("paused_mode", u64::from(allowed)),
                    Why::Heat => ("paused_heat", u64::from(hot_critical)),
                    _ => ("paused_memory", sys.free_pct.round() as u64),
                };
                push_event(
                    &mut s,
                    Event {
                        kind,
                        card: Some(a.0.meta.card),
                        title: Some(a.0.meta.card_title.clone()),
                        n: Some(n),
                        ..Default::default()
                    },
                );
                notes.push((a.0.meta.id.clone(), format!("Paused by Performance: {reason}. Nothing is lost; it continues where it was when there is room.")));
            }
        } else if self.enabled() && (working.len() as u32) < allowed && !tight {
            let mut held: Vec<_> = cards
                .iter()
                .filter(|a| s.holds.get(&a.0.meta.id).is_some_and(|h| h.why != Why::You))
                .collect();
            held.sort_by_key(|a| s.holds[&a.0.meta.id].since);
            if let Some(a) = held.first()
                && let Some(h) = s.holds.remove(&a.0.meta.id)
            {
                let mut pids = h.pids;
                pids.extend(a.1.iter().copied());
                thaw(&pids);
                let paused = now().saturating_sub(h.since);
                push_event(
                    &mut s,
                    Event {
                        kind: "resumed",
                        card: Some(a.0.meta.card),
                        title: Some(a.0.meta.card_title.clone()),
                        n: Some(paused),
                        ..Default::default()
                    },
                );
                notes.push((
                    a.0.meta.id.clone(),
                    format!("Resumed by Performance after {}.", span(paused)),
                ));
            }
        }

        // Background priority in Light.
        let background = mode == Mode::Light && s.cfg.light_background && self.enabled();
        s.throttled.retain(|p| all_pids.contains(p));
        for p in &all_pids {
            let on = s.throttled.contains(p);
            if background && !on {
                set_background(*p, true);
                s.throttled.insert(*p);
            } else if !background && on {
                set_background(*p, false);
                s.throttled.remove(p);
            }
        }

        // Keep the Mac awake while agents work in Full.
        let keep_awake = self.enabled() && mode == Mode::Full && !sessions.is_empty();
        match (keep_awake, s.caffeinate.is_some()) {
            (true, false) => {
                s.caffeinate = std::process::Command::new("/usr/bin/caffeinate")
                    .args(["-i", "-w", &std::process::id().to_string()])
                    .spawn()
                    .ok();
            }
            (false, true) => {
                if let Some(mut c) = s.caffeinate.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            _ => {}
        }

        // The picture.
        let agents_mem: u64 = agents.iter().map(|a| a.2).sum();
        let agents_cpu: f64 = agents.iter().map(|a| a.3).sum();
        let list: Vec<Value> = agents
            .iter()
            .map(|(l, pids, mem, cpu)| {
                let m = &l.meta;
                let hold = s.holds.get(&m.id);
                let state = if hold.is_some() {
                    "paused"
                } else if s.waiting.contains(&m.id) {
                    "waiting"
                } else if m.pid.is_none() {
                    "starting"
                } else if background {
                    "background"
                } else {
                    "working"
                };
                let mut top: Vec<(&str, u64, f64)> = pids.iter().filter_map(|p| procs.get(p).map(|x| (x.name.as_str(), footprint(*p).unwrap_or(x.rss), x.cpu))).collect();
                top.sort_by_key(|x| std::cmp::Reverse(x.1));
                json!({
                    "id": m.id, "card": m.card, "title": m.card_title, "kind": m.kind, "repo": m.repo, "branch": m.branch,
                    "model": m.model, "started_at": m.started_at, "pid": m.pid, "state": state,
                    "paused_by": hold.map(|h| h.why.word()), "paused_since": hold.map(|h| h.since),
                    "mem": mem, "cpu": cpu, "procs": pids.len(),
                    "top": top.iter().take(4).map(|(n, mm, c)| json!({ "name": n, "mem": mm, "cpu": c })).collect::<Vec<_>>(),
                })
            })
            .collect();
        // Everything else on the Mac, by app.
        let mut apps: HashMap<&str, (u64, f64)> = HashMap::new();
        for (pid, p) in &procs {
            if all_pids.contains(pid) {
                continue;
            }
            let e = apps.entry(p.app.as_str()).or_default();
            e.0 += p.rss;
            e.1 += p.cpu;
        }
        let mut apps: Vec<_> = apps.into_iter().collect();
        apps.sort_by_key(|x| std::cmp::Reverse(x.1.0));
        let others: Vec<Value> = apps
            .iter()
            .take(6)
            .map(|(n, (m, c))| json!({ "name": n, "mem": m, "cpu": c }))
            .collect();

        let t = now();
        s.history.push_back(Sample {
            t,
            cpu: sys.cpu,
            mem_used: sys.mem_used,
            agents_mem,
            agents_cpu,
        });
        while s.history.len() > 180 {
            s.history.pop_front();
        }
        s.snapshot = json!({
            "at": t,
            "cap": cap, "max": max, "adaptive": adaptive, "idle_secs": idle, "tight": mem_tight, "critical": mem_critical, "hot": hot, "hot_critical": hot_critical,
            "background": background, "awake": s.caffeinate.is_some(),
            "system": {
                "mem_total": sys.mem_total, "mem_used": sys.mem_used, "free_pct": sys.free_pct, "pressure": match sys.pressure { 4 => "critical", 2 => "warn", _ => "normal" },
                "swap_used": sys.swap_used, "swap_total": sys.swap_total, "cpu": sys.cpu, "cores": sys.cores, "p_cores": sys.p_cores,
                "load": sys.load,
            },
            "temps": temps,
            "agents": list,
            "agents_mem": agents_mem, "agents_cpu": agents_cpu,
            "others": others,
        });
        notes
    }
}

fn push_event(s: &mut State, mut ev: Event) {
    ev.t = now();
    s.events.push_back(ev);
    while s.events.len() > 40 {
        s.events.pop_front();
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Full => "Full",
        Mode::Light => "Light",
    }
}

fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn span(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---- The Mac -------------------------------------------------------------------------------

struct System {
    mem_total: u64,
    mem_used: u64,
    free_pct: f64,
    /// 1 normal, 2 warn, 4 critical (the dispatch memory-pressure levels).
    pressure: i32,
    swap_used: u64,
    swap_total: u64,
    /// Share of all cores in use, 0..100.
    cpu: f64,
    cores: u32,
    p_cores: u32,
    load: f64,
}

fn system(procs: &HashMap<i32, Proc>) -> System {
    let mem_total: u64 = sysctl("hw.memsize").unwrap_or(0);
    let level: i32 = sysctl("kern.memorystatus_level").unwrap_or(100);
    let free_pct = f64::from(level.clamp(0, 100));
    let pressure: i32 = sysctl("kern.memorystatus_vm_pressure_level").unwrap_or(1);
    let swap: Option<libc::xsw_usage> = sysctl("vm.swapusage");
    let cores = sysctl::<i32>("hw.ncpu").unwrap_or(8).max(1) as u32;
    let p_cores = sysctl::<i32>("hw.perflevel0.physicalcpu").map_or(cores, |n| n.max(1) as u32);
    let mut avg = [0f64; 3];
    // SAFETY: getloadavg writes at most `nelem` doubles into the buffer we pass.
    let load = if unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) } >= 1 {
        avg[0]
    } else {
        0.0
    };
    // `ps` %CPU is a decaying average per live process: it misses the short-lived compilers, git
    // and test runs agents start by the hundred, and lags. The kernel's tick counters are what
    // Activity Monitor and `top` read; `ps` stays as the fallback for the first tick.
    let cpu = cpu_busy()
        .unwrap_or_else(|| procs.values().map(|p| p.cpu).sum::<f64>() / f64::from(cores))
        .clamp(0.0, 100.0);
    System {
        mem_total,
        mem_used: (mem_total as f64 * (100.0 - free_pct) / 100.0) as u64,
        free_pct,
        pressure,
        swap_used: swap.map_or(0, |s| s.xsu_used),
        swap_total: swap.map_or(0, |s| s.xsu_total),
        cpu,
        cores,
        p_cores,
        load,
    }
}

unsafe extern "C" {
    fn mach_host_self() -> libc::mach_port_t;
}

/// Share of all cores busy since the last call, from the kernel's per-state tick counters
/// (user, system, idle, nice). Like Activity Monitor it counts a busy efficiency core the same
/// as a busy performance core and ignores clock speed, so it says how occupied the Mac is, not
/// how much more it could do.
fn cpu_busy() -> Option<f64> {
    static HOST: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    static LAST: Mutex<Option<[u32; 4]>> = Mutex::new(None);
    // SAFETY: returns this task's send right to the host port; asked once and kept.
    let host = *HOST.get_or_init(|| unsafe { mach_host_self() });
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; 4] };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: HOST_CPU_LOAD_INFO fills a host_cpu_load_info of `count` integers, the size we pass.
    let r = unsafe { libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, (&mut info as *mut libc::host_cpu_load_info).cast(), &mut count) };
    if r != 0 {
        return None;
    }
    let now = info.cpu_ticks;
    let prev = LAST.lock().ok()?.replace(now)?;
    // The counters are 32-bit and wrap; a difference across the wrap is still right.
    let d: Vec<u64> = (0..4).map(|i| u64::from(now[i].wrapping_sub(prev[i]))).collect();
    let total: u64 = d.iter().sum();
    let idle = d[libc::CPU_STATE_IDLE as usize];
    (total > 0).then(|| (total - idle) as f64 * 100.0 / total as f64)
}

fn sysctl<T: Copy>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let mut v = std::mem::MaybeUninit::<T>::zeroed();
    let mut len = std::mem::size_of::<T>();
    // SAFETY: the kernel writes at most `len` bytes into `v`, which is `len` bytes long; we read
    // it only when the call succeeded and filled exactly that size.
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            v.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && len == std::mem::size_of::<T>()).then(|| unsafe { v.assume_init() })
}

/// Every process: parent, CPU (% of one core), resident memory, name and the app it belongs to.
fn processes() -> HashMap<i32, Proc> {
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pcpu=,rss=,comm="])
        .output()
    else {
        return HashMap::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let cpu = it.next()?.parse().ok()?;
            let rss: u64 = it.next()?.parse().ok()?;
            let comm = skip_fields(l, 4)?;
            let name = comm.rsplit('/').next().unwrap_or(comm).to_owned();
            let app = comm
                .split('/')
                .find(|c| c.ends_with(".app"))
                .map(|a| a.trim_end_matches(".app").to_owned())
                .unwrap_or_else(|| name.clone());
            Some((
                pid,
                Proc {
                    ppid,
                    cpu,
                    rss: rss * 1024,
                    name,
                    app,
                },
            ))
        })
        .collect()
}

/// The text after the first `n` whitespace-separated fields (a command path may hold spaces).
fn skip_fields(line: &str, n: usize) -> Option<&str> {
    let mut rest = line.trim_start();
    for _ in 0..n {
        let at = rest.find(char::is_whitespace)?;
        rest = rest[at..].trim_start();
    }
    (!rest.is_empty()).then_some(rest)
}

/// A process and everything under it.
fn tree(root: i32, procs: &HashMap<i32, Proc>) -> Vec<i32> {
    let mut kids: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, p) in procs {
        kids.entry(p.ppid).or_default().push(*pid);
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(k) = kids.get(&out[i]) {
            let me = out[i];
            out.extend(k.iter().copied().filter(|k| *k != me));
        }
        i += 1;
    }
    out
}

/// Memory as Activity Monitor counts it (compressed and swapped pages included).
fn footprint(pid: i32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: proc_pid_rusage fills a rusage_info_v4 for flavor RUSAGE_INFO_V4; we read it only on success.
    let r = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };
    (r == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
}

/// SIGSTOP the session first (so it starts nothing new), then what it runs.
fn freeze(pids: &[i32]) {
    for p in pids {
        // SAFETY: kill(2) on processes of our own agent's tree; a stale id only yields ESRCH.
        unsafe {
            libc::kill(*p, libc::SIGSTOP);
        }
    }
}

/// SIGCONT what it runs first, then the session.
fn thaw(pids: &[i32]) {
    for p in pids.iter().rev() {
        // SAFETY: as in `freeze`.
        unsafe {
            libc::kill(*p, libc::SIGCONT);
        }
    }
}

/// Background priority: efficiency cores and throttled disk on Apple silicon (`taskpolicy -b`).
fn set_background(pid: i32, on: bool) {
    // SAFETY: setpriority on a process we own; failure (it ended) is harmless.
    unsafe {
        libc::setpriority(
            libc::PRIO_DARWIN_PROCESS,
            pid as libc::id_t,
            if on { libc::PRIO_DARWIN_BG } else { 0 },
        );
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// Seconds since the last key press or mouse move anywhere on this Mac.
pub(crate) fn idle_seconds() -> f64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a pure query; combined session state (0), any input event (!0).
        unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_mac() {
        let procs = processes();
        assert!(procs.contains_key(&(std::process::id() as i32)));
        let sys = system(&procs);
        assert!(sys.mem_total > 0 && sys.mem_used <= sys.mem_total);
        assert!(footprint(std::process::id() as i32).is_some_and(|f| f > 0));
        let me = tree(std::process::id() as i32, &procs);
        assert_eq!(me[0], std::process::id() as i32);
    }

    #[test]
    fn cpu_comes_from_the_kernel_ticks() {
        // Ticks come at 100 Hz per core: spin long enough for some to land. Another test sampling
        // at the same time takes the interval for itself, so it is asked until one comes back.
        cpu_busy();
        let start = std::time::Instant::now();
        let mut got = None;
        while got.is_none() && start.elapsed() < std::time::Duration::from_secs(2) {
            let spin = std::time::Instant::now();
            while spin.elapsed() < std::time::Duration::from_millis(50) {
                std::hint::spin_loop();
            }
            got = cpu_busy();
        }
        assert!(got.is_some_and(|c| (0.0..=100.0).contains(&c)), "{got:?}");
    }

    fn stat(pid: u32) -> String {
        let o = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_owned()
    }

    #[test]
    fn a_pause_freezes_the_whole_tree_and_resume_thaws_it() {
        let home = tempfile::tempdir().unwrap();
        let p = Perf::new(home.path());
        // A shell (the session) running a command (its tool).
        let mut sh = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let kids = tree(sh.id() as i32, &processes());
        assert!(kids.len() >= 2, "the shell and its sleep");
        assert!(p.hold("r1", sh.id()));
        assert!(p.is_held("r1"));
        for k in &kids {
            assert!(stat(*k as u32).starts_with('T'), "stopped");
        }
        assert!(p.release("r1", Some(sh.id())));
        for k in &kids {
            assert!(!stat(*k as u32).starts_with('T'), "running again");
        }
        let _ = sh.kill();
        let _ = sh.wait();
        for k in kids {
            unsafe { libc::kill(k, libc::SIGKILL) };
        }
    }

    #[test]
    fn a_tick_paints_the_mac() {
        let home = tempfile::tempdir().unwrap();
        let p = Perf::new(home.path());
        assert!(p.tick(&[]).is_empty());
        let v = p.snapshot();
        assert!(v["system"]["mem_total"].as_u64().unwrap() > 0);
        assert_eq!(v["agents"].as_array().unwrap().len(), 0);
        assert_eq!(v["history"].as_array().unwrap().len(), 1);
        assert!(!v["others"].as_array().unwrap().is_empty());
        assert_eq!(v["mode"], "full");
    }

    #[test]
    fn a_mode_picked_by_hand_turns_auto_off() {
        let home = tempfile::tempdir().unwrap();
        let p = Perf::new(home.path());
        p.set(&json!({ "auto": true })).unwrap();
        let c = p.set(&json!({ "mode": "light" })).unwrap();
        assert_eq!(c.mode, Mode::Light);
        assert!(!c.auto);
        assert_eq!(load_config(home.path()).mode, Mode::Light);
    }
}
