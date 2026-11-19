//! yapacc interactive TUI — the friendly front door, ratatui-based.
//!
//! Home screen: one line about this machine, then a menu of plain-language
//! questions. Each question opens a pane filled by the core `yapacc` binary
//! (`--json`, run in a background thread). The "Browse" area keeps the
//! package list, search, external installs, docs, the category tree and a
//! free-form command line.
//!
//! Every action runs the core binary. A change is first run with `--dryrun`
//! and shown as a plan; only an explicit Enter on "[ Apply ]" runs the same
//! command without `--dryrun`, attached to the real terminal, so that sudo
//! and the core's own [y/N] gate ask the questions. This interface never
//! passes `--yes` or `--noconfirm`.

use crate::{backend, docs, ledger};
use ratatui::{
    Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use serde_json::Value;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// What a missing or empty value is shown as.
const DASH: &str = "—";

const PREVIEW_TITLE: &str = "This is what will happen — nothing has been changed yet";

// ---------------------------------------------------------------------------
// home menu
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum HomeAction {
    Timeline,
    Briefing,
    Why,
    Browse,
    Search,
    Clean,
    Restart,
    Configs,
    Doctor,
    Undo,
}

struct MenuEntry {
    key: char,
    title: &'static str,
    explain: &'static str,
    action: HomeAction,
}

static MENU: [MenuEntry; 10] = [
    MenuEntry {
        key: 't',
        title: "What changed recently?",
        explain: "Every install, upgrade and removal on this machine, newest first.",
        action: HomeAction::Timeline,
    },
    MenuEntry {
        key: 'b',
        title: "What will the next upgrade do?",
        explain: "Reboot needed, security fixes, news, configuration and disk — before you upgrade.",
        action: HomeAction::Briefing,
    },
    MenuEntry {
        key: 'w',
        title: "Why is a package installed?",
        explain: "Type a name: who asked for it, what needs it, where it came from.",
        action: HomeAction::Why,
    },
    MenuEntry {
        key: 'l',
        title: "Browse installed software",
        explain: "The list of what is installed; filter it, look inside, remove.",
        action: HomeAction::Browse,
    },
    MenuEntry {
        key: 's',
        title: "Search for software",
        explain: "Find something to install; the plan is shown before anything happens.",
        action: HomeAction::Search,
    },
    MenuEntry {
        key: 'c',
        title: "What can I clean up?",
        explain: "Unused packages and caches, with the space each would give back.",
        action: HomeAction::Clean,
    },
    MenuEntry {
        key: 'r',
        title: "What needs restarting?",
        explain: "Services and programs still running old files after an upgrade.",
        action: HomeAction::Restart,
    },
    MenuEntry {
        key: 'm',
        title: "Unmerged configuration files",
        explain: "Configuration files an upgrade left beside yours, waiting to be compared.",
        action: HomeAction::Configs,
    },
    MenuEntry {
        key: 'd',
        title: "Is everything working?",
        explain: "A health check of yapacc on this machine: one line per check.",
        action: HomeAction::Doctor,
    },
    MenuEntry {
        key: 'u',
        title: "Undo the last change",
        explain: "Take the last package change back; the plan is shown first.",
        action: HomeAction::Undo,
    },
];

/// The menu entry a key opens: its letter, or its position (1-9, 0 = tenth).
fn menu_action(c: char) -> Option<HomeAction> {
    let c = c.to_ascii_lowercase();
    if let Some(d) = c.to_digit(10) {
        let idx = if d == 0 { 9 } else { d as usize - 1 };
        return MENU.get(idx).map(|m| m.action);
    }
    MENU.iter().find(|m| m.key == c).map(|m| m.action)
}

// ---------------------------------------------------------------------------
// core invocations (pure argv builders)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Report {
    Timeline,
    Briefing,
    Why,
    Clean,
    Restart,
    Configs,
    Doctor,
}

fn report_title(kind: Report, arg: &str) -> String {
    match kind {
        Report::Timeline => "What changed recently".to_string(),
        Report::Briefing => "What the next upgrade will do".to_string(),
        Report::Why => format!("Why is {arg} installed"),
        Report::Clean => "What can be cleaned up".to_string(),
        Report::Restart => "What needs restarting".to_string(),
        Report::Configs => "Unmerged configuration files".to_string(),
        Report::Doctor => "Is everything working".to_string(),
    }
}

/// The read command behind a pane. Always `--json`, never a gate flag.
fn report_argv(kind: Report, arg: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["--json".into()];
    match kind {
        Report::Timeline => argv.extend([
            "timeline".to_string(),
            "--limit".to_string(),
            "500".to_string(),
        ]),
        Report::Briefing => argv.push("briefing".into()),
        Report::Why => {
            argv.push("why".into());
            argv.push("--".into());
            argv.push(arg.to_string());
        }
        Report::Clean => argv.push("clean".into()),
        Report::Restart => argv.push("restart".into()),
        Report::Configs => argv.push("configs".into()),
        Report::Doctor => argv.push("doctor".into()),
    }
    argv
}

/// A change to the system. The interface only ever plans it (`--dryrun`) or
/// hands it to the core on the real terminal; the core's gate asks.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    Install { name: String, aur: bool },
    Remove(Vec<String>),
    Upgrade,
    Undo,
    RemoveOrphans,
    RemoveExternal(String),
    MarkExplicit(Vec<String>),
}

fn change_title(c: &Change) -> String {
    match c {
        Change::Install { name, .. } => format!("Install {name}"),
        Change::Remove(names) => format!("Remove {}", names.join(", ")),
        Change::Upgrade => "Upgrade the whole system".to_string(),
        Change::Undo => "Undo the last change".to_string(),
        Change::RemoveOrphans => "Remove packages nothing needs any more".to_string(),
        Change::RemoveExternal(id) => format!("Remove the external install {id}"),
        Change::MarkExplicit(names) => {
            format!("Mark as installed on purpose: {}", names.join(", "))
        }
    }
}

/// The core command for a change. With `dryrun` it only plans. It never
/// contains `--yes` or `--noconfirm`: consent is asked by the core, on the
/// real terminal. Names go after `--` so that none can be read as a flag.
fn change_argv(c: &Change, dryrun: bool) -> Vec<String> {
    let (head, names): (Vec<&str>, Vec<String>) = match c {
        Change::Install { name, aur: false } => (vec!["install", "repo"], vec![name.clone()]),
        Change::Install { name, aur: true } => (vec!["install", "aur"], vec![name.clone()]),
        Change::Remove(names) => (vec!["remove", "repo"], names.clone()),
        Change::Upgrade => (vec!["upgrade"], Vec::new()),
        Change::Undo => (vec!["undo"], Vec::new()),
        Change::RemoveOrphans => (vec!["remove", "orphans"], Vec::new()),
        Change::RemoveExternal(id) => (vec!["remove", "external"], vec![id.clone()]),
        Change::MarkExplicit(names) => (vec!["mark", "explicit"], names.clone()),
    };
    let mut argv: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    if dryrun {
        argv.push("--dryrun".into());
    }
    if !names.is_empty() {
        argv.push("--".into());
        argv.extend(names);
    }
    argv
}

/// True when a typed command line carries a flag that consents in advance.
fn passes_consent(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "--yes" || a == "--noconfirm" || a == "-y")
}

/// The command word of a typed line: the first argument that is not one of
/// the core's global options (`--config` takes a value).
fn typed_verb(args: &[String]) -> Option<&str> {
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "--config" => {
                it.next();
            }
            "--json" | "--script" | "--noconfirm" => {}
            a if a.starts_with("--config=") => {}
            a => return Some(a),
        }
    }
    None
}

/// Why a typed line is not run from the Run tab, when it is not: commands
/// that start a program of their own on this terminal. `yapacc i` would
/// start a second interface nobody sees that takes the keyboard; `yapacc
/// mcp` would wait for a protocol client on a closed input.
fn refused_in_run_tab(args: &[String]) -> Option<&'static str> {
    match typed_verb(args)? {
        "i" | "interactive" => Some(
            "This is the interactive interface already: `i` is not run from inside it.\n\
             Press Esc to go back to the home screen.",
        ),
        "mcp" => Some(
            "`mcp` starts the server an AI client talks to over standard input; it has no \
             use inside this interface.\nRun `yapacc mcp` from the client's configuration \
             instead.",
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// running the core
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct CoreOut {
    ok: bool,
    stdout: String,
    stderr: String,
    status: String,
}

/// The core argv with the `--config PATH` this interface was started with
/// placed first, so that the core plans and applies under the same
/// configuration (snapshot policy, ledger paths, sandbox) as the one loaded
/// here. Left alone when the typed arguments carry their own `--config`.
fn with_config(config: Option<&str>, args: &[String]) -> Vec<String> {
    let mut v = Vec::with_capacity(args.len() + 2);
    if let Some(c) = config
        && !names_a_config(args)
    {
        v.push("--config".to_string());
        v.push(c.to_string());
    }
    v.extend(args.iter().cloned());
    v
}

/// True when a typed command line names a configuration file itself.
fn names_a_config(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "--config" || a.starts_with("--config="))
}

/// A core command, ready to run: the forwarded `--config`, and the mark
/// that says the configuration warnings were already printed (by this
/// interface, when it started; a line typed with another `--config` is
/// about another file, whose warnings the core prints itself).
fn core_command(args: &[String]) -> Command {
    let mut cmd = Command::new(crate::util::core_exe());
    cmd.args(with_config(crate::config::explicit_path(), args));
    if !names_a_config(args) {
        cmd.env(crate::config::WARNED_ENV, "1");
    }
    cmd
}

/// Run the core and capture what it prints. No terminal is attached to its
/// input, so it cannot prompt (and so cannot be consented to) from here.
fn run_core(args: &[String]) -> CoreOut {
    let exe = crate::util::core_exe();
    match core_command(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) => CoreOut {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            status: out.status.to_string(),
        },
        Err(e) => CoreOut {
            ok: false,
            stdout: String::new(),
            stderr: format!("could not start the yapacc core ({exe:?}): {e}"),
            status: "not started".to_string(),
        },
    }
}

/// Remove terminal escape sequences and carriage returns; tabs become spaces.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
            }
            '\r' => {}
            '\t' => out.push_str("    "),
            c => out.push(c),
        }
    }
    out
}

/// Everything a captured run printed, as one text.
fn combined_text(out: &CoreOut) -> String {
    let mut s = String::new();
    if !out.stdout.trim().is_empty() {
        s.push_str(out.stdout.trim_end());
    }
    if !out.stderr.trim().is_empty() {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(out.stderr.trim_end());
    }
    if !out.ok {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!("[{}]", out.status));
    }
    if s.trim().is_empty() {
        s.push_str("(no output)");
    }
    clean_text(&s)
}

fn parse_json(out: &CoreOut) -> Option<Value> {
    serde_json::from_str(out.stdout.trim()).ok()
}

// ---------------------------------------------------------------------------
// JSON -> rows (pure; a missing field is "—", never a panic)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone {
    Plain,
    Heading,
    Info,
    Care,
    Reboot,
    Good,
    Dim,
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    cells: Vec<String>,
    tone: Tone,
    /// extra text shown when the row is opened with Enter
    detail: String,
}

