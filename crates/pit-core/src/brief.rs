//! The briefing compiler: from a card and the repository index, where to look. Files are ranked by
//! the card's words against paths and symbols (BM25), exact mentions, the import graph and what
//! changed together in git; the result is a compact map (files, symbols with lines and signatures,
//! tests nearby) under a size budget. Deterministic: same card and index, same bytes.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::cards::Card;
use crate::index::{self, FileEntry, Index, IndexStats, Lang, Symbol};

/// About 3k tokens.
pub const DEFAULT_BUDGET: usize = 12_000;
/// About 1k tokens: below this budget the map is lean (paths, and only the symbols the card names).
pub const LEAN_BUDGET: usize = 4_000;
const MAX_FILES: usize = 12;
const MAX_SYMBOLS: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefFile {
    pub path: String,
    pub score: f32,
    /// Why it is here: matched words, a mention, a neighbour.
    pub why: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BriefStats {
    pub files: usize,
    pub chars: usize,
    pub ms: u64,
    pub indexed: usize,
    /// The files the map points to, most relevant first.
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Brief {
    /// Markdown for the agent; empty when nothing in the index matches the card.
    pub text: String,
    pub files: Vec<BriefFile>,
    pub tests: Vec<String>,
    pub stats: BriefStats,
    pub index: IndexStats,
}

/// Updates the index, then compiles the briefing for a card.
pub fn compile(repo: &Repo, pitlane_home: &Path, card: &Card, budget: usize) -> Result<Brief> {
    let t = Instant::now();
    let (idx, istats) = index::update(repo, pitlane_home)?;
    let cochange = cochanges(repo, 300);
    let mut b = compile_with(&idx, card, &cochange, budget);
    b.index = istats;
    b.stats.ms = t.elapsed().as_millis() as u64;
    Ok(b)
}

// ---------------------------------------------------------------- words

const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "when", "what", "where", "which", "should", "would", "could",
    "will", "not", "but", "are", "was", "were", "has", "have", "had", "can", "all", "any", "add", "make", "use", "new", "get",
    "set", "its", "also", "only", "one", "two", "each", "more", "some", "than", "then", "there", "their", "they", "them",
    "you", "your", "our", "out", "about", "after", "before", "like", "just", "does", "done", "keep", "need", "needs", "want",
    "card", "spec", "file", "files", "code", "change", "changes", "fix", "test", "tests", "work", "way", "why", "how",
    // Portuguese
    "que", "para", "com", "uma", "um", "dos", "das", "nos", "nas", "por", "mais", "como", "quando", "onde", "isso", "esse",
    "essa", "este", "esta", "não", "nao", "ser", "ter", "tem", "pelo", "pela", "seu", "sua", "fazer", "precisa", "deve",
];

/// Lowercase words of a text, identifiers split (`laneDiff`, `lane_diff`, `lane-diff` → lane, diff).
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let mut cur = String::new();
        let chars: Vec<char> = raw.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let boundary = i > 0
                && c.is_uppercase()
                && (chars[i - 1].is_lowercase() || chars.get(i + 1).is_some_and(|n| n.is_lowercase()) && chars[i - 1].is_uppercase());
            if boundary && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.extend(c.to_lowercase());
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out.into_iter().filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit())).map(stem).filter(|w| !STOP.contains(&w.as_str())).collect()
}

/// A light stem so `lanes` meets `lane` and `shipping` meets `ship`.
fn stem(w: String) -> String {
    for suf in ["ings", "ing", "ies", "es", "s", "ed"] {
        if w.len() > suf.len() + 3
            && let Some(base) = w.strip_suffix(suf)
        {
            let base = if suf == "ies" { format!("{base}y") } else { base.to_owned() };
            // shipping → shipp → ship
            let b: Vec<char> = base.chars().collect();
            if suf.starts_with("ing") && b.len() > 3 && b[b.len() - 1] == b[b.len() - 2] {
                return b[..b.len() - 1].iter().collect();
            }
            return base;
        }
    }
    w
}

