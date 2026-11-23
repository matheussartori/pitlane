//! The planner: where each open card will probably work (its footprint, from the index), which
//! cards and lanes would touch the same files, and an order that keeps clashing work apart.
//! A prediction, said as one: it points at files, it does not promise.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::brief::{self, DEFAULT_BUDGET};
use crate::cards::{self, Card, CardState};
use crate::index;

/// Predicted files kept per card: the head of the map, where a change most likely lands.
const PREDICTED: usize = 4;
/// A shared file is a clash only when it is this near the head of both footprints (a lane's own
/// files all count as head): the tail of a prediction is too unsure to hold work back.
const HEAD: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Footprint {
    pub card: u32,
    pub title: String,
    /// "predicted" from the index, or "lane": the files its lane already changed.
    pub source: String,
    pub lane: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clash {
    pub card: u32,
    /// The other card, or a lane already at work.
    pub with_card: Option<u32>,
    pub with_lane: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    pub footprints: Vec<Footprint>,
    pub clashes: Vec<Clash>,
    /// Cards that can start together, batch by batch (the first batch can start now).
    pub batches: Vec<Vec<u32>>,
    /// Cards that should wait for a lane at work on the same files to land.
    pub waits: Vec<Clash>,
}

fn waiting(c: &Card) -> bool {
    matches!(c.state, CardState::Inbox | CardState::Ready) && c.lane.is_none()
}

fn at_work(c: &Card) -> bool {
    c.lane.is_some() && matches!(c.state, CardState::Running | CardState::Verifying | CardState::Review | CardState::Blocked | CardState::Ready | CardState::InDev)
}

/// The files a lane changed itself: since it left the branch it lands in.
///
/// Measured from production alone, a lane cut from an integration branch seems to have changed
/// everything that branch already holds, and every card near those files waits for a lane that
/// has nothing of the kind to land. So the base is the nearest of the branches the lane can have
/// left: the card's target, the flow's integration and production, each here and on the remote
/// (work lands locally before it is pushed).
pub fn lane_files(repo: &Repo, branch: &str, target: Option<&str>) -> Vec<String> {
    let Ok(Some(head)) = repo.git.rev_parse(&format!("refs/heads/{branch}")) else { return Vec::new() };
    let flow = &repo.flow.flow;
    let ahead = |base: &String| -> u64 {
        repo.git.run(&["rev-list", "--count", &format!("{base}..{head}")]).ok().and_then(|n| n.trim().parse().ok()).unwrap_or(u64::MAX)
    };
    let base = target
        .into_iter()
        .chain(flow.integration())
        .chain([flow.production.as_str()])
        .flat_map(|name| [repo.remote_ref(name), format!("refs/heads/{name}")])
        .filter_map(|r| repo.git.rev_parse(&r).ok().flatten())
        .filter_map(|sha| repo.git.run(&["merge-base", &sha, &head]).ok())
        .min_by_key(ahead);
    let Some(base) = base else { return Vec::new() };
    repo.git.lines(&["diff", "--name-only", &format!("{base}..{head}")]).unwrap_or_default()
}

pub fn plan(repo: &Repo, pitlane_home: &Path) -> Result<Plan> {
    let (all, _) = cards::list(repo)?;
    let (idx, _) = index::update(repo, pitlane_home)?;
    let co = brief::cochanges(repo, 300);
    let mut fps = Vec::new();
    for c in &all {
        if waiting(c) {
            let b = brief::compile_with(&idx, c, &co, DEFAULT_BUDGET);
            let files: Vec<String> = b.files.iter().take(PREDICTED).map(|f| f.path.clone()).collect();
            fps.push(Footprint { card: c.id, title: c.title.clone(), source: "predicted".into(), lane: None, files });
        } else if at_work(c) {
            let lane = c.lane.clone().unwrap_or_default();
            let files = lane_files(repo, &lane, c.target.as_deref());
            if !files.is_empty() {
                fps.push(Footprint { card: c.id, title: c.title.clone(), source: "lane".into(), lane: Some(lane), files });
            }
        }
    }
    Ok(arrange(fps))
}

/// Clashes and batches from footprints. Separate from `plan` so it is testable without git.
pub fn arrange(fps: Vec<Footprint>) -> Plan {
    let head = |f: &Footprint| -> Vec<String> { if f.source == "lane" { f.files.clone() } else { f.files.iter().take(HEAD).cloned().collect() } };
    let shared = |a: &Footprint, b: &Footprint| -> Vec<String> {
        let x: BTreeSet<String> = head(a).into_iter().collect();
        head(b).into_iter().filter(|f| x.contains(f)).collect()
    };
    let pending: Vec<&Footprint> = fps.iter().filter(|f| f.source == "predicted").collect();
    let working: Vec<&Footprint> = fps.iter().filter(|f| f.source == "lane").collect();

    let mut clashes = Vec::new();
    let mut waits = Vec::new();
    for (i, a) in pending.iter().enumerate() {
        for b in &pending[i + 1..] {
            let files = shared(a, b);
            if !files.is_empty() {
                clashes.push(Clash { card: a.card, with_card: Some(b.card), with_lane: None, files });
            }
        }
        for w in &working {
            let files = shared(a, w);
            if !files.is_empty() {
                waits.push(Clash { card: a.card, with_card: Some(w.card), with_lane: w.lane.clone(), files });
            }
        }
    }

    // Greedy: lowest card first, into the first batch with nothing it clashes with. Cards that wait
    // on a lane go after every batch that can start now.
    let clash_with: HashMap<u32, BTreeSet<u32>> = {
        let mut m: HashMap<u32, BTreeSet<u32>> = HashMap::new();
        for c in &clashes {
            let o = c.with_card.unwrap_or_default();
            m.entry(c.card).or_default().insert(o);
            m.entry(o).or_default().insert(c.card);
        }
        m
    };
    let waiting_ids: BTreeSet<u32> = waits.iter().map(|w| w.card).collect();
    let mut ids: Vec<u32> = pending.iter().map(|f| f.card).collect();
    ids.sort();
    let mut batches: Vec<Vec<u32>> = Vec::new();
    for id in ids.iter().filter(|i| !waiting_ids.contains(i)) {
        let none = BTreeSet::new();
        let mine = clash_with.get(id).unwrap_or(&none);
        match batches.iter_mut().find(|b| b.iter().all(|o| !mine.contains(o))) {
            Some(b) => b.push(*id),
            None => batches.push(vec![*id]),
        }
    }
    let late: Vec<u32> = ids.iter().filter(|i| waiting_ids.contains(i)).copied().collect();
    if !late.is_empty() {
        batches.push(late);
    }
    Plan { footprints: fps, clashes, batches, waits }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(card: u32, source: &str, files: &[&str]) -> Footprint {
        Footprint {
            card,
            title: format!("#{card}"),
            source: source.into(),
            lane: (source == "lane").then(|| format!("feat/{card}")),
            files: files.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn keeps_clashing_cards_apart_and_late_ones_last() {
        let p = arrange(vec![
            fp(1, "predicted", &["src/kart/drift.ts", "src/kart/physics.ts", "src/context.ts"]),
            fp(2, "predicted", &["src/lobby/timer.ts", "src/types.ts", "src/context.ts"]),
            fp(3, "predicted", &["src/kart/drift.ts"]),
            fp(4, "predicted", &["src/ui/avatar.tsx"]),
            fp(9, "lane", &["src/ui/avatar.tsx", "src/ui/upload.ts"]),
        ]);
        assert_eq!(p.clashes, vec![Clash { card: 1, with_card: Some(3), with_lane: None, files: vec!["src/kart/drift.ts".into()] }]);
        assert_eq!(p.waits.len(), 1);
        assert_eq!((p.waits[0].card, p.waits[0].with_lane.as_deref()), (4, Some("feat/9")));
        assert_eq!(p.batches, vec![vec![1, 2], vec![3], vec![4]]);
    }

    /// The night the board stood still: lanes cut from the batch's branch had changed nothing in
    /// this repository, but counted as having changed all 31 files that branch was ahead of main
    /// by, so six cards waited for them to land.
    #[test]
    fn a_lane_owns_what_it_changed_not_what_its_branch_already_held() {
        use std::process::Command;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("r");
        std::fs::create_dir_all(&root).unwrap();
        let git = |a: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(&root).args(a).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").status().unwrap();
            assert!(ok.success(), "git {a:?}");
        };
        let commit = |file: &str| {
            std::fs::write(root.join(file), file).unwrap();
            git(&["add", "-A"]);
            git(&["commit", "-qm", file]);
        };
        git(&["init", "-q", "-b", "main"]);
        commit("base.ts");
        // The batch's branch, a day ahead of production.
        git(&["switch", "-q", "-c", "feat/today"]);
        commit("lobby.ts");
        // One lane that changed a file, one that changed nothing yet, one already landed.
        git(&["switch", "-q", "-c", "feat/1"]);
        commit("own.ts");
        git(&["branch", "feat/2", "feat/today"]);
        git(&["switch", "-q", "feat/today"]);
        git(&["switch", "-q", "-c", "feat/3"]);
        commit("landed.ts");
        git(&["switch", "-q", "feat/today"]);
        git(&["merge", "-q", "--no-ff", "-m", "land", "feat/3"]);

        let home = tmp.path().join("home");
        let repo = Repo::open(&root, &home, &home.join(".pitlane")).unwrap();
        assert_eq!(lane_files(&repo, "feat/1", Some("feat/today")), ["own.ts"]);
        assert!(lane_files(&repo, "feat/2", Some("feat/today")).is_empty());
        assert!(lane_files(&repo, "feat/3", Some("feat/today")).is_empty());
        // Without a target, production is all there is to measure from.
        assert_eq!(lane_files(&repo, "feat/1", None), ["lobby.ts", "own.ts"]);
    }
}
