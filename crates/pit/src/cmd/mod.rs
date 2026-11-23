//! What each subcommand does, grouped by what it works on.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use pit_core::runs::Client;
use serde::Serialize;

pub mod accounts;
pub mod cards;
pub mod lanes;
pub mod review;
pub mod runs;
mod yard;

/// Pitlane's own home (`$PITLANE_HOME`, else ~/.pitlane).
pub fn pitlane_home() -> PathBuf {
    pit_core::default_homes().1
}

/// Print `v` as pretty JSON, the output of every `--json`.
pub fn print_json(v: &impl Serialize) -> Result<ExitCode> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(ExitCode::SUCCESS)
}

pub fn daemon() -> Result<Client> {
    Client::connect(&pitlane_home(), std::env::current_exe().ok().as_deref())
}