/// Identifiers and paths as written in the card: `laneDiff`, `src/lanes.tsx`, `pit ship`.
// Sorted: the reasons a file is in the briefing must read the same on every run.
fn mentions(text: &str) -> BTreeSet<String> {
    // Anything in backticks is a name, whatever its case.
    let quoted = text.split('`').skip(1).step_by(2).map(str::trim).filter(|w| w.len() >= 3 && !w.contains(' ')).map(str::to_owned);
    let mut found: BTreeSet<String> = quoted.collect();
    found.extend(text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '(' | ')' | ',' | ';' | ':' | '"' | '\'' | '[' | ']'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '/' && c != '.'))
        .map(|w| w.trim_end_matches('.'))
        .filter(|w| w.len() >= 3)
        .filter(|w| {
            let ident = w.chars().all(|c| c.is_alphanumeric() || c == '_');
            let mixed = w.chars().any(|c| c.is_uppercase()) && w.chars().any(|c| c.is_lowercase()) && !w.chars().next().is_some_and(|c| c.is_uppercase() && w.chars().skip(1).all(|c| c.is_lowercase()));
            (ident && (w.contains('_') || mixed)) || w.contains('/') || (w.contains('.') && !w.ends_with('.'))
        })
        .map(str::to_owned));
    found
}

// ---------------------------------------------------------------- ranking

fn is_test(path: &str) -> bool {
    let p = path.to_lowercase();
    p.contains("/tests/") || p.contains("__tests__") || p.contains(".test.") || p.contains(".spec.") || p.contains("/test/") || p.starts_with("tests/") || p.starts_with("e2e/") || p.contains("/e2e/") || p.rsplit('/').next().is_some_and(|f| f.starts_with("test_"))
}

fn stem_of(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}

/// The words a file is made of: its path counts twice, symbol names once (exported ones a bit more).
fn doc_terms(f: &FileEntry) -> HashMap<String, f32> {
    let mut tf: HashMap<String, f32> = HashMap::new();
    for w in words(&f.path) {
        *tf.entry(w).or_default() += 2.0;
    }
    for s in &f.symbols {
        let weight = if s.exported { 1.2 } else { 0.8 };
        for w in words(&s.name) {
            *tf.entry(w).or_default() += weight;
        }
    }
    tf
}

/// The files each file imports, resolved inside the repository where the specifier allows it.
fn import_graph(idx: &Index) -> HashMap<usize, Vec<usize>> {
    let by_path: HashMap<&str, usize> = idx.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
    let mut by_suffix: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in idx.files.iter().enumerate() {
        let no_ext = f.path.rsplit_once('.').map_or(f.path.as_str(), |(a, _)| a);
        by_suffix.entry(no_ext.trim_end_matches("/index").trim_end_matches("/__init__").trim_end_matches("/mod").to_owned()).or_default().push(i);
    }
    let mut g: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, f) in idx.files.iter().enumerate() {
        let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d);
        for spec in &f.imports {
            let targets: Vec<usize> = match f.lang {
                Lang::Ts | Lang::Tsx | Lang::Js if spec.starts_with('.') => {
                    let joined = normalize(&format!("{dir}/{spec}"));
                    let base = joined.rsplit_once('.').filter(|(_, e)| ["js", "ts", "tsx", "mjs", "jsx"].contains(e)).map_or(joined.as_str(), |(a, _)| a).to_owned();
                    by_suffix.get(&base).cloned().unwrap_or_default()
                }
                Lang::Py => {
                    let p = spec.trim_start_matches('.').replace('.', "/");
                    by_suffix.iter().filter(|(k, _)| !p.is_empty() && (k.as_str() == p || k.ends_with(&format!("/{p}")))).flat_map(|(_, v)| v.clone()).collect()
                }
                Lang::Rust if spec.starts_with("crate::") || spec.starts_with("super::") => {
                    // crate::git::Git → <this crate's src>/git(.rs|/mod.rs)
                    let src = f.path.find("/src/").map_or("src".to_owned(), |at| f.path[..at + 4].to_owned());
                    let module = spec.trim_start_matches("crate::").trim_start_matches("super::").split("::").next().unwrap_or_default();
                    let key = format!("{src}/{module}");
                    by_suffix.get(&key).cloned().unwrap_or_default()
                }
                _ => Vec::new(),
            };
            for t in targets {
                if t != i && by_path.contains_key(idx.files[t].path.as_str()) {
                    g.entry(i).or_default().push(t);
                }
            }
        }
    }
    g
}

fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    out.join("/")
}

