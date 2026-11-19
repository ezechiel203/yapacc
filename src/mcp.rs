//! MCP (Model Context Protocol) server — yapacc for an AI client.
//!
//! `yapacc mcp` runs a stdio JSON-RPC 2.0 server (one message per line) any
//! MCP client (desktop assistants, editors, agents) can connect to. The
//! intelligence lives in the client; yapacc provides structured, gated
//! tools:
//!
//!   yapacc_search      the core's `--json search` (exact name first, then
//!                      the offline word expansion), plus the AUR on the
//!                      Arch family; `categories` and `kind` only filter
//!   yapacc_resolve     where a name would come from (`get --explain`)
//!   yapacc_plan        the same, optionally forced to one source
//!   yapacc_check / news
//!                      `--json check`; `--json news` (Arch only: refused
//!                      here, before anything is fetched, on other systems)
//!   yapacc_timeline / briefing / doctor / clean_report / restart_report /
//!   pending_configs / names / snapshots
//!                      read-only reports: the core binary's `--json`
//!                      output, unchanged (`read_argv` builds the command)
//!   yapacc_platform / capabilities / ledger / why
//!                      read-only, answered in this process from code that
//!                      is the same on every package manager
//!   yapacc_install / undo_last / upgrade / remove
//!                      SAFETY: by default these can only STAGE. The tool
//!                      runs the plan (`get --explain`, or `--dryrun`) and
//!                      returns it with the command the human must run in
//!                      their own terminal, where the normal interactive
//!                      gate asks them. `confirm=true` is an assertion made
//!                      by the model, so it is not consent: it is used only
//!                      when the machine owner opted in with `[mcp] apply =
//!                      "allow"` in a configuration file owned by root
//!                      (`apply_policy`). One code path, `run_staged`.
//!
//! One resource, `yapacc://docs/commands`, is the command reference.
//!
//! Everything that reaches the package manager goes through the core
//! binary, so the answers are the ones the command line gives on this
//! system, whatever its package manager.
//!
//! Every argument coming from the client is checked before a command line
//! exists (names, lanes, dates, integer ranges, booleans, search words);
//! each becomes its own argv entry — no shell is involved — and names and
//! the search query go after `--`. A refused argument runs nothing.
//!
//! What a change applied for an assistant leaves behind: the server sets
//! `YAPACC_VIA=mcp` on the core it runs for it, and with a package manager
//! other than pacman the core prints "requested via mcp" before applying.
//! Nothing is written to the ledger or to the package manager's log that
//! tells such a change from one typed by the human.
//!
//! No embedded LLM, no API keys, no connection to an AI provider. Tools
//! that can reach the network (package sources, news feeds) are marked
//! `openWorldHint` and say what they send.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const SERVER_NAME: &str = "yapacc";
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// The sources `from` may force, for `yapacc_plan` and `yapacc_install`
/// alike: the schema, the check and the error message all read this list.
/// Every entry is a lane `yapacc get --from` accepts (`tap` is known to the
/// resolver but refused by the core, so it is not offered).
const INSTALL_LANES: [&str; 5] = ["repo", "aur", "flatpak", "brew", "recipe"];

/// The largest `limit` the timeline tool accepts.
const TIMELINE_MAX_LIMIT: u64 = 500;
/// The most packages one `yapacc_remove` call may name.
const REMOVE_MAX_NAMES: usize = 64;
/// `yapacc_search`: results returned by default, and at most.
const SEARCH_DEFAULT_LIMIT: u64 = 20;
const SEARCH_MAX_LIMIT: u64 = 100;
/// What is asked of the core when `categories` or `kind` filter afterwards.
const SEARCH_FETCH: u64 = 300;
/// The longest search query, in bytes.
const QUERY_MAX_BYTES: usize = 200;
/// The most `categories` one search may name, and the longest of them.
const FACET_MAX_COUNT: usize = 16;
const FACET_MAX_BYTES: usize = 64;
/// `yapacc_news`: headlines returned by default, and at most.
const NEWS_DEFAULT_LIMIT: u64 = 5;
const NEWS_MAX_LIMIT: u64 = 50;

/// Annotations of a tool that changes nothing. `open_world`: it can reach
/// the network. Always written out: a client reads a missing
/// `openWorldHint` as true.
fn reads(open_world: bool) -> Value {
    json!({ "readOnlyHint": true, "openWorldHint": open_world })
}

/// Annotations of a tool that can change the system (when the owner lets it).
fn changes(open_world: bool) -> Value {
    json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false,
            "openWorldHint": open_world })
}

fn tools() -> Value {
    let no_args = || json!({ "type": "object", "properties": {} });
    let one_name = || {
        json!({ "type": "object", "properties": { "name": { "type": "string" } },
                "required": ["name"] })
    };
    let confirm_only = || {
        json!({ "type": "object",
                "properties": { "confirm": { "type": "boolean", "default": false } } })
    };
    json!([
        { "name": "yapacc_search", "annotations": reads(true),
          "description": "Search installable packages by name or description; an exact name comes first. With pacman a few plain words work too (\"draw in the terminal\"): when nothing matches directly they are expanded offline. On the Arch family the AUR is searched as well, which sends the query to aur.archlinux.org; nothing else leaves the machine. `categories` and `kind` only filter the results, and need pacman's data (categories such as development, network, multimedia, system, games, office; kind: gui-app | cli-tool | library). Returns JSON.",
          "inputSchema": { "type": "object", "properties": {
            "query": { "type": "string", "maxLength": QUERY_MAX_BYTES,
                       "description": "a name or a few words; no word may start with '-'" },
            "categories": { "type": "array", "items": { "type": "string" }, "maxItems": FACET_MAX_COUNT,
                            "description": "keep only results in one of these categories" },
            "kind": { "type": "string", "description": "keep only results of this kind" },
            "limit": { "type": "integer", "minimum": 1, "maximum": SEARCH_MAX_LIMIT,
                       "default": SEARCH_DEFAULT_LIMIT } },
            "required": ["query"] } },
        { "name": "yapacc_resolve", "annotations": reads(true),
          "description": "Where would this package come from: the decision table of `yapacc get --explain NAME` (the distribution's own package first, then the other sources this system has) and the source chosen. Changes nothing. The name is looked up in the sources that are online (AUR, Flathub), so it is sent to them.",
          "inputSchema": one_name() },
        { "name": "yapacc_plan", "annotations": reads(true),
          "description": "What installing this package would do: the same decision table as yapacc_resolve, optionally forced to one source with `from`. Changes nothing. The name is sent to the sources that are online (AUR, Flathub, Homebrew).",
          "inputSchema": { "type": "object", "properties": {
            "name": { "type": "string" },
            "from": { "type": "string", "enum": INSTALL_LANES } },
            "required": ["name"] } },
        { "name": "yapacc_install", "annotations": changes(true),
          "description": "Stage (and, only if the machine owner enabled it, apply) an install. By default this NEVER changes the system: it returns the plan and the exact command the human must run themselves. Do not tell the human it was installed unless the result starts with APPLIED. The name is sent to the sources that are online (AUR, Flathub, Homebrew).",
          "inputSchema": { "type": "object", "properties": {
            "name": { "type": "string" },
            "from": { "type": "string", "enum": INSTALL_LANES },
            "confirm": { "type": "boolean", "default": false } },
            "required": ["name"] } },
        { "name": "yapacc_capabilities", "annotations": reads(false),
          "description": "Which package formats and safety features (snapshots, pretest) this host supports (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_check", "annotations": reads(true),
          "description": "Pending updates, as of the last refresh of the package databases (JSON). Refreshes nothing. On the Arch family the names of the packages that are not from the repositories are sent to aur.archlinux.org to look for AUR updates.",
          "inputSchema": no_args() },
        { "name": "yapacc_news", "annotations": reads(true),
          "description": "Recent Arch Linux news headlines, to read before upgrading (JSON; fetched from archlinux.org). Arch only: on a system whose package manager is not pacman this returns an error and fetches nothing.",
          "inputSchema": { "type": "object", "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": NEWS_MAX_LIMIT,
                       "default": NEWS_DEFAULT_LIMIT } } } },
        { "name": "yapacc_ledger", "annotations": reads(false),
          "description": "What yapacc installed outside the package manager (AppImages, extracted foreign packages, recipes): the ledger records of the user and system scopes (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_platform", "annotations": reads(false),
          "description": "What kind of machine this is: distribution, native package manager, init, boot, log systems, snapshot providers, support tier (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_why", "annotations": reads(false),
          "description": "Why is this package here: install reason, what needs it, origin, verification (from the native package manager / flatpak), plus the yapacc ledger record when there is one (JSON).",
          "inputSchema": one_name() },
        { "name": "yapacc_timeline", "annotations": reads(false),
          "description": "What changed: package installs, upgrades and removals from every package manager's log, newest first (JSON).",
          "inputSchema": { "type": "object", "properties": {
            "since": { "type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "description": "only changes on or after this day (YYYY-MM-DD, UTC)" },
            "limit": { "type": "integer", "minimum": 1, "maximum": TIMELINE_MAX_LIMIT, "default": 50 },
            "since_boot": { "type": "boolean", "default": false, "description": "only changes since the last boot before the current one" } } } },
        { "name": "yapacc_briefing", "annotations": reads(true),
          "description": "What the next upgrade will do: kernel, reboot, security fixes, news, unmerged configuration, disk, safety net (JSON). On the Arch family this reaches the network: the Arch news feed is fetched unless offline=true, and `checkupdates`, when installed, asks the mirrors for the current package lists whatever `offline` says. Other distributions have no news feed yapacc reads.",
          "inputSchema": { "type": "object", "properties": { "offline": { "type": "boolean", "default": false } } } },
        { "name": "yapacc_doctor", "annotations": reads(false),
          "description": "What works on this machine: one entry per check with ok, warn, fail or n/a (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_clean_report", "annotations": reads(false),
          "description": "What can be reclaimed: orphans, package cache, unused runtimes, with the command for each (JSON). Reports only; removes nothing.",
          "inputSchema": no_args() },
        { "name": "yapacc_restart_report", "annotations": reads(false),
          "description": "What must be restarted after an upgrade: services and programs still using replaced files (JSON). Reports only; restarts nothing.",
          "inputSchema": no_args() },
        { "name": "yapacc_pending_configs", "annotations": reads(false),
          "description": "Configuration files left unmerged by upgrades (.pacnew, .dpkg-dist, .rpmnew...), with how to compare each (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_names", "annotations": reads(true),
          "description": "What a package is called on other distributions (JSON). This sends the package name to repology.org over the network; do not call it for a name the human would not want disclosed.",
          "inputSchema": one_name() },
        { "name": "yapacc_snapshots", "annotations": reads(false),
          "description": "Existing filesystem snapshots, newest first, from the tool this machine has (snapper, timeshift, btrfs, zfs) (JSON).",
          "inputSchema": no_args() },
        { "name": "yapacc_undo_last", "annotations": changes(true),
          "description": "Stage (and, only if the machine owner enabled it, apply) taking the last package transaction back. By default this NEVER changes the system: it returns the plan and the exact command the human must run themselves. Do not tell the human it was undone unless the result starts with APPLIED. Working out the plan can contact the distribution's package archive for the earlier versions.",
          "inputSchema": confirm_only() },
        { "name": "yapacc_upgrade", "annotations": changes(true),
          "description": "Stage (and, only if the machine owner enabled it, apply) a full system upgrade through the native package manager. By default this NEVER changes the system: it returns the plan and the exact command the human must run themselves. Do not tell the human it was upgraded unless the result starts with APPLIED. Applying downloads packages from the distribution's mirrors.",
          "inputSchema": confirm_only() },
        { "name": "yapacc_remove", "annotations": changes(false),
          "description": "Stage (and, only if the machine owner enabled it, apply) the removal of native packages. By default this NEVER changes the system: it returns the plan and the exact command the human must run themselves. Do not tell the human anything was removed unless the result starts with APPLIED.",
          "inputSchema": { "type": "object", "properties": {
            "names": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": REMOVE_MAX_NAMES },
            "confirm": { "type": "boolean", "default": false } },
            "required": ["names"] } }
    ])
}

