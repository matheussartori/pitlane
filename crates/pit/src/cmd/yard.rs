//! The yard table: every lane in columns that fit the terminal.

use pit_core::lanes::{self, Lane, Status, Sync, Yard};

use crate::term::{ACCENT, BOLD, DONE, FAIL, MUTED, RUN, paint, term_width};

pub fn print_yard(y: &Yard) {
    let stage = match &y.integration {
        Some(i) => format!("{} → {}", i, y.production),
        None => y.production.clone(),
    };
    let mut head = vec![y.repo.clone(), stage];
    head.push(if y.fetched { "fetched".into() } else { "not fetched".into() });
    if y.github {
        head.push("GitHub PRs".into());
    }
    println!("{}\n", paint(MUTED, &head.join("  ·  ")));

    if y.lanes.is_empty() {
        println!("no lanes. Start one: pit new feat/<thing>");
    } else {
        let integ = y.integration.as_deref();
        let mut rows: Vec<[Cell; 9]> = vec![[
            Cell::plain("#"),
            Cell::plain("LANE"),
            Cell::plain("WORKTREE"),
            Cell::plain(&integ.unwrap_or("").to_uppercase()),
            Cell::plain(&y.production.to_uppercase()),
            Cell::plain("REMOTE"),
            Cell::plain("IDLE"),
            Cell::plain("STATUS"),
            Cell::plain("NEXT"),
        ]];
        let header_len = rows.len();
        let build = |compact: bool| -> Vec<[Cell; 9]> { y.lanes.iter().map(|l| row(l, integ, compact)).collect() };
        rows.extend(build(false));
        let measure = |rows: &[[Cell; 9]]| -> Vec<usize> {
            (0..9).map(|c| rows.iter().map(|r| r[c].text.chars().count()).max().unwrap_or(0)).collect()
        };
        let mut shown = [true; 9];
        shown[3] = integ.is_some();
        let total = |w: &[usize], shown: &[bool; 9]| -> usize {
            let cols: Vec<usize> = (0..9).filter(|&c| shown[c]).map(|c| w[c]).collect();
            cols.iter().sum::<usize>() + 2 * cols.len().saturating_sub(1)
        };
        let mut widths = measure(&rows);
        // Narrow terminal: drop IDLE, then UNPUSHED, then shrink WORKTREE to its change count.
        if let Some(max) = term_width() {
            for col in [6, 5] {
                if total(&widths, &shown) > max {
                    shown[col] = false;
                }
            }
            if total(&widths, &shown) > max {
                rows.truncate(header_len);
                rows[0][2] = Cell::plain("WT");
                rows.extend(build(true));
                widths = measure(&rows);
            }
        }
        let last = (0..9).rev().find(|&c| shown[c]).unwrap_or(8);
        for (i, r) in rows.iter().enumerate() {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                if !shown[c] {
                    continue;
                }
                let pad = widths[c] - cell.text.chars().count();
                let text = if i == 0 { paint(MUTED, &cell.text) } else { cell.render() };
                if c == 0 {
                    line.push_str(&" ".repeat(pad));
                    line.push_str(&text);
                } else {
                    line.push_str(&text);
                    if c < last {
                        line.push_str(&" ".repeat(pad));
                    }
                }
                if c < last {
                    line.push_str("  ");
                }
            }
            println!("{}", line.trim_end());
        }
    }

    if !y.only_in_integration.is_empty() {
        let integ = y.integration.as_deref().unwrap_or("integration");
        println!(
            "\n{} only in {integ}: {} commit(s) made straight on {integ}. The flow never takes them to {}.",
            paint(ACCENT, "●"),
            y.only_in_integration.len(),
            y.production
        );
        for c in &y.only_in_integration {
            println!("  {}  {}  {}", c.short, c.subject, paint(MUTED, &format!("{} · {}d", c.author, c.days_ago)));
        }
    }
}