/// Pairs of files that changed in the same commit, over the last `n` commits.
pub fn cochanges(repo: &Repo, n: usize) -> HashMap<String, HashMap<String, u32>> {
    let log = repo.git.run(&["log", &format!("-n{n}"), "--name-only", "--format=%x00", "--no-merges"]).unwrap_or_default();
    let mut pairs: HashMap<String, HashMap<String, u32>> = HashMap::new();
    for commit in log.split('\0') {
        let files: Vec<&str> = commit.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if files.len() > 30 {
            continue; // sweeping commits say nothing about what belongs together
        }
        for a in &files {
            for b in &files {
                if a != b {
                    *pairs.entry((*a).to_owned()).or_default().entry((*b).to_owned()).or_default() += 1;
                }
            }
        }
    }
    pairs
}

pub fn compile_with(idx: &Index, card: &Card, cochange: &HashMap<String, HashMap<String, u32>>, budget: usize) -> Brief {
    let mut text = format!("{}\n{}", card.title, card.body);
    for q in &card.questions {
        text.push_str(&format!("\n{} {}", q.text, q.answer.as_deref().unwrap_or_default()));
    }
    let query: Vec<String> = {
        let mut seen = HashSet::new();
        words(&text).into_iter().filter(|w| seen.insert(w.clone())).collect()
    };
    let named = mentions(&text);
    let docs: Vec<HashMap<String, f32>> = idx.files.iter().map(doc_terms).collect();
    let n = docs.len().max(1) as f32;
    let avg_len = docs.iter().map(|d| d.values().sum::<f32>()).sum::<f32>() / n;
    let df = |t: &str| docs.iter().filter(|d| d.contains_key(t)).count() as f32;
    let idf: HashMap<&str, f32> = query.iter().map(|t| (t.as_str(), ((n - df(t) + 0.5) / (df(t) + 0.5) + 1.0).ln())).collect();

    let mut scores: Vec<(f32, Vec<String>)> = vec![(0.0, Vec::new()); idx.files.len()];
    for (i, (d, f)) in docs.iter().zip(&idx.files).enumerate() {
        let len = d.values().sum::<f32>();
        let (k1, b) = (1.2, 0.75);
        for t in &query {
            if let Some(&tf) = d.get(t) {
                scores[i].0 += idf[t.as_str()] * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * len / avg_len.max(1.0)));
                scores[i].1.push(t.clone());
            }
        }
        for m in &named {
            if f.path == *m || f.path.ends_with(&format!("/{m}")) || (m.contains('/') && f.path.contains(m.as_str())) {
                scores[i].0 += 25.0;
                scores[i].1.insert(0, format!("named {m}"));
            } else if f.symbols.iter().any(|s| s.name == *m) {
                scores[i].0 += 12.0;
                scores[i].1.insert(0, format!("defines {m}"));
            }
        }
        if is_test(&f.path) {
            scores[i].0 *= 0.5; // tests are listed apart
        }
    }

    // Neighbours of the best files: what they import, what imports them, what changes with them.
    let graph = import_graph(idx);
    let mut order: Vec<usize> = (0..idx.files.len()).filter(|&i| scores[i].0 > 0.0).collect();
    order.sort_by(|&a, &b| scores[b].0.total_cmp(&scores[a].0).then(idx.files[a].path.cmp(&idx.files[b].path)));
    let seeds: Vec<(usize, f32)> = order.iter().take(6).map(|&i| (i, scores[i].0)).collect();
    let by_path: HashMap<&str, usize> = idx.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
    for &(s, sc) in &seeds {
        let name = idx.files[s].path.clone();
        for &t in graph.get(&s).into_iter().flatten() {
            scores[t].0 += sc * 0.25;
            scores[t].1.push(format!("imported by {}", stem_of(&name)));
        }
        for (from, tos) in &graph {
            if tos.contains(&s) {
                scores[*from].0 += sc * 0.2;
                scores[*from].1.push(format!("imports {}", stem_of(&name)));
            }
        }
        if let Some(co) = cochange.get(&name) {
            for (other, count) in co {
                if let Some(&j) = by_path.get(other.as_str())
                    && *count >= 2
                {
                    scores[j].0 += sc * 0.1 * (*count as f32).min(5.0);
                    scores[j].1.push(format!("changes with {}", stem_of(&name)));
                }
            }
        }
    }

    let mut ranked: Vec<usize> = (0..idx.files.len()).filter(|&i| scores[i].0 > 0.0 && !is_test(&idx.files[i].path)).collect();
    ranked.sort_by(|&a, &b| scores[b].0.total_cmp(&scores[a].0).then(idx.files[a].path.cmp(&idx.files[b].path)));
    let top = ranked.first().map_or(0.0, |&i| scores[i].0);
    let picked: Vec<usize> = ranked.into_iter().filter(|&i| scores[i].0 >= top * 0.15).take(MAX_FILES).collect();
    if picked.is_empty() {
        return Brief { stats: BriefStats { indexed: idx.files.len(), ..Default::default() }, ..Default::default() };
    }

    // Tests that go with the picked files, by name, in the order of the files they test.
    // An index, mod or main file is named by its folder: `index.test.ts` alone says nothing.
    let stems: Vec<&str> = picked
        .iter()
        .map(|&i| {
            let p = idx.files[i].path.as_str();
            let s = stem_of(p);
            if matches!(s, "index" | "mod" | "main" | "lib" | "__init__") { p.rsplit('/').nth(1).unwrap_or(s) } else { s }
        })
        .filter(|s| s.len() >= 3)
        .collect();
    let rank = |path: &str| -> Option<usize> {
        let s = stem_of(path).trim_start_matches("test_");
        stems.iter().position(|st| *st == s || path.contains(&format!("/{st}.")) || path.contains(&format!("/{st}/")))
    };
    let mut ranked_tests: Vec<(usize, String)> = idx.files.iter().filter(|f| is_test(&f.path)).filter_map(|f| rank(&f.path).map(|r| (r, f.path.clone()))).collect();
    ranked_tests.sort();
    let mut tests: Vec<String> = ranked_tests.into_iter().map(|(_, p)| p).collect();
    tests.truncate(8);

    let query_set: HashSet<&str> = query.iter().map(String::as_str).collect();
    let lean = budget <= LEAN_BUDGET;
    let mut out = String::from(if lean {
        "## Where to look\n\nLikely files for this card, from Pitlane's index (a map, not the answer):\n"
    } else {
        "## Where to look\n\nFrom Pitlane's index of this repository: the files most likely to matter for this card, most relevant first, \
with their symbols (line numbers from the main checkout; your branch may differ slightly). It is a map, not the answer: open what you need and look further when it misses.\n"
    });
    let mut files = Vec::new();
    for &i in &picked {
        let f = &idx.files[i];
        let mut why: Vec<String> = Vec::new();
        for w in &scores[i].1 {
            if !why.contains(w) {
                why.push(w.clone());
            }
        }
        why.truncate(5);
        if lean {
            // One line per file: the path and the symbols the card's words name, with lines.
            let named: Vec<String> = f
                .symbols
                .iter()
                .filter(|s| words(&s.name).iter().any(|w| query_set.contains(w.as_str())))
                .take(4)
                .map(|s| format!("{} L{}", s.name, s.line))
                .collect();
            let line = if named.is_empty() { format!("- {}\n", f.path) } else { format!("- {}: {}\n", f.path, named.join(", ")) };
            if out.len() + line.len() > budget && !files.is_empty() {
                break;
            }
            out.push_str(&line);
            files.push(BriefFile { path: f.path.clone(), score: (scores[i].0 * 100.0).round() / 100.0, why });
            continue;
        }
        let mut section = format!("\n### {} ({} lines; {})\n", f.path, f.lines, why.join(", "));
        for s in pick_symbols(&f.symbols, &query_set) {
            let indent = if s.parent.is_some() { "  " } else { "" };
            section.push_str(&format!("{indent}- L{} {}\n", s.line, s.sig));
        }
        if f.symbols.len() > MAX_SYMBOLS {
            section.push_str(&format!("- … {} more symbols\n", f.symbols.len() - MAX_SYMBOLS));
        }
        if out.len() + section.len() > budget && !files.is_empty() {
            break;
        }
        out.push_str(&section);
        files.push(BriefFile { path: f.path.clone(), score: (scores[i].0 * 100.0).round() / 100.0, why });
    }
    if !tests.is_empty() {
        let t = if lean {
            format!("Tests nearby: {}\n", tests.join(", "))
        } else {
            format!("\n### Tests nearby\n{}", tests.iter().map(|t| format!("- {t}\n")).collect::<String>())
        };
        if out.len() + t.len() <= budget + 600 {
            out.push_str(&t);
        }
    }
    let chars = out.len();
    Brief { text: out, stats: BriefStats { files: files.len(), chars, ms: 0, indexed: idx.files.len(), paths: files.iter().map(|f: &BriefFile| f.path.clone()).collect() }, files, tests, index: IndexStats::default() }
}

