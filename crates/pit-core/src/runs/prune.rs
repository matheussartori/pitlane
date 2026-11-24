//! Which session records are old enough to delete. A run's folder (its log, briefing and
//! screenshots) is kept for `KEEP_DAYS` after it ended; without this the folder only grows.

use std::collections::HashMap;

use super::{RunMeta, SessionKind};

/// How long an ended session's record is kept.
pub const KEEP_DAYS: u64 = 30;

/// The ids of the sessions in `all` whose records can go at `now`: ended more than `KEEP_DAYS`
/// ago, and neither the latest run of their card (a card is resumed and reviewed from it,
/// however old) nor one `referenced` says is still shown somewhere (a report's screenshots).
/// A session with no end recorded is never listed.
pub fn expired(all: &[RunMeta], now: u64, referenced: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut latest: HashMap<(&std::path::Path, u32), &RunMeta> = HashMap::new();
    for m in all.iter().filter(|m| m.kind == SessionKind::Run) {
        let slot = latest.entry((m.repo.as_path(), m.card)).or_insert(m);
        if (m.started_at, m.round) > (slot.started_at, slot.round) {
            *slot = m;
        }
    }
    all.iter()
        .filter(|m| m.ended_at.is_some_and(|end| now.saturating_sub(end) > KEEP_DAYS * 86_400))
        .filter(|m| m.kind != SessionKind::Run || latest.get(&(m.repo.as_path(), m.card)).is_none_or(|l| l.id != m.id))
        .filter(|m| !referenced(&m.id))
        .map(|m| m.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, card: u32, started: u64, ended: Option<u64>, kind: SessionKind) -> RunMeta {
        RunMeta { id: id.into(), repo: "/r".into(), card, started_at: started, ended_at: ended, kind, ..Default::default() }
    }

    #[test]
    fn only_old_ended_sessions_nobody_needs_expire() {
        let day = 86_400;
        let now = 100 * day;
        let all = [
            run("old-first", 1, day, Some(2 * day), SessionKind::Run),
            run("old-latest", 1, 3 * day, Some(4 * day), SessionKind::Run),
            run("old-analysis", 1, day, Some(day), SessionKind::Analysis),
            run("old-in-report", 2, day, Some(day), SessionKind::Run),
            run("old-latest-2", 2, 2 * day, Some(2 * day), SessionKind::Run),
            run("recent", 3, 90 * day, Some(90 * day), SessionKind::Run),
            run("recent-latest", 3, 91 * day, Some(91 * day), SessionKind::Run),
            run("open", 4, day, None, SessionKind::Analysis),
        ];
        let gone = expired(&all, now, &|id| id == "old-in-report");
        assert_eq!(gone, ["old-first", "old-analysis"]);
    }
}