impl Row {
    fn line(text: impl Into<String>, tone: Tone) -> Row {
        Row {
            cells: vec![text.into()],
            tone,
            detail: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Table {
    /// column names; empty for a plain list
    header: Vec<String>,
    /// width of every column but the last, which takes what is left
    widths: Vec<usize>,
    rows: Vec<Row>,
}

/// A value as display text; missing, null and empty are "—".
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(true)) => "yes".to_string(),
        Some(Value::Bool(false)) => "no".to_string(),
        Some(Value::Array(a)) if !a.is_empty() => a
            .iter()
            .map(|x| text_of(Some(x)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(o)) if !o.is_empty() => o
            .iter()
            .map(|(k, x)| format!("{k}: {}", text_of(Some(x))))
            .collect::<Vec<_>>()
            .join(", "),
        _ => DASH.to_string(),
    }
}

fn field(v: &Value, key: &str) -> String {
    text_of(v.get(key))
}

/// A name out of a value that may be a string or an object carrying one.
fn name_of(v: Option<&Value>, keys: &[&str]) -> String {
    match v {
        Some(Value::Object(o)) => keys
            .iter()
            .filter_map(|k| o.get(*k))
            .map(|x| text_of(Some(x)))
            .find(|s| s != DASH)
            .unwrap_or_else(|| DASH.to_string()),
        other => text_of(other),
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Unix seconds as `YYYY-MM-DD HH:MM` (UTC); 0 is unknown.
fn fmt_time(ts: u64) -> String {
    if ts == 0 {
        return DASH.to_string();
    }
    let days = (ts / 86_400) as i64;
    let secs = ts % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

fn ts_of(v: &Value) -> u64 {
    match v.get("ts") {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Column widths from the content, each capped; one cap per column but the last.
fn col_widths(header: &[String], rows: &[Row], caps: &[usize]) -> Vec<usize> {
    caps.iter()
        .enumerate()
        .map(|(i, cap)| {
            let head = header.get(i).map(|h| h.chars().count()).unwrap_or(0);
            let body = rows
                .iter()
                .filter_map(|r| r.cells.get(i))
                .map(|c| c.chars().count())
                .max()
                .unwrap_or(0);
            head.max(body).min(*cap).max(1)
        })
        .collect()
}

fn table(header: &[&str], rows: Vec<Row>, caps: &[usize]) -> Table {
    let header: Vec<String> = header.iter().map(|s| s.to_string()).collect();
    let widths = col_widths(&header, &rows, caps);
    Table {
        header,
        widths,
        rows,
    }
}

fn list_table(rows: Vec<Row>) -> Table {
    Table {
        header: Vec::new(),
        widths: Vec::new(),
        rows,
    }
}

/// timeline = array of {ts, source, action, name, old_version, new_version};
/// shown newest first.
fn timeline_table(v: &Value) -> Table {
    let Some(items) = v.as_array() else {
        return generic_table(v);
    };
    let mut items: Vec<&Value> = items.iter().collect();
    items.sort_by_key(|e| std::cmp::Reverse(ts_of(e)));
    let rows: Vec<Row> = items
        .into_iter()
        .map(|e| {
            let (old, new) = (field(e, "old_version"), field(e, "new_version"));
            let versions = match (old.as_str(), new.as_str()) {
                (DASH, DASH) => DASH.to_string(),
                (DASH, n) => n.to_string(),
                (o, DASH) => o.to_string(),
                (o, n) => format!("{o} → {n}"),
            };
            Row {
                cells: vec![
                    fmt_time(ts_of(e)),
                    field(e, "source"),
                    field(e, "action"),
                    field(e, "name"),
                    versions,
                ],
                tone: Tone::Plain,
                detail: String::new(),
            }
        })
        .collect();
    if rows.is_empty() {
        return list_table(vec![Row::line("(no package changes recorded)", Tone::Dim)]);
    }
    table(
        &["TIME (UTC)", "SOURCE", "ACTION", "NAME", "VERSIONS"],
        rows,
        &[16, 10, 10, 32],
    )
}

/// One section of a list pane: a heading, then one line per item ("—" when
/// there is none).
fn section(rows: &mut Vec<Row>, title: &str, items: Vec<Row>) {
    rows.push(Row::line(title, Tone::Heading));
    if items.is_empty() {
        rows.push(Row::line(format!("  {DASH}"), Tone::Dim));
    } else {
        rows.extend(items);
    }
    rows.push(Row::line("", Tone::Plain));
}

fn string_items(v: Option<&Value>) -> Vec<Row> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| Row::line(format!("  {}", text_of(Some(x))), Tone::Plain))
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => {
            vec![Row::line(format!("  {}", s.trim()), Tone::Plain)]
        }
        _ => Vec::new(),
    }
}

/// briefing: the attention flags first (toned by level), then security,
/// news, configuration, disk, safety net; then the packages and the notes.
fn briefing_table(v: &Value) -> Table {
    if !v.is_object() {
        return generic_table(v);
    }
    let mut rows = Vec::new();
    let flags: Vec<Row> = v
        .get("flags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| {
                    let level = field(f, "level");
                    let tone = match level.as_str() {
                        "reboot" => Tone::Reboot,
                        "care" => Tone::Care,
                        _ => Tone::Info,
                    };
                    Row::line(format!("  [{level}] {}", field(f, "text")), tone)
                })
                .collect()
        })
        .unwrap_or_default();
    section(&mut rows, "Needs your attention", flags);
    section(&mut rows, "Security fixes", string_items(v.get("security")));
    let news: Vec<Row> = v
        .get("news")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|n| {
                    let text = match n {
                        Value::Array(pair) => {
                            format!("{}  {}", text_of(pair.get(1)), text_of(pair.first()))
                        }
                        Value::Object(_) => format!("{}  {}", field(n, "date"), field(n, "title")),
                        other => text_of(Some(other)),
                    };
                    Row::line(format!("  {text}"), Tone::Plain)
                })
                .collect()
        })
        .unwrap_or_default();
    section(&mut rows, "News from the distribution", news);
    section(
        &mut rows,
        "Unmerged configuration files",
        string_items(v.get("unmerged_config")),
    );
    section(&mut rows, "Disk", string_items(v.get("disk")));
    section(&mut rows, "Safety net", string_items(v.get("safety_net")));
    let updates: Vec<Row> = v
        .get("updates")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|u| {
                    Row::line(
                        format!(
                            "  {}  {} → {}  ({})",
                            field(u, "name"),
                            field(u, "old_version"),
                            field(u, "new_version"),
                            field(u, "repo")
                        ),
                        Tone::Plain,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let title = format!(
        "Packages that will change ({}, through {})",
        updates.len(),
        field(v, "manager")
    );
    section(&mut rows, &title, updates);
    section(&mut rows, "Notes", string_items(v.get("notes")));
    list_table(rows)
}

/// clean = array of {what, size_bytes, count, safety, command, detail, note}.
fn clean_table(v: &Value) -> Table {
    let Some(items) = v.as_array() else {
        return generic_table(v);
    };
    let rows: Vec<Row> = items
        .iter()
        .map(|e| {
            let size = match e.get("size_bytes").and_then(Value::as_u64) {
                Some(b) => human_size(b),
                None => DASH.to_string(),
            };
            let mut detail = format!("note: {}", field(e, "note"));
            if let Some(list) = e.get("detail").and_then(Value::as_array) {
                for d in list {
                    detail.push_str(&format!("\n  {}", text_of(Some(d))));
                }
            }
            Row {
                cells: vec![
                    field(e, "what"),
                    size,
                    field(e, "count"),
                    field(e, "safety"),
                    field(e, "command"),
                ],
                tone: Tone::Plain,
                detail,
            }
        })
        .collect();
    if rows.is_empty() {
        return list_table(vec![Row::line("(nothing to clean up)", Tone::Dim)]);
    }
    table(
        &["WHAT", "SIZE", "COUNT", "SAFETY", "COMMAND"],
        rows,
        &[30, 10, 6, 12],
    )
}

/// doctor = array of {name, state, detail}.
fn doctor_table(v: &Value) -> Table {
    let Some(items) = v.as_array() else {
        return generic_table(v);
    };
    let rows: Vec<Row> = items
        .iter()
        .map(|e| {
            let state = field(e, "state");
            let tone = match state.as_str() {
                "ok" => Tone::Good,
                "warn" => Tone::Care,
                "fail" => Tone::Reboot,
                "n/a" => Tone::Dim,
                _ => Tone::Plain,
            };
            Row {
                cells: vec![state, field(e, "name"), field(e, "detail")],
                tone,
                detail: String::new(),
            }
        })
        .collect();
    if rows.is_empty() {
        return list_table(vec![Row::line("(no checks reported)", Tone::Dim)]);
    }
    table(&["STATE", "CHECK", "DETAIL"], rows, &[5, 24])
}

fn generic_lines(v: &Value, indent: usize, out: &mut Vec<Row>) {
    let pad = "  ".repeat(indent);
    match v {
        Value::Object(o) if !o.is_empty() => {
            for (k, x) in o {
                match x {
                    Value::Object(inner) if !inner.is_empty() => {
                        out.push(Row::line(format!("{pad}{k}:"), Tone::Heading));
                        generic_lines(x, indent + 1, out);
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        out.push(Row::line(format!("{pad}{k}:"), Tone::Heading));
                        generic_lines(x, indent + 1, out);
                    }
                    other => out.push(Row::line(
                        format!("{pad}{k}: {}", text_of(Some(other))),
                        Tone::Plain,
                    )),
                }
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for x in a {
                match x {
                    Value::Object(inner) if !inner.is_empty() => {
                        generic_lines(x, indent, out);
                        out.push(Row::line("", Tone::Plain));
                    }
                    Value::Array(inner) if !inner.is_empty() => out.push(Row::line(
                        format!("{pad}- {}", text_of(Some(x))),
                        Tone::Plain,
                    )),
                    other => out.push(Row::line(
                        format!("{pad}- {}", text_of(Some(other))),
                        Tone::Plain,
                    )),
                }
            }
        }
        other => out.push(Row::line(
            format!("{pad}{}", text_of(Some(other))),
            Tone::Plain,
        )),
    }
}

/// Any JSON as "key: value" lines (used for why, restart and configs).
fn generic_table(v: &Value) -> Table {
    let mut rows = Vec::new();
    let empty = match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    };
    if empty {
        rows.push(Row::line("(nothing to report)", Tone::Dim));
    } else {
        generic_lines(v, 0, &mut rows);
    }
    list_table(rows)
}

/// The pane for a report, from what the core printed.
fn build_table(kind: Report, out: &CoreOut) -> Table {
    match parse_json(out) {
        Some(v) => match kind {
            Report::Timeline => timeline_table(&v),
            Report::Briefing => briefing_table(&v),
            Report::Clean => clean_table(&v),
            Report::Doctor => doctor_table(&v),
            Report::Why | Report::Restart | Report::Configs => generic_table(&v),
        },
        None => {
            let mut rows = vec![
                Row::line("The yapacc core did not return a report.", Tone::Care),
                Row::line("", Tone::Plain),
            ];
            rows.extend(
                combined_text(out)
                    .lines()
                    .map(|l| Row::line(l.to_string(), Tone::Plain)),
            );
            list_table(rows)
        }
    }
}

/// Indices of the rows containing `needle` in any cell (case-insensitive);
/// every row when the needle is empty.
fn filter_rows(rows: &[Row], needle: &str) -> Vec<usize> {
    let n = needle.trim().to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| n.is_empty() || r.cells.iter().any(|c| c.to_lowercase().contains(&n)))
        .map(|(i, _)| i)
        .collect()
}

/// One row with its column names, for the Enter popup.
fn row_detail(t: &Table, row: &Row) -> String {
    let mut s = String::new();
    for (i, c) in row.cells.iter().enumerate() {
        match t.header.get(i) {
            Some(h) => s.push_str(&format!("{h}: {c}\n")),
            None => s.push_str(&format!("{}\n", c.trim())),
        }
    }
    if !row.detail.is_empty() {
        s.push('\n');
        s.push_str(&row.detail);
    }
    s
}

/// The machine line of the home screen, from `--json platform`.
fn machine_summary(v: &Value) -> String {
    let distro = name_of(v.get("os"), &["pretty_name", "name", "id"]);
    let manager = name_of(v.get("native_pkg_manager"), &["name", "id"]);
    let tier = match v.get("support_tier") {
        Some(Value::Number(n)) if n.as_u64() == Some(0) => "none".to_string(),
        other => text_of(other),
    };
    format!("Distribution: {distro}  ·  Package manager: {manager}  ·  Support tier: {tier}")
}

/// Where a row comes from, which decides what can describe it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    /// the installed list
    Local,
    /// a pacman sync repository (`search`, pacman shape)
    Repo,
    /// the AUR (`search --all`, pacman shape)
    Aur,
    /// another native manager's search (portable shape)
    Native,
}

#[derive(Clone, Debug, PartialEq)]
struct PkgRow {
    name: String,
    version: String,
    size: String,
    reason: String,
    desc: String,
    origin: Origin,
    installed: bool,
}

/// `--json list`, in either shape: pacman's {name, version, description,
/// installed_size, reason, repo} or the portable {name, version, arch,
/// size_bytes, description, explicit}.
fn list_rows(v: &Value) -> Vec<PkgRow> {
    let Some(items) = v.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|e| {
            let size = match (e.get("installed_size"), e.get("size_bytes")) {
                (Some(Value::String(s)), _) if !s.trim().is_empty() => s.trim().to_string(),
                (_, Some(Value::Number(n))) => match n.as_u64() {
                    Some(0) | None => DASH.to_string(),
                    Some(b) => human_size(b),
                },
                _ => DASH.to_string(),
            };
            let reason = match (e.get("reason"), e.get("explicit")) {
                (Some(Value::String(s)), _) if !s.trim().is_empty() => s.trim().to_string(),
                (_, Some(Value::Bool(true))) => "explicit".to_string(),
                (_, Some(Value::Bool(false))) => "dependency".to_string(),
                _ => DASH.to_string(),
            };
            PkgRow {
                name: field(e, "name"),
                version: field(e, "version"),
                size,
                reason,
                desc: field(e, "description"),
                origin: Origin::Local,
                installed: true,
            }
        })
        .collect()
}

/// `--json search`, in either shape; `reason` holds where the result comes
/// from (pacman: `source`, e.g. "aur"; portable: `repo`).
fn search_rows(v: &Value) -> Vec<PkgRow> {
    let Some(items) = v.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|e| {
            let source = field(e, "source");
            let origin = match source.as_str() {
                "aur" => Origin::Aur,
                s if s == DASH => Origin::Native,
                _ => Origin::Repo,
            };
            let shown = if source == DASH {
                field(e, "repo")
            } else {
                source
            };
            PkgRow {
                name: field(e, "name"),
                version: field(e, "version"),
                size: field(e, "download_size"),
                reason: shown,
                desc: field(e, "description"),
                origin,
                installed: e.get("installed").and_then(Value::as_bool) == Some(true),
            }
        })
        .collect()
}

/// The search command of the Search tab. `aur` adds the AUR (`--all`),
/// which sends the query to aur.archlinux.org; it is off until asked for.
fn search_argv(query: &str, aur: bool) -> Vec<String> {
    let mut argv: Vec<String> = ["--json", "search", "--limit", "50"]
        .map(String::from)
        .to_vec();
    if aur {
        argv.push("--all".into());
    }
    argv.push("--".into());
    argv.push(query.to_string());
    argv
}

/// What describes a search result.
#[derive(Debug, PartialEq)]
enum SearchInfo {
    /// a read command of the core
    Core(Vec<String>),
    /// all there is to say is in the row itself
    Text(String),
}

/// `info` describes an installed package only, so it is used for a result
/// that is installed. One that is not is described from the sync metadata
/// (`query`, pacman) or, with another package manager, from the result
/// itself, which is everything its search returned.
fn search_info(row: &PkgRow, installed: bool) -> SearchInfo {
    let name = row.name.clone();
    if installed {
        return SearchInfo::Core(vec!["info".into(), "--".into(), name]);
    }
    match row.origin {
        Origin::Aur => SearchInfo::Core(vec!["query".into(), "--aur".into(), "--".into(), name]),
        Origin::Repo => SearchInfo::Core(vec!["query".into(), "--repo".into(), "--".into(), name]),
        Origin::Native | Origin::Local => SearchInfo::Text(format!(
            "{name} {}\nfrom: {}\n\n{}\n\nNot installed. Press a on the result to see what installing it would do.",
            row.version, row.reason, row.desc
        )),
    }
}

// ---------------------------------------------------------------------------
// text fitting (pure)
// ---------------------------------------------------------------------------

