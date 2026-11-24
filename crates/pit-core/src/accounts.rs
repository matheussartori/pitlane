//! Accounts and the route a run takes through them, from `~/.pitlane/routing.toml`.
//!
//! The rules come from the providers' terms (PLANO, "Roteador de contas e modelos"):
//! - as many accounts as you like, but moving between two subscription accounts of the same
//!   provider is always your hand (`pit account use`), never automatic and never suggested;
//! - the route may move on its own only between models, between providers, or to an API key;
//! - Pitlane never touches a credential: each account is a directory where *you* signed in to the
//!   official CLI, and Pitlane only points `CLAUDE_CONFIG_DIR` at it. An API key account names a
//!   command of yours that Claude Code itself runs (`apiKeyHelper`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::flow::glob_match;

pub const ROUTING_FILE: &str = "routing.toml";
/// Your normal Claude Code login: no `CLAUDE_CONFIG_DIR`. Always there.
pub const DEFAULT_ACCOUNT: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    #[default]
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Subscription,
    Team,
    ApiKey,
}

impl Kind {
    /// Paid per token: the route only takes it with your confirmation.
    pub fn is_api_key(self) -> bool {
        self == Self::ApiKey
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub kind: Kind,
    /// Runs at once on this account (ordinary, individual use). None: no limit.
    #[serde(default)]
    pub max_parallel: Option<u32>,
    /// Repositories that use this account unless you choose another (globs, `~` allowed).
    #[serde(default)]
    pub repos: Vec<String>,
    /// For `api-key` accounts: a command of yours that prints the key. Claude Code runs it.
    #[serde(default)]
    pub api_key_helper: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SwitchAt {
    /// Percent of the 5-hour window at which the route moves to its next option.
    pub five_hour: f64,
    pub seven_day: f64,
}

impl Default for SwitchAt {
    fn default() -> Self {
        Self { five_hour: 90.0, seven_day: 95.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Route {
    /// `model@account`; `@repo` is the account chosen for the repository; an empty model is the
    /// CLI's default.
    pub prefer: Vec<String>,
    pub switch_at: SwitchAt,
}

impl Default for Route {
    fn default() -> Self {
        Self { prefer: vec!["@repo".into()], switch_at: SwitchAt::default() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routes {
    pub run: Route,
}

/// The account you chose by hand for one repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoChoice {
    pub path: PathBuf,
    pub account: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routing {
    #[serde(rename = "account")]
    pub accounts: Vec<Account>,
    pub route: Routes,
    #[serde(rename = "repo")]
    pub repos: Vec<RepoChoice>,
}

/// One option of a route, resolved for a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub account: String,
    pub kind: Kind,
    pub model: Option<String>,
    /// `CLAUDE_CONFIG_DIR` for the agent; None for the default login.
    pub config_dir: Option<PathBuf>,
    pub api_key_helper: Option<String>,
}

impl Choice {
    /// `opus on default`, `the CLI's model on work`.
    pub fn label(&self) -> String {
        format!("{} on {}", self.model.as_deref().unwrap_or("the default model"), self.account)
    }
}

impl Routing {
    pub fn path(pitlane_home: &Path) -> PathBuf {
        pitlane_home.join(ROUTING_FILE)
    }

    /// Reads and validates the file; without one, the default account and the CLI's model.
    pub fn load(pitlane_home: &Path) -> Result<Self> {
        let path = Self::path(pitlane_home);
        match crate::fsutil::read_optional(&path)? {
            Some(t) => Self::from_text(&t).with_context(|| format!("in {}", path.display())),
            None => Self::from_text(""),
        }
    }

    /// Parses routing.toml's text, adds the default account when it is not listed, and validates.
    fn from_text(text: &str) -> Result<Self> {
        let mut r: Routing = toml::from_str(text).context("invalid routing")?;
        if !r.accounts.iter().any(|a| a.id == DEFAULT_ACCOUNT) {
            r.accounts.insert(0, Account {
                id: DEFAULT_ACCOUNT.into(),
                provider: Provider::Anthropic,
                kind: Kind::Subscription,
                max_parallel: None,
                repos: Vec::new(),
                api_key_helper: None,
            });
        }
        r.validate()?;
        Ok(r)
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for a in &self.accounts {
            if a.id.is_empty() || !a.id.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)) {
                bail!("account id {:?}: use letters, digits, - and _", a.id);
            }
            if !seen.insert(&a.id) {
                bail!("account {} is listed twice", a.id);
            }
            if a.kind.is_api_key() && a.api_key_helper.as_deref().is_none_or(|h| h.trim().is_empty()) {
                bail!("account {} is an api-key account: give it an api_key_helper, a command that prints the key", a.id);
            }
            if a.id == DEFAULT_ACCOUNT && a.kind.is_api_key() {
                bail!("the default account is your normal Claude Code login, not an API key");
            }
        }
        for r in &self.repos {
            if self.account(&r.account).is_none() {
                bail!("{} is set to account {}, which is not listed", r.path.display(), r.account);
            }
        }
        // The same-provider rule: within a route, every subscription option of a provider is the
        // same account, and that account is the repository's (`@repo`). Moving between your
        // subscription accounts is your decision, never the route's.
        let mut subs: HashMap<Provider, String> = HashMap::new();
        for entry in &self.route.run.prefer {
            let (_, account) = split_choice(entry)?;
            if account == "repo" {
                subs.entry(Provider::Anthropic).or_insert_with(|| "@repo".into());
                continue;
            }
            let a = self.account(account).with_context(|| format!("route option {entry:?} names account {account}, which is not listed"))?;
            if a.kind.is_api_key() {
                continue;
            }
            let slot = subs.entry(a.provider).or_insert_with(|| a.id.clone());
            if *slot != a.id {
                bail!(
                    "route option {entry:?}: the route already uses {slot} for this provider. Pitlane never moves between \
                     two subscription accounts on its own; list your subscription as @repo and switch accounts with \
                     pit account use"
                );
            }
        }
        Ok(())
    }

    /// The account this repository uses: chosen by hand, else the first whose `repos` match,
    /// else the default login.
    pub fn repo_account(&self, repo: &Path, home: &Path) -> String {
        if let Some(r) = self.repos.iter().find(|r| r.path == repo) {
            return r.account.clone();
        }
        let root = repo.to_string_lossy();
        self.accounts
            .iter()
            .find(|a| {
                a.repos.iter().any(|g| {
                    let g = g.strip_prefix("~/").map(|r| home.join(r).to_string_lossy().into_owned()).unwrap_or_else(|| g.clone());
                    glob_match(&g, &root)
                })
            })
            .map(|a| a.id.clone())
            .unwrap_or_else(|| DEFAULT_ACCOUNT.into())
    }

    /// The route's options, resolved for this repository, in order.
    pub fn choices(&self, repo: &Path, home: &Path, pitlane_home: &Path) -> Result<Vec<Choice>> {
        let repo_account = self.repo_account(repo, home);
        let mut out = Vec::new();
        for entry in &self.route.run.prefer {
            let (model, account) = split_choice(entry)?;
            let id = if account == "repo" { repo_account.clone() } else { account.to_owned() };
            let a = self.account(&id).with_context(|| format!("no account {id}"))?;
            out.push(Choice {
                account: a.id.clone(),
                kind: a.kind,
                model: model.map(str::to_owned),
                config_dir: profile_dir(&a.id, pitlane_home),
                api_key_helper: a.api_key_helper.clone(),
            });
        }
        Ok(out)
    }
}

/// `opus@work` → (Some("opus"), "work"); `@repo` → (None, "repo").
fn split_choice(entry: &str) -> Result<(Option<&str>, &str)> {
    let (model, account) = entry.rsplit_once('@').with_context(|| format!("route option {entry:?}: write it as model@account, or @repo"))?;
    if account.is_empty() {
        bail!("route option {entry:?} names no account");
    }
    Ok(((!model.is_empty()).then_some(model), account))
}

/// Where an account's own Claude Code configuration (and login) lives.
pub fn profile_dir(id: &str, pitlane_home: &Path) -> Option<PathBuf> {
    (id != DEFAULT_ACCOUNT).then(|| pitlane_home.join("profiles").join(id))
}

/// Whether the account is signed in, as the official CLI reports it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountStatus {
    pub logged_in: bool,
    pub method: Option<String>,
    pub email: Option<String>,
    /// The plan as the CLI names it (`max_20x`, `pro`), when it says.
    #[serde(default)]
    pub plan: Option<String>,
    /// The plan as the CLI shows it to people (`Ultra`, `Max`), when it says.
    #[serde(default)]
    pub plan_name: Option<String>,
    /// The organization the login belongs to.
    #[serde(default)]
    pub org: Option<String>,
    /// When the login has to be renewed (RFC 3339), when the CLI says.
    #[serde(default)]
    pub login_expires_at: Option<String>,
    /// Why it could not be told, when it could not.
    pub problem: Option<String>,
}

/// How long `claude auth status` may take: it reads a local file, and a start waits on it.
const STATUS_WITHIN: std::time::Duration = std::time::Duration::from_secs(15);

/// `claude auth status` with the account's `CLAUDE_CONFIG_DIR`: the CLI reads its own login, Pitlane
/// only reads the answer. API key accounts are "signed in" when they have a helper (Pitlane never
/// runs it).
pub fn status(account: &Account, pitlane_home: &Path, claude_bin: &str) -> AccountStatus {
    if account.kind.is_api_key() {
        return AccountStatus { logged_in: account.api_key_helper.is_some(), method: Some("api key helper".into()), ..Default::default() };
    }
    let mut cmd = Command::new(claude_bin);
    cmd.args(["auth", "status"]).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = profile_dir(&account.id, pitlane_home) {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    cmd.env_remove("CLAUDECODE");
    match crate::procs::output_within(&mut cmd, STATUS_WITHIN) {
        Ok(None) => AccountStatus { problem: Some(format!("claude auth status took longer than {} seconds", STATUS_WITHIN.as_secs())), ..Default::default() },
        Ok(Some(out)) => match serde_json::from_slice::<serde_json::Value>(&out) {
            Ok(v) => AccountStatus {
                plan_name: if v.get("loggedIn").and_then(|x| x.as_bool()).unwrap_or(false) { plan_name(account, pitlane_home, claude_bin) } else { None },
                logged_in: v.get("loggedIn").and_then(|x| x.as_bool()).unwrap_or(false),
                method: v.get("authMethod").and_then(|x| x.as_str()).filter(|m| *m != "none").map(str::to_owned),
                email: v.get("email").and_then(|x| x.as_str()).map(str::to_owned),
                plan: v.get("planTier").or_else(|| v.get("subscriptionType")).and_then(|x| x.as_str()).map(str::to_owned),
                org: v.get("orgName").and_then(|x| x.as_str()).map(str::to_owned),
                login_expires_at: v.get("loginExpiresAt").and_then(|x| x.as_str()).map(str::to_owned),
                problem: None,
            },
            Err(_) => AccountStatus { problem: Some("claude auth status gave no JSON".into()), ..Default::default() },
        },
        Err(e) => AccountStatus { problem: Some(format!("could not run {claude_bin}: {e}")), ..Default::default() },
    }
}

fn claude_for(account: &Account, pitlane_home: &Path, claude_bin: &str) -> Command {
    let mut cmd = Command::new(claude_bin);
    if let Some(dir) = profile_dir(&account.id, pitlane_home) {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    cmd.env_remove("CLAUDECODE");
    cmd
}

/// The plan's name as the CLI shows it (`Login method: Claude Ultra account` → `Ultra`): the JSON
/// only carries the tier (`max_20x`), which the plans no longer go by.
fn plan_name(account: &Account, pitlane_home: &Path, claude_bin: &str) -> Option<String> {
    let mut cmd = claude_for(account, pitlane_home, claude_bin);
    cmd.args(["auth", "status", "--text"]).stdin(Stdio::null()).stderr(Stdio::null());
    let out = crate::procs::output_within(&mut cmd, STATUS_WITHIN).ok()??;
    plan_name_in(&String::from_utf8_lossy(&out))
}

fn plan_name_in(text: &str) -> Option<String> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("Login method:"))?.trim();
    let name = line.strip_prefix("Claude ")?.strip_suffix(" account")?.trim();
    (!name.is_empty() && name != "API").then(|| name.to_owned())
}

/// One usage meter as Claude Code's /usage shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    /// The server's meter kind: `session` (the 5-hour window), `weekly_all`, `weekly_scoped`…
    pub kind: String,
    /// Percent used, 0 to 100.
    pub percent: f64,
    /// Unix seconds; 0 when not said.
    pub resets_at: u64,
    /// The model a scoped meter counts (`Fable`), when it is one.
    pub model: Option<String>,
}

/// What /usage would show for an account: its meters and whether extra usage is on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveUsage {
    pub limits: Vec<Limit>,
    /// Whether extra usage past the plan is turned on, when said.
    pub extra_usage: Option<bool>,
}

const USAGE_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

/// The account's plan meters, asked of the official CLI with its `get_usage` control request: the
/// CLI fetches them with its own login, Pitlane only reads the answer. No prompt is sent, so it
/// costs no usage; hooks and MCP servers stay off so asking starts nothing else.
pub fn live_usage(account: &Account, pitlane_home: &Path, claude_bin: &str) -> std::result::Result<LiveUsage, String> {
    if account.kind.is_api_key() {
        return Err("an API key has no plan limits".into());
    }
    let (reader, mut writer) = std::io::pipe().map_err(|e| e.to_string())?;
    {
        use std::io::Write as _;
        let ask = r#"{"type":"control_request","request_id":"usage","request":{"subtype":"get_usage","skip_behaviors":true}}"#;
        writeln!(writer, "{ask}").map_err(|e| e.to_string())?;
    }
    drop(writer);
    let mut cmd = claude_for(account, pitlane_home, claude_bin);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--settings", r#"{"disableAllHooks":true}"#])
        .current_dir(pitlane_home)
        .stdin(reader)
        .stderr(Stdio::null());
    let out = match crate::procs::output_within(&mut cmd, USAGE_WITHIN) {
        Ok(Some(out)) => out,
        Ok(None) => return Err(format!("{claude_bin} took longer than {} seconds", USAGE_WITHIN.as_secs())),
        Err(e) => return Err(format!("could not run {claude_bin}: {e}")),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["type"] == "control_response" && v["response"]["request_id"] == "usage")
        .ok_or_else(|| "Claude Code did not answer about usage".to_owned())
        .and_then(|v| usage_in(&v["response"]))
}

fn usage_in(response: &serde_json::Value) -> std::result::Result<LiveUsage, String> {
    if response["subtype"] != "success" {
        return Err(response["error"].as_str().unwrap_or("Claude Code could not read usage").to_owned());
    }
    let r = &response["response"]["rate_limits"];
    if r.is_null() {
        return Err("no plan limits for this login".into());
    }
    let at = |v: &serde_json::Value| v.as_str().and_then(unix_of).unwrap_or(0);
    let mut limits: Vec<Limit> = r["limits"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|l| {
                    Some(Limit {
                        kind: l["kind"].as_str()?.to_owned(),
                        percent: l["percent"].as_f64()?,
                        resets_at: at(&l["resets_at"]),
                        model: l["scope"]["model"]["display_name"].as_str().map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Older answers carry only the named windows.
    if limits.is_empty() {
        for (key, kind) in [("five_hour", "session"), ("seven_day", "weekly_all")] {
            if let Some(p) = r[key]["utilization"].as_f64() {
                limits.push(Limit { kind: kind.into(), percent: p, resets_at: at(&r[key]["resets_at"]), model: None });
            }
        }
    }
    Ok(LiveUsage { limits, extra_usage: r["extra_usage"]["is_enabled"].as_bool() })
}

/// Unix seconds of an RFC 3339 time (`2026-11-27T15:00:00.268613+00:00`, `…Z`).
fn unix_of(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        Some(b'+' | b'-') => {
            let h: i64 = rest.get(1..3)?.parse().ok()?;
            let m: i64 = rest.get(4..6)?.parse().ok()?;
            (h * 3600 + m * 60) * if rest.starts_with('-') { -1 } else { 1 }
        }
        _ => 0,
    };
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec - offset).ok()
}

/// Last usage seen on an account, percent of each window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Pick {
    Run { choice: Choice, reason: String },
    /// The next usable option is paid per token: it needs your yes.
    NeedsApiKey { choice: Choice, reason: String },
    Nothing { reason: String },
}

/// What the route looks at when it picks: usage per account, runs in flight, sign-in, your yes
/// to the API key, and accounts this card may not use again in this attempt (they hit a limit).
pub struct Situation<'a> {
    pub usage: &'a HashMap<String, Usage>,
    pub running: &'a HashMap<String, usize>,
    pub max_parallel: &'a HashMap<String, u32>,
    pub logged_in: &'a dyn Fn(&Choice) -> bool,
    pub allow_api_key: bool,
    pub skip: &'a [String],
}

/// The first option of the route that can run now. Every option passed over says why.
pub fn pick(choices: &[Choice], switch_at: &SwitchAt, s: &Situation) -> Pick {
    let mut passed: Vec<String> = Vec::new();
    for c in choices {
        let why_not = if s.skip.contains(&c.account) {
            Some(format!("{} hit its usage limit", c.account))
        } else if !(s.logged_in)(c) {
            Some(format!("{} is not signed in (pit account login {})", c.account, c.account))
        } else if let Some(u) = s.usage.get(&c.account).and_then(|u| over(u, switch_at)) {
            Some(format!("{} is at {u}", c.account))
        } else if let (Some(max), n) = (s.max_parallel.get(&c.account), s.running.get(&c.account).copied().unwrap_or(0))
            && n >= *max as usize
        {
            Some(format!("{} already runs {n} of its {max}", c.account))
        } else {
            None
        };
        if let Some(w) = why_not {
            passed.push(w);
            continue;
        }
        let reason = if passed.is_empty() {
            format!("{}: first in the route", c.label())
        } else {
            format!("{}: {}", c.label(), passed.join("; "))
        };
        if c.kind.is_api_key() && !s.allow_api_key {
            return Pick::NeedsApiKey { choice: c.clone(), reason };
        }
        return Pick::Run { choice: c.clone(), reason };
    }
    Pick::Nothing {
        reason: if passed.is_empty() { "the route has no options".into() } else { format!("nothing in the route can run now: {}", passed.join("; ")) },
    }
}

fn over(u: &Usage, at: &SwitchAt) -> Option<String> {
    if let Some(p) = u.five_hour.filter(|p| *p >= at.five_hour) {
        return Some(format!("{p:.0}% of its 5-hour window"));
    }
    u.seven_day.filter(|p| *p >= at.seven_day).map(|p| format!("{p:.0}% of its 7-day window"))
}

/// Sets the account of one repository in routing.toml, keeping the rest of the file as it is.
pub fn set_repo_account(pitlane_home: &Path, repo: &Path, account: &str) -> Result<()> {
    let r = Routing::load(pitlane_home)?;
    if r.account(account).is_none() {
        bail!("no account {account}; add it with pit account add {account}");
    }
    let key = repo.to_string_lossy().into_owned();
    crate::fsutil::edit_toml(
        &Routing::path(pitlane_home),
        "",
        |doc| {
            let repos = doc
                .entry("repo")
                .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                .as_array_of_tables_mut()
                .context("repo in routing.toml must be [[repo]] tables")?;
            if let Some(t) = repos.iter_mut().find(|t| t.get("path").and_then(|p| p.as_str()) == Some(key.as_str())) {
                t["account"] = toml_edit::value(account);
            } else {
                let mut t = toml_edit::Table::new();
                t["path"] = toml_edit::value(key.as_str());
                t["account"] = toml_edit::value(account);
                repos.push(t);
            }
            Ok(())
        },
        |text| Routing::from_text(text).map(drop),
    )
}

/// Adds an account to routing.toml and makes its profile directory. You sign in yourself, with the
/// official CLI (`pit account login <id>` opens it).
pub fn add_account(pitlane_home: &Path, id: &str, kind: Kind, api_key_helper: Option<&str>) -> Result<Option<PathBuf>> {
    crate::fsutil::edit_toml(
        &Routing::path(pitlane_home),
        "",
        |doc| {
            let accounts = doc
                .entry("account")
                .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                .as_array_of_tables_mut()
                .context("account in routing.toml must be [[account]] tables")?;
            if accounts.iter().any(|t| t.get("id").and_then(|v| v.as_str()) == Some(id)) {
                bail!("account {id} already exists");
            }
            let mut t = toml_edit::Table::new();
            t["id"] = toml_edit::value(id);
            t["provider"] = toml_edit::value("anthropic");
            t["kind"] = toml_edit::value(match kind {
                Kind::Subscription => "subscription",
                Kind::Team => "team",
                Kind::ApiKey => "api-key",
            });
            if let Some(h) = api_key_helper {
                t["api_key_helper"] = toml_edit::value(h);
            }
            accounts.push(t);
            Ok(())
        },
        |text| Routing::from_text(text).map(drop),
    )?;
    let dir = profile_dir(id, pitlane_home);
    if let Some(d) = &dir {
        std::fs::create_dir_all(d)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routing(toml_text: &str) -> Result<Routing> {
        let tmp = tempfile_dir();
        std::fs::write(tmp.join(ROUTING_FILE), toml_text).unwrap();
        Routing::load(&tmp)
    }

    fn tempfile_dir() -> PathBuf {
        // A counter, not the clock: parallel tests read the clock in the same microsecond.
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pit-accounts-{}-{n}-{}", std::process::id(), uuid_ish()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn uuid_ish() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    const FILE: &str = r#"
[[account]]
id = "work"
kind = "team"
repos = ["/work/**"]
max_parallel = 1

[[account]]
id = "api"
kind = "api-key"
api_key_helper = "security find-generic-password -s anthropic -w"

[route.run]
prefer = ["opus@repo", "sonnet@repo", "opus@api"]
"#;

    #[test]
    fn defaults_without_a_file() {
        let r = Routing::load(&tempfile_dir()).unwrap();
        assert_eq!(r.accounts.len(), 1);
        let c = r.choices(Path::new("/x"), Path::new("/home"), Path::new("/p")).unwrap();
        assert_eq!(c, vec![Choice { account: "default".into(), kind: Kind::Subscription, model: None, config_dir: None, api_key_helper: None }]);
    }

    #[test]
    fn the_same_provider_rule() {
        assert!(routing(FILE).is_ok());
        let two_subs = "[[account]]\nid = \"work\"\n[route.run]\nprefer = [\"opus@default\", \"opus@work\"]\n";
        let e = routing(two_subs).unwrap_err();
        assert!(format!("{e:#}").contains("never moves between two subscription accounts"), "{e:#}");
        let mixed = "[[account]]\nid = \"work\"\n[route.run]\nprefer = [\"opus@repo\", \"opus@work\"]\n";
        assert!(routing(mixed).is_err(), "@repo and another subscription in one route");
        assert!(routing("[[account]]\nid = \"k\"\nkind = \"api-key\"\n").is_err(), "api key without helper");
        assert!(routing("[route.run]\nprefer = [\"opus\"]\n").is_err(), "no @account");
    }

    #[test]
    fn repo_account_and_choices() {
        let r = routing(FILE).unwrap();
        let home = Path::new("/home/me");
        assert_eq!(r.repo_account(Path::new("/work/app"), home), "work");
        assert_eq!(r.repo_account(Path::new("/mine/app"), home), "default");
        let c = r.choices(Path::new("/work/app"), home, Path::new("/p")).unwrap();
        assert_eq!(c.iter().map(|c| c.label()).collect::<Vec<_>>(), ["opus on work", "sonnet on work", "opus on api"]);
        assert_eq!(c[0].config_dir.as_deref(), Some(Path::new("/p/profiles/work")));
    }

    #[test]
    fn picking_an_option() {
        let r = routing(FILE).unwrap();
        let c = r.choices(Path::new("/mine/app"), Path::new("/h"), Path::new("/p")).unwrap();
        let at = SwitchAt::default();
        let mut usage = HashMap::new();
        let none = HashMap::new();
        let maxp: HashMap<String, u32> = [("work".to_string(), 1)].into();
        let yes = |_: &Choice| true;

        let p = pick(&c, &at, &Situation { usage: &usage, running: &none, max_parallel: &maxp, logged_in: &yes, allow_api_key: false, skip: &[] });
        assert!(matches!(&p, Pick::Run { choice, reason } if choice.label() == "opus on default" && reason.contains("first")));

        usage.insert("default".into(), Usage { five_hour: Some(93.0), seven_day: Some(40.0) });
        let p = pick(&c, &at, &Situation { usage: &usage, running: &none, max_parallel: &maxp, logged_in: &yes, allow_api_key: false, skip: &[] });
        match p {
            Pick::NeedsApiKey { choice, reason } => {
                assert_eq!(choice.account, "api");
                assert!(reason.contains("93% of its 5-hour window"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(pick(&c, &at, &Situation { usage: &usage, running: &none, max_parallel: &maxp, logged_in: &yes, allow_api_key: true, skip: &[] }), Pick::Run { .. }));

        let skipped = pick(&c, &at, &Situation { usage: &HashMap::new(), running: &none, max_parallel: &maxp, logged_in: &yes, allow_api_key: false, skip: &["default".into()] });
        assert!(matches!(skipped, Pick::NeedsApiKey { .. }));
        let nobody = |_: &Choice| false;
        assert!(matches!(pick(&c, &at, &Situation { usage: &HashMap::new(), running: &none, max_parallel: &maxp, logged_in: &nobody, allow_api_key: true, skip: &[] }), Pick::Nothing { .. }));

        let w = r.choices(Path::new("/work/app"), Path::new("/h"), Path::new("/p")).unwrap();
        let busy: HashMap<String, usize> = [("work".to_string(), 1)].into();
        assert!(matches!(pick(&w, &at, &Situation { usage: &HashMap::new(), running: &busy, max_parallel: &maxp, logged_in: &yes, allow_api_key: false, skip: &[] }), Pick::NeedsApiKey { .. }));
    }

    #[test]
    fn editing_the_file() {
        let home = tempfile_dir();
        let dir = add_account(&home, "work", Kind::Subscription, None).unwrap().unwrap();
        assert!(dir.ends_with("profiles/work") && dir.exists());
        assert!(add_account(&home, "work", Kind::Subscription, None).is_err());
        assert!(add_account(&home, "k", Kind::ApiKey, None).is_err(), "helper required");
        set_repo_account(&home, Path::new("/x/repo"), "work").unwrap();
        set_repo_account(&home, Path::new("/x/repo"), "default").unwrap();
        let r = Routing::load(&home).unwrap();
        assert_eq!(r.repos, vec![RepoChoice { path: "/x/repo".into(), account: "default".into() }]);
        assert!(set_repo_account(&home, Path::new("/x/repo"), "nope").is_err());
    }

    #[test]
    fn reads_the_plan_name_the_cli_shows() {
        assert_eq!(plan_name_in("Login method: Claude Ultra account\nOrganization: x\n").as_deref(), Some("Ultra"));
        assert_eq!(plan_name_in("Login method: Claude Max account").as_deref(), Some("Max"));
        assert_eq!(plan_name_in("Login method: Claude API account"), None);
        assert_eq!(plan_name_in("Not logged in"), None);
    }

    #[test]
    fn reads_every_meter_get_usage_reports() {
        let answer = serde_json::json!({"subtype":"success","request_id":"usage","response":{"rate_limits":{
            "five_hour":null,
            "seven_day":{"utilization":60,"resets_at":"2026-11-27T15:00:00.268613+00:00"},
            "extra_usage":{"is_enabled":false},
            "limits":[
                {"kind":"weekly_all","group":"weekly","percent":60,"resets_at":"2026-11-27T15:00:00.268613+00:00","scope":null},
                {"kind":"weekly_scoped","group":"weekly","percent":32,"resets_at":"2026-11-27T15:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}
            ]}}});
        let u = usage_in(&answer).unwrap();
        assert_eq!(u.extra_usage, Some(false));
        assert_eq!(u.limits.len(), 2);
        assert_eq!(u.limits[0], Limit { kind: "weekly_all".into(), percent: 60.0, resets_at: 1795791600, model: None });
        assert_eq!(u.limits[1].model.as_deref(), Some("Fable"));
        // Without the rows, the named windows still count.
        let old = serde_json::json!({"subtype":"success","response":{"rate_limits":{"five_hour":{"utilization":14,"resets_at":"2026-11-24T18:00:00-03:00"}}}});
        assert_eq!(usage_in(&old).unwrap().limits, vec![Limit { kind: "session".into(), percent: 14.0, resets_at: 1795554000, model: None }]);
        assert!(usage_in(&serde_json::json!({"subtype":"success","response":{"rate_limits":null}})).is_err());
        assert!(usage_in(&serde_json::json!({"subtype":"error","error":"nope"})).is_err());
    }
}
