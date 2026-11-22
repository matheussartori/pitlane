//! The language agents write in. Chosen in the desktop's Settings (`prefs.agent_language` in
//! `desktop.json`): a BCP 47 tag such as `pt-BR`, or `system` for the language this Mac uses.

use std::path::Path;
use std::process::Command;

/// The language every session writes in for a person, as an English name ("Portuguese (Brazil)").
pub fn agent_language(pitlane_home: &Path) -> String {
    let chosen = std::fs::read_to_string(pitlane_home.join("desktop.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("prefs")?.get("agent_language")?.as_str().map(str::to_owned))
        .filter(|s| !s.is_empty() && s != "system");
    name(&chosen.unwrap_or_else(system_locale))
}

/// The rule appended to every session's system prompt.
pub fn rule(language: &str) -> String {
    format!(
        "\n\n# Language\n\nWrite everything a person reads in {language}: replies, questions and their options, notes, \
decisions, summaries, handoffs, findings, card titles and specs. Keep code, identifiers, file paths, commands, \
tool arguments and JSON keys as they are, and write commit messages and code comments the way the repository already does."
    )
}

/// The Mac's language (`AppleLanguages`), else `LANG`, else English.
fn system_locale() -> String {
    let mac = Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.split('"').nth(1).map(str::to_owned)
        });
    mac.or_else(|| std::env::var("LANG").ok().map(|l| l.split('.').next().unwrap_or_default().replace('_', "-")))
        .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
        .unwrap_or_else(|| "en".into())
}

/// A language tag as an English name the model reads unambiguously.
pub fn name(tag: &str) -> String {
    let tag = tag.replace('_', "-");
    let mut parts = tag.split('-');
    let lang = parts.next().unwrap_or("en").to_ascii_lowercase();
    let region = parts.last().map(str::to_ascii_uppercase).unwrap_or_default();
    let base = match lang.as_str() {
        "en" => "English",
        "pt" => return if region == "PT" { "Portuguese (Portugal)".into() } else { "Portuguese (Brazil)".into() },
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "nl" => "Dutch",
        "pl" => "Polish",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "tr" => "Turkish",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" => return if region == "TW" || region == "HK" || region == "HANT" { "Traditional Chinese".into() } else { "Simplified Chinese".into() },
        "hi" => "Hindi",
        "ar" => "Arabic",
        "he" => "Hebrew",
        "sv" => "Swedish",
        "da" => "Danish",
        "nb" | "no" => "Norwegian",
        "fi" => "Finnish",
        "cs" => "Czech",
        "id" => "Indonesian",
        "vi" => "Vietnamese",
        "th" => "Thai",
        _ => return format!("the language with the code {tag}"),
    };
    base.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name("pt-BR"), "Portuguese (Brazil)");
        assert_eq!(name("pt_PT"), "Portuguese (Portugal)");
        assert_eq!(name("pt"), "Portuguese (Brazil)");
        assert_eq!(name("en-US"), "English");
        assert_eq!(name("zh-Hant-TW"), "Traditional Chinese");
        assert_eq!(name("xx"), "the language with the code xx");
    }

    #[test]
    fn reads_the_choice() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("desktop.json"), r#"{"repos":[],"prefs":{"agent_language":"es"}}"#).unwrap();
        assert_eq!(agent_language(d.path()), "Spanish");
    }
}