/// At most `n` characters, ending in "…" when cut.
fn tr(s: &str, n: usize) -> String {
    if n == 0 {
        String::new()
    } else if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

/// Shrink the fixed columns so that the last one keeps a share of `total`.
fn scale_widths(widths: &[usize], total: usize) -> Vec<usize> {
    let fixed: usize = widths.iter().sum::<usize>() + widths.len();
    let budget = total * 3 / 4;
    if fixed <= budget || fixed == 0 {
        return widths.to_vec();
    }
    widths.iter().map(|w| (w * budget / fixed).max(1)).collect()
}

/// Cells laid out in columns and cut to `total` characters.
fn fit_row(cells: &[String], widths: &[usize], total: usize) -> String {
    let mut out = String::new();
    for (i, c) in cells.iter().enumerate() {
        if i + 1 == cells.len() {
            out.push_str(c);
        } else {
            let w = widths.get(i).copied().unwrap_or(12);
            out.push_str(&format!("{:<w$} ", tr(c, w), w = w));
        }
    }
    tr(&out, total)
}

/// NO_COLOR (https://no-color.org): set and not empty means no colours.
fn color_enabled(no_color: Option<&str>) -> bool {
    !matches!(no_color, Some(v) if !v.is_empty())
}

fn tone_style(tone: Tone, color: bool) -> Style {
    let base = Style::default();
    match tone {
        Tone::Heading => base.add_modifier(Modifier::BOLD),
        Tone::Plain | Tone::Info => base,
        Tone::Reboot if !color => base.add_modifier(Modifier::BOLD),
        _ if !color => base,
        Tone::Reboot => base.fg(Color::Red),
        Tone::Care => base.fg(Color::Yellow),
        Tone::Good => base.fg(Color::Green),
        Tone::Dim => base.fg(Color::DarkGray),
    }
}

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Screen {
    Home,
    Browse,
    Report,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tab {
    Packages,
    Search,
    External,
    Docs,
    Tree,
    Run,
}

const TABS: [(&str, &str); 6] = [
    (
        "1:Installed",
        "j/k move  / filter  space select  i why  d deps  x remove  e mark explicit  r refresh",
    ),
    (
        "2:Search",
        "type, Enter searches  j/k move  a install  i info  A with/without the AUR  / edit the query",
    ),
    ("3:External", "j/k move  Enter details  x remove  r refresh"),
    ("4:Docs", "j/k topics  Enter open  Esc back"),
    ("5:Tree", "j/k move  Enter open  Esc back"),
    (
        "6:Run",
        "type any yapacc arguments, Enter runs them  Esc clears / leaves",
    ),
];

#[derive(Clone, Copy, PartialEq)]
enum TreeLevel {
    Tops,
    Subs,
    Packages,
}

enum TreeItem {
    Cat(String),
    Sub(String),
    Pkg {
        name: String,
        desc: String,
        installed: bool,
    },
}

struct Popup {
    title: String,
    body: String,
    scroll: u16,
    /// the background run that will fill the body, while it is running
    job: Option<u64>,
}

struct Input {
    title: String,
    buf: String,
}

struct ReportView {
    kind: Report,
    arg: String,
    job: u64,
    loading: bool,
    table: Table,
    filter: String,
    filter_edit: bool,
    shown: Vec<usize>,
    state: ListState,
    page: usize,
}

struct Preview {
    change: Change,
    job: u64,
    loading: bool,
    /// the dry run succeeded, so the plan can be trusted
    ok: bool,
    text: String,
    scroll: u16,
    focus_apply: bool,
}

impl Preview {
    fn can_apply(&self) -> bool {
        !self.loading && self.ok
    }
}

/// Where the output of a background run goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dest {
    Platform,
    List,
    Search,
    Report,
    Preview,
    Popup,
    Run,
}

struct JobMsg {
    id: u64,
    dest: Dest,
    out: CoreOut,
}

/// What the event loop does after a key.
#[derive(Clone, Debug, PartialEq)]
enum Flow {
    Continue,
    Quit,
    Apply(Change),
}

struct App {
    screen: Screen,
    color: bool,
    machine: String,
    home_state: ListState,
    status: String,
    // background runs
    tx: Sender<JobMsg>,
    rx: Receiver<JobMsg>,
    job_seq: u64,
    // browse
    tab: Tab,
    pkgs: Vec<PkgRow>,
    pkgs_loading: bool,
    pkgs_error: Option<String>,
    pkg_filter: String,
    pkg_filtered: Vec<usize>,
    pkg_state: ListState,
    pkg_selected: Vec<String>,
    filter_edit: bool,
    // search
    query: String,
    search_edit: bool,
    results: Vec<PkgRow>,
    search_state: ListState,
    search_job: u64,
    search_loading: bool,
    search_error: Option<String>,
    /// the search also asks the AUR (the query then leaves this machine)
    search_aur: bool,
    /// pacman is this machine's package manager (from `--json platform`)
    pacman: bool,
    // docs / run text panes
    text: String,
    text_title: String,
    scroll: u16,
    // external (ledger)
    records: Vec<ledger::InstallRecord>,
    ext_state: ListState,
    // docs
    doc_topics: Vec<String>,
    doc_state: ListState,
    // tree browser: breadcrumb navigation tops -> subs -> packages
    tree_level: TreeLevel,
    tree_top: String,
    tree_sub: String,
    tree_state: ListState,
    tree_items: Vec<TreeItem>,
    /// the not-installed packages of the open subcategory are worked out in
    /// a background thread (it classifies the whole repository index)
    tree_tx: Sender<(u64, Vec<String>)>,
    tree_rx: Receiver<(u64, Vec<String>)>,
    tree_job: u64,
    tree_loading: bool,
    // run
    run_input: String,
    run_job: u64,
    // report pane
    report: Option<ReportView>,
    report_back: Screen,
    // overlays
    preview: Option<Preview>,
    input: Option<Input>,
    popup: Option<Popup>,
    help: bool,
}

fn first_or_none(len: usize) -> Option<usize> {
    if len == 0 { None } else { Some(0) }
}

impl App {
    /// The initial state. Touches nothing outside the process; `start` does.
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (tree_tx, tree_rx) = mpsc::channel();
        let tree_items: Vec<TreeItem> = crate::taxonomy::tops()
            .into_iter()
            .map(TreeItem::Cat)
            .collect();
        let mut app = App {
            screen: Screen::Home,
            color: true,
            machine: "reading this machine…".to_string(),
            home_state: ListState::default(),
            status: String::new(),
            tx,
            rx,
            job_seq: 0,
            tab: Tab::Packages,
            pkgs: Vec::new(),
            pkgs_loading: false,
            pkgs_error: None,
            pkg_filter: String::new(),
            pkg_filtered: Vec::new(),
            pkg_state: ListState::default(),
            pkg_selected: Vec::new(),
            filter_edit: false,
            query: String::new(),
            search_edit: true,
            results: Vec::new(),
            search_state: ListState::default(),
            search_job: 0,
            search_loading: false,
            search_error: None,
            search_aur: false,
            pacman: false,
            text: String::new(),
            text_title: String::new(),
            scroll: 0,
            records: Vec::new(),
            ext_state: ListState::default(),
            doc_topics: docs::topics()
                .iter()
                .map(|(s, _, _)| s.to_string())
                .collect(),
            doc_state: ListState::default(),
            tree_level: TreeLevel::Tops,
            tree_top: String::new(),
            tree_sub: String::new(),
            tree_state: ListState::default(),
            tree_items,
            tree_tx,
            tree_rx,
            tree_job: 0,
            tree_loading: false,
            run_input: String::new(),
            run_job: 0,
            report: None,
            report_back: Screen::Home,
            preview: None,
            input: None,
            popup: None,
            help: false,
        };
        app.home_state.select(Some(0));
        app.tree_state.select(first_or_none(app.tree_items.len()));
        app.doc_state.select(first_or_none(app.doc_topics.len()));
        app
    }

    /// Ask the core about this machine and what is installed.
    fn start(&mut self) {
        let no_color = std::env::var("NO_COLOR").ok();
        self.color = color_enabled(no_color.as_deref());
        self.spawn(Dest::Platform, vec!["--json".into(), "platform".into()]);
        self.reload_packages();
        self.refresh_external();
    }

    /// Run the core in a background thread; the result arrives on `rx`.
    fn spawn(&mut self, dest: Dest, args: Vec<String>) -> u64 {
        self.job_seq += 1;
        let id = self.job_seq;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let out = run_core(&args);
            let _ = tx.send(JobMsg { id, dest, out });
        });
        id
    }

    fn reload_packages(&mut self) {
        self.pkgs_loading = true;
        self.spawn(Dest::List, vec!["--json".into(), "list".into()]);
    }

    /// The not-installed packages of a subcategory arrived. They are added
    /// only if that subcategory is still the one on screen.
    fn on_tree(&mut self, id: u64, names: Vec<String>) {
        if id != self.tree_job || self.tree_level != TreeLevel::Packages {
            return;
        }
        self.tree_loading = false;
        self.tree_items
            .extend(names.into_iter().map(|name| TreeItem::Pkg {
                name,
                desc: String::new(),
                installed: false,
            }));
        if self.tree_state.selected().is_none() {
            self.tree_state.select(first_or_none(self.tree_items.len()));
        }
    }

    /// Search for the typed query (nothing happens while it is empty).
    fn run_search(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.search_loading = true;
        self.search_error = None;
        self.search_job = self.spawn(Dest::Search, search_argv(&q, self.search_aur));
    }

    fn on_job(&mut self, msg: JobMsg) {
        match msg.dest {
            Dest::Platform => {
                self.machine = match parse_json(&msg.out) {
                    Some(v) => {
                        self.pacman = name_of(v.get("native_pkg_manager"), &["name", "id"])
                            .eq_ignore_ascii_case("pacman");
                        machine_summary(&v)
                    }
                    None => "this machine could not be described (see \"Is everything working?\")"
                        .to_string(),
                };
            }
            Dest::List => {
                self.pkgs_loading = false;
                match parse_json(&msg.out) {
                    Some(v) => {
                        self.pkgs = list_rows(&v);
                        self.pkgs_error = None;
                    }
                    None => self.pkgs_error = Some(combined_text(&msg.out)),
                }
                self.apply_pkg_filter();
            }
            Dest::Search => {
                if msg.id != self.search_job {
                    return;
                }
                self.search_loading = false;
                match parse_json(&msg.out) {
                    Some(v) => {
                        self.results = search_rows(&v);
                        self.search_error = None;
                    }
                    None => {
                        self.results.clear();
                        self.search_error = Some(combined_text(&msg.out));
                    }
                }
                self.search_state.select(first_or_none(self.results.len()));
            }
            Dest::Report => {
                if let Some(r) = &mut self.report
                    && r.job == msg.id
                {
                    r.loading = false;
                    r.table = build_table(r.kind, &msg.out);
                    r.shown = filter_rows(&r.table.rows, &r.filter);
                    r.state.select(first_or_none(r.shown.len()));
                }
            }
            Dest::Preview => {
                if let Some(p) = &mut self.preview
                    && p.job == msg.id
                {
                    p.loading = false;
                    p.ok = msg.out.ok;
                    p.text = combined_text(&msg.out);
                }
            }
            Dest::Popup => {
                if let Some(p) = &mut self.popup
                    && p.job == Some(msg.id)
                {
                    p.job = None;
                    p.body = combined_text(&msg.out);
                }
            }
            Dest::Run => {
                if msg.id == self.run_job {
                    self.text = combined_text(&msg.out);
                    self.scroll = 0;
                }
            }
        }
    }

    fn refresh_external(&mut self) {
        let mut recs = Vec::new();
        for scope in [&ledger::Scope::User, &ledger::Scope::System] {
            if let Ok(list) = ledger::list_records(&ledger::default_root(scope)) {
                recs.extend(list.into_iter().filter(|r| r.status != "removed"));
            }
        }
        self.records = recs;
        self.ext_state.select(first_or_none(self.records.len()));
    }

    fn apply_pkg_filter(&mut self) {
        let f = self.pkg_filter.to_lowercase();
        self.pkg_filtered = self
            .pkgs
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                f.is_empty()
                    || p.name.to_lowercase().contains(&f)
                    || p.desc.to_lowercase().contains(&f)
            })
            .map(|(i, _)| i)
            .collect();
        self.pkg_state
            .select(first_or_none(self.pkg_filtered.len()));
    }

    fn pkg_at_cursor(&self) -> Option<&PkgRow> {
        self.pkg_state
            .selected()
            .and_then(|s| self.pkg_filtered.get(s))
            .and_then(|&i| self.pkgs.get(i))
    }

    /// The selected packages, or the one under the cursor.
    fn pkg_targets(&self) -> Vec<String> {
        if self.pkg_selected.is_empty() {
            self.pkg_at_cursor()
                .map(|p| vec![p.name.clone()])
                .unwrap_or_default()
        } else {
            self.pkg_selected.clone()
        }
    }

    fn show_text(&mut self, title: &str, body: String) {
        self.popup = Some(Popup {
            title: title.to_string(),
            body,
            scroll: 0,
            job: None,
        });
    }

    /// A popup filled by a read command of the core, run in the background.
    fn show_core_text(&mut self, title: &str, args: Vec<String>) {
        let job = self.spawn(Dest::Popup, args);
        self.popup = Some(Popup {
            title: title.to_string(),
            body: "loading…".to_string(),
            scroll: 0,
            job: Some(job),
        });
    }

    fn open_report(&mut self, kind: Report, arg: &str) {
        let job = self.spawn(Dest::Report, report_argv(kind, arg));
        if self.screen != Screen::Report {
            self.report_back = self.screen;
        }
        self.report = Some(ReportView {
            kind,
            arg: arg.to_string(),
            job,
            loading: true,
            table: Table::default(),
            filter: String::new(),
            filter_edit: false,
            shown: Vec::new(),
            state: ListState::default(),
            page: 10,
        });
        self.screen = Screen::Report;
    }

    fn reload_report(&mut self) {
        let Some((kind, arg)) = self.report.as_ref().map(|r| (r.kind, r.arg.clone())) else {
            return;
        };
        let job = self.spawn(Dest::Report, report_argv(kind, &arg));
        if let Some(r) = &mut self.report {
            r.job = job;
            r.loading = true;
        }
    }

    /// Plan a change with `--dryrun` and show the plan; nothing is changed.
    fn start_preview(&mut self, change: Change) {
        let job = self.spawn(Dest::Preview, change_argv(&change, true));
        self.preview = Some(Preview {
            change,
            job,
            loading: true,
            ok: false,
            text: String::new(),
            scroll: 0,
            focus_apply: false,
        });
    }

    fn go_home(&mut self) {
        self.screen = Screen::Home;
        self.filter_edit = false;
    }

    /// After a change ran on the real terminal: read the system again.
    fn after_change(&mut self) {
        self.pkg_selected.clear();
        self.reload_packages();
        self.refresh_external();
        if self.screen == Screen::Report {
            self.reload_report();
        }
    }

    fn activate(&mut self, action: HomeAction) {
        match action {
            HomeAction::Timeline => self.open_report(Report::Timeline, ""),
            HomeAction::Briefing => self.open_report(Report::Briefing, ""),
            HomeAction::Clean => self.open_report(Report::Clean, ""),
            HomeAction::Restart => self.open_report(Report::Restart, ""),
            HomeAction::Configs => self.open_report(Report::Configs, ""),
            HomeAction::Doctor => self.open_report(Report::Doctor, ""),
            HomeAction::Why => {
                self.input = Some(Input {
                    title: "Why is a package installed? Type its name, then Enter".to_string(),
                    buf: String::new(),
                })
            }
            HomeAction::Browse => {
                self.screen = Screen::Browse;
                self.tab = Tab::Packages;
            }
            HomeAction::Search => {
                self.screen = Screen::Browse;
                self.tab = Tab::Search;
                self.search_edit = true;
            }
            HomeAction::Undo => self.start_preview(Change::Undo),
        }
    }

    /// True while keys are being typed into a text field of the browse area.
    fn browse_typing(&self) -> bool {
        match self.tab {
            Tab::Packages => self.filter_edit,
            Tab::Search => self.search_edit,
            Tab::Run => true,
            _ => false,
        }
    }

    /// True when Esc has nothing left to back out of inside the current tab.
    fn browse_at_top(&self) -> bool {
        match self.tab {
            Tab::Docs => !self.text_title.starts_with("docs:"),
            Tab::Tree => self.tree_level == TreeLevel::Tops,
            Tab::Run => self.run_input.is_empty(),
            _ => true,
        }
    }

    fn typing(&self) -> bool {
        match self.screen {
            Screen::Home => false,
            Screen::Browse => self.browse_typing(),
            Screen::Report => self.report.as_ref().is_some_and(|r| r.filter_edit),
        }
    }
}

