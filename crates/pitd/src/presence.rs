//! Presence: whether you are here to answer the calls a run would otherwise make by itself.
//! Here, a run asks you what is yours to decide (behaviour you will see, scope, a choice between
//! real alternatives) and waits a while for the answer. Away, it goes with its own recommendation
//! and stops only for what it cannot do or what would be costly to undo. Either way it never
//! stalls: an answer that does not come in time is the recommendation.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Here,
    /// What Pitlane always did: ask once in the analysis, then never stop to ask.
    #[default]
    Away,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceConfig {
    pub mode: Mode,
    /// Here while you use the Mac, Away after Performance's `away_minutes` without a key or the mouse.
    pub auto: bool,
    /// How long a run waits for your answer before it goes with its recommendation.
    pub wait_minutes: u32,
    /// Questions one run may ask you; past them it decides by itself.
    pub per_run: u32,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self { mode: Mode::Away, auto: false, wait_minutes: 10, per_run: 3 }
    }
}

fn config_path(home: &Path) -> PathBuf {
    home.join("presence.json")
}

pub struct Presence {
    home: PathBuf,
    cfg: Mutex<PresenceConfig>,
}

impl Presence {
    pub fn new(home: &Path) -> Self {
        let cfg = std::fs::read_to_string(config_path(home))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { home: home.to_owned(), cfg: Mutex::new(cfg) }
    }

    pub fn config(&self) -> PresenceConfig {
        self.cfg.lock().unwrap().clone()
    }

    /// Changes the settings: a mode picked by hand turns "Switch by itself" off.
    pub fn set(&self, patch: &Value) -> anyhow::Result<PresenceConfig> {
        let mut cur = self.cfg.lock().unwrap();
        let mut cfg = cur.clone();
        if let Some(m) = patch.get("mode").cloned() {
            cfg.mode = serde_json::from_value(m)?;
            if patch.get("auto").is_none() {
                cfg.auto = false;
            }
        }
        if let Some(a) = patch.get("auto").and_then(Value::as_bool) {
            cfg.auto = a;
        }
        if let Some(n) = patch.get("wait_minutes").and_then(Value::as_u64) {
            cfg.wait_minutes = (n as u32).clamp(1, 120);
        }
        if let Some(n) = patch.get("per_run").and_then(Value::as_u64) {
            cfg.per_run = (n as u32).clamp(1, 20);
        }
        std::fs::create_dir_all(&self.home)?;
        // Whole or not at all: a half-written file would read as "no settings" on the next start.
        pit_core::fsutil::write_atomic(&config_path(&self.home), serde_json::to_string_pretty(&cfg)?)?;
        *cur = cfg.clone();
        Ok(cfg)
    }

    /// The mode in force now. With `auto`, you are here while the Mac saw a key or the mouse in
    /// the last `away_minutes` (Performance's), the same "away" Performance goes Full on.
    pub fn effective(&self, away_minutes: u32) -> Mode {
        let cfg = self.config();
        if !cfg.auto {
            return cfg.mode;
        }
        effective_for(crate::perf::idle_seconds(), away_minutes)
    }

    /// The settings and the mode in force, for the screens.
    pub fn snapshot(&self, away_minutes: u32) -> Value {
        json!({ "config": self.config(), "mode": self.effective(away_minutes), "away_minutes": away_minutes })
    }
}

fn effective_for(idle_secs: f64, away_minutes: u32) -> Mode {
    if idle_secs < f64::from(away_minutes) * 60.0 { Mode::Here } else { Mode::Away }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_is_the_default_and_picking_a_mode_turns_auto_off() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = Presence::new(tmp.path());
        assert_eq!(p.config().mode, Mode::Away);
        p.set(&json!({ "auto": true })).unwrap();
        let c = p.set(&json!({ "mode": "here" })).unwrap();
        assert_eq!((c.mode, c.auto), (Mode::Here, false));
        assert_eq!(p.effective(10), Mode::Here);
        let c = p.set(&json!({ "wait_minutes": 0, "per_run": 99 })).unwrap();
        assert_eq!((c.wait_minutes, c.per_run), (1, 20));
        assert_eq!(Presence::new(tmp.path()).config().mode, Mode::Here, "kept on disk");
    }

    #[test]
    fn auto_follows_the_idle_time() {
        assert_eq!(effective_for(30.0, 10), Mode::Here);
        assert_eq!(effective_for(601.0, 10), Mode::Away);
    }
}