/// Where the lane stands against its branch on the remote.
fn remote_cell(l: &Lane) -> Cell {
    let at_risk = l.unpushed > 0 && l.in_production.is_none();
    let mut cell = match l.sync {
        Sync::Synced => Cell::new(DONE, "✓ synced"),
        Sync::Ahead { ahead } => Cell::plain(&format!("↑{ahead}")),
        Sync::Behind { behind } => Cell::new(ACCENT, &format!("↓{behind}")),
        Sync::Diverged { ahead, behind } => Cell::new(FAIL, &format!("↑{ahead} ↓{behind}")),
        Sync::LocalOnly if at_risk => Cell::new(ACCENT, &format!("local only, {}↑", l.unpushed)),
        Sync::LocalOnly => Cell::muted("local only"),
        Sync::Gone if at_risk => Cell::new(ACCENT, &format!("deleted on remote, {}↑", l.unpushed)),
        Sync::Gone => Cell::muted("deleted on remote"),
        Sync::RemoteOnly => Cell::muted("remote only"),
    };
    if let Some(other) = &l.tracks_other {
        cell = Cell::new(ACCENT, &format!("{} · tracks {other}", cell.text));
    }
    cell
}

fn row(l: &Lane, integ: Option<&str>, compact: bool) -> [Cell; 9] {
    let wt = match &l.worktree {
        None => Cell::muted("–"),
        Some(w) if compact => {
            if w.dirty > 0 { Cell::new(ACCENT, &format!("+{}", w.dirty)) } else { Cell::plain("✓") }
        }
        Some(w) => {
            let name = if w.is_main {
                "(main checkout)".to_owned()
            } else {
                w.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            };
            let mark = if w.is_current { "*" } else { "" };
            if w.dirty > 0 {
                Cell::new(ACCENT, &format!("{mark}{name} +{}", w.dirty))
            } else {
                Cell::plain(&format!("{mark}{name}"))
            }
        }
    };
    let landed = |x: Option<lanes::Landed>| match x {
        Some(h) => Cell::new(DONE, h.label()),
        None => Cell::muted("–"),
    };
    let unpushed = remote_cell(l);
    let (status, next) = match l.status {
        Status::Contaminated => (
            Cell::new(FAIL, &format!("✕ carries {} {} commit(s)", l.integration_commits.len(), integ.unwrap_or("integration"))),
            Cell::new(FAIL, "rebuild from base"),
        ),
        Status::CanDelete => (Cell::new(DONE, "✓ can delete"), Cell::plain("pit clean")),
        Status::Shipped => (Cell::new(DONE, "✓ shipped"), Cell::muted("–")),
        Status::ShippedDirty => (Cell::new(ACCENT, "● shipped, uncommitted"), Cell::new(ACCENT, "commit or discard")),
        Status::NeedsProduction => (Cell::new(ACCENT, "● needs ship"), Cell::new(ACCENT, "ship → main")),
        Status::InProgress => (
            Cell::new(RUN, "▸ in progress"),
            match (integ, l.worktree.as_ref().map_or(0, |w| w.dirty)) {
                _ if !l.local => Cell::plain("check out"),
                (Some(i), 0) if l.ahead > 0 => Cell::plain(&format!("ship → {i}")),
                _ => Cell::muted("–"),
            },
        ),
        Status::New => (Cell::muted("○ new"), Cell::muted("–")),
        Status::Stale => (Cell::muted("◌ stale"), Cell::muted("archive?")),
    };
    [
        Cell::muted(&l.number.to_string()),
        if l.status.needs_you() { Cell::bold(&l.branch) } else { Cell::plain(&l.branch) },
        wt,
        if integ.is_some() { landed(l.in_integration) } else { Cell::plain("") },
        landed(l.in_production),
        unpushed,
        Cell::muted(&format!("{}d", l.idle_days)),
        status,
        next,
    ]
}

struct Cell {
    text: String,
    style: Option<&'static str>,
}

impl Cell {
    fn new(style: &'static str, text: &str) -> Self {
        Self { text: text.to_owned(), style: Some(style) }
    }
    fn plain(text: &str) -> Self {
        Self { text: text.to_owned(), style: None }
    }
    fn muted(text: &str) -> Self {
        Self::new(MUTED, text)
    }
    fn bold(text: &str) -> Self {
        Self::new(BOLD, text)
    }
    fn render(&self) -> String {
        match self.style {
            Some(s) => paint(s, &self.text),
            None => self.text.clone(),
        }
    }
}