/// The symbols worth listing: all of a small file; for a big one, those the card names first, then
/// exported ones, in file order.
fn pick_symbols<'a>(all: &'a [Symbol], query: &HashSet<&str>) -> Vec<&'a Symbol> {
    if all.len() <= MAX_SYMBOLS {
        return all.iter().collect();
    }
    let hits = |s: &Symbol| words(&s.name).iter().any(|w| query.contains(w.as_str()));
    let mut keep: Vec<&Symbol> = all.iter().filter(|s| hits(s)).collect();
    for s in all.iter().filter(|s| s.exported && s.parent.is_none()) {
        if keep.len() >= MAX_SYMBOLS {
            break;
        }
        if !keep.iter().any(|k| std::ptr::eq(*k, s)) {
            keep.push(s);
        }
    }
    keep.truncate(MAX_SYMBOLS);
    keep.sort_by_key(|s| s.line);
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, syms: &[(&str, bool)], imports: &[&str]) -> FileEntry {
        FileEntry {
            path: path.into(),
            blob: String::new(),
            lang: Lang::of(path).unwrap(),
            lines: 40,
            symbols: syms
                .iter()
                .enumerate()
                .map(|(i, (n, e))| Symbol { name: (*n).into(), kind: "fn".into(), line: i as u32 + 1, end: i as u32 + 1, sig: format!("function {n}()"), exported: *e, parent: None })
                .collect(),
            imports: imports.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn splits_words() {
        assert_eq!(words("laneDiff lane_diff HTTPServer shipping Lanes"), ["lane", "diff", "lane", "diff", "http", "server", "ship", "lane"]);
        assert!(mentions("Fix `laneDiff` in src/lanes.tsx and SHIP now").contains("laneDiff"));
        assert!(mentions("Fix `laneDiff` in src/lanes.tsx.").contains("src/lanes.tsx"));
        assert!(!mentions("The Board").contains("Board"));
        // Words that start with a non-ASCII letter (Âmbar, Été) never split a char.
        assert!(!mentions("Maré de Âmbar, Éclair_x e Ôla").contains("Âmbar"));
    }

    #[test]
    fn ranks_by_words_mentions_and_imports() {
        let idx = Index {
            files: vec![
                file("src/kart/drift.ts", &[("driftAngle", true), ("applyDrift", true)], &["./physics"]),
                file("src/kart/physics.ts", &[("integrate", true)], &[]),
                file("src/lobby/timer.ts", &[("countdown", true)], &[]),
                file("src/kart/drift.test.ts", &[], &["./drift"]),
                file("src/ui/avatar.tsx", &[("Avatar", true)], &[]),
            ],
            ..Default::default()
        };
        let card = Card { title: "Drift tier 3 for the kart".into(), body: "Raise the drift angle cap.".into(), ..Default::default() };
        let b = compile_with(&idx, &card, &HashMap::new(), DEFAULT_BUDGET);
        let paths: Vec<&str> = b.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths[0], "src/kart/drift.ts");
        assert!(paths.contains(&"src/kart/physics.ts"), "the import neighbour comes along: {paths:?}");
        assert!(!paths.contains(&"src/lobby/timer.ts"));
        assert_eq!(b.tests, ["src/kart/drift.test.ts"]);
        assert!(b.text.contains("- L1 function driftAngle()"));

        let named = Card { title: "Avatar".into(), body: "See `countdown`.".into(), ..Default::default() };
        let b = compile_with(&idx, &named, &HashMap::new(), DEFAULT_BUDGET);
        assert!(b.files.iter().any(|f| f.path == "src/lobby/timer.ts" && f.why[0] == "defines countdown"), "{:?}", b.files);

        let nothing = Card { title: "Quarterly taxes".into(), ..Default::default() };
        assert!(compile_with(&idx, &nothing, &HashMap::new(), DEFAULT_BUDGET).text.is_empty());
    }
}