// ---------------------------------------------------------------------------
// run loop
// ---------------------------------------------------------------------------

const NO_TERMINAL: &str = "the interactive interface needs a terminal: standard input and standard output must both be one. In a script or a pipe, use the yapacc commands themselves (`yapacc --help` lists them).";

/// Without a terminal there is nothing to draw on and no keyboard to read:
/// that is a missing requirement (exit 6), said in one sentence, and not a
/// panic from the terminal library.
pub fn run() -> crate::error::Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(crate::Error::unavailable(NO_TERMINAL));
    }
    let mut terminal = ratatui::try_init()
        .map_err(|e| crate::Error::unavailable(format!("{NO_TERMINAL} ({e})")))?;
    let mut app = App::new();
    app.start();
    let res = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    res.map_err(crate::Error::from)
}

/// Run a command attached to the terminal and wait for it, with the
/// terminal's interrupt keys belonging to that command alone.
///
/// The interface has left raw mode at this point, so Ctrl-C and Ctrl-\ are
/// signals again, sent to every process in the foreground: this one too.
/// It ignores them while the command runs (the command itself gets the
/// default behaviour back, since an ignored signal would otherwise be
/// inherited), so that Ctrl-C at the core's "Proceed [y/N]?" stops the core
/// and not the interface under it.
fn run_attached(cmd: &mut Command) -> std::io::Result<std::process::ExitStatus> {
    use std::os::unix::process::CommandExt;
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGINT: i32 = 2;
    const SIGQUIT: i32 = 3;
    const SIG_DFL: usize = 0;
    const SIG_IGN: usize = 1;
    // SAFETY: signal(2) is async-signal-safe, and that is all the closure
    // calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            signal(SIGINT, SIG_DFL);
            signal(SIGQUIT, SIG_DFL);
            Ok(())
        });
    }
    // SAFETY: signal(2) with SIG_IGN, then with the value it returned.
    let previous = unsafe { (signal(SIGINT, SIG_IGN), signal(SIGQUIT, SIG_IGN)) };
    let status = cmd.status();
    // SAFETY: as above; restores what was in place before the command ran.
    unsafe {
        signal(SIGINT, previous.0);
        signal(SIGQUIT, previous.1);
    }
    status
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<(), String> {
    loop {
        while let Ok(msg) = app.rx.try_recv() {
            app.on_job(msg);
        }
        while let Ok((id, names)) = app.tree_rx.try_recv() {
            app.on_tree(id, names);
        }
        terminal.draw(|f| draw(f, app)).map_err(|e| e.to_string())?;
        if !event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
            continue;
        }
        if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(app, key.code, key.modifiers) {
                Flow::Continue => {}
                Flow::Quit => return Ok(()),
                Flow::Apply(change) => apply_change(terminal, app, &change)?,
            }
        }
    }
}

/// Leave the interface, run the change through the core on the real
/// terminal (sudo and the core's own gate prompt there), wait for a key,
/// come back.
fn apply_change(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    change: &Change,
) -> Result<(), String> {
    use std::io::Write;
    let argv = change_argv(change, false);
    ratatui::restore();
    println!(
        "\n$ yapacc {}\n",
        with_config(crate::config::explicit_path(), &argv).join(" ")
    );
    let _ = std::io::stdout().flush();
    let status = run_attached(&mut core_command(&argv));
    let msg = match status {
        Ok(s) if s.success() => format!("{}: done.", change_title(change)),
        Ok(s) => format!(
            "{}: the command did not complete ({s}).",
            change_title(change)
        ),
        Err(e) => format!("could not start the yapacc core: {e}"),
    };
    println!("\n{msg}\nPress any key to return to the interface…");
    let _ = std::io::stdout().flush();
    wait_for_key();
    *terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;
    app.status = msg;
    app.after_change();
    Ok(())
}

fn wait_for_key() {
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    if enable_raw_mode().is_err() {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return;
    }
    loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = disable_raw_mode();
}

// ---------------------------------------------------------------------------
// key handling
// ---------------------------------------------------------------------------

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Flow {
    if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
        return Flow::Quit;
    }
    app.status.clear();
    if app.preview.is_some() {
        return handle_preview(app, code);
    }
    if app.input.is_some() {
        handle_input(app, code);
        return Flow::Continue;
    }
    if app.help {
        if matches!(
            code,
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter
        ) {
            app.help = false;
        }
        return Flow::Continue;
    }
    if app.popup.is_some() {
        handle_popup(app, code);
        return Flow::Continue;
    }
    if code == KeyCode::Char('?') && !app.typing() {
        app.help = true;
        return Flow::Continue;
    }
    match app.screen {
        Screen::Home => handle_home(app, code),
        Screen::Report => {
            handle_report(app, code);
            Flow::Continue
        }
        Screen::Browse => {
            handle_browse(app, code);
            Flow::Continue
        }
    }
}

/// New scroll offset of a text pane after a key, or None when the key does
/// not scroll.
fn scroll_key(code: KeyCode, scroll: u16, lines: usize) -> Option<u16> {
    let last = lines.saturating_sub(1).min(u16::MAX as usize) as u16;
    let next = match code {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown => scroll.saturating_add(10),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::Char('g') | KeyCode::Home => 0,
        KeyCode::Char('G') | KeyCode::End => last,
        _ => return None,
    };
    Some(next.min(last))
}

fn handle_preview(app: &mut App, code: KeyCode) -> Flow {
    let Some(p) = app.preview.as_mut() else {
        return Flow::Continue;
    };
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.preview = None,
        KeyCode::Left
        | KeyCode::Right
        | KeyCode::Tab
        | KeyCode::BackTab
        | KeyCode::Char('h')
        | KeyCode::Char('l') => {
            if p.can_apply() {
                p.focus_apply = !p.focus_apply;
            }
        }
        KeyCode::Enter => {
            let apply = p.focus_apply && p.can_apply();
            let change = p.change.clone();
            app.preview = None;
            if apply {
                return Flow::Apply(change);
            }
        }
        other => {
            if let Some(s) = scroll_key(other, p.scroll, p.text.lines().count()) {
                p.scroll = s;
            }
        }
    }
    Flow::Continue
}

fn handle_input(app: &mut App, code: KeyCode) {
    let Some(input) = app.input.as_mut() else {
        return;
    };
    match code {
        KeyCode::Esc => app.input = None,
        KeyCode::Backspace => {
            input.buf.pop();
        }
        KeyCode::Char(c) => input.buf.push(c),
        KeyCode::Enter => {
            let name = input.buf.trim().to_string();
            app.input = None;
            if !name.is_empty() {
                app.open_report(Report::Why, &name);
            }
        }
        _ => {}
    }
}

fn handle_popup(app: &mut App, code: KeyCode) {
    let Some(p) = app.popup.as_mut() else {
        return;
    };
    match code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => app.popup = None,
        other => {
            if let Some(s) = scroll_key(other, p.scroll, p.body.lines().count()) {
                p.scroll = s;
            }
        }
    }
}

fn handle_home(app: &mut App, code: KeyCode) -> Flow {
    match code {
        KeyCode::Char('q') => return Flow::Quit,
        KeyCode::Down | KeyCode::Char('j') => move_state(&mut app.home_state, MENU.len(), 1),
        KeyCode::Up | KeyCode::Char('k') => move_state(&mut app.home_state, MENU.len(), -1),
        KeyCode::Home => move_state(&mut app.home_state, MENU.len(), -(MENU.len() as i32)),
        KeyCode::End => move_state(&mut app.home_state, MENU.len(), MENU.len() as i32),
        KeyCode::Enter => {
            if let Some(m) = app.home_state.selected().and_then(|i| MENU.get(i)) {
                app.activate(m.action);
            }
        }
        KeyCode::Char(c) => {
            if let Some(action) = menu_action(c) {
                if let Some(i) = MENU.iter().position(|m| m.action == action) {
                    app.home_state.select(Some(i));
                }
                app.activate(action);
            }
        }
        _ => {}
    }
    Flow::Continue
}

fn move_state(state: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        state.select(None);
        return;
    }
    let last = len.min(i32::MAX as usize) as i32 - 1;
    let i = state.selected().unwrap_or(0).min(i32::MAX as usize) as i32;
    let n = i.saturating_add(delta).clamp(0, last);
    state.select(Some(n as usize));
}

/// j/k, arrows, PgUp/PgDn, g/G on a list; true when the key was one of them.
fn list_key(state: &mut ListState, len: usize, page: usize, code: KeyCode) -> bool {
    let page = page.clamp(1, 1000) as i32;
    let all = len.min(i32::MAX as usize) as i32;
    match code {
        KeyCode::Down | KeyCode::Char('j') => move_state(state, len, 1),
        KeyCode::Up | KeyCode::Char('k') => move_state(state, len, -1),
        KeyCode::PageDown => move_state(state, len, page),
        KeyCode::PageUp => move_state(state, len, -page),
        KeyCode::Char('g') | KeyCode::Home => move_state(state, len, -all),
        KeyCode::Char('G') | KeyCode::End => move_state(state, len, all),
        _ => return false,
    }
    true
}

fn handle_report(app: &mut App, code: KeyCode) {
    let Some(r) = app.report.as_mut() else {
        app.screen = Screen::Home;
        return;
    };
    if r.filter_edit {
        match code {
            KeyCode::Enter | KeyCode::Esc => r.filter_edit = false,
            KeyCode::Backspace => {
                r.filter.pop();
            }
            KeyCode::Char(c) => r.filter.push(c),
            _ => return,
        }
        r.shown = filter_rows(&r.table.rows, &r.filter);
        r.state.select(first_or_none(r.shown.len()));
        return;
    }
    if list_key(&mut r.state, r.shown.len(), r.page, code) {
        return;
    }
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.screen = app.report_back;
            app.report = None;
        }
        KeyCode::Char('/') => r.filter_edit = true,
        KeyCode::Char('r') => app.reload_report(),
        KeyCode::Enter => {
            let detail = r
                .state
                .selected()
                .and_then(|s| r.shown.get(s))
                .and_then(|&i| r.table.rows.get(i))
                .map(|row| row_detail(&r.table, row));
            if let Some(body) = detail
                && !body.trim().is_empty()
            {
                app.show_text("Details", body);
            }
        }
        KeyCode::Char('u') if r.kind == Report::Briefing => app.start_preview(Change::Upgrade),
        KeyCode::Char('o') if r.kind == Report::Clean => app.start_preview(Change::RemoveOrphans),
        _ => {}
    }
}

fn handle_browse(app: &mut App, code: KeyCode) {
    if code == KeyCode::Tab {
        app.tab = match app.tab {
            Tab::Packages => Tab::Search,
            Tab::Search => Tab::External,
            Tab::External => Tab::Docs,
            Tab::Docs => Tab::Tree,
            Tab::Tree => Tab::Run,
            Tab::Run => Tab::Packages,
        };
        app.filter_edit = false;
        app.search_edit = app.tab == Tab::Search && app.results.is_empty();
        return;
    }
    if !app.browse_typing() {
        match code {
            KeyCode::Char('q') => {
                app.go_home();
                return;
            }
            KeyCode::Char(n @ '1'..='6') => {
                app.tab = match n {
                    '1' => Tab::Packages,
                    '2' => Tab::Search,
                    '3' => Tab::External,
                    '4' => Tab::Docs,
                    '5' => Tab::Tree,
                    _ => Tab::Run,
                };
                app.search_edit = app.tab == Tab::Search && app.results.is_empty();
                return;
            }
            KeyCode::Esc if app.browse_at_top() => {
                app.go_home();
                return;
            }
            _ => {}
        }
    } else if app.tab == Tab::Run && code == KeyCode::Esc && app.browse_at_top() {
        app.go_home();
        return;
    }
    match app.tab {
        Tab::Packages => handle_packages(app, code),
        Tab::Search => handle_search(app, code),
        Tab::External => handle_external(app, code),
        Tab::Docs => handle_docs(app, code),
        Tab::Tree => handle_tree(app, code),
        Tab::Run => handle_run(app, code),
    }
}