/// The one resource: the command reference (`yapacc docs commands`).
pub const DOCS_COMMANDS_URI: &str = "yapacc://docs/commands";

fn resources() -> Value {
    json!([
        { "uri": DOCS_COMMANDS_URI, "name": "yapacc commands",
          "description": "Every yapacc command: the everyday ones and the specialist ones `--help` does not list.",
          "mimeType": "text/plain" }
    ])
}

fn text_result(s: String) -> Value {
    json!({ "content": [ { "type": "text", "text": s } ] })
}

/// A tool result that says the call did not do what was asked.
fn failed_result(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": true })
}

fn err_result(msg: &str) -> Value {
    failed_result(format!("error: {msg}"))
}

// ---------------------------------------------------------------------------
// the machine a call runs on
// ---------------------------------------------------------------------------

/// Whether a staged tool may apply.
#[derive(Clone, Debug, PartialEq)]
pub enum Apply {
    /// `[mcp] apply = "plan"`, the default: a tool only returns the plan.
    Plan,
    /// `[mcp] apply = "allow"`, from a file only root can write.
    Allow,
    /// `[mcp] apply = "allow"` from this file, which someone other than
    /// root can write: not honoured, the server stays in plan mode.
    Untrusted(String),
}

/// `[mcp] apply` as it is honoured. `file` is where the configuration was
/// read from, with its owner and mode.
///
/// The setting lets a program apply changes to the system on the word of a
/// model. A coding agent with file access can write the user's own
/// `~/.config/yapacc/config.toml`; if that were enough, an agent could give
/// itself the right and the promise "an assistant cannot approve its own
/// request" would hold only until the next server start. So `allow` counts
/// only from a file owned by root that nobody else can write.
pub fn apply_policy(setting: &str, file: Option<(&Path, u32, u32)>) -> Apply {
    if setting != "allow" {
        return Apply::Plan;
    }
    match file {
        Some((_, 0, mode)) if mode & 0o022 == 0 => Apply::Allow,
        Some((path, _, _)) => Apply::Untrusted(path.display().to_string()),
        None => Apply::Untrusted("the configuration".to_string()),
    }
}

/// What a tool call can reach: the core binary, what precedes every command
/// given to it, what the owner allows, and the package manager.
pub struct Host {
    core: PathBuf,
    /// arguments placed before every core command: the forwarded
    /// `--config PATH`, or nothing
    lead: Vec<String>,
    apply: Apply,
    /// pacman is this system's package manager (the AUR exists, and the
    /// Arch news are this system's news)
    pacman: bool,
}

impl Host {
    /// This machine, as configured. Call after `config::init`.
    fn current() -> Host {
        use std::os::unix::fs::MetadataExt;
        let file = crate::config::loaded_path()
            .and_then(|p| std::fs::metadata(p).ok().map(|m| (p, m.uid(), m.mode())));
        Host {
            core: crate::util::core_exe(),
            lead: with_config(CORE_CONFIG.get().map(String::as_str), &[]),
            apply: apply_policy(&crate::config::get().mcp.apply, file),
            pacman: crate::platform::native_manager().1
                == Some(crate::platform::PkgManager::Pacman),
        }
    }
}

fn host() -> &'static Host {
    static HOST: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
    HOST.get_or_init(Host::current)
}

/// The `--config` this server was started with, forwarded to every core run.
static CORE_CONFIG: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Check that `path` is an existing regular file and remember it so every
/// core invocation gets `--config PATH`. Call before `serve`.
pub fn set_core_config(path: &str) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("config {path}: not a regular file")),
        Err(e) => return Err(format!("cannot read config {path}: {e}")),
    }
    let _ = CORE_CONFIG.set(path.to_string());
    Ok(())
}

/// The core argv with the global `--config PATH` placed first (one argv
/// entry each, never through a shell) when a config was given.
fn with_config(config: Option<&str>, argv: &[String]) -> Vec<String> {
    let mut v = Vec::with_capacity(argv.len() + 2);
    if let Some(c) = config {
        v.push("--config".to_string());
        v.push(c.to_string());
    }
    v.extend(argv.iter().cloned());
    v
}

/// What one run of the core printed, and whether it succeeded.
struct CoreRun {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl CoreRun {
    /// Standard output, then standard error under a `[stderr]` line.
    fn text(&self) -> String {
        let mut s = self.stdout.clone();
        if !self.stderr.trim().is_empty() {
            s.push_str("\n[stderr]\n");
            s.push_str(&self.stderr);
        }
        s
    }

    fn result(&self) -> Value {
        if self.ok {
            text_result(self.text())
        } else {
            failed_result(self.text())
        }
    }
}

/// Run the core binary with `argv` (no shell; standard input closed, since
/// this process's own standard input is the protocol stream). `via` marks a
/// change applied for an assistant (see the module header for what the core
/// does with it).
fn core_run(host: &Host, argv: &[String], via: Option<&str>) -> CoreRun {
    let mut cmd = std::process::Command::new(&host.core);
    cmd.args(&host.lead)
        .args(argv)
        .stdin(std::process::Stdio::null())
        // this server printed the configuration warnings when it started
        .env(crate::config::WARNED_ENV, "1");
    if let Some(via) = via {
        cmd.env("YAPACC_VIA", via);
    }
    match cmd.output() {
        Ok(o) => CoreRun {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        },
        Err(e) => CoreRun {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to exec {:?}: {e}", host.core.display()),
        },
    }
}

// ---------------------------------------------------------------------------
// protocol
// ---------------------------------------------------------------------------

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const RESOURCE_NOT_FOUND: i32 = -32002;

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// The answer to one line of input: nothing (an empty line, a notification,
/// a response), one message, or one array for a batch. A line that is not
/// JSON, and a message that is not a request, are answered with an error
/// (`id: null` when none could be read) instead of being dropped, so that a
/// client never waits for a reply that will not come.
pub fn handle_line(host: &Host, line: &[u8]) -> Option<Value> {
    let Ok(text) = std::str::from_utf8(line) else {
        return Some(rpc_error(
            Value::Null,
            PARSE_ERROR,
            "parse error: the line is not valid UTF-8",
        ));
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: the line is not valid JSON ({e})"),
            ));
        }
    };
    match message {
        Value::Array(items) if items.is_empty() => Some(rpc_error(
            Value::Null,
            INVALID_REQUEST,
            "invalid request: an empty batch",
        )),
        // a batch: every request in it is answered, in one array
        Value::Array(items) => {
            let answers: Vec<Value> = items
                .iter()
                .filter_map(|m| handle_message(host, m))
                .collect();
            (!answers.is_empty()).then_some(Value::Array(answers))
        }
        one => handle_message(host, &one),
    }
}

/// Handle one JSON-RPC message on this machine → optional response.
pub fn handle_request(req: &Value) -> Option<Value> {
    handle_message(host(), req)
}

/// One message: a request is answered, a notification and a response are
/// not, anything else is an invalid request.
fn handle_message(host: &Host, msg: &Value) -> Option<Value> {
    let invalid = |id: Value, why: &str| {
        Some(rpc_error(
            id,
            INVALID_REQUEST,
            &format!("invalid request: {why}"),
        ))
    };
    let Some(obj) = msg.as_object() else {
        return invalid(Value::Null, "a message must be a JSON object");
    };
    // a response (to a request this server never sent): nothing to say,
    // whatever its id (an error response may carry a null one)
    if !obj.contains_key("method") && (obj.contains_key("result") || obj.contains_key("error")) {
        return None;
    }
    let id = match obj.get("id") {
        None => None,
        Some(v @ (Value::String(_) | Value::Number(_))) => Some(v.clone()),
        Some(_) => return invalid(Value::Null, "id must be a string or a number"),
    };
    let method = match obj.get("method") {
        Some(Value::String(m)) => m.as_str(),
        Some(_) => return invalid(id.unwrap_or(Value::Null), "method must be a string"),
        None => return invalid(id.unwrap_or(Value::Null), "no method"),
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid(id.unwrap_or(Value::Null), "jsonrpc must be \"2.0\"");
    }
    // a notification: never answered, and nothing is run for it
    let id = id?;
    let params = match obj.get("params") {
        None | Some(Value::Null) => None,
        Some(Value::Object(p)) => Some(p),
        Some(_) => {
            return Some(rpc_error(
                id,
                INVALID_PARAMS,
                "invalid params: params must be an object",
            ));
        }
    };
    let bad_params = |id: Value, why: &str| {
        Some(rpc_error(
            id,
            INVALID_PARAMS,
            &format!("invalid params: {why}"),
        ))
    };
    match method {
        // Version negotiation: this server knows one revision. Whatever the
        // client asks for (the same, a newer one, nothing), the answer is
        // the revision supported here; a client that cannot speak it
        // disconnects, as the specification says.
        "initialize" => Some(rpc_result(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {}, "resources": {} },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({ "tools": tools() }))),
        "resources/list" => Some(rpc_result(id, json!({ "resources": resources() }))),
        "resources/read" => {
            let Some(uri) = params.and_then(|p| p.get("uri")).and_then(Value::as_str) else {
                return bad_params(id, "resources/read needs a uri (a string)");
            };
            let text = if uri == DOCS_COMMANDS_URI {
                crate::docs::render("commands")
            } else {
                None
            };
            Some(match text {
                Some(text) => rpc_result(
                    id,
                    json!({ "contents": [
                        { "uri": DOCS_COMMANDS_URI, "mimeType": "text/plain", "text": text }
                    ] }),
                ),
                None => rpc_error(
                    id,
                    RESOURCE_NOT_FOUND,
                    &format!("resource not found: {}", shown(uri)),
                ),
            })
        }
        "tools/call" => {
            let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) else {
                return bad_params(id, "tools/call needs the name of a tool (a string)");
            };
            let empty = json!({});
            let args = match params.and_then(|p| p.get("arguments")) {
                None | Some(Value::Null) => &empty,
                Some(a @ Value::Object(_)) => a,
                Some(_) => return bad_params(id, "arguments must be an object"),
            };
            match dispatch_tool(host, name, args) {
                Some(result) => Some(rpc_result(id, result)),
                None => bad_params(id, &format!("unknown tool: {}", shown(name))),
            }
        }
        other => Some(rpc_error(
            id,
            METHOD_NOT_FOUND,
            &format!("method not found: {}", shown(other)),
        )),
    }
}

