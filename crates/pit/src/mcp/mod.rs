//! `pit mcp`: a Model Context Protocol server over stdio, so an agent in the terminal (Claude
//! Code, Codex) creates, asks about and moves cards. JSON-RPC 2.0, one message per line.
//! Install: `claude mcp add pitlane -- pit mcp`.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pit_core::Repo;
use pit_core::cards::{self, Card, CardOut, CardPatch, CardState, MoveTo, NewCard, NewQuestion};
use serde_json::{Value, json};

/// Set by the daemon in a card run's environment (the run id): the board tools then hold back
/// what only the person or the daemon decides.
pub const RUN_ENV: &str = "PITLANE_RUN";

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Pitlane keeps this repository's board (cards and decisions) outside the repository, so read and change cards only through these tools. \
Use add_card for each piece of work. Before work starts, ask every question you have in one ask call, \
each with options and the one you recommend; the user answers them in Pitlane's Inbox and each answer \
becomes a recorded project decision that get_card and every run include. Read get_card before working on a card.";

mod approver;
mod board;

pub use approver::*;
pub use board::*;

/// The longest message either server reads: far past any real card or approval, short of one
/// line taking the memory.
const MAX_MESSAGE: u64 = 8 << 20;

/// The next non-empty line, or `None` at the end of input. A line past `MAX_MESSAGE` is skipped
/// whole, and bytes that are not UTF-8 are replaced rather than ending the server.
fn read_message(input: &mut impl BufRead) -> Result<Option<String>> {
    loop {
        let mut buf = Vec::new();
        let n = (&mut *input).take(MAX_MESSAGE).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if buf.last() != Some(&b'\n') && n as u64 == MAX_MESSAGE {
            // Too long: drop the rest of the line.
            let mut rest = Vec::new();
            while (&mut *input).take(MAX_MESSAGE).read_until(b'\n', &mut rest)? > 0 && rest.last() != Some(&b'\n') {
                rest.clear();
            }
            continue;
        }
        let line = String::from_utf8_lossy(&buf).trim().to_owned();
        if !line.is_empty() {
            return Ok(Some(line));
        }
    }
}

/// The answer to `initialize`: the protocol version asked for when it is one this server
/// speaks (the newest otherwise), tools, and who is answering.
fn initialize(params: &Value, name: &str, instructions: Option<&str>) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
    let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
    let mut out = json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": name, "version": env!("CARGO_PKG_VERSION") },
    });
    if let Some(i) = instructions {
        out["instructions"] = json!(i);
    }
    out
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