fn handle_packages(app: &mut App, code: KeyCode) {
    if app.filter_edit {
        match code {
            KeyCode::Enter | KeyCode::Esc => app.filter_edit = false,
            KeyCode::Backspace => {
                app.pkg_filter.pop();
                app.apply_pkg_filter();
            }
            KeyCode::Char(c) => {
                app.pkg_filter.push(c);
                app.apply_pkg_filter();
            }
            _ => {}
        }
        return;
    }
    if list_key(&mut app.pkg_state, app.pkg_filtered.len(), 10, code) {
        return;
    }
    match code {
        KeyCode::Char('/') => app.filter_edit = true,
        KeyCode::Char('r') => app.reload_packages(),
        KeyCode::Char(' ') => {
            if let Some(p) = app.pkg_at_cursor() {
                let n = p.name.clone();
                if let Some(pos) = app.pkg_selected.iter().position(|x| x == &n) {
                    app.pkg_selected.remove(pos);
                } else {
                    app.pkg_selected.push(n);
                }
            }
        }
        KeyCode::Char('i') | KeyCode::Enter => {
            if let Some(name) = app.pkg_at_cursor().map(|p| p.name.clone()) {
                app.open_report(Report::Why, &name);
            }
        }
        KeyCode::Char('d') => {
            if let Some(name) = app.pkg_at_cursor().map(|p| p.name.clone()) {
                app.show_core_text(
                    "What it depends on",
                    vec!["inspect".into(), "deps".into(), "--".into(), name],
                );
            }
        }
        KeyCode::Char('e') => {
            let names = app.pkg_targets();
            if !names.is_empty() {
                app.start_preview(Change::MarkExplicit(names));
            }
        }
        KeyCode::Char('x') => {
            let names = app.pkg_targets();
            if !names.is_empty() {
                app.start_preview(Change::Remove(names));
            }
        }
        _ => {}
    }
}

fn handle_search(app: &mut App, code: KeyCode) {
    if app.search_edit {
        match code {
            KeyCode::Esc => app.search_edit = false,
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Enter => {
                if app.query.trim().is_empty() {
                    return;
                }
                app.search_edit = false;
                app.run_search();
            }
            KeyCode::Char(c) => app.query.push(c),
            _ => {}
        }
        return;
    }
    if list_key(&mut app.search_state, app.results.len(), 10, code) {
        return;
    }
    let at_cursor = app
        .search_state
        .selected()
        .and_then(|s| app.results.get(s))
        .cloned();
    match code {
        KeyCode::Char('/') => app.search_edit = true,
        KeyCode::Char('A') => {
            if !app.pacman {
                app.status = "The AUR exists only on the Arch family.".to_string();
                return;
            }
            app.search_aur = !app.search_aur;
            app.status = if app.search_aur {
                "Searches now include the AUR: the query is sent to aur.archlinux.org."
            } else {
                "Searches no longer include the AUR."
            }
            .to_string();
            app.run_search();
        }
        KeyCode::Char('i') | KeyCode::Enter => {
            if let Some(row) = at_cursor {
                let installed = row.installed || app.pkgs.iter().any(|p| p.name == row.name);
                match search_info(&row, installed) {
                    SearchInfo::Core(argv) => app.show_core_text("About this package", argv),
                    SearchInfo::Text(body) => app.show_text("About this package", body),
                }
            }
        }
        KeyCode::Char('a') => {
            if let Some(row) = at_cursor {
                app.start_preview(install_change(&row));
            }
        }
        _ => {}
    }
}

/// Installing a search result: through the AUR lane when that is where it
/// was found, through the native package manager otherwise.
fn install_change(row: &PkgRow) -> Change {
    Change::Install {
        name: row.name.clone(),
        aur: row.origin == Origin::Aur,
    }
}

fn handle_external(app: &mut App, code: KeyCode) {
    if list_key(&mut app.ext_state, app.records.len(), 10, code) {
        return;
    }
    match code {
        KeyCode::Char('r') => app.refresh_external(),
        KeyCode::Enter => {
            if let Some(r) = app.ext_state.selected().and_then(|s| app.records.get(s)) {
                let body = serde_json::to_string_pretty(r).unwrap_or_else(|e| e.to_string());
                app.show_text("Ledger record", body);
            }
        }
        KeyCode::Char('x') => {
            if let Some(id) = app
                .ext_state
                .selected()
                .and_then(|s| app.records.get(s))
                .map(|r| r.id.clone())
            {
                app.start_preview(Change::RemoveExternal(id));
            }
        }
        _ => {}
    }
}

fn handle_docs(app: &mut App, code: KeyCode) {
    if app.text_title.starts_with("docs:") {
        if code == KeyCode::Esc {
            app.text_title.clear();
            app.scroll = 0;
        } else if let Some(s) = scroll_key(code, app.scroll, app.text.lines().count()) {
            app.scroll = s;
        }
        return;
    }
    if list_key(&mut app.doc_state, app.doc_topics.len(), 10, code) {
        return;
    }
    if code == KeyCode::Enter
        && let Some(t) = app.doc_state.selected().and_then(|s| app.doc_topics.get(s))
        && let Some(body) = docs::render(t)
    {
        app.text = body;
        app.text_title = format!("docs: {t}");
        app.scroll = 0;
    }
}

fn handle_tree(app: &mut App, code: KeyCode) {
    if list_key(&mut app.tree_state, app.tree_items.len(), 10, code) {
        return;
    }
    match code {
        KeyCode::Esc => match app.tree_level {
            TreeLevel::Packages => {
                // a list still being worked out is for a pane no longer shown
                app.tree_job = 0;
                app.tree_loading = false;
                app.tree_level = TreeLevel::Subs;
                app.tree_items = crate::taxonomy::subs(&app.tree_top)
                    .unwrap_or_default()
                    .into_iter()
                    .map(TreeItem::Sub)
                    .collect();
                app.tree_state.select(first_or_none(app.tree_items.len()));
            }
            TreeLevel::Subs => {
                app.tree_level = TreeLevel::Tops;
                app.tree_items = crate::taxonomy::tops()
                    .into_iter()
                    .map(TreeItem::Cat)
                    .collect();
                app.tree_state.select(first_or_none(app.tree_items.len()));
            }
            TreeLevel::Tops => {}
        },
        KeyCode::Enter => {
            let Some(sel) = app.tree_state.selected() else {
                return;
            };
            let Some(item) = app.tree_items.get(sel) else {
                return;
            };
            match (app.tree_level, item) {
                (TreeLevel::Tops, TreeItem::Cat(top)) => {
                    app.tree_top = top.clone();
                    app.tree_level = TreeLevel::Subs;
                    app.tree_items = crate::taxonomy::subs(&app.tree_top)
                        .unwrap_or_default()
                        .into_iter()
                        .map(TreeItem::Sub)
                        .collect();
                    app.tree_state.select(first_or_none(app.tree_items.len()));
                }
                (TreeLevel::Subs, TreeItem::Sub(sub)) => {
                    app.tree_sub = sub.clone();
                    app.tree_level = TreeLevel::Packages;
                    let full = format!("{}.{}", app.tree_top, app.tree_sub);
                    // installed software: what the core listed, on any manager
                    let pkgs: Vec<TreeItem> = app
                        .pkgs
                        .iter()
                        .filter(|p| crate::taxonomy::classify_pkg(&p.name, &p.desc) == full)
                        .map(|p| TreeItem::Pkg {
                            name: p.name.clone(),
                            desc: p.desc.clone(),
                            installed: true,
                        })
                        .collect();
                    // not installed: only where a pacman sync index exists
                    // (elsewhere this is an error and the list stays as it
                    // is). Reading and classifying the whole index takes
                    // about a second, so it is done off the interface's
                    // thread and added when it arrives.
                    let have: std::collections::HashSet<String> = pkgs
                        .iter()
                        .filter_map(|i| match i {
                            TreeItem::Pkg { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect();
                    app.job_seq += 1;
                    let id = app.job_seq;
                    app.tree_job = id;
                    app.tree_loading = true;
                    let tx = app.tree_tx.clone();
                    std::thread::spawn(move || {
                        let names: Vec<String> = backend::build_repo_index(0)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|r| r.1)
                            .filter(|n| {
                                !have.contains(n) && crate::taxonomy::classify_pkg(n, "") == full
                            })
                            .collect();
                        let _ = tx.send((id, names));
                    });
                    app.tree_items = pkgs;
                    app.tree_state.select(first_or_none(app.tree_items.len()));
                }
                (
                    TreeLevel::Packages,
                    TreeItem::Pkg {
                        name, installed, ..
                    },
                ) => {
                    // not-installed entries come from the pacman sync index
                    let row = PkgRow {
                        name: name.clone(),
                        version: DASH.to_string(),
                        size: DASH.to_string(),
                        reason: DASH.to_string(),
                        desc: String::new(),
                        origin: Origin::Repo,
                        installed: *installed,
                    };
                    if let SearchInfo::Core(argv) = search_info(&row, *installed) {
                        app.show_core_text("About this package", argv);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn handle_run(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.run_input.clear(),
        KeyCode::Backspace => {
            app.run_input.pop();
        }
        KeyCode::Enter => {
            let input = app.run_input.trim().to_string();
            if input.is_empty() {
                return;
            }
            let argv: Vec<String> = input.split_whitespace().map(|s| s.to_string()).collect();
            app.text_title = format!("$ yapacc {input}");
            app.scroll = 0;
            app.run_input.clear();
            if passes_consent(&argv) {
                app.run_job = 0;
                app.text = "This interface never agrees to a change on your behalf, so it does \
                            not run commands with --yes or --noconfirm.\nAdd --dryrun to see the \
                            plan here, or run the command in a shell, where yapacc can ask you."
                    .to_string();
                return;
            }
            if let Some(why) = refused_in_run_tab(&argv) {
                app.run_job = 0;
                app.text = why.to_string();
                return;
            }
            app.text = "running…".to_string();
            app.run_job = app.spawn(Dest::Run, argv);
        }
        KeyCode::Down | KeyCode::Up | KeyCode::PageDown | KeyCode::PageUp => {
            if let Some(s) = scroll_key(code, app.scroll, app.text.lines().count()) {
                app.scroll = s;
            }
        }
        KeyCode::Char(c) => app.run_input.push(c),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

const HELP: &str = "\
Everywhere
  ?            this help
  Esc or q     back (q on the home screen quits)
  Ctrl-C       quit at once
  j/k, arrows  move      PgUp/PgDn  a page      g/G  first / last

Home
  Enter opens the highlighted line; each line also has its own letter.

Report panes (timeline, briefing, clean, doctor…)
  /            filter by text       r  read again
  Enter        the whole line, when it is cut
  u            (upgrade briefing) plan the upgrade
  o            (clean up) plan the removal of unneeded packages

Browse (Tab or 1-6 changes tab)
  Installed    / filter  space select  i why  d deps  x remove  e mark explicit
  Search       type, Enter  a install  i info  / edit the query
               A  also search the AUR (Arch family; the query is then sent
                  to aur.archlinux.org), A again to stop
  External     Enter details  x remove
  Docs         Enter opens a topic   Tree  Enter opens a category
  Run          any yapacc arguments; output is shown, nothing can prompt.
               Not run from here: --yes, --noconfirm, i, mcp

Changing the system
  Every change is first shown as a plan (a dry run): nothing has happened
  yet. Move to [ Apply ] and press Enter to leave this interface and run
  the command in your terminal, where yapacc itself asks you to confirm.
  This interface never answers that question for you. Ctrl-C there stops
  that command and brings you back here.";

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let (pct_x, pct_y) = (pct_x.min(100), pct_y.min(100));
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}

/// Usable text width inside a bordered list with a "> " cursor.
fn list_width(area: Rect) -> usize {
    area.width.saturating_sub(4) as usize
}

fn titled<'a>(title: &str, area: Rect) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(tr(title, area.width.saturating_sub(2) as usize))
}

fn reversed() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < 20 || area.height < 6 {
        f.render_widget(
            Paragraph::new(tr("window too small", area.width as usize)),
            area,
        );
        return;
    }
    if app.preview.is_some() {
        draw_preview(f, app, area);
        return;
    }
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    match app.screen {
        Screen::Home => draw_home(f, app, chunks[0]),
        Screen::Browse => draw_browse(f, app, chunks[0]),
        Screen::Report => draw_report(f, app, chunks[0]),
    }

    let hint = if !app.status.is_empty() {
        app.status.clone()
    } else {
        match app.screen {
            Screen::Home => "Enter: open   j/k: move   ?: help   q: quit".to_string(),
            Screen::Report => {
                if app.report.as_ref().is_some_and(|r| r.filter_edit) {
                    "FILTER — type; Enter or Esc to finish".to_string()
                } else {
                    let extra = match app.report.as_ref().map(|r| r.kind) {
                        Some(Report::Briefing) => "u: plan the upgrade   ",
                        Some(Report::Clean) => "o: plan removing unneeded packages   ",
                        _ => "",
                    };
                    format!(
                        "{extra}/: filter   Enter: whole line   r: reload   Esc: back   ?: help"
                    )
                }
            }
            Screen::Browse => {
                if app.filter_edit {
                    "FILTER — type; Enter or Esc to finish".to_string()
                } else if app.tab == Tab::Search && app.search_edit {
                    "SEARCH — type; Enter searches, Esc stops typing".to_string()
                } else {
                    format!(
                        "{}   Tab: next tab   Esc: home   ?: help",
                        TABS[app.tab as usize].1
                    )
                }
            }
        }
    };
    f.render_widget(
        Paragraph::new(tr(&hint, chunks[1].width as usize)),
        chunks[1],
    );

    if app.help {
        let rect = centered_rect(90, 90, area);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(HELP)
                .wrap(Wrap { trim: false })
                .block(titled("Help — ? or Esc closes", rect)),
            rect,
        );
    }

    if let Some(p) = &app.popup {
        let rect = centered_rect(90, 85, area);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(p.body.clone())
                .wrap(Wrap { trim: false })
                .scroll((p.scroll, 0))
                .block(titled(&format!("{} — Esc closes", p.title), rect)),
            rect,
        );
    }

    if let Some(input) = &app.input {
        let rect = centered_rect(80, 30, area);
        let rect = Rect {
            height: rect.height.clamp(3.min(area.height), 5),
            ..rect
        };
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(format!("{}_", input.buf))
                .block(titled(&format!("{} (Esc cancels)", input.title), rect)),
            rect,
        );
    }
}

fn draw_home(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area);
    let inner_w = chunks[0].width.saturating_sub(2) as usize;
    f.render_widget(
        Paragraph::new(tr(&app.machine, inner_w)).block(titled("yapacc — this machine", chunks[0])),
        chunks[0],
    );

    let width = list_width(chunks[1]);
    let title_w = MENU
        .iter()
        .map(|m| m.title.chars().count())
        .max()
        .unwrap_or(0);
    let color = app.color;
    let items: Vec<ListItem> = MENU
        .iter()
        .map(|m| {
            let head = format!("[{}] {:<title_w$}", m.key, m.title, title_w = title_w);
            let head = tr(&head, width);
            let rest = width.saturating_sub(head.chars().count() + 2);
            let mut spans = vec![Span::styled(head, bold())];
            if rest > 8 {
                spans.push(Span::styled(
                    format!("  {}", tr(m.explain, rest)),
                    tone_style(Tone::Dim, color),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(titled("What would you like to know or do?", chunks[1]));
    f.render_stateful_widget(list, chunks[1], &mut app.home_state);

    let explain = app
        .home_state
        .selected()
        .and_then(|i| MENU.get(i))
        .map(|m| m.explain)
        .unwrap_or("");
    f.render_widget(
        Paragraph::new(explain)
            .wrap(Wrap { trim: true })
            .block(titled("About this choice", chunks[2])),
        chunks[2],
    );
}

fn draw_report(f: &mut Frame, app: &mut App, area: Rect) {
    let color = app.color;
    let Some(r) = app.report.as_mut() else {
        return;
    };
    let mut title = report_title(r.kind, &r.arg);
    if !r.loading {
        title.push_str(&format!(" ({})", r.shown.len()));
    }
    if r.filter_edit || !r.filter.is_empty() {
        title.push_str(&format!(
            " — filter: '{}{}'",
            r.filter,
            if r.filter_edit { "_" } else { "" }
        ));
    }
    let block = titled(&title, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if r.loading {
        f.render_widget(Paragraph::new("loading…"), inner);
        return;
    }
    let has_header = !r.table.header.is_empty() && inner.height > 2;
    let chunks = Layout::vertical([
        Constraint::Length(if has_header { 1 } else { 0 }),
        Constraint::Min(0),
    ])
    .split(inner);
    let width = inner.width.saturating_sub(2) as usize;
    let widths = scale_widths(&r.table.widths, width);
    if has_header {
        let head = format!("  {}", fit_row(&r.table.header, &widths, width));
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(head, bold())])),
            chunks[0],
        );
    }
    r.page = (chunks[1].height.max(2) - 1) as usize;
    if r.shown.is_empty() {
        f.render_widget(Paragraph::new("(nothing matches the filter)"), chunks[1]);
        return;
    }
    let items: Vec<ListItem> = r
        .shown
        .iter()
        .filter_map(|&i| r.table.rows.get(i))
        .map(|row| {
            ListItem::new(Line::from(vec![Span::styled(
                fit_row(&row.cells, &widths, width),
                tone_style(row.tone, color),
            )]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ");
    f.render_stateful_widget(list, chunks[1], &mut r.state);
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let color = app.color;
    let Some(p) = app.preview.as_ref() else {
        return;
    };
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled(change_title(&p.change), bold())]),
        Line::from(format!(
            "The command: yapacc {}",
            change_argv(&p.change, false).join(" ")
        )),
        Line::from(""),
    ];
    if p.loading {
        lines.push(Line::from("working out the plan…"));
    } else {
        if !p.ok {
            lines.push(Line::from(vec![Span::styled(
                "The plan could not be worked out, so this change cannot be applied from here.",
                tone_style(Tone::Care, color),
            )]));
            lines.push(Line::from(""));
        }
        lines.extend(p.text.lines().map(|l| Line::from(l.to_string())));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((p.scroll, 0))
            .block(titled(PREVIEW_TITLE, chunks[0])),
        chunks[0],
    );

    let focus = |on: bool| if on { reversed() } else { Style::default() };
    let apply_style = if p.can_apply() {
        focus(p.focus_apply)
    } else {
        tone_style(Tone::Dim, color)
    };
    let hint = if p.can_apply() {
        "   ←/→ choose, Enter confirms, Esc cancels"
    } else if p.loading {
        "   please wait…  Esc cancels"
    } else {
        "   Enter or Esc goes back"
    };
    let buttons = Line::from(vec![
        Span::raw(" "),
        Span::styled("[ Cancel ]", focus(!p.focus_apply)),
        Span::raw("   "),
        Span::styled("[ Apply ]", apply_style),
        Span::raw(tr(hint, chunks[1].width.saturating_sub(28) as usize)),
    ]);
    f.render_widget(
        Paragraph::new(buttons).block(Block::default().borders(Borders::ALL)),
        chunks[1],
    );
}

fn draw_browse(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let titles: Vec<Line> = TABS.iter().map(|(t, _)| Line::from(*t)).collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.tab as usize)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        chunks[0],
    );
    match app.tab {
        Tab::Packages => draw_packages(f, app, chunks[1]),
        Tab::Search => draw_search(f, app, chunks[1]),
        Tab::External => draw_external(f, app, chunks[1]),
        Tab::Docs => draw_docs(f, app, chunks[1]),
        Tab::Tree => draw_tree(f, app, chunks[1]),
        Tab::Run => draw_run(f, app, chunks[1]),
    }
}

const PKG_WIDTHS: [usize; 5] = [3, 28, 16, 10, 12];

fn pkg_list_items(rows: &[&PkgRow], selected: &[String], width: usize) -> Vec<ListItem<'static>> {
    let widths = scale_widths(&PKG_WIDTHS, width);
    rows.iter()
        .map(|p| {
            let mark = if selected.contains(&p.name) {
                "[*]"
            } else {
                "[ ]"
            };
            let cells = vec![
                mark.to_string(),
                p.name.clone(),
                p.version.clone(),
                p.size.clone(),
                p.reason.clone(),
                p.desc.clone(),
            ];
            ListItem::new(Line::from(fit_row(&cells, &widths, width)))
        })
        .collect()
}

fn draw_packages(f: &mut Frame, app: &mut App, area: Rect) {
    let title = format!(
        "Installed software ({}, selected {}) filter: '{}{}'",
        app.pkg_filtered.len(),
        app.pkg_selected.len(),
        app.pkg_filter,
        if app.filter_edit { "_" } else { "" }
    );
    let block = titled(&title, area);
    if app.pkgs.is_empty() && (app.pkgs_loading || app.pkgs_error.is_some()) {
        let body = match &app.pkgs_error {
            Some(e) if !app.pkgs_loading => {
                format!("The list of installed software could not be read.\n\n{e}")
            }
            _ => "loading…".to_string(),
        };
        f.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
            area,
        );
        return;
    }
    let rows: Vec<&PkgRow> = app
        .pkg_filtered
        .iter()
        .filter_map(|&i| app.pkgs.get(i))
        .collect();
    let items = pkg_list_items(&rows, &app.pkg_selected, list_width(area));
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(block);
    f.render_stateful_widget(list, area, &mut app.pkg_state);
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    let input = if app.search_edit {
        format!("query: {}_", app.query)
    } else {
        format!("query: {}  (press / to edit)", app.query)
    };
    let title = if app.search_aur {
        "Search for software — repositories and the AUR (the query is sent to aur.archlinux.org)"
    } else {
        "Search for software"
    };
    f.render_widget(
        Paragraph::new(tr(&input, chunks[0].width.saturating_sub(2) as usize))
            .block(titled(title, chunks[0])),
        chunks[0],
    );
    let block = titled(&format!("Results ({})", app.results.len()), chunks[1]);
    if app.search_loading || app.search_error.is_some() {
        let body = match &app.search_error {
            Some(e) if !app.search_loading => format!("The search did not work.\n\n{e}"),
            _ => "searching…".to_string(),
        };
        f.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
            chunks[1],
        );
        return;
    }
    let rows: Vec<&PkgRow> = app.results.iter().collect();
    let items = pkg_list_items(&rows, &[], list_width(chunks[1]));
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(block);
    f.render_stateful_widget(list, chunks[1], &mut app.search_state);
}