/// A value from the client as it is quoted back in a message: cut short,
/// and with nothing in it that would move the cursor or start a new line.
fn shown(s: &str) -> String {
    const MAX_CHARS: usize = 60;
    let mut out: String = s
        .chars()
        .take(MAX_CHARS)
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect();
    if s.chars().nth(MAX_CHARS).is_some() {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------------
// tools
// ---------------------------------------------------------------------------

/// The result of a tool call, or `None` when there is no tool of that name.
/// Arguments are checked first; a refused argument runs nothing.
fn dispatch_tool(host: &Host, name: &str, args: &Value) -> Option<Value> {
    // Reports answered by the core binary. The arguments are checked and
    // turned into an argument vector first; nothing runs if that fails.
    if let Some(argv) = read_argv(name, args) {
        return Some(match argv {
            Ok(argv) => core_run(host, &argv, None).result(),
            Err(e) => err_result(&e),
        });
    }
    if let Some(staged) = staged_call(name, args) {
        return Some(match staged {
            Ok(s) => run_staged(host, &s),
            Err(e) => err_result(&e),
        });
    }
    Some(match name {
        "yapacc_search" => match search_request(args, host.pacman) {
            Ok(req) => run_search(host, &req),
            Err(e) => err_result(&e),
        },
        "yapacc_news" => match news_argv(args) {
            Err(e) => err_result(&e),
            Ok(_) if !host.pacman => err_result(NEWS_IS_ARCH_ONLY),
            Ok(argv) => core_run(host, &argv, None).result(),
        },
        "yapacc_platform" => text_result(
            serde_json::to_string_pretty(&crate::platform::detect()).unwrap_or_default(),
        ),
        "yapacc_capabilities" => {
            let caps = crate::cross_format::detect_host_capabilities();
            let safety = crate::safety::detect_safety_capabilities();
            text_result(
                serde_json::to_string_pretty(&json!({
                    "capabilities": caps, "safety": safety
                }))
                .unwrap_or_default(),
            )
        }
        "yapacc_ledger" => ledger_records(),
        "yapacc_why" => {
            let name = match checked_name(args) {
                Ok(n) => n,
                Err(e) => return Some(err_result(&e)),
            };
            let report = crate::why::collect(&name);
            if !report.found() {
                return Some(text_result(format!(
                    "'{name}' is not installed as a native package, a flatpak, or a ledger-tracked install"
                )));
            }
            text_result(serde_json::to_string_pretty(&report).unwrap_or_default())
        }
        _ => return None,
    })
}

/// The ledger records of both scopes. A ledger that fails its integrity
/// check is reported as such, not skipped: its records are not to be relied
/// on. One that cannot be read for another reason (the system scope, as an
/// ordinary user) contributes nothing.
fn ledger_records() -> Value {
    let mut all = Vec::new();
    for scope in [&crate::ledger::Scope::User, &crate::ledger::Scope::System] {
        match crate::ledger::list_records(&crate::ledger::default_root(scope)) {
            Ok(recs) => all.extend(recs.into_iter().filter(|r| r.status != "removed")),
            Err(e) if e.kind == crate::Kind::Integrity => return err_result(&e.to_string()),
            Err(_) => {}
        }
    }
    text_result(serde_json::to_string_pretty(&all).unwrap_or_default())
}

const NEWS_IS_ARCH_ONLY: &str = "yapacc_news is Arch only: it reports Arch Linux news, and this system's package manager is not pacman. Nothing was fetched. yapacc reads no news feed for this distribution: its release notes say what an upgrade needs.";

/// The `name` argument: a string that is a valid package name. It ends up
/// on a command line, so it may not look like an option or contain anything
/// but package-name characters.
fn checked_name(args: &Value) -> Result<String, String> {
    match args.get("name") {
        Some(Value::String(n)) if crate::util::valid_pkg_name(n) => Ok(n.clone()),
        Some(Value::String(n)) => Err(format!("'{}' is not a valid package name", shown(n))),
        _ => Err("'name' must be a valid package name, written as a string".into()),
    }
}

/// Validate the `name`/`from` arguments of a tool call. Both end up on a
/// command line.
fn checked_target(args: &Value) -> Result<(String, Option<String>), String> {
    let name = checked_name(args)?;
    let from = match args.get("from") {
        None | Some(Value::Null) => None,
        Some(Value::String(f)) if INSTALL_LANES.contains(&f.as_str()) => Some(f.clone()),
        Some(Value::String(f)) => {
            return Err(format!(
                "unknown lane '{}' (expected one of: {})",
                shown(f),
                INSTALL_LANES.join(", ")
            ));
        }
        Some(_) => {
            return Err(format!(
                "'from' must be one of: {}",
                INSTALL_LANES.join(", ")
            ));
        }
    };
    Ok((name, from))
}

/// `yapacc get [--from LANE] FLAG -- NAME`
fn get_argv(name: &str, from: Option<&str>, flag: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["get".into()];
    if let Some(f) = from {
        argv.extend(["--from".into(), f.into()]);
    }
    argv.extend([flag.into(), "--".into(), name.into()]);
    argv
}

/// An optional boolean argument: absent or null is `false`; anything that
/// is not a boolean is refused rather than guessed at.
fn opt_bool(args: &Value, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("'{key}' must be true or false")),
    }
}

/// An optional integer argument within `min..=max`; absent or null is
/// `default`. A string, a fraction or a value out of range is refused.
fn opt_int(args: &Value, key: &str, min: u64, max: u64, default: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => match v.as_u64() {
            Some(n) if (min..=max).contains(&n) => Ok(n),
            _ => Err(format!("'{key}' must be an integer from {min} to {max}")),
        },
    }
}

/// `YYYY-MM-DD`, digits only, month 1..12, day 1..31.
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let ok = if i == 4 || i == 7 {
            *c == b'-'
        } else {
            c.is_ascii_digit()
        };
        if !ok {
            return false;
        }
    }
    // all ten bytes are ASCII from here on, so slicing by byte is safe
    let num = |from: usize, to: usize| s[from..to].parse::<u32>().unwrap_or(0);
    (1..=12).contains(&num(5, 7)) && (1..=31).contains(&num(8, 10))
}

/// `--json` followed by fixed words.
fn json_argv(words: &[&str]) -> Vec<String> {
    std::iter::once("--json")
        .chain(words.iter().copied())
        .map(String::from)
        .collect()
}

/// The core command line behind a read-only tool that is answered by one
/// core command, or `None` when `tool` is not one of them. Pure: nothing is
/// run here, so everything a client can put on a command line is decided
/// (and testable) in one place. Every value from the client is either
/// refused or becomes one argv entry that cannot be read as an option: a
/// checked date, a number formatted here, a fixed flag, a lane from a fixed
/// list, or a package name placed after `--`.
fn read_argv(tool: &str, args: &Value) -> Option<Result<Vec<String>, String>> {
    Some(match tool {
        "yapacc_timeline" => timeline_argv(args),
        "yapacc_briefing" => opt_bool(args, "offline").map(|offline| {
            let mut argv = json_argv(&["briefing"]);
            if offline {
                argv.push("--offline".into());
            }
            argv
        }),
        "yapacc_doctor" => Ok(json_argv(&["doctor"])),
        "yapacc_clean_report" => Ok(json_argv(&["clean"])),
        "yapacc_restart_report" => Ok(json_argv(&["restart"])),
        "yapacc_pending_configs" => Ok(json_argv(&["configs"])),
        "yapacc_snapshots" => Ok(json_argv(&["snapshot", "list"])),
        "yapacc_check" => Ok(json_argv(&["check"])),
        "yapacc_names" => checked_name(args).map(|name| json_argv(&["names", "--", &name])),
        // the decision table, as the command line prints it on this system
        "yapacc_resolve" => checked_name(args).map(|name| get_argv(&name, None, "--explain")),
        "yapacc_plan" => {
            checked_target(args).map(|(name, from)| get_argv(&name, from.as_deref(), "--explain"))
        }
        _ => return None,
    })
}

/// `yapacc --json timeline [--since S] [--limit N] [--since-boot]`
fn timeline_argv(args: &Value) -> Result<Vec<String>, String> {
    let mut argv = json_argv(&["timeline"]);
    match args.get("since") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if valid_date(s) => {
            argv.push("--since".into());
            argv.push(s.clone());
        }
        Some(_) => return Err("'since' must be a date written YYYY-MM-DD".into()),
    }
    match args.get("limit") {
        None | Some(Value::Null) => {}
        Some(_) => {
            let n = opt_int(args, "limit", 1, TIMELINE_MAX_LIMIT, 0)?;
            argv.push("--limit".into());
            argv.push(n.to_string());
        }
    }
    if opt_bool(args, "since_boot")? {
        argv.push("--since-boot".into());
    }
    Ok(argv)
}

/// `yapacc --json news --limit N`
fn news_argv(args: &Value) -> Result<Vec<String>, String> {
    let n = opt_int(args, "limit", 1, NEWS_MAX_LIMIT, NEWS_DEFAULT_LIMIT)?;
    let mut argv = json_argv(&["news", "--limit"]);
    argv.push(n.to_string());
    Ok(argv)
}

// ---------------------------------------------------------------- search

/// A checked search: the core command, and what is filtered afterwards.
#[derive(Debug, PartialEq)]
struct SearchRequest {
    argv: Vec<String>,
    categories: Vec<String>,
    kind: Option<String>,
    limit: usize,
}

/// The `query` argument. The core passes it to the package manager's search
/// — whole, and, when nothing matches, word by word — so no word of it may
/// start with `-` (it would be read as an option: `-y` refreshes pacman's
/// databases), and it may not carry control characters.
fn checked_query(v: Option<&Value>) -> Result<String, String> {
    let Some(Value::String(q)) = v else {
        return Err("'query' must be a string: a name or a few words".into());
    };
    let q = q.trim();
    if q.is_empty() {
        return Err("'query' is empty".into());
    }
    if q.len() > QUERY_MAX_BYTES {
        return Err(format!(
            "'query' is too long (at most {QUERY_MAX_BYTES} bytes)"
        ));
    }
    if q.chars().any(char::is_control) {
        return Err("'query' contains a control character".into());
    }
    if let Some(word) = q.split_whitespace().find(|w| w.starts_with('-')) {
        return Err(format!(
            "'query': the word '{}' starts with '-', which a package manager would read as an option",
            shown(word)
        ));
    }
    Ok(q.to_string())
}

/// A `categories` entry or the `kind`: a short word to compare with, lower
/// case. It never reaches a command line.
fn facet_word(key: &str, v: &Value) -> Result<String, String> {
    match v.as_str().map(str::trim) {
        Some(w)
            if !w.is_empty() && w.len() <= FACET_MAX_BYTES && !w.chars().any(char::is_control) =>
        {
            Ok(w.to_lowercase())
        }
        _ => Err(format!(
            "'{key}' must be a short word (at most {FACET_MAX_BYTES} bytes)"
        )),
    }
}

/// `yapacc --json search --limit N [--all] -- QUERY`, with the filters.
/// `aur`: also search the AUR (the Arch family; elsewhere the core refuses
/// `--all`).
fn search_request(args: &Value, aur: bool) -> Result<SearchRequest, String> {
    let query = checked_query(args.get("query"))?;
    let limit = opt_int(args, "limit", 1, SEARCH_MAX_LIMIT, SEARCH_DEFAULT_LIMIT)?;
    let categories = match args.get("categories") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) if list.len() <= FACET_MAX_COUNT => list
            .iter()
            .map(|c| facet_word("categories", c))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(format!(
                "'categories' must be a list of at most {FACET_MAX_COUNT} words"
            ));
        }
    };
    let kind = match args.get("kind") {
        None | Some(Value::Null) => None,
        Some(k) => Some(facet_word("kind", k)?),
    };
    // a filter is applied to what the core returns, so more is asked for
    let fetch = if categories.is_empty() && kind.is_none() {
        limit
    } else {
        SEARCH_FETCH
    };
    let mut argv = json_argv(&["search", "--limit"]);
    argv.push(fetch.to_string());
    if aur {
        argv.push("--all".into());
    }
    argv.push("--".into());
    argv.push(query);
    Ok(SearchRequest {
        argv,
        categories,
        kind,
        limit: limit as usize,
    })
}

/// Keep the rows the filters ask for, in the core's order (exact name
/// first), then cut to the limit. The filters never add or reorder.
fn filter_search(rows: Vec<Value>, req: &SearchRequest) -> Result<Vec<Value>, String> {
    let filtering = !req.categories.is_empty() || req.kind.is_some();
    if filtering
        && rows
            .iter()
            .any(|r| !r["categories"].is_array() || !r["kind"].is_string())
    {
        return Err("'categories' and 'kind' cannot be applied here: this system's package search returns neither (they come from pacman's data). Call again without them.".into());
    }
    let keep = |row: &Value| -> bool {
        let in_category = req.categories.is_empty()
            || row["categories"].as_array().is_some_and(|cats| {
                cats.iter().filter_map(Value::as_str).any(|c| {
                    let c = c.to_lowercase();
                    req.categories.iter().any(|w| c.contains(w))
                })
            });
        let of_kind = req.kind.as_ref().is_none_or(|k| {
            row["kind"]
                .as_str()
                .is_some_and(|rk| rk.to_lowercase().contains(k))
        });
        in_category && of_kind
    };
    Ok(rows.into_iter().filter(keep).take(req.limit).collect())
}

