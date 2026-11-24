//! The desktop app's preferences, kept under `prefs` in `desktop.json` in Pitlane's home. The app
//! writes them; the daemon and the CLI read what they need (the agents' language, how many agents
//! Run all keeps working) through the same declaration.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The app's file in Pitlane's home: its repositories, the last one open, and these preferences.
pub const FILE: &str = "desktop.json";

/// The app's own preferences (a repository's flow lives in its .pitlane/flow.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "system", "light" or "dark".
    pub theme: String,
    /// System notifications, each when Pitlane is not in front.
    pub notify_approvals: bool,
    pub notify_runs: bool,
    pub notify_terminals: bool,
    /// Load VS Code in the background when a repository opens.
    pub code_prewarm: bool,
    /// Pitlane's own words: "system", "en" or "pt-BR".
    pub ui_language: String,
    /// What agents write in for you (questions, notes, replies): "system" or a language tag.
    /// The daemon reads it from this file when it starts a session.
    pub agent_language: String,
    /// Run all: at most this many agents work at once.
    pub max_agents: u32,
    /// Run all: fewer agents while this Mac is loaded, back up when it has room.
    pub adaptive_agents: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            notify_approvals: true,
            notify_runs: true,
            notify_terminals: true,
            code_prewarm: true,
            ui_language: "system".into(),
            agent_language: "system".into(),
            max_agents: 3,
            adaptive_agents: true,
        }
    }
}

impl Prefs {
    /// What the desktop app last saved under `pitlane_home`; the defaults when it never ran or
    /// the file cannot be read.
    pub fn load(pitlane_home: &Path) -> Self {
        std::fs::read_to_string(pitlane_home.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v.get("prefs")?.clone()).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_missing_takes_its_default() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Prefs::load(d.path()).max_agents, 3);
        std::fs::write(d.path().join(FILE), r#"{"repos":[],"prefs":{"max_agents":5,"agent_language":"es"}}"#).unwrap();
        let p = Prefs::load(d.path());
        assert_eq!((p.max_agents, p.agent_language.as_str(), p.adaptive_agents), (5, "es", true));
    }
}