fn draw_text(f: &mut Frame, app: &mut App, area: Rect, fallback_title: &str) {
    let title = if app.text_title.is_empty() {
        fallback_title
    } else {
        &app.text_title
    };
    f.render_widget(
        Paragraph::new(app.text.clone())
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0))
            .block(titled(title, area)),
        area,
    );
}

fn draw_external(f: &mut Frame, app: &mut App, area: Rect) {
    let width = list_width(area);
    let widths = scale_widths(&[34, 16, 12, 7], width);
    let items: Vec<ListItem> = app
        .records
        .iter()
        .map(|r| {
            let cells = vec![
                r.id.clone(),
                r.name.clone(),
                r.format.clone(),
                r.scope.clone(),
                r.install_root.clone(),
            ];
            ListItem::new(Line::from(fit_row(&cells, &widths, width)))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(titled(
            &format!(
                "Installed outside the package manager ({})",
                app.records.len()
            ),
            area,
        ));
    f.render_stateful_widget(list, area, &mut app.ext_state);
}

fn draw_docs(f: &mut Frame, app: &mut App, area: Rect) {
    if app.text_title.starts_with("docs:") {
        draw_text(f, app, area, "docs");
        return;
    }
    let width = list_width(area);
    let items: Vec<ListItem> = app
        .doc_topics
        .iter()
        .map(|t| {
            let title = docs::topics()
                .iter()
                .find(|(s, _, _)| s == t)
                .map(|(_, ti, _)| *ti)
                .unwrap_or("");
            ListItem::new(Line::from(tr(&format!("{t:<18} {title}"), width)))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(titled("Documentation topics (Enter opens)", area));
    f.render_stateful_widget(list, area, &mut app.doc_state);
}

fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let crumb = match app.tree_level {
        TreeLevel::Tops => "categories".to_string(),
        TreeLevel::Subs => format!("categories / {}", app.tree_top),
        TreeLevel::Packages => format!("categories / {} / {}", app.tree_top, app.tree_sub),
    };
    let width = list_width(area);
    let widths = scale_widths(&[33], width);
    let items: Vec<ListItem> = app
        .tree_items
        .iter()
        .map(|i| {
            let text = match i {
                TreeItem::Cat(c) => tr(&format!("▸ {c}"), width),
                TreeItem::Sub(s) => tr(&format!("  ▸ {s}"), width),
                TreeItem::Pkg {
                    name,
                    desc,
                    installed,
                } => {
                    let cells = vec![
                        format!("{}{name}", if *installed { "*" } else { " " }),
                        desc.clone(),
                    ];
                    fit_row(&cells, &widths, width)
                }
            };
            ListItem::new(Line::from(text))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ")
        .block(titled(
            &format!(
                "{crumb} (* = installed){}",
                if app.tree_loading {
                    " — reading the repositories…"
                } else {
                    ""
                }
            ),
            area,
        ));
    f.render_stateful_widget(list, area, &mut app.tree_state);
}

fn draw_run(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    f.render_widget(
        Paragraph::new(format!("yapacc {}_", app.run_input))
            .block(titled("Run any yapacc command", chunks[0])),
        chunks[0],
    );
    draw_text(f, app, chunks[1], "output");
}

// ---------------------------------------------------------------------------
// tests (pure parts only: nothing here starts a process or a terminal)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cells(t: &Table) -> Vec<Vec<String>> {
        t.rows.iter().map(|r| r.cells.clone()).collect()
    }

    fn texts(t: &Table) -> Vec<String> {
        t.rows.iter().map(|r| r.cells.join("|")).collect()
    }

    #[test]
    fn every_menu_entry_has_its_own_letter_and_number() {
        for (i, m) in MENU.iter().enumerate() {
            assert_eq!(menu_action(m.key), Some(m.action), "letter {}", m.key);
            assert_eq!(menu_action(m.key.to_ascii_uppercase()), Some(m.action));
            let digit = char::from_digit(((i + 1) % 10) as u32, 10).unwrap();
            assert_eq!(menu_action(digit), Some(m.action), "digit {digit}");
            assert_eq!(MENU.iter().filter(|o| o.key == m.key).count(), 1);
        }
        assert_eq!(menu_action('t'), Some(HomeAction::Timeline));
        assert_eq!(menu_action('b'), Some(HomeAction::Briefing));
        assert_eq!(menu_action('u'), Some(HomeAction::Undo));
        assert_eq!(menu_action('0'), Some(HomeAction::Undo));
        assert_eq!(menu_action('z'), None);
        assert_eq!(menu_action('?'), None);
    }

    #[test]
    fn a_change_never_consents_on_the_users_behalf() {
        let changes = [
            Change::Install {
                name: "ripgrep".into(),
                aur: false,
            },
            Change::Install {
                name: "yay".into(),
                aur: true,
            },
            Change::Remove(vec!["a".into(), "b".into()]),
            Change::Upgrade,
            Change::Undo,
            Change::RemoveOrphans,
            Change::RemoveExternal("id-1".into()),
            Change::MarkExplicit(vec!["a".into()]),
        ];
        for c in &changes {
            for dryrun in [true, false] {
                let argv = change_argv(c, dryrun);
                for bad in ["--yes", "--noconfirm", "-y", "--script"] {
                    assert!(!argv.iter().any(|a| a == bad), "{c:?}: {argv:?}");
                }
                assert!(!passes_consent(&argv));
                assert_eq!(argv.iter().any(|a| a == "--dryrun"), dryrun, "{argv:?}");
            }
            // the applied command is the planned one, minus --dryrun
            let planned: Vec<String> = change_argv(c, true)
                .into_iter()
                .filter(|a| a != "--dryrun")
                .collect();
            assert_eq!(planned, change_argv(c, false));
        }
    }

    #[test]
    fn change_commands_are_the_core_commands() {
        let argv = |c: &Change, d: bool| change_argv(c, d).join(" ");
        assert_eq!(
            argv(
                &Change::Install {
                    name: "ripgrep".into(),
                    aur: false
                },
                true
            ),
            "install repo --dryrun -- ripgrep"
        );
        assert_eq!(
            argv(&Change::Remove(vec!["a".into(), "b".into()]), false),
            "remove repo -- a b"
        );
        assert_eq!(argv(&Change::Upgrade, true), "upgrade --dryrun");
        assert_eq!(argv(&Change::Upgrade, false), "upgrade");
        assert_eq!(argv(&Change::Undo, true), "undo --dryrun");
        assert_eq!(
            argv(&Change::RemoveOrphans, true),
            "remove orphans --dryrun"
        );
        // a name that looks like a flag stays a name
        let sneaky = change_argv(&Change::Remove(vec!["--yes".into()]), false);
        assert_eq!(sneaky, ["remove", "repo", "--", "--yes"]);
        assert!(!passes_consent(&sneaky));
    }

    #[test]
    fn typed_consent_flags_are_recognised() {
        let v = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
        assert!(passes_consent(&v("upgrade --yes")));
        assert!(passes_consent(&v("--noconfirm upgrade")));
        assert!(!passes_consent(&v("upgrade --dryrun")));
        assert!(!passes_consent(&v("--json list")));
    }

    #[test]
    fn report_commands_are_json_reads() {
        assert_eq!(
            report_argv(Report::Timeline, ""),
            ["--json", "timeline", "--limit", "500"]
        );
        assert_eq!(report_argv(Report::Briefing, ""), ["--json", "briefing"]);
        assert_eq!(
            report_argv(Report::Why, "bash"),
            ["--json", "why", "--", "bash"]
        );
        assert_eq!(report_argv(Report::Clean, ""), ["--json", "clean"]);
        assert_eq!(report_argv(Report::Restart, ""), ["--json", "restart"]);
        assert_eq!(report_argv(Report::Configs, ""), ["--json", "configs"]);
        assert_eq!(report_argv(Report::Doctor, ""), ["--json", "doctor"]);
        for k in [
            Report::Timeline,
            Report::Briefing,
            Report::Why,
            Report::Clean,
            Report::Restart,
            Report::Configs,
            Report::Doctor,
        ] {
            assert!(!passes_consent(&report_argv(k, "x")));
        }
    }

    #[test]
    fn timeline_rows_are_newest_first_and_survive_missing_fields() {
        let v = json!([
            {"ts": 86400, "source": "pacman", "action": "install", "name": "old",
             "old_version": "", "new_version": "1.0"},
            {"ts": 1700000000u64, "source": "apt", "action": "upgrade", "name": "new",
             "old_version": "1.0", "new_version": "2.0"},
            {"name": "bare"},
            {},
            "not an object"
        ]);
        let t = timeline_table(&v);
        assert_eq!(t.header.len(), 5);
        assert_eq!(
            cells(&t),
            vec![
                vec!["2023-11-14 22:13", "apt", "upgrade", "new", "1.0 → 2.0"],
                vec!["1970-01-02 00:00", "pacman", "install", "old", "1.0"],
                vec![DASH, DASH, DASH, "bare", DASH],
                vec![DASH, DASH, DASH, DASH, DASH],
                vec![DASH, DASH, DASH, DASH, DASH],
            ]
        );
        // not an array at all: still rows, never a panic
        assert!(!timeline_table(&json!({"error": "x"})).rows.is_empty());
        assert!(!timeline_table(&json!(null)).rows.is_empty());
        assert_eq!(
            texts(&timeline_table(&json!([]))),
            ["(no package changes recorded)"]
        );
    }

    #[test]
    fn the_filter_matches_any_cell_without_case() {
        let v = json!([
            {"ts": 3, "source": "pacman", "action": "install", "name": "Firefox"},
            {"ts": 2, "source": "apt", "action": "remove", "name": "vim"},
            {"ts": 1, "source": "pacman", "action": "upgrade", "name": "linux"}
        ]);
        let t = timeline_table(&v);
        assert_eq!(filter_rows(&t.rows, ""), [0, 1, 2]);
        assert_eq!(filter_rows(&t.rows, "  "), [0, 1, 2]);
        assert_eq!(filter_rows(&t.rows, "fire"), [0]);
        assert_eq!(filter_rows(&t.rows, "PACMAN"), [0, 2]);
        assert_eq!(filter_rows(&t.rows, "remove"), [1]);
        assert!(filter_rows(&t.rows, "nothing-like-this").is_empty());
    }

    #[test]
    fn briefing_puts_flags_first_toned_by_level() {
        let v = json!({
            "manager": "pacman",
            "updates": [{"name": "linux", "old_version": "6.1", "new_version": "6.2", "repo": "core"}],
            "flags": [
                {"level": "reboot", "text": "kernel upgrade"},
                {"level": "care", "text": "glibc"},
                {"level": "info", "text": "3 packages"},
                {"text": "no level"}
            ],
            "news": [["Title", "2026-11-01"]],
            "security": ["CVE-1 in openssl"],
            "unmerged_config": ["/etc/pacman.conf.pacnew"],
            "disk": ["/ has 10 GiB free"],
            "safety_net": "snapper",
            "notes": ["a note"]
        });
        let t = briefing_table(&v);
        let lines = texts(&t);
        assert_eq!(lines[0], "Needs your attention");
        assert_eq!(lines[1], "  [reboot] kernel upgrade");
        assert_eq!(t.rows[1].tone, Tone::Reboot);
        assert_eq!(t.rows[2].tone, Tone::Care);
        assert_eq!(t.rows[3].tone, Tone::Info);
        assert_eq!(lines[4], format!("  [{DASH}] no level"));
        assert_eq!(t.rows[4].tone, Tone::Info);
        let pos = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no line with {needle}: {lines:?}"))
        };
        let order = [
            pos("Needs your attention"),
            pos("Security fixes"),
            pos("CVE-1 in openssl"),
            pos("News from the distribution"),
            pos("2026-11-01  Title"),
            pos("Unmerged configuration files"),
            pos("/etc/pacman.conf.pacnew"),
            pos("Disk"),
            pos("/ has 10 GiB free"),
            pos("Safety net"),
            pos("snapper"),
            pos("Packages that will change (1, through pacman)"),
            pos("linux  6.1 → 6.2  (core)"),
            pos("Notes"),
            pos("a note"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    }

    #[test]
    fn briefing_with_nothing_in_it_shows_dashes() {
        let t = briefing_table(&json!({}));
        let lines = texts(&t);
        assert_eq!(lines[0], "Needs your attention");
        assert_eq!(lines[1], format!("  {DASH}"));
        assert!(lines.iter().any(|l| l == "Safety net"));
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("Packages that will change (0, through {DASH})"))
        );
        // wrong types everywhere: still no panic
        let odd = briefing_table(&json!({
            "flags": "x", "news": [1, {"title": "t"}, ["only title"]],
            "security": 3, "updates": [1, null], "safety_net": null
        }));
        assert!(!odd.rows.is_empty());
        assert!(!briefing_table(&json!([1, 2])).rows.is_empty());
    }

    #[test]
    fn clean_and_doctor_rows() {
        let t = clean_table(&json!([
            {"what": "orphans", "size_bytes": 2048, "count": 3, "safety": "safe",
             "command": "yapacc remove orphans", "detail": ["a", "b"], "note": "n"},
            {"what": "cache"}
        ]));
        assert_eq!(
            cells(&t),
            vec![
                vec!["orphans", "2.0 KiB", "3", "safe", "yapacc remove orphans"],
                vec!["cache", DASH, DASH, DASH, DASH],
            ]
        );
        assert_eq!(t.rows[0].detail, "note: n\n  a\n  b");
        assert!(row_detail(&t, &t.rows[0]).contains("COMMAND: yapacc remove orphans"));

        let d = doctor_table(&json!([
            {"name": "platform", "state": "ok", "detail": "fine"},
            {"name": "x", "state": "warn", "detail": "hm"},
            {"name": "y", "state": "fail", "detail": "bad"},
            {"name": "z", "state": "n/a", "detail": ""},
            {}
        ]));
        assert_eq!(d.rows[0].cells, ["ok", "platform", "fine"]);
        let tones: Vec<Tone> = d.rows.iter().map(|r| r.tone).collect();
        assert_eq!(
            tones,
            [Tone::Good, Tone::Care, Tone::Reboot, Tone::Dim, Tone::Plain]
        );
        assert_eq!(d.rows[3].cells[2], DASH);
    }

    #[test]
    fn any_json_becomes_key_value_lines() {
        // keys in alphabetical order: the same lines whether or not the
        // JSON map keeps insertion order
        let t = generic_table(&json!({
            "empty": [],
            "inner": {"k": 1, "none": null},
            "name": "bash",
            "needs": ["a", "b"]
        }));
        assert_eq!(
            texts(&t),
            [
                format!("empty: {DASH}"),
                "inner:".to_string(),
                "  k: 1".to_string(),
                format!("  none: {DASH}"),
                "name: bash".to_string(),
                "needs:".to_string(),
                "  - a".to_string(),
                "  - b".to_string(),
            ]
        );
        assert_eq!(texts(&generic_table(&json!([]))), ["(nothing to report)"]);
        assert_eq!(texts(&generic_table(&json!(null))), ["(nothing to report)"]);
        assert_eq!(texts(&generic_table(&json!("plain"))), ["plain"]);
        assert!(!generic_table(&json!([{"a": 1}, {"a": 2}])).rows.is_empty());
    }

    #[test]
    fn a_report_that_is_not_json_shows_what_the_core_said() {
        let out = CoreOut {
            ok: false,
            stdout: String::new(),
            stderr: "error: no such package\n".into(),
            status: "exit status: 4".into(),
        };
        let lines = texts(&build_table(Report::Why, &out));
        assert_eq!(lines[0], "The yapacc core did not return a report.");
        assert!(lines.iter().any(|l| l == "error: no such package"));
        assert!(lines.iter().any(|l| l == "[exit status: 4]"));
    }

    #[test]
    fn installed_list_accepts_both_shapes() {
        let pacman = json!([{"name": "bash", "version": "5.2", "description": "shell",
            "installed_size": "9.5 MiB", "reason": "explicit", "repo": "core"}]);
        assert_eq!(
            list_rows(&pacman),
            [PkgRow {
                name: "bash".into(),
                version: "5.2".into(),
                size: "9.5 MiB".into(),
                reason: "explicit".into(),
                desc: "shell".into(),
                origin: Origin::Local,
                installed: true,
            }]
        );
        let portable = json!([
            {"name": "bash", "version": "5.2", "arch": "amd64", "size_bytes": 1048576,
             "description": "shell", "explicit": true},
            {"name": "libc6", "size_bytes": 0, "explicit": false},
            {}
        ]);
        let rows = list_rows(&portable);
        assert_eq!(rows[0].size, "1.0 MiB");
        assert_eq!(rows[0].reason, "explicit");
        assert_eq!(rows[1].size, DASH);
        assert_eq!(rows[1].reason, "dependency");
        assert_eq!(rows[1].version, DASH);
        assert_eq!(rows[2].name, DASH);
        assert!(list_rows(&json!({"not": "a list"})).is_empty());
    }

    #[test]
    fn search_results_accept_both_shapes() {
        let rows = search_rows(&json!([
            {"repo": "aur", "name": "yay", "version": "12", "description": "helper",
             "source": "aur", "download_size": null},
            {"name": "ripgrep", "version": "14", "repo": "main", "description": "grep"}
        ]));
        assert_eq!(rows[0].reason, "aur");
        assert_eq!(rows[0].size, DASH);
        assert_eq!(rows[1].reason, "main");
    }

    #[test]
    fn machine_line_from_platform_json() {
        let line = machine_summary(&json!({
            "os": {"id": "debian", "pretty_name": "Debian GNU/Linux 13"},
            "native_pkg_manager": "apt",
            "support_tier": 1
        }));
        assert_eq!(
            line,
            "Distribution: Debian GNU/Linux 13  ·  Package manager: apt  ·  Support tier: 1"
        );
        let bare = machine_summary(&json!({"os": "Arch Linux", "native_pkg_manager": null}));
        assert!(bare.contains("Distribution: Arch Linux"));
        assert!(bare.contains(&format!("Package manager: {DASH}")));
        assert!(bare.contains(&format!("Support tier: {DASH}")));
        assert!(machine_summary(&json!({"support_tier": 0})).contains("Support tier: none"));
    }

    #[test]
    fn text_is_cut_with_an_ellipsis_and_never_overflows() {
        assert_eq!(tr("abcdef", 4), "abc…");
        assert_eq!(tr("abc", 3), "abc");
        assert_eq!(tr("abc", 1), "…");
        assert_eq!(tr("abc", 0), "");
        assert_eq!(tr("", 0), "");
        let cells: Vec<String> = [
            "2026-01-01 10:00",
            "pacman",
            "upgrade",
            "a-long-name",
            "1 → 2",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let widths = [16, 10, 10, 32];
        for total in 0..120 {
            let scaled = scale_widths(&widths, total);
            assert_eq!(scaled.len(), widths.len());
            let line = fit_row(&cells, &scaled, total);
            assert!(line.chars().count() <= total, "{total}: {line:?}");
        }
        assert_eq!(scale_widths(&widths, 200), widths);
        assert!(scale_widths(&[], 0).is_empty());
        assert_eq!(fit_row(&[], &[], 10), "");
    }

    #[test]
    fn times_sizes_and_escape_sequences() {
        assert_eq!(fmt_time(0), DASH);
        assert_eq!(fmt_time(951_782_400), "2000-02-29 00:00");
        assert_eq!(fmt_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(clean_text("\u{1b}[1;31mred\u{1b}[0m\tx\r\n"), "red    x\n");
    }

    #[test]
    fn no_color_turns_colours_off() {
        assert!(color_enabled(None));
        assert!(color_enabled(Some("")));
        assert!(!color_enabled(Some("1")));
        for tone in [Tone::Reboot, Tone::Care, Tone::Good, Tone::Dim, Tone::Info] {
            assert_eq!(tone_style(tone, false).fg, None, "{tone:?}");
        }
        assert_eq!(tone_style(Tone::Reboot, true).fg, Some(Color::Red));
        assert_eq!(tone_style(Tone::Care, true).fg, Some(Color::Yellow));
        assert_eq!(tone_style(Tone::Info, true).fg, None);
    }

    fn preview_of(change: Change, loading: bool, ok: bool) -> Preview {
        Preview {
            change,
            job: 0,
            loading,
            ok,
            text: "plan".into(),
            scroll: 0,
            focus_apply: false,
        }
    }

    #[test]
    fn a_plan_is_applied_only_by_enter_on_apply() {
        let none = KeyModifiers::NONE;
        // default focus is Cancel: Enter changes nothing
        let mut app = App::new();
        app.preview = Some(preview_of(Change::Upgrade, false, true));
        assert_eq!(handle_key(&mut app, KeyCode::Enter, none), Flow::Continue);
        assert!(app.preview.is_none());

        // 'y' is not consent
        app.preview = Some(preview_of(Change::Upgrade, false, true));
        assert_eq!(
            handle_key(&mut app, KeyCode::Char('y'), none),
            Flow::Continue
        );
        assert!(app.preview.is_some());

        // moving to Apply, then Enter
        assert_eq!(handle_key(&mut app, KeyCode::Right, none), Flow::Continue);
        assert_eq!(
            handle_key(&mut app, KeyCode::Enter, none),
            Flow::Apply(Change::Upgrade)
        );
        assert!(app.preview.is_none());

        // Esc cancels even with Apply focused
        app.preview = Some(preview_of(Change::Undo, false, true));
        handle_key(&mut app, KeyCode::Tab, none);
        assert_eq!(handle_key(&mut app, KeyCode::Esc, none), Flow::Continue);
        assert!(app.preview.is_none());

        // a plan still loading, or one that failed, cannot be applied
        for (loading, ok) in [(true, false), (false, false)] {
            app.preview = Some(preview_of(Change::Undo, loading, ok));
            handle_key(&mut app, KeyCode::Right, none);
            assert_eq!(handle_key(&mut app, KeyCode::Enter, none), Flow::Continue);
            assert!(app.preview.is_none());
        }
    }

    #[test]
    fn home_keys_navigate_without_starting_anything() {
        let none = KeyModifiers::NONE;
        let mut app = App::new();
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.home_state.selected(), Some(0));
        handle_key(&mut app, KeyCode::Char('j'), none);
        handle_key(&mut app, KeyCode::Down, none);
        assert_eq!(app.home_state.selected(), Some(2));
        handle_key(&mut app, KeyCode::End, none);
        assert_eq!(app.home_state.selected(), Some(MENU.len() - 1));
        handle_key(&mut app, KeyCode::Home, none);
        assert_eq!(app.home_state.selected(), Some(0));

        // "why" asks for a name first; Esc gives it up
        handle_key(&mut app, KeyCode::Char('w'), none);
        assert!(app.input.is_some());
        handle_key(&mut app, KeyCode::Char('q'), none);
        assert_eq!(app.input.as_ref().map(|i| i.buf.as_str()), Some("q"));
        handle_key(&mut app, KeyCode::Esc, none);
        assert!(app.input.is_none());
        assert_eq!(app.screen, Screen::Home);

        // help opens and closes
        handle_key(&mut app, KeyCode::Char('?'), none);
        assert!(app.help);
        handle_key(&mut app, KeyCode::Esc, none);
        assert!(!app.help);

        // browse and back: q and Esc return home, q on home quits
        handle_key(&mut app, KeyCode::Char('l'), none);
        assert_eq!(app.screen, Screen::Browse);
        assert_eq!(
            handle_key(&mut app, KeyCode::Char('q'), none),
            Flow::Continue
        );
        assert_eq!(app.screen, Screen::Home);
        handle_key(&mut app, KeyCode::Char('s'), none);
        assert_eq!(app.screen, Screen::Browse);
        assert!(app.tab == Tab::Search && app.search_edit);
        // typing a 'q' into the query is not "quit"
        handle_key(&mut app, KeyCode::Char('q'), none);
        assert_eq!(app.query, "q");
        handle_key(&mut app, KeyCode::Esc, none); // stop typing
        handle_key(&mut app, KeyCode::Esc, none); // back home
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(handle_key(&mut app, KeyCode::Char('q'), none), Flow::Quit);
        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Flow::Quit
        );
    }

    #[test]
    fn list_and_scroll_keys_stay_in_range() {
        let mut st = ListState::default();
        assert!(list_key(&mut st, 0, 10, KeyCode::Down));
        assert_eq!(st.selected(), None);
        st.select(Some(0));
        assert!(list_key(&mut st, 30, 10, KeyCode::PageDown));
        assert_eq!(st.selected(), Some(10));
        assert!(list_key(&mut st, 30, 10, KeyCode::Char('G')));
        assert_eq!(st.selected(), Some(29));
        assert!(list_key(&mut st, 30, 10, KeyCode::PageUp));
        assert_eq!(st.selected(), Some(19));
        assert!(list_key(&mut st, 30, 0, KeyCode::Char('g')));
        assert_eq!(st.selected(), Some(0));
        assert!(list_key(&mut st, 30, 10, KeyCode::Up));
        assert_eq!(st.selected(), Some(0));
        assert!(!list_key(&mut st, 30, 10, KeyCode::Char('x')));

        assert_eq!(scroll_key(KeyCode::Down, 0, 3), Some(1));
        assert_eq!(scroll_key(KeyCode::PageDown, 0, 3), Some(2));
        assert_eq!(scroll_key(KeyCode::Char('G'), 0, 100), Some(99));
        assert_eq!(scroll_key(KeyCode::Up, 0, 0), Some(0));
        assert_eq!(scroll_key(KeyCode::Char('x'), 0, 3), None);
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_config_is_forwarded_before_the_command() {
        let argv = words("undo --dryrun");
        assert_eq!(with_config(None, &argv), argv);
        assert_eq!(
            with_config(Some("/tmp/a b;rm"), &argv),
            vec!["--config", "/tmp/a b;rm", "undo", "--dryrun"]
        );
        // every kind of run this interface makes goes through it
        for argv in [
            report_argv(Report::Doctor, ""),
            change_argv(&Change::Upgrade, true),
            change_argv(&Change::Upgrade, false),
            search_argv("x", false),
        ] {
            let forwarded = with_config(Some("/etc/site.toml"), &argv);
            assert_eq!(&forwarded[..2], ["--config", "/etc/site.toml"]);
            assert_eq!(&forwarded[2..], &argv[..]);
        }
        // a line typed with its own --config keeps it, and gets no second one
        for own in ["--config /other.toml list", "--config=/other.toml list"] {
            assert_eq!(with_config(Some("/etc/site.toml"), &words(own)), words(own));
        }
        // after `--` it is a name, not the option
        let named = words("why -- --config");
        assert_eq!(
            with_config(Some("/c"), &named),
            vec!["--config", "/c", "why", "--", "--config"]
        );
    }

    #[test]
    fn the_run_tab_does_not_start_the_interface_or_the_server() {
        for line in [
            "i",
            "interactive",
            "mcp",
            "--json i",
            "--config /x.toml i",
            "--config=/x.toml mcp",
            "--script --noconfirm interactive",
        ] {
            assert!(refused_in_run_tab(&words(line)).is_some(), "{line}");
        }
        for line in [
            "list",
            "search mcp",
            "why i",
            "info -- i",
            "--config i list",
            "docs ai",
            "",
        ] {
            assert!(refused_in_run_tab(&words(line)).is_none(), "{line}");
        }
        assert_eq!(
            typed_verb(&words("--json --config /c timeline")),
            Some("timeline")
        );
        assert_eq!(typed_verb(&words("--config")), None);

        // through the keys: nothing is started, and the pane says why
        let none = KeyModifiers::NONE;
        for line in ["i", "interactive", "mcp"] {
            let mut app = App::new();
            handle_key(&mut app, KeyCode::Char('l'), none);
            handle_key(&mut app, KeyCode::Char('6'), none);
            assert!(app.tab == Tab::Run);
            for c in line.chars() {
                handle_key(&mut app, KeyCode::Char(c), none);
            }
            let jobs_before = app.job_seq;
            handle_key(&mut app, KeyCode::Enter, none);
            assert_eq!(app.job_seq, jobs_before, "{line}: a process was started");
            assert_eq!(app.run_job, 0);
            assert!(app.text.contains("interface"), "{line}: {}", app.text);
            assert_eq!(app.text_title, format!("$ yapacc {line}"));
        }
    }

    #[test]
    fn a_search_result_that_is_not_installed_is_described_from_the_repositories() {
        let rows = search_rows(&json!([
            {"repo": "extra", "name": "repgrep", "version": "0.16", "description": "rg + sed",
             "source": "repo", "installed": false},
            {"repo": "aur", "name": "yay", "version": "12", "description": "helper",
             "source": "aur", "installed": false},
            {"repo": "core", "name": "bash", "version": "5.3", "description": "shell",
             "source": "repo", "installed": true},
            {"name": "ripgrep", "version": "14", "repo": "main", "description": "grep"}
        ]));
        assert_eq!(
            rows.iter().map(|r| r.origin).collect::<Vec<_>>(),
            [Origin::Repo, Origin::Aur, Origin::Repo, Origin::Native]
        );
        assert_eq!(
            search_info(&rows[0], rows[0].installed),
            SearchInfo::Core(words("query --repo -- repgrep"))
        );
        assert_eq!(
            search_info(&rows[1], rows[1].installed),
            SearchInfo::Core(words("query --aur -- yay"))
        );
        // `info` only for what is installed
        assert_eq!(
            search_info(&rows[2], rows[2].installed),
            SearchInfo::Core(words("info -- bash"))
        );
        // another package manager: installed per the installed list, else
        // the row itself is all that is known
        assert_eq!(
            search_info(&rows[3], true),
            SearchInfo::Core(words("info -- ripgrep"))
        );
        match search_info(&rows[3], false) {
            SearchInfo::Text(t) => {
                assert!(t.starts_with("ripgrep 14\nfrom: main\n\ngrep\n"), "{t}");
                assert!(t.contains("Not installed"), "{t}");
            }
            other => panic!("{other:?}"),
        }
        // no described row is ever asked about through `info`
        for row in &rows {
            if let SearchInfo::Core(argv) = search_info(row, false) {
                assert_ne!(argv[0], "info", "{row:?}");
                assert_eq!(argv[argv.len() - 2], "--");
            }
        }
    }

    #[test]
    fn the_aur_is_searched_only_when_asked_and_only_on_pacman() {
        assert_eq!(
            search_argv("rip grep", false),
            vec!["--json", "search", "--limit", "50", "--", "rip grep"]
        );
        assert_eq!(
            search_argv("-y", true),
            vec!["--json", "search", "--limit", "50", "--all", "--", "-y"]
        );
        let none = KeyModifiers::NONE;
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('s'), none);
        handle_key(&mut app, KeyCode::Esc, none); // stop typing
        assert!(!app.search_aur, "off until asked for");
        // not a pacman machine: the key explains and changes nothing
        handle_key(&mut app, KeyCode::Char('A'), none);
        assert!(!app.search_aur);
        assert!(app.status.contains("Arch family"), "{}", app.status);
        // pacman: A turns it on and says where the query goes; A again off
        app.pacman = true;
        handle_key(&mut app, KeyCode::Char('A'), none);
        assert!(app.search_aur);
        assert!(app.status.contains("aur.archlinux.org"), "{}", app.status);
        handle_key(&mut app, KeyCode::Char('A'), none);
        assert!(!app.search_aur);
        assert_eq!(app.job_seq, 0, "an empty query starts no search");

        // an AUR result is installed through the AUR lane, any other
        // through the repositories
        let rows = search_rows(&json!([
            {"repo": "aur", "name": "yay", "version": "12", "source": "aur"},
            {"repo": "extra", "name": "fd", "version": "10", "source": "repo"}
        ]));
        assert_eq!(
            change_argv(&install_change(&rows[0]), true),
            words("install aur --dryrun -- yay")
        );
        assert_eq!(
            change_argv(&install_change(&rows[1]), true),
            words("install repo --dryrun -- fd")
        );
    }

    #[test]
    fn the_package_manager_is_read_from_the_platform_report() {
        let out = |json: &str| JobMsg {
            id: 1,
            dest: Dest::Platform,
            out: CoreOut {
                ok: true,
                stdout: json.to_string(),
                ..CoreOut::default()
            },
        };
        let mut app = App::new();
        app.on_job(out(r#"{"native_pkg_manager": "apt"}"#));
        assert!(!app.pacman);
        app.on_job(out(r#"{"native_pkg_manager": "pacman"}"#));
        assert!(app.pacman);
    }

    #[test]
    fn late_tree_results_are_added_only_to_the_pane_they_were_asked_for() {
        let mut app = App::new();
        app.tree_level = TreeLevel::Packages;
        app.tree_items.clear();
        app.tree_state.select(None);
        app.tree_job = 7;
        app.tree_loading = true;
        app.on_tree(6, vec!["stale".into()]);
        assert!(app.tree_items.is_empty() && app.tree_loading);
        app.on_tree(7, vec!["ripgrep".into(), "fd".into()]);
        assert!(!app.tree_loading);
        assert_eq!(app.tree_items.len(), 2);
        assert_eq!(app.tree_state.selected(), Some(0));
        assert!(matches!(
            &app.tree_items[0],
            TreeItem::Pkg { name, installed: false, .. } if name == "ripgrep"
        ));
        // Esc leaves the pane: what arrives afterwards is dropped
        handle_tree(&mut app, KeyCode::Esc);
        assert!(app.tree_level == TreeLevel::Subs && app.tree_job == 0);
        let before = app.tree_items.len();
        app.on_tree(7, vec!["late".into()]);
        assert_eq!(app.tree_items.len(), before);
    }

    /// Ctrl-C at the core's prompt: the signal reaches every process in the
    /// foreground. The command must die of it and this process must not.
    #[test]
    fn an_interrupt_stops_the_attached_command_and_not_this_process() {
        let mut cmd = Command::new("sh");
        // the child interrupts its parent (this process), then itself
        cmd.args([
            "-c",
            "kill -INT $PPID; kill -QUIT $PPID; kill -INT $$; exit 0",
        ])
        .stdin(Stdio::null());
        let status = run_attached(&mut cmd).expect("sh runs");
        // still here: the interrupts sent to this process were ignored
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(2),
            "the command keeps the default behaviour and is stopped: {status}"
        );
        // and afterwards a command exits normally again
        let status = run_attached(Command::new("true").stdin(Stdio::null())).unwrap();
        assert!(status.success());
    }

    #[test]
    fn the_help_and_hints_name_the_keys_that_exist() {
        assert!(HELP.contains("Not run from here: --yes, --noconfirm, i, mcp"));
        assert!(HELP.contains("Ctrl-C there stops"));
        assert!(
            TABS[Tab::Search as usize]
                .1
                .contains("A with/without the AUR")
        );
        assert!(NO_TERMINAL.contains("needs a terminal"));
    }
}