fn run_search(host: &Host, req: &SearchRequest) -> Value {
    let run = core_run(host, &req.argv, None);
    if !run.ok {
        return failed_result(run.text());
    }
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(run.stdout.trim()) else {
        return failed_result(format!(
            "the search did not return a list. What the core said:\n\n{}",
            run.text()
        ));
    };
    match filter_search(rows, req) {
        Ok(rows) => {
            let mut text = serde_json::to_string_pretty(&rows).unwrap_or_default();
            if !run.stderr.trim().is_empty() {
                text.push_str("\n[stderr]\n");
                text.push_str(&run.stderr);
            }
            text_result(text)
        }
        Err(e) => err_result(&e),
    }
}

// ---------------------------------------------------------------- staged

/// A change an assistant may stage: the command that only plans, the
/// command that applies, and the line a human types to do it themselves.
#[derive(Debug, PartialEq)]
struct Staged {
    plan: Vec<String>,
    apply: Vec<String>,
    human: String,
    confirm: bool,
}

/// The staged form of a change tool, or `None` when `tool` is not one of
/// them. Pure, like `read_argv`: validation happens before anything runs.
fn staged_call(tool: &str, args: &Value) -> Option<Result<Staged, String>> {
    let verb: &[&str] = match tool {
        "yapacc_install" => return Some(staged_install(args)),
        "yapacc_undo_last" => &["undo"],
        "yapacc_upgrade" => &["upgrade"],
        "yapacc_remove" => &["remove", "repo"],
        _ => return None,
    };
    Some(staged_parts(verb, tool == "yapacc_remove", args))
}

/// `yapacc get`: the plan is the decision table (`--explain`), which never
/// installs; applying runs the same command without asking again.
fn staged_install(args: &Value) -> Result<Staged, String> {
    let (name, from) = checked_target(args)?;
    let confirm = opt_bool(args, "confirm")?;
    let from = from.as_deref();
    Ok(Staged {
        plan: get_argv(&name, from, "--explain"),
        apply: get_argv(&name, from, "--noconfirm"),
        human: format!(
            "yapacc get {}{name}",
            from.map(|f| format!("--from {f} ")).unwrap_or_default()
        ),
        confirm,
    })
}

fn staged_parts(verb: &[&str], takes_names: bool, args: &Value) -> Result<Staged, String> {
    let names = if takes_names {
        checked_names(args)?
    } else {
        Vec::new()
    };
    let confirm = opt_bool(args, "confirm")?;
    let argv = |flag: &str| -> Vec<String> {
        let mut v: Vec<String> = verb.iter().map(|w| w.to_string()).collect();
        v.push(flag.to_string());
        if !names.is_empty() {
            v.push("--".to_string());
            v.extend(names.iter().cloned());
        }
        v
    };
    let mut human = format!("yapacc {}", verb.join(" "));
    for n in &names {
        human.push(' ');
        human.push_str(n);
    }
    Ok(Staged {
        plan: argv("--dryrun"),
        apply: argv("--yes"),
        human,
        confirm,
    })
}

/// The `names` argument of `yapacc_remove`: a non-empty array of package
/// names, every one valid.
fn checked_names(args: &Value) -> Result<Vec<String>, String> {
    let Some(list) = args.get("names").and_then(|n| n.as_array()) else {
        return Err("'names' must be a list of package names".into());
    };
    if list.is_empty() || list.len() > REMOVE_MAX_NAMES {
        return Err(format!(
            "'names' must hold between 1 and {REMOVE_MAX_NAMES} package names"
        ));
    }
    let mut names = Vec::with_capacity(list.len());
    for item in list {
        match item.as_str() {
            Some(n) if crate::util::valid_pkg_name(n) => names.push(n.to_string()),
            Some(n) => return Err(format!("'{}' is not a valid package name", shown(n))),
            None => return Err("'names' must be a list of package names".into()),
        }
    }
    Ok(names)
}

/// The one path every change tool takes. Four answers:
/// the plan could not be worked out (an error, and no command is handed
/// out); plan only (the default); plan only pending the human's approval;
/// applied — or failed — because the machine owner opted in.
fn run_staged(host: &Host, s: &Staged) -> Value {
    let plan = core_run(host, &s.plan, None);
    let human_cmd = &s.human;
    if !plan.ok {
        return failed_result(format!(
            "PLAN FAILED — nothing was changed.\nThe plan for `{human_cmd}` could not be worked out, so there is no command to give the human. What the core said:\n\n{}",
            plan.text()
        ));
    }
    let plan = plan.text();
    // The model cannot approve its own request: applying happens only in
    // the human's terminal, behind the interactive gate.
    let plan_only = |why: &str| {
        text_result(format!(
            "PLAN ONLY — nothing was changed.\n{why}\nShow the plan to the human; if they want it, THEY run this in their own terminal:\n\n    {human_cmd}\n\n{plan}"
        ))
    };
    match &host.apply {
        Apply::Plan => {
            return plan_only(
                "This machine does not let an assistant apply changes ([mcp] apply = \"plan\").",
            );
        }
        Apply::Untrusted(file) => {
            return plan_only(&format!(
                "This machine does not let an assistant apply changes: [mcp] apply = \"allow\" is set in {file}, which is not a file only root can write, so it is not honoured."
            ));
        }
        Apply::Allow => {}
    }
    if !s.confirm {
        return text_result(format!(
            "PLAN ONLY (confirm=false) — nothing was changed. Relay this plan to the human; call again with confirm=true only after they explicitly approve.\n\n{plan}"
        ));
    }
    // The owner opted in and the client asserts approval: apply through
    // the core. YAPACC_VIA is set for it (module header: what that does).
    let applied = core_run(host, &s.apply, Some("mcp"));
    let out = applied.text();
    if applied.ok {
        text_result(format!("APPLIED: {human_cmd}\n\n{out}"))
    } else {
        failed_result(format!(
            "FAILED: `{human_cmd}` did not complete. Do not report it as done; the output says how far it got.\n\n{out}"
        ))
    }
}

/// Stdio serve loop: one JSON-RPC message (or batch) per line.
pub fn serve() -> Result<(), String> {
    let host = host();
    if let Apply::Untrusted(file) = &host.apply {
        eprintln!(
            "warning: {file}: [mcp] apply = \"allow\" is not honoured, because the file is not owned by root or can be written by others. An assistant that can edit files must not be able to give itself this right. The server stays in plan mode; to allow applying, put the setting in a file only root can write and start yapacc-mcp with --config FILE."
        );
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut line = Vec::new();
    loop {
        line.clear();
        // bytes, not `lines()`: a line that is not UTF-8 is answered with a
        // parse error and the server goes on
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("reading standard input: {e}")),
        }
        if let Some(resp) = handle_line(host, &line)
            && writeln!(out, "{resp}").and_then(|()| out.flush()).is_err()
        {
            // the client closed its end: nobody is left to answer
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // A stand-in for the core: it writes its arguments to a log and prints
    // what the test prepared. Nothing in these tests runs a real yapacc,
    // a package manager, or touches the home directory.
    // ------------------------------------------------------------------

    struct Fake {
        dir: PathBuf,
        host: Host,
    }

    impl Fake {
        fn new(tag: &str) -> Fake {
            let dir = std::env::temp_dir().join(format!(
                "yapacc-mcp-unit-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let d = dir.display();
            let script = dir.join("core.sh");
            std::fs::write(
                &script,
                format!(
                    "{{ printf 'VIA=%s' \"${{YAPACC_VIA-}}\"; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; echo; }} >> '{d}/argv.log'\n\
                     if [ -f '{d}/stdout' ]; then cat '{d}/stdout'; else echo \"fake core ran: $*\"; fi\n\
                     if [ -f '{d}/stderr' ]; then cat '{d}/stderr' >&2; fi\n\
                     if [ -f '{d}/fail' ]; then exit 1; fi\n\
                     exit 0\n"
                ),
            )
            .unwrap();
            // the script is read by /bin/sh, never executed itself
            Fake {
                host: Host {
                    core: PathBuf::from("/bin/sh"),
                    lead: vec![script.display().to_string()],
                    apply: Apply::Plan,
                    pacman: true,
                },
                dir,
            }
        }

        fn allowing(tag: &str) -> Fake {
            let mut f = Fake::new(tag);
            f.host.apply = Apply::Allow;
            f
        }

        /// One line per run of the core: `VIA=… [arg] [arg]…`.
        fn log(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("argv.log"))
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        }

        fn stdout(&self, text: &str) {
            std::fs::write(self.dir.join("stdout"), text).unwrap();
        }

        fn fail(&self) {
            std::fs::write(self.dir.join("fail"), "").unwrap();
        }

        fn request(&self, req: Value) -> Option<Value> {
            handle_message(&self.host, &req)
        }

        /// The whole result of a tool call.
        fn result(&self, tool: &str, arguments: Value) -> Value {
            let resp = self
                .request(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": { "name": tool, "arguments": arguments } }))
                .unwrap();
            assert!(resp.get("error").is_none(), "{tool}: {resp}");
            resp["result"].clone()
        }

        /// (text, isError) of a tool call.
        fn call(&self, tool: &str, arguments: Value) -> (String, bool) {
            let r = self.result(tool, arguments);
            (
                r["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                r["isError"] == json!(true),
            )
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn tool_list() -> Vec<Value> {
        tools().as_array().unwrap().clone()
    }

    #[test]
    fn initialize_and_tools_list() {
        let f = Fake::new("init");
        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
            .unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "yapacc");
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);

        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 21, "tools: {names:?}");
        for expected in [
            "yapacc_search",
            "yapacc_resolve",
            "yapacc_plan",
            "yapacc_install",
            "yapacc_why",
        ] {
            assert!(names.contains(&expected), "tools: {names:?}");
        }
        assert!(f.log().is_empty(), "listing runs nothing");
    }

    /// Answered in this process, from this machine's own state (package
    /// database, ledger). Unit tests do not run them: `tests/mcp_stdio.rs`
    /// does, in a server whose home directory is a temporary one.
    const IN_PROCESS: [&str; 4] = [
        "yapacc_platform",
        "yapacc_capabilities",
        "yapacc_ledger",
        "yapacc_why",
    ];

    #[test]
    fn every_listed_tool_is_answered_and_no_other_name_is() {
        let f = Fake::new("dispatch");
        for tool in tool_list() {
            let name = tool["name"].as_str().unwrap();
            if IN_PROCESS.contains(&name) {
                continue;
            }
            assert!(
                dispatch_tool(&f.host, name, &json!({})).is_some(),
                "{name} is listed but not handled"
            );
        }
        // a name that fails its check is refused by the tool, in this process
        let refused = dispatch_tool(&f.host, "yapacc_why", &json!({ "name": "-y" })).unwrap();
        assert_eq!(refused["isError"], json!(true));
        for name in [
            "",
            "yapacc",
            "yapacc_",
            "yapacc_search ",
            "tools/list",
            "--yes",
        ] {
            assert!(
                dispatch_tool(&f.host, name, &json!({})).is_none(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn notifications_have_no_reply_and_run_nothing() {
        let f = Fake::new("notif");
        for req in [
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } }),
            json!({ "jsonrpc": "2.0", "method": "no/such/notification" }),
            // a tool call without an id cannot be answered, so it is not run
            json!({ "jsonrpc": "2.0", "method": "tools/call",
                    "params": { "name": "yapacc_upgrade", "arguments": { "confirm": true } } }),
        ] {
            assert!(f.request(req.clone()).is_none(), "{req}");
        }
        assert!(f.log().is_empty(), "{:?}", f.log());
    }

    #[test]
    fn unknown_method_is_error() {
        let f = Fake::new("method");
        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 9, "method": "bogus" }))
            .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["id"], 9);
    }

    // ------------------------------------------------------- protocol (23)

    fn line(f: &Fake, bytes: &[u8]) -> Option<Value> {
        handle_line(&f.host, bytes)
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error_with_a_null_id() {
        let f = Fake::new("parse");
        for bad in [
            &b"{not json"[..],
            b"}",
            b"nonsense",
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"",
            // not UTF-8 at all, and UTF-8 broken inside a string
            b"\xff\xfe\xfd",
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pi\xc3\x28ng\"}",
        ] {
            let resp = line(&f, bad).unwrap_or_else(|| panic!("{bad:?} was dropped"));
            assert_eq!(resp["error"]["code"], -32700, "{bad:?}: {resp}");
            assert_eq!(resp["id"], Value::Null, "{resp}");
            assert_eq!(resp["jsonrpc"], "2.0");
        }
        // an empty line is not a message
        for blank in [&b""[..], b"\n", b"  \r\n"] {
            assert!(line(&f, blank).is_none());
        }
        // CRLF and no final newline are fine
        let resp = line(
            &f,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\r\n",
        )
        .unwrap();
        assert_eq!(resp["result"], json!({}));
    }

    #[test]
    fn what_is_not_a_request_is_an_invalid_request() {
        let f = Fake::new("invalid");
        for (bad, id) in [
            (json!({ "id": 8 }), json!(8)),
            (json!({ "jsonrpc": "2.0", "id": 8 }), json!(8)),
            (
                json!({ "jsonrpc": "2.0", "id": "a", "method": 5 }),
                json!("a"),
            ),
            (
                json!({ "jsonrpc": "1.0", "id": 8, "method": "ping" }),
                json!(8),
            ),
            (json!({ "id": 8, "method": "ping" }), json!(8)),
            (
                json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }),
                Value::Null,
            ),
            (
                json!({ "jsonrpc": "2.0", "id": [1], "method": "ping" }),
                Value::Null,
            ),
            (
                json!({ "jsonrpc": "2.0", "id": {}, "method": "ping" }),
                Value::Null,
            ),
            (
                json!({ "jsonrpc": "2.0", "id": true, "method": "ping" }),
                Value::Null,
            ),
            (json!(7), Value::Null),
            (json!("ping"), Value::Null),
            (json!(null), Value::Null),
            (json!(true), Value::Null),
        ] {
            let resp = f
                .request(bad.clone())
                .unwrap_or_else(|| panic!("{bad} was dropped"));
            assert_eq!(resp["error"]["code"], -32600, "{bad}: {resp}");
            assert_eq!(resp["id"], id, "{bad}: {resp}");
        }
    }

    #[test]
    fn a_response_is_not_answered() {
        let f = Fake::new("response");
        for resp in [
            json!({ "jsonrpc": "2.0", "id": 16, "result": {} }),
            json!({ "id": 16, "result": {} }),
            json!({ "jsonrpc": "2.0", "id": 16, "error": { "code": -1, "message": "x" } }),
            json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "x" } }),
        ] {
            assert!(f.request(resp.clone()).is_none(), "{resp}");
        }
    }

    #[test]
    fn a_batch_is_answered_as_one_array() {
        let f = Fake::new("batch");
        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": 2, "method": "nope" },
            7,
            { "jsonrpc": "2.0", "id": 3, "method": "tools/call",
              "params": { "name": "yapacc_doctor" } }
        ]);
        let resp = line(&f, batch.to_string().as_bytes()).unwrap();
        let answers = resp.as_array().expect("an array for a batch");
        assert_eq!(answers.len(), 4, "{resp}");
        assert_eq!(answers[0]["id"], 1);
        assert_eq!(answers[0]["result"], json!({}));
        assert_eq!(answers[1]["error"]["code"], -32601);
        assert_eq!(answers[2]["error"]["code"], -32600);
        assert_eq!(answers[2]["id"], Value::Null);
        assert_eq!(answers[3]["id"], 3);
        assert!(answers[3]["result"]["content"][0]["text"].is_string());
        assert_eq!(f.log(), ["VIA= [--json] [doctor]"]);

        // an empty batch is an invalid request; one of notifications only
        // has nothing to answer
        let resp = line(&f, b"[]").unwrap();
        assert_eq!(resp["error"]["code"], -32600);
        assert_eq!(resp["id"], Value::Null);
        let only_notifications =
            json!([{ "jsonrpc": "2.0", "method": "notifications/initialized" }]);
        assert!(line(&f, only_notifications.to_string().as_bytes()).is_none());
    }

    #[test]
    fn a_bad_tool_call_is_invalid_params_and_runs_nothing() {
        let f = Fake::new("params");
        for params in [
            json!(null),
            json!({}),
            json!({ "name": 7 }),
            json!({ "name": "no_such_tool" }),
            json!({ "name": "yapacc_upgrade ", "arguments": {} }),
            json!({ "name": "yapacc_upgrade", "arguments": [] }),
            json!({ "name": "yapacc_upgrade", "arguments": "confirm" }),
            json!({ "name": "yapacc_upgrade", "arguments": 3 }),
            json!({ "name": "yapacc_doctor", "arguments": true }),
            json!("yapacc_doctor"),
            json!(["yapacc_doctor"]),
        ] {
            let mut req = json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call" });
            if !params.is_null() {
                req["params"] = params.clone();
            }
            let resp = f.request(req).unwrap();
            assert_eq!(resp["error"]["code"], -32602, "{params}: {resp}");
            assert_eq!(resp["id"], 5);
            assert!(resp.get("result").is_none(), "{resp}");
        }
        // a missing uri likewise
        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/read" }))
            .unwrap();
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        assert!(f.log().is_empty(), "{:?}", f.log());

        // absent or null arguments mean "no arguments"
        for req in [
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": { "name": "yapacc_doctor" } }),
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                    "params": { "name": "yapacc_doctor", "arguments": null } }),
        ] {
            let resp = f.request(req).unwrap();
            assert!(resp["result"]["content"][0]["text"].is_string(), "{resp}");
        }
        assert_eq!(f.log().len(), 2);
    }

    #[test]
    fn quoted_input_is_cut_and_cannot_start_a_line() {
        assert_eq!(shown("abc"), "abc");
        assert_eq!(
            shown("a\nb\u{1b}[31m\u{0}"),
            "a\u{fffd}b\u{fffd}[31m\u{fffd}"
        );
        let long = "x".repeat(100_000);
        assert_eq!(shown(&long).chars().count(), 61);
        let f = Fake::new("shown");
        let (text, is_err) = f.call("yapacc_plan", json!({ "name": format!("a\n{long}") }));
        assert!(is_err);
        assert!(text.len() < 400, "{} bytes echoed", text.len());
        assert!(!text.contains('\n'), "{text:?}");
    }

    // ------------------------------------------------------------ the list

    const STAGED_TOOLS: [&str; 4] = [
        "yapacc_install",
        "yapacc_undo_last",
        "yapacc_upgrade",
        "yapacc_remove",
    ];

    #[test]
    fn mutating_tool_is_annotated_destructive_and_reads_are_read_only() {
        for tool in tool_list() {
            let ro = tool["annotations"]["readOnlyHint"].as_bool();
            if STAGED_TOOLS.contains(&tool["name"].as_str().unwrap()) {
                assert_eq!(ro, Some(false));
                assert_eq!(tool["annotations"]["destructiveHint"], json!(true));
            } else {
                assert_eq!(ro, Some(true), "{} must be read-only", tool["name"]);
            }
        }
    }

    /// Review finding 30: a client reads a missing hint as "open world", and
    /// a tool that sends something off the machine must say so.
    #[test]
    fn every_tool_that_can_reach_the_network_says_so() {
        const NETWORK: [&str; 10] = [
            "yapacc_search",
            "yapacc_resolve",
            "yapacc_plan",
            "yapacc_install",
            "yapacc_check",
            "yapacc_news",
            "yapacc_briefing",
            "yapacc_names",
            "yapacc_undo_last",
            "yapacc_upgrade",
        ];
        let tools = tool_list();
        for name in NETWORK {
            assert!(tools.iter().any(|t| t["name"] == name), "{name}");
        }
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let open = tool["annotations"]["openWorldHint"]
                .as_bool()
                .unwrap_or_else(|| panic!("{name}: openWorldHint must be written out"));
            assert_eq!(open, NETWORK.contains(&name), "{name}");
            let description = tool["description"].as_str().unwrap();
            if open {
                assert!(
                    ["sent to", "sends", "etch", "ownload", "contact"]
                        .iter()
                        .any(|w| description.contains(w)),
                    "{name} must say what goes over the network: {description}"
                );
            }
        }
    }

    /// Review finding 7: nothing records that a change came from an
    /// assistant, so no description may say that something does.
    #[test]
    fn no_description_claims_a_provenance_record() {
        for tool in tool_list() {
            let d = tool["description"].as_str().unwrap().to_lowercase();
            for claim in [
                "via ai",
                "via mcp",
                "via=mcp",
                "audit",
                "provenance",
                "recorded",
            ] {
                assert!(!d.contains(claim), "{}: {d}", tool["name"]);
            }
        }
    }

    const NEW_READ_TOOLS: [&str; 8] = [
        "yapacc_timeline",
        "yapacc_briefing",
        "yapacc_doctor",
        "yapacc_clean_report",
        "yapacc_restart_report",
        "yapacc_pending_configs",
        "yapacc_names",
        "yapacc_snapshots",
    ];

    #[test]
    fn every_report_tool_is_listed_read_only_and_has_a_command() {
        let listed = tool_list();
        let mut expected: Vec<&str> = NEW_READ_TOOLS.to_vec();
        expected.push("yapacc_platform");
        for name in expected {
            let tool = listed
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not in tools/list"));
            assert_eq!(tool["annotations"]["readOnlyHint"], json!(true), "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        }
        for name in NEW_READ_TOOLS {
            assert!(read_argv(name, &json!({})).is_some(), "{name}");
            assert!(staged_call(name, &json!({})).is_none(), "{name}");
        }
        // a staged tool is never answered by the read path
        for name in STAGED_TOOLS {
            assert!(read_argv(name, &json!({})).is_none(), "{name}");
            assert!(staged_call(name, &json!({})).is_some(), "{name}");
        }
        let names_tool = listed.iter().find(|t| t["name"] == "yapacc_names").unwrap();
        assert!(
            names_tool["description"]
                .as_str()
                .unwrap()
                .contains("repology.org"),
            "the names tool must say where the name is sent"
        );
    }

    #[test]
    fn config_is_forwarded_as_two_argv_entries_before_the_command() {
        let base = vec!["--json".to_string(), "doctor".to_string()];
        assert_eq!(with_config(None, &base), base);
        assert_eq!(
            with_config(Some("/tmp/a b;rm"), &base),
            vec!["--config", "/tmp/a b;rm", "--json", "doctor"]
        );
        // and that is what precedes every command the server runs
        let mut f = Fake::new("config");
        f.host.lead.extend(with_config(Some("/tmp/a b;rm"), &[]));
        f.call("yapacc_doctor", json!({}));
        assert_eq!(f.log(), ["VIA= [--config] [/tmp/a b;rm] [--json] [doctor]"]);
    }

    #[test]
    fn core_config_must_be_an_existing_regular_file() {
        assert!(set_core_config("/nonexistent/yapacc.toml").is_err());
        assert!(
            set_core_config("/tmp")
                .unwrap_err()
                .contains("regular file")
        );
    }

    fn argv(tool: &str, args: Value) -> Result<Vec<String>, String> {
        read_argv(tool, &args).expect("a report tool")
    }

    #[test]
    fn report_tools_build_the_documented_commands() {
        assert_eq!(
            argv("yapacc_timeline", json!({})).unwrap(),
            vec!["--json", "timeline"]
        );
        assert_eq!(
            argv(
                "yapacc_timeline",
                json!({ "since": "2026-11-10", "limit": 500, "since_boot": true })
            )
            .unwrap(),
            vec![
                "--json",
                "timeline",
                "--since",
                "2026-11-10",
                "--limit",
                "500",
                "--since-boot"
            ]
        );
        assert_eq!(
            argv(
                "yapacc_timeline",
                json!({ "limit": 1, "since_boot": false })
            )
            .unwrap(),
            vec!["--json", "timeline", "--limit", "1"]
        );
        assert_eq!(
            argv("yapacc_briefing", json!({})).unwrap(),
            vec!["--json", "briefing"]
        );
        assert_eq!(
            argv("yapacc_briefing", json!({ "offline": true })).unwrap(),
            vec!["--json", "briefing", "--offline"]
        );
        assert_eq!(
            argv("yapacc_doctor", json!({})).unwrap(),
            vec!["--json", "doctor"]
        );
        assert_eq!(
            argv("yapacc_clean_report", json!({})).unwrap(),
            vec!["--json", "clean"]
        );
        assert_eq!(
            argv("yapacc_restart_report", json!({})).unwrap(),
            vec!["--json", "restart"]
        );
        assert_eq!(
            argv("yapacc_pending_configs", json!({})).unwrap(),
            vec!["--json", "configs"]
        );
        assert_eq!(
            argv("yapacc_snapshots", json!({})).unwrap(),
            vec!["--json", "snapshot", "list"]
        );
        assert_eq!(
            argv("yapacc_names", json!({ "name": "ripgrep" })).unwrap(),
            vec!["--json", "names", "--", "ripgrep"]
        );
        // arguments a tool does not take never reach its command line
        assert_eq!(
            argv(
                "yapacc_doctor",
                json!({ "config": "/x", "root": "--root=/x" })
            )
            .unwrap(),
            vec!["--json", "doctor"]
        );
    }

    /// Review finding 3: these were answered in this process by pacman-only
    /// code. They are core commands now, so the answer is the one the
    /// command line gives on this system.
    #[test]
    fn resolve_plan_and_check_are_core_commands() {
        assert_eq!(
            argv("yapacc_resolve", json!({ "name": "ripgrep" })).unwrap(),
            vec!["get", "--explain", "--", "ripgrep"]
        );
        // `from` is not an argument of resolve: it is not passed on
        assert_eq!(
            argv(
                "yapacc_resolve",
                json!({ "name": "ripgrep", "from": "aur" })
            )
            .unwrap(),
            vec!["get", "--explain", "--", "ripgrep"]
        );
        assert_eq!(
            argv("yapacc_plan", json!({ "name": "ripgrep" })).unwrap(),
            vec!["get", "--explain", "--", "ripgrep"]
        );
        assert_eq!(
            argv(
                "yapacc_plan",
                json!({ "name": "org.gnome.Foo", "from": "flatpak" })
            )
            .unwrap(),
            vec![
                "get",
                "--from",
                "flatpak",
                "--explain",
                "--",
                "org.gnome.Foo"
            ]
        );
        assert_eq!(
            argv("yapacc_check", json!({ "anything": "--refresh" })).unwrap(),
            vec!["--json", "check"]
        );

        let f = Fake::new("core-reads");
        let (text, is_err) = f.call("yapacc_resolve", json!({ "name": "ripgrep" }));
        assert!(!is_err && text.contains("fake core ran"), "{text}");
        f.call("yapacc_check", json!({}));
        assert_eq!(
            f.log(),
            [
                "VIA= [get] [--explain] [--] [ripgrep]",
                "VIA= [--json] [check]"
            ]
        );
        // a plan the core cannot make is an error, not a quiet text
        f.fail();
        let (_, is_err) = f.call("yapacc_plan", json!({ "name": "ripgrep", "from": "repo" }));
        assert!(is_err);
        let (_, is_err) = f.call("yapacc_check", json!({}));
        assert!(is_err);
    }

    #[test]
    fn news_is_refused_off_arch_before_anything_is_fetched() {
        assert_eq!(
            news_argv(&json!({})).unwrap(),
            vec!["--json", "news", "--limit", "5"]
        );
        assert_eq!(
            news_argv(&json!({ "limit": 50 })).unwrap(),
            vec!["--json", "news", "--limit", "50"]
        );
        for bad in [
            json!(0),
            json!(51),
            json!(-1),
            json!("5"),
            json!(2.5),
            json!([5]),
        ] {
            assert!(
                news_argv(&json!({ "limit": bad.clone() })).is_err(),
                "{bad}"
            );
        }

        let mut f = Fake::new("news");
        let (text, is_err) = f.call("yapacc_news", json!({ "limit": 3 }));
        assert!(!is_err, "{text}");
        assert_eq!(f.log(), ["VIA= [--json] [news] [--limit] [3]"]);

        f.host.pacman = false;
        let (text, is_err) = f.call("yapacc_news", json!({}));
        assert!(is_err);
        assert!(text.contains("Arch only"), "{text}");
        assert!(text.contains("Nothing was fetched"), "{text}");
        assert_eq!(f.log().len(), 1, "the core is not run off Arch");
    }

    // -------------------------------------------------------------- search

    fn search(args: Value, aur: bool) -> Result<SearchRequest, String> {
        search_request(&args, aur)
    }

    #[test]
    fn search_is_the_core_search_with_the_query_after_the_separator() {
        let r = search(json!({ "query": "gedit" }), true).unwrap();
        assert_eq!(
            r.argv,
            vec!["--json", "search", "--limit", "20", "--all", "--", "gedit"]
        );
        assert_eq!((r.limit, r.categories.len(), r.kind), (20, 0, None));
        // not the Arch family: no AUR, so no --all (the core refuses it there)
        let r = search(
            json!({ "query": "  draw in the terminal ", "limit": 7 }),
            false,
        )
        .unwrap();
        assert_eq!(
            r.argv,
            vec![
                "--json",
                "search",
                "--limit",
                "7",
                "--",
                "draw in the terminal"
            ]
        );
        // filters are applied afterwards and never reach the command line
        let r = search(
            json!({ "query": "edit", "categories": ["Development", "--config=/x"], "kind": "--root=/", "limit": 5 }),
            true,
        )
        .unwrap();
        assert_eq!(
            r.argv,
            vec!["--json", "search", "--limit", "300", "--all", "--", "edit"]
        );
        assert_eq!(r.categories, vec!["development", "--config=/x"]);
        assert_eq!(r.kind.as_deref(), Some("--root=/"));
        assert_eq!(r.limit, 5);
    }

    /// Review finding 1 (blocker). The core hands the query to `pacman -Ss`,
    /// whole and then word by word, so no word may be an option.
    #[test]
    fn a_hostile_query_is_refused_before_anything_runs() {
        let long = "a".repeat(QUERY_MAX_BYTES + 1);
        for bad in [
            json!("-y"),
            json!("-yy"),
            json!("--config=/x"),
            json!("--dbpath=/tmp/x"),
            json!("--root=/"),
            json!("-"),
            json!("--"),
            json!("  -y"),
            json!("ripgrep -y"),
            json!("draw --refresh terminal"),
            json!("x\t-y"),
            json!("a\nb"),
            json!("a\u{0}b"),
            json!("\u{1b}[31mred"),
            json!("a\rb"),
            json!(""),
            json!("   "),
            json!(long),
            json!(7),
            json!(true),
            json!(null),
            json!(["gedit"]),
            json!({ "q": "gedit" }),
        ] {
            assert!(
                search(json!({ "query": bad.clone() }), true).is_err(),
                "query={bad}"
            );
        }
        assert!(search(json!({}), true).is_err(), "query is required");
        // what is not an option is a query, however odd: one argv entry
        for odd in [
            "a;b",
            "$(id)",
            "`id`",
            "c++",
            "a-b",
            "x ~ y",
            "../../etc/passwd",
            "été",
        ] {
            let r = search(json!({ "query": odd }), true).unwrap();
            assert_eq!(r.argv.last().map(String::as_str), Some(odd));
            assert_eq!(r.argv[r.argv.len() - 2], "--");
        }
        // limit is bounded (it was unbounded, and multiplied by three)
        for bad in [
            json!(0),
            json!(101),
            json!(1_000_000),
            json!(-1),
            json!("5"),
            json!(1.5),
            json!(true),
        ] {
            assert!(
                search(json!({ "query": "x", "limit": bad.clone() }), true).is_err(),
                "limit={bad}"
            );
        }
        assert_eq!(
            search(json!({ "query": "x", "limit": 100 }), true)
                .unwrap()
                .limit,
            100
        );
        // filters: lists of short words only
        let many: Vec<&str> = vec!["x"; FACET_MAX_COUNT + 1];
        for bad in [
            json!({ "query": "x", "categories": "development" }),
            json!({ "query": "x", "categories": [7] }),
            json!({ "query": "x", "categories": [""] }),
            json!({ "query": "x", "categories": ["a\nb"] }),
            json!({ "query": "x", "categories": many }),
            json!({ "query": "x", "categories": ["y".repeat(FACET_MAX_BYTES + 1)] }),
            json!({ "query": "x", "kind": 7 }),
            json!({ "query": "x", "kind": "" }),
            json!({ "query": "x", "kind": ["cli-tool"] }),
        ] {
            assert!(search(bad.clone(), true).is_err(), "{bad}");
        }

        // through the protocol: an error result, and the core never runs
        let f = Fake::new("hostile-query");
        for bad in ["-y", "--config=/x", "ripgrep -y", "a\nb"] {
            let (text, is_err) = f.call("yapacc_search", json!({ "query": bad }));
            assert!(is_err, "{bad:?}: {text}");
        }
        let (_, is_err) = f.call("yapacc_search", json!({ "query": "x", "limit": 100000 }));
        assert!(is_err);
        assert!(f.log().is_empty(), "{:?}", f.log());
    }

    fn rows() -> Value {
        json!([
            { "name": "gedit", "source": "repo", "kind": "gui-app",
              "categories": ["office", "gui"], "description": "GNOME text editor" },
            { "name": "debugedit", "source": "repo", "kind": "cli-tool",
              "categories": ["development"], "description": "debuginfo tool" },
            { "name": "libgedit-tepl", "source": "repo", "kind": "library",
              "categories": ["library", "Development"], "description": "library" },
            { "name": "gedit-git", "source": "aur", "kind": "gui-app",
              "categories": [], "description": "text editor" }
        ])
    }

    /// Review finding 9: "edit" used to force the category `development`
    /// and drop `gedit` itself. The core's list is returned as it is.
    #[test]
    fn search_returns_what_the_core_found_and_filters_only_when_asked() {
        let f = Fake::new("search");
        f.stdout(&rows().to_string());
        let (text, is_err) = f.call("yapacc_search", json!({ "query": "gedit" }));
        assert!(!is_err, "{text}");
        let found: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(found, rows(), "nothing dropped, order kept");
        assert_eq!(found[0]["name"], "gedit", "the exact match stays first");

        let names = |args: Value| -> Vec<String> {
            let (text, is_err) = f.call("yapacc_search", args);
            assert!(!is_err, "{text}");
            serde_json::from_str::<Value>(&text)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            names(json!({ "query": "gedit", "categories": ["development"] })),
            ["debugedit", "libgedit-tepl"]
        );
        assert_eq!(
            names(json!({ "query": "gedit", "kind": "GUI" })),
            ["gedit", "gedit-git"]
        );
        assert_eq!(
            names(
                json!({ "query": "gedit", "categories": ["office", "library"], "kind": "gui-app" })
            ),
            ["gedit"]
        );
        assert_eq!(
            names(json!({ "query": "gedit", "limit": 2 })),
            ["gedit", "debugedit"]
        );
        assert_eq!(
            names(json!({ "query": "gedit", "kind": "gui", "limit": 1 })),
            ["gedit"],
            "the limit applies after the filter"
        );
        assert!(names(json!({ "query": "gedit", "kind": "no-such-kind" })).is_empty());
        assert_eq!(
            f.log()[0],
            "VIA= [--json] [search] [--limit] [20] [--all] [--] [gedit]"
        );
        assert!(f.log()[1].contains("[--limit] [300]"), "{:?}", f.log());
    }

    #[test]
    fn search_filters_say_when_the_system_has_no_facets() {
        // the shape another package manager's search returns
        let portable = json!([
            { "name": "ripgrep", "version": "14", "repo": "main", "description": "grep" }
        ]);
        let mut f = Fake::new("portable-search");
        f.host.pacman = false;
        f.stdout(&portable.to_string());
        let (text, is_err) = f.call("yapacc_search", json!({ "query": "ripgrep" }));
        assert!(!is_err, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), portable);
        assert_eq!(
            f.log(),
            ["VIA= [--json] [search] [--limit] [20] [--] [ripgrep]"],
            "no --all without the AUR"
        );
        let (text, is_err) = f.call(
            "yapacc_search",
            json!({ "query": "ripgrep", "kind": "cli-tool" }),
        );
        assert!(is_err);
        assert!(text.contains("cannot be applied here"), "{text}");

        // what is not a list is an error, with what the core said
        f.stdout("search is not available\n");
        let (text, is_err) = f.call("yapacc_search", json!({ "query": "ripgrep" }));
        assert!(is_err);
        assert!(text.contains("search is not available"), "{text}");
        // and so is a core that failed
        f.stdout("[]");
        f.fail();
        let (_, is_err) = f.call("yapacc_search", json!({ "query": "ripgrep" }));
        assert!(is_err);
    }

    // ------------------------------------------------------------- targets

    #[test]
    fn option_like_names_and_unknown_lanes_are_rejected() {
        let f = Fake::new("names");
        for bad in [
            "--no-review",
            "-y",
            "a b",
            "x;id",
            "",
            "--installation=x",
            "a\nb",
        ] {
            for tool in [
                "yapacc_install",
                "yapacc_plan",
                "yapacc_resolve",
                "yapacc_why",
                "yapacc_names",
            ] {
                let (text, is_err) = f.call(tool, json!({ "name": bad, "confirm": true }));
                assert!(is_err, "{tool} {bad:?}");
                assert!(
                    text.contains("not a valid package name"),
                    "{tool} {bad:?}: {text}"
                );
            }
        }
        for bad in [
            json!(7),
            json!(null),
            json!(true),
            json!(["ripgrep"]),
            json!({ "n": 1 }),
        ] {
            for tool in [
                "yapacc_install",
                "yapacc_plan",
                "yapacc_resolve",
                "yapacc_why",
                "yapacc_names",
            ] {
                let (text, is_err) = f.call(tool, json!({ "name": bad.clone() }));
                assert!(is_err, "{tool} {bad}");
                assert!(text.contains("written as a string"), "{tool} {bad}: {text}");
            }
        }
        for tool in ["yapacc_install", "yapacc_plan"] {
            for bad in [
                json!("--yes"),
                json!("tap"),
                json!("plugin"),
                json!(""),
                json!("AUR"),
                json!(3),
                json!(["aur"]),
                json!(true),
            ] {
                let (text, is_err) = f.call(tool, json!({ "name": "ok", "from": bad.clone() }));
                assert!(is_err, "{tool} from={bad}: {text}");
                assert!(
                    text.contains("repo, aur, flatpak, brew, recipe"),
                    "{tool} from={bad}: {text}"
                );
            }
        }
        assert!(f.log().is_empty(), "{:?}", f.log());
    }

    /// One list for the schema, the check and the message, and every entry
    /// is a lane the core's `get --from` takes.
    #[test]
    fn the_lanes_offered_are_the_lanes_the_core_accepts() {
        for lane in INSTALL_LANES {
            assert!(crate::resolve::LANES.contains(&lane), "{lane}");
            assert_ne!(lane, "tap", "the core refuses --from tap");
            let s = staged_call("yapacc_install", &json!({ "name": "x", "from": lane }))
                .unwrap()
                .unwrap();
            assert_eq!(s.human, format!("yapacc get --from {lane} x"));
        }
        for tool in tool_list() {
            let name = tool["name"].as_str().unwrap();
            if name == "yapacc_plan" || name == "yapacc_install" {
                assert_eq!(
                    tool["inputSchema"]["properties"]["from"]["enum"],
                    json!(INSTALL_LANES),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn get_argv_keeps_the_name_after_the_separator() {
        assert_eq!(
            get_argv("ripgrep", Some("aur"), "--explain"),
            vec!["get", "--from", "aur", "--explain", "--", "ripgrep"]
        );
    }

    #[test]
    fn hostile_report_arguments_are_rejected_before_anything_runs() {
        // read_argv is pure: an Err here means no command line exists, so
        // no process can have been started for it.
        for bad in [
            json!("--root=/x"),
            json!("a;b"),
            json!("2026-13-99x"),
            json!("2026-13-01"),
            json!("2026-11-00"),
            json!("2026-11-1"),
            json!("2026/11/10"),
            json!("+026-11-10"),
            json!(""),
            json!(20261110),
            json!(["2026-11-10"]),
        ] {
            assert!(
                argv("yapacc_timeline", json!({ "since": bad.clone() })).is_err(),
                "since={bad}"
            );
        }
        for bad in [
            json!(0),
            json!(100000),
            json!(501),
            json!(-1),
            json!(1.5),
            json!("5"),
            json!("--root=/x"),
            json!(true),
        ] {
            assert!(
                argv("yapacc_timeline", json!({ "limit": bad.clone() })).is_err(),
                "limit={bad}"
            );
        }
        for bad in [json!("--since-boot"), json!(1), json!("true")] {
            assert!(
                argv("yapacc_timeline", json!({ "since_boot": bad.clone() })).is_err(),
                "since_boot={bad}"
            );
            assert!(
                argv("yapacc_briefing", json!({ "offline": bad.clone() })).is_err(),
                "offline={bad}"
            );
        }
        for bad in [
            json!("--root=/x"),
            json!("a;b"),
            json!("a b"),
            json!("-y"),
            json!(""),
            json!(7),
            json!(["ripgrep"]),
        ] {
            for tool in ["yapacc_names", "yapacc_resolve", "yapacc_plan"] {
                assert!(
                    argv(tool, json!({ "name": bad.clone() })).is_err(),
                    "{tool} name={bad}"
                );
            }
        }
        assert!(argv("yapacc_names", json!({})).is_err());
        assert!(argv("yapacc_resolve", json!({})).is_err());

        // and through the protocol the refusal is an error result
        let f = Fake::new("hostile-reports");
        let (_, is_err) = f.call("yapacc_timeline", json!({ "limit": 100000 }));
        assert!(is_err);
        let (text, _) = f.call("yapacc_names", json!({ "name": "--root=/x" }));
        assert!(text.contains("not a valid package name"), "{text}");
        let (text, _) = f.call("yapacc_timeline", json!({ "since": "2026-13-99x" }));
        assert!(text.contains("YYYY-MM-DD"), "{text}");
        assert!(f.log().is_empty(), "{:?}", f.log());
    }

    #[test]
    fn dates_are_strict() {
        for good in ["2026-11-10", "1999-01-31", "2026-12-01"] {
            assert!(valid_date(good), "{good}");
        }
        for bad in [
            "2026-13-99x",
            "2026-13-99",
            "2026-00-10",
            "26-11-10",
            "2026-11-10 ",
            "２０２６-11-10",
        ] {
            assert!(!valid_date(bad), "{bad:?}");
        }
    }

    // -------------------------------------------------------------- staged

    #[test]
    fn staged_tools_plan_with_dryrun_and_apply_with_yes() {
        let s = staged_call("yapacc_undo_last", &json!({}))
            .unwrap()
            .unwrap();
        assert_eq!(s.plan, vec!["undo", "--dryrun"]);
        assert_eq!(s.apply, vec!["undo", "--yes"]);
        assert_eq!(s.human, "yapacc undo");
        assert!(!s.confirm);

        let s = staged_call("yapacc_upgrade", &json!({ "confirm": true }))
            .unwrap()
            .unwrap();
        assert_eq!(s.plan, vec!["upgrade", "--dryrun"]);
        assert_eq!(s.apply, vec!["upgrade", "--yes"]);
        assert_eq!(s.human, "yapacc upgrade");
        assert!(s.confirm);

        let s = staged_call("yapacc_remove", &json!({ "names": ["foo", "bar-baz"] }))
            .unwrap()
            .unwrap();
        assert_eq!(
            s.plan,
            vec!["remove", "repo", "--dryrun", "--", "foo", "bar-baz"]
        );
        assert_eq!(
            s.apply,
            vec!["remove", "repo", "--yes", "--", "foo", "bar-baz"]
        );
        assert_eq!(s.human, "yapacc remove repo foo bar-baz");
        // the plan never carries a consent flag
        assert!(!s.plan.iter().any(|a| a == "--yes" || a == "--noconfirm"));

        // install is staged the same way: its plan is the decision table
        let s = staged_call(
            "yapacc_install",
            &json!({ "name": "ripgrep", "from": "aur", "confirm": true }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            s.plan,
            vec!["get", "--from", "aur", "--explain", "--", "ripgrep"]
        );
        assert_eq!(
            s.apply,
            vec!["get", "--from", "aur", "--noconfirm", "--", "ripgrep"]
        );
        assert_eq!(s.human, "yapacc get --from aur ripgrep");
        assert!(s.confirm);
        assert!(!s.plan.iter().any(|a| a == "--yes" || a == "--noconfirm"));
    }

    #[test]
    fn hostile_staged_arguments_are_rejected_before_anything_runs() {
        let too_many: Vec<&str> = vec!["x"; REMOVE_MAX_NAMES + 1];
        for bad in [
            json!({}),
            json!({ "names": [] }),
            json!({ "names": "foo" }),
            json!({ "names": ["--root=/x"] }),
            json!({ "names": ["ok", "a;b"] }),
            json!({ "names": ["ok", "-y"] }),
            json!({ "names": ["ok", ""] }),
            json!({ "names": ["ok", 3] }),
            json!({ "names": ["ok"], "confirm": "yes" }),
            json!({ "names": too_many }),
        ] {
            assert!(
                staged_call("yapacc_remove", &bad).unwrap().is_err(),
                "{bad}"
            );
        }
        assert!(
            staged_call("yapacc_upgrade", &json!({ "confirm": "--yes" }))
                .unwrap()
                .is_err()
        );
        assert!(
            staged_call("yapacc_undo_last", &json!({ "confirm": 1 }))
                .unwrap()
                .is_err()
        );
        // a string is not a boolean, for install as for the others
        for bad in [json!("true"), json!("yes"), json!(1), json!(["true"])] {
            assert!(
                staged_call(
                    "yapacc_install",
                    &json!({ "name": "ok", "confirm": bad.clone() })
                )
                .unwrap()
                .is_err(),
                "confirm={bad}"
            );
        }
        let f = Fake::allowing("hostile-staged");
        let (text, is_err) = f.call(
            "yapacc_remove",
            json!({ "names": ["--root=/x"], "confirm": true }),
        );
        assert!(
            is_err && text.contains("not a valid package name"),
            "{text}"
        );
        let (text, is_err) = f.call("yapacc_install", json!({ "name": "ok", "confirm": "true" }));
        assert!(is_err && text.contains("must be true or false"), "{text}");
        assert!(
            f.log().is_empty(),
            "even when applying is allowed: {:?}",
            f.log()
        );
    }

    /// default config: [mcp] apply = "plan". Even confirm=true must not apply.
    fn assert_only_staged(tool: &str, arguments: Value, human_cmd: &str, plan_argv: &str) {
        for confirm in [json!(false), json!(true), Value::Null] {
            let f = Fake::new("only-staged");
            let mut arguments = arguments.clone();
            if !confirm.is_null() {
                arguments["confirm"] = confirm.clone();
            }
            let (text, is_err) = f.call(tool, arguments);
            assert!(!is_err, "{tool}: {text}");
            assert!(
                text.starts_with("PLAN ONLY"),
                "{tool} confirm={confirm}: {text}"
            );
            assert!(text.contains("nothing was changed"), "{tool}: {text}");
            assert!(
                text.contains(human_cmd),
                "{tool} must hand the human the command to run: {text}"
            );
            // exactly one run of the core, and it is the plan
            assert_eq!(f.log(), [format!("VIA= {plan_argv}")], "{tool}");
        }
    }

    #[test]
    fn a_model_cannot_approve_its_own_install() {
        assert_only_staged(
            "yapacc_install",
            json!({ "name": "whatever-x" }),
            "    yapacc get whatever-x\n",
            "[get] [--explain] [--] [whatever-x]",
        );
        assert_only_staged(
            "yapacc_install",
            json!({ "name": "whatever-x", "from": "aur" }),
            "    yapacc get --from aur whatever-x\n",
            "[get] [--from] [aur] [--explain] [--] [whatever-x]",
        );
    }

    #[test]
    fn a_model_cannot_approve_its_own_undo() {
        assert_only_staged(
            "yapacc_undo_last",
            json!({}),
            "    yapacc undo\n",
            "[undo] [--dryrun]",
        );
    }

    #[test]
    fn a_model_cannot_approve_its_own_upgrade() {
        assert_only_staged(
            "yapacc_upgrade",
            json!({}),
            "    yapacc upgrade\n",
            "[upgrade] [--dryrun]",
        );
    }

    #[test]
    fn a_model_cannot_approve_its_own_remove() {
        assert_only_staged(
            "yapacc_remove",
            json!({ "names": ["whatever-x", "whatever-y"] }),
            "    yapacc remove repo whatever-x whatever-y\n",
            "[remove] [repo] [--dryrun] [--] [whatever-x] [whatever-y]",
        );
    }

    fn staged_calls() -> [(&'static str, Value, &'static str); 4] {
        [
            (
                "yapacc_install",
                json!({ "name": "pkg-x" }),
                "[get] [--noconfirm] [--] [pkg-x]",
            ),
            ("yapacc_undo_last", json!({}), "[undo] [--yes]"),
            ("yapacc_upgrade", json!({}), "[upgrade] [--yes]"),
            (
                "yapacc_remove",
                json!({ "names": ["pkg-x"] }),
                "[remove] [repo] [--yes] [--] [pkg-x]",
            ),
        ]
    }

    /// Review finding 8: a plan that cannot be made is an error, and no
    /// command is handed out for the human to run.
    #[test]
    fn a_plan_that_fails_is_an_error_and_hands_out_no_command() {
        for (tool, args, _) in staged_calls() {
            for allowed in [false, true] {
                let f = if allowed {
                    Fake::allowing("plan-fails")
                } else {
                    Fake::new("plan-fails")
                };
                f.stdout("invalid --from lane 'x'\n");
                f.fail();
                let mut args = args.clone();
                args["confirm"] = json!(true);
                let (text, is_err) = f.call(tool, args);
                assert!(is_err, "{tool}: {text}");
                assert!(
                    text.starts_with("PLAN FAILED — nothing was changed."),
                    "{tool}: {text}"
                );
                assert!(text.contains("invalid --from lane"), "{tool}: {text}");
                assert!(!text.contains("THEY run this"), "{tool}: {text}");
                assert_eq!(
                    f.log().len(),
                    1,
                    "{tool}: nothing is applied after a failed plan"
                );
            }
        }
    }

    /// `[mcp] apply = "allow"` from a file only root can write.
    #[test]
    fn with_the_owners_consent_a_confirmed_call_applies_and_says_so() {
        for (tool, args, apply_argv) in staged_calls() {
            // not confirmed: the plan only
            let f = Fake::allowing("allow-unconfirmed");
            let (text, is_err) = f.call(tool, args.clone());
            assert!(
                !is_err && text.starts_with("PLAN ONLY (confirm=false)"),
                "{tool}: {text}"
            );
            assert_eq!(f.log().len(), 1, "{tool}");
            assert!(f.log()[0].starts_with("VIA= "), "{tool}: {:?}", f.log());

            // confirmed: the plan, then the change, marked as coming from here
            let f = Fake::allowing("allow-confirmed");
            let mut confirmed = args.clone();
            confirmed["confirm"] = json!(true);
            let (text, is_err) = f.call(tool, confirmed.clone());
            assert!(
                !is_err && text.starts_with("APPLIED: yapacc "),
                "{tool}: {text}"
            );
            assert_eq!(f.log().len(), 2, "{tool}");
            assert_eq!(f.log()[1], format!("VIA=mcp {apply_argv}"), "{tool}");
        }
    }

    /// Review finding 8: install used to answer isError=false whatever the
    /// core's exit status was.
    #[test]
    fn a_change_that_fails_is_reported_as_failed_for_every_staged_tool() {
        for (tool, args, _) in staged_calls() {
            let f = Fake::allowing("apply-fails");
            // the plan succeeds; the change then fails
            let d = f.dir.display().to_string();
            std::fs::write(
                f.dir.join("core.sh"),
                format!(
                    "{{ printf 'VIA=%s' \"${{YAPACC_VIA-}}\"; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; echo; }} >> '{d}/argv.log'\n\
                     case \" $* \" in *' --explain '*|*' --dryrun '*) echo plan; exit 0;; esac\n\
                     echo 'error: target not found' >&2; exit 1\n"
                ),
            )
            .unwrap();
            let mut args = args.clone();
            args["confirm"] = json!(true);
            let (text, is_err) = f.call(tool, args);
            assert!(is_err, "{tool}: {text}");
            assert!(text.starts_with("FAILED: `yapacc "), "{tool}: {text}");
            assert!(text.contains("target not found"), "{tool}: {text}");
            assert!(!text.contains("APPLIED"), "{tool}: {text}");
            assert_eq!(f.log().len(), 2, "{tool}");
        }
    }

    /// Review finding 30: the switch lives in a file. From one the user (and
    /// so an agent with file access) can write, it is not honoured.
    #[test]
    fn allow_counts_only_from_a_file_only_root_can_write() {
        let p = Path::new("/etc/yapacc/config.toml");
        assert_eq!(apply_policy("plan", Some((p, 0, 0o644))), Apply::Plan);
        assert_eq!(apply_policy("plan", None), Apply::Plan);
        assert_eq!(apply_policy("allow", Some((p, 0, 0o644))), Apply::Allow);
        assert_eq!(apply_policy("allow", Some((p, 0, 0o600))), Apply::Allow);
        for (uid, mode) in [
            (1000, 0o644),
            (1000, 0o600),
            (0, 0o664),
            (0, 0o646),
            (0, 0o666),
            (65534, 0o444),
        ] {
            assert_eq!(
                apply_policy("allow", Some((p, uid, mode))),
                Apply::Untrusted("/etc/yapacc/config.toml".into()),
                "uid={uid} mode={mode:o}"
            );
        }
        assert!(matches!(apply_policy("allow", None), Apply::Untrusted(_)));

        // and then every staged tool only plans, and says why
        for (tool, args, _) in staged_calls() {
            let mut f = Fake::new("untrusted");
            f.host.apply = Apply::Untrusted("/home/u/.config/yapacc/config.toml".into());
            let mut args = args.clone();
            args["confirm"] = json!(true);
            let (text, is_err) = f.call(tool, args);
            assert!(
                !is_err && text.starts_with("PLAN ONLY — nothing was changed."),
                "{tool}: {text}"
            );
            assert!(
                text.contains("/home/u/.config/yapacc/config.toml")
                    && text.contains("not honoured"),
                "{tool}: {text}"
            );
            assert_eq!(f.log().len(), 1, "{tool}");
            assert!(!f.log()[0].contains("--yes") && !f.log()[0].contains("--noconfirm"));
        }
    }

    /// The promise, for every tool and whatever it is sent: with the
    /// default `[mcp] apply = "plan"` the only commands that exist are
    /// reads and plans.
    #[test]
    fn in_plan_mode_no_argument_of_any_tool_produces_a_changing_command() {
        let long = "A".repeat(100_000);
        let hostile: Vec<Value> = vec![
            json!("-y"),
            json!("--yes"),
            json!("--noconfirm"),
            json!("--config=/x"),
            json!("--root=/"),
            json!("x --yes"),
            json!("a\nb"),
            json!("a\u{0}b"),
            json!("\u{1b}[2J"),
            json!("$(id)"),
            json!("; rm -rf /"),
            json!(""),
            json!(long),
            json!("ripgrep"),
            json!("2026-11-10"),
            json!("aur"),
            json!(true),
            json!(false),
            json!(7),
            json!(-1),
            json!(1.5),
            json!(null),
            json!(["ripgrep", "--yes"]),
            json!(["--yes"]),
            json!({ "confirm": true }),
        ];
        let f = Fake::new("promise");
        f.stdout("[]");
        let keys = [
            "name",
            "names",
            "from",
            "confirm",
            "query",
            "categories",
            "kind",
            "limit",
            "since",
            "since_boot",
            "offline",
        ];
        let mut calls = 0;
        for tool in tool_list() {
            let tool = tool["name"].as_str().unwrap().to_string();
            // the in-process readers run no command built from an argument
            if IN_PROCESS.contains(&tool.as_str()) {
                continue;
            }
            for key in keys {
                for value in &hostile {
                    // one hostile argument, the rest valid and confirming
                    let mut args = json!({ "name": "ripgrep", "names": ["ripgrep"],
                        "query": "ripgrep", "confirm": true });
                    args[key] = value.clone();
                    let resp = f
                        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": tool, "arguments": args } }))
                        .unwrap();
                    assert!(
                        resp["result"]["content"][0]["text"].is_string(),
                        "{tool} {key}={value}: {resp}"
                    );
                    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
                    assert!(!text.starts_with("APPLIED"), "{tool} {key}={value}");
                    calls += 1;
                }
            }
        }
        assert!(calls > 4000, "{calls}");
        let log = f.log();
        assert!(!log.is_empty());
        for entry in &log {
            assert!(
                entry.starts_with("VIA= ["),
                "run as an applied change: {entry}"
            );
            assert!(
                !entry.contains("[--yes]")
                    && !entry.contains("[--noconfirm]")
                    && !entry.contains("[-y]"),
                "a consent flag reached the core: {entry}"
            );
            let reads = entry.starts_with("VIA= [--json] ");
            let plans = entry.starts_with("VIA= [get] ") && entry.contains(" [--explain] [--] ")
                || entry.contains(" [--dryrun]");
            assert!(reads || plans, "neither a read nor a plan: {entry}");
            // an option-like value is only ever a search query after `--`…
            if let Some((before, _)) = entry.split_once(" [--] ") {
                for word in ["[--config=/x]", "[--root=/]", "[x --yes]", "[$(id)]"] {
                    assert!(!before.contains(word), "{entry}");
                }
            }
        }
        // …and never one whose words could be options
        assert!(
            !log.iter().any(|e| e.contains("[--config=/x]")
                || e.contains("[--root=/]")
                || e.contains("[x --yes]")),
            "{log:?}"
        );
    }

    #[test]
    fn a_newer_protocol_version_is_answered_with_the_supported_one() {
        let f = Fake::new("version");
        for asked in ["2024-11-05", "2025-06-18", "2099-01-01", "nonsense"] {
            let resp = f
                .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": asked, "capabilities": {},
                            "clientInfo": { "name": "t", "version": "0" } } }))
                .unwrap();
            assert_eq!(
                resp["result"]["protocolVersion"], "2024-11-05",
                "asked for {asked}"
            );
            assert!(resp.get("error").is_none(), "asked for {asked}: {resp}");
        }
    }

    #[test]
    fn the_command_reference_is_a_resource() {
        let f = Fake::new("resource");
        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
            .unwrap();
        assert!(resp["result"]["capabilities"]["resources"].is_object());

        let resp = f
            .request(json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" }))
            .unwrap();
        let list = resp["result"]["resources"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["uri"], DOCS_COMMANDS_URI);

        let resp = f
            .request(
                json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/read",
            "params": { "uri": DOCS_COMMANDS_URI } }),
            )
            .unwrap();
        let content = &resp["result"]["contents"][0];
        assert_eq!(content["uri"], DOCS_COMMANDS_URI);
        assert_eq!(
            content["text"].as_str().unwrap(),
            crate::docs::render("commands").unwrap()
        );

        for bad in ["yapacc://docs/../../etc/passwd", "file:///etc/passwd", ""] {
            let resp = f
                .request(
                    json!({ "jsonrpc": "2.0", "id": 7, "method": "resources/read",
                "params": { "uri": bad } }),
                )
                .unwrap();
            assert_eq!(resp["error"]["code"], -32002, "{bad:?}: {resp}");
        }
    }
}
