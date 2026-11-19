// yapacc — `timeline`: what changed, package side
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Read-only: merges the package logs that already exist on this machine
//! into one list, newest first.

use crate::error::{Error, Result};
use crate::ledger::{self, Scope};
use crate::pkglog::{self, Action, LogEntry};
use crate::platform::PkgManager;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// `YYYY-MM-DD` -> unix seconds of 00:00 UTC on that day.
pub fn parse_since(s: &str) -> Option<u64> {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3 || !p.iter().all(|x| x.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let (year, month, day) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    ((1..=12).contains(&month) && (1..=31).contains(&day))
        .then(|| pkglog::utc_to_unix(year, month, day, 0, 0, 0))
}

/// Keep the entries at or after `since`, newest first, at most `limit`.
/// Entries of the same second come out in the reverse of their log order.
pub fn select(mut entries: Vec<LogEntry>, since: Option<u64>, limit: usize) -> Vec<LogEntry> {
    entries.retain(|e| since.is_none_or(|s| e.ts >= s));
    entries.reverse();
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    entries.truncate(limit);
    entries
}

/// The time of an entry as shown: UTC, `(local)` for the logs that record
/// local time without a zone, or `(time unknown)`.
fn time_label(e: &LogEntry) -> String {
    match (e.ts, e.source.as_str()) {
        (0, _) => "(time unknown)".to_string(),
        // dpkg.log, the zypp history and syslog (pkg) have local time without a zone
        (ts, "apt" | "zypper" | "pkg") => crate::util::fmt_utc(ts).replace(" UTC", " (local)"),
        (ts, _) => crate::util::fmt_utc(ts),
    }
}

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Install => "install",
        Action::Upgrade => "upgrade",
        Action::Downgrade => "downgrade",
        Action::Reinstall => "reinstall",
        Action::Remove => "remove",
    }
}

/// One line per entry: time, source, action, name, version(s).
pub fn render(entries: &[LogEntry]) -> String {
    render_with_boots(entries, &[])
}

/// [`render`], with a `── boot: <time> ──` line wherever one of `boots`
/// (unix seconds of boot starts, any order) falls between the entry below
/// it and the entry above it. A boot newer than every entry is shown on
/// top; a boot older than every entry is not shown. Entries without a time
/// never get a separator.
pub fn render_with_boots(entries: &[LogEntry], boots: &[u64]) -> String {
    if entries.is_empty() {
        return "(no package changes recorded)\n".into();
    }
    let mut pending: Vec<u64> = boots.to_vec();
    pending.sort_unstable();
    pending.dedup();
    let mut out = String::new();
    for e in entries {
        while e.ts != 0 && pending.last().is_some_and(|b| *b > e.ts) {
            let boot = pending.pop().unwrap_or(0);
            out.push_str(&format!("── boot: {} ──\n", crate::util::fmt_utc(boot)));
        }
        let time = time_label(e);
        let action = action_word(e.action);
        let arrow = if e.old_version.is_empty() || e.new_version.is_empty() {
            ""
        } else {
            " -> "
        };
        let line = format!(
            "{time:<24}  {:<6}  {action:<9}  {}  {}{arrow}{}",
            e.source, e.name, e.old_version, e.new_version
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------------ boots

/// Boot start instants from the output of
/// `journalctl --list-boots --no-pager -o json`, oldest first.
///
/// Read, in this order: a JSON array of objects with a `first_entry` in
/// microseconds; one such object per line; or the plain table journalctl
/// prints when it does not know `-o json` (`IDX BOOT-ID DAY DATE TIME ...`),
/// whose first date and time are read as UTC — so the command must be run
/// with `TZ=UTC` for the table form to be right.
pub fn parse_list_boots(text: &str) -> Vec<u64> {
    let micros = |v: &serde_json::Value| {
        let first = &v["first_entry"];
        first
            .as_u64()
            .or_else(|| first.as_str().and_then(|s| s.parse::<u64>().ok()))
            .map(|us| us / 1_000_000)
    };
    let mut out: Vec<u64> = Vec::new();
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(serde_json::Value::Array(boots)) => out.extend(boots.iter().filter_map(micros)),
        Ok(v) => out.extend(micros(&v)),
        Err(_) => {
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('{') {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        out.extend(micros(&v));
                    }
                    continue;
                }
                let f: Vec<&str> = line.split_whitespace().collect();
                let is_row = f.len() >= 4
                    && f[0].parse::<i64>().is_ok()
                    && f[1].len() == 32
                    && f[1].bytes().all(|b| b.is_ascii_hexdigit());
                if !is_row {
                    continue;
                }
                out.extend((2..f.len() - 1).find_map(|i| pkglog::date_time(f[i], f[i + 1])));
            }
        }
    }
    out.retain(|t| *t > 0);
    out.sort_unstable();
    out.dedup();
    out
}

/// The `btime` line of `/proc/stat`: when the running kernel booted.
pub fn parse_btime(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|t| *t > 0)
}

/// `sysctl -n kern.boottime` (FreeBSD): `{ sec = 1795007776, usec = 1 } ...`.
pub fn parse_kern_boottime(out: &str) -> Option<u64> {
    let rest = out.split("sec =").nth(1)?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok().filter(|t| *t > 0)
}

/// The instant `--since-boot` filters from, given the known boot starts
/// (oldest first): the start of the boot before the current one, and `true`;
/// or, when only one boot is known, the start of that one, and `false`.
pub fn since_boot_instant(starts: &[u64]) -> Option<(u64, bool)> {
    match starts {
        [] => None,
        [only] => Some((*only, false)),
        [.., previous, _current] => Some((*previous, true)),
    }
}

/// Boot starts and where they were read.
struct Boots {
    starts: Vec<u64>,
    from: &'static str,
}

/// The boots this machine can name: the journal's list when journalctl is
/// installed and answers, else the current boot from `/proc/stat`. Nothing
/// is estimated: when neither answers, the feature is unavailable.
fn known_boots() -> Result<Boots> {
    if crate::util::have("journalctl") {
        let attempts: [&[&str]; 2] = [
            &["--list-boots", "--no-pager", "-o", "json"],
            &["--list-boots", "--no-pager"],
        ];
        for args in attempts {
            let run = Command::new("journalctl")
                .args(args)
                .env("LC_ALL", "C")
                .env("TZ", "UTC")
                .output();
            match run {
                Ok(o) if o.status.success() => {
                    let starts = parse_list_boots(&String::from_utf8_lossy(&o.stdout));
                    if !starts.is_empty() {
                        return Ok(Boots {
                            starts,
                            from: "journalctl --list-boots",
                        });
                    }
                }
                _ => {}
            }
        }
    }
    let btime = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| parse_btime(&s))
        .or_else(|| {
            let o = Command::new("sysctl")
                .args(["-n", "kern.boottime"])
                .output()
                .ok()?;
            o.status
                .success()
                .then(|| parse_kern_boottime(&String::from_utf8_lossy(&o.stdout)))
                .flatten()
        });
    match btime {
        Some(t) => Ok(Boots {
            starts: vec![t],
            from: "/proc/stat btime or sysctl kern.boottime",
        }),
        None => Err(Error::unavailable(
            "--since-boot needs the time of a boot and none could be read on this machine; looked for: journalctl --list-boots, the btime line of /proc/stat, sysctl kern.boottime",
        )),
    }
}

// ------------------------------------------------------------------- HTML

/// `text` with the five characters HTML gives a meaning replaced, so that
/// it can stand in element content and in a quoted attribute.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// What the HTML page says about the machine and the run. No host name,
/// user name or path of a home directory belongs here.
#[derive(Clone, Debug, Default)]
pub struct HtmlMeta {
    /// distribution name, as os-release gives it
    pub distribution: String,
    /// the native package manager, or that it could not be determined
    pub pkg_manager: String,
    /// when the page was generated, already formatted
    pub generated: String,
    /// what the rows are limited to, e.g. the `--since-boot` header line
    pub scope: Option<String>,
    /// the same notes the text output prints (unreadable sources, limits)
    pub notes: Vec<String>,
}

const HTML_HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>yapacc timeline</title>
<style>
:root { color-scheme: light dark; --bg: #ffffff; --fg: #1b1b1b; --dim: #5c5c5c; --line: #d6d6d6; --head: #f1f1f1; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #16181b; --fg: #e4e4e4; --dim: #a0a0a0; --line: #3a3d42; --head: #22252a; }
}
body { background: var(--bg); color: var(--fg); font: 14px/1.45 system-ui, sans-serif; margin: 2rem auto; max-width: 64rem; padding: 0 1rem; }
h1 { font-size: 1.3rem; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.2rem 1rem; }
dt { color: var(--dim); }
dd { margin: 0; }
ul.notes { color: var(--dim); padding-left: 1.2rem; }
table { border-collapse: collapse; width: 100%; margin-top: 1rem; }
th, td { border-bottom: 1px solid var(--line); padding: 0.3rem 0.6rem; text-align: left; vertical-align: top; }
th { background: var(--head); }
td.t, td.v { font-family: ui-monospace, monospace; white-space: nowrap; }
</style>
</head>
<body>
"#;

/// One self-contained HTML document with the same rows as [`render`]: no
/// script, no external asset, colours following `prefers-color-scheme`.
/// Every piece of text that comes from a log or from the machine goes
/// through [`escape_html`].
pub fn render_html(entries: &[LogEntry], meta: &HtmlMeta) -> String {
    let mut out = String::from(HTML_HEAD);
    out.push_str("<h1>Package changes</h1>\n<dl>\n");
    for (key, value) in [
        ("Distribution", &meta.distribution),
        ("Package manager", &meta.pkg_manager),
        ("Generated", &meta.generated),
    ] {
        out.push_str(&format!("<dt>{key}</dt><dd>{}</dd>\n", escape_html(value)));
    }
    out.push_str("</dl>\n");
    if let Some(scope) = &meta.scope {
        out.push_str(&format!("<p>{}</p>\n", escape_html(scope)));
    }
    if !meta.notes.is_empty() {
        out.push_str("<ul class=\"notes\">\n");
        for note in &meta.notes {
            out.push_str(&format!("<li>note: {}</li>\n", escape_html(note)));
        }
        out.push_str("</ul>\n");
    }
    if entries.is_empty() {
        out.push_str("<p>(no package changes recorded)</p>\n");
    } else {
        out.push_str(
            "<table>\n<thead><tr><th>Time</th><th>Source</th><th>Action</th><th>Package</th><th>Old version</th><th>New version</th></tr></thead>\n<tbody>\n",
        );
        for e in entries {
            out.push_str(&format!(
                "<tr><td class=\"t\">{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"v\">{}</td><td class=\"v\">{}</td></tr>\n",
                escape_html(&time_label(e)),
                escape_html(&e.source),
                action_word(e.action),
                escape_html(&e.name),
                escape_html(&e.old_version),
                escape_html(&e.new_version),
            ));
        }
        out.push_str("</tbody>\n</table>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// Entries recorded in a yapacc install ledger (the text of `journal.jsonl`).
/// An `upsert` is an install, unless the record it carries has the status
/// `removed`; a `remove` names its record by id only.
pub fn ledger_entries(journal: &str) -> Vec<LogEntry> {
    let mut known: HashMap<String, (String, String)> = HashMap::new();
    let mut out = Vec::new();
    for line in journal.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let field = |key: &str| ev["payload"][key].as_str().unwrap_or("").to_string();
        let (action, name, version) = match ev["event_type"].as_str() {
            Some("upsert") => {
                known.insert(field("id"), (field("name"), field("version")));
                let action = if field("status") == "removed" {
                    Action::Remove
                } else {
                    Action::Install
                };
                (action, field("name"), field("version"))
            }
            Some("remove") => {
                let (name, version) = known
                    .remove(&field("id"))
                    .unwrap_or((field("id"), String::new()));
                (Action::Remove, name, version)
            }
            _ => continue,
        };
        let (old_version, new_version) = match action {
            Action::Remove => (version, String::new()),
            _ => (String::new(), version),
        };
        out.push(LogEntry {
            ts: ev["timestamp"].as_u64().unwrap_or(0),
            source: "yapacc".into(),
            action,
            name,
            old_version,
            new_version,
        });
    }
    out
}

/// `dnf ARGS` as `timeline` runs it: `LC_ALL=C` for the wording, and
/// `TZ=UTC` because dnf 4 prints its history in local time (`Begin time :
/// Thu Nov 19 21:29:42 2026` at 12:29:42 UTC with `TZ=XXX-9`, Rocky 9) and
/// these times are shown as UTC. dnf 5 prints UTC whatever the zone.
fn dnf_command(args: &[&str]) -> Command {
    let mut command = Command::new("dnf");
    command.args(args).env("LC_ALL", "C").env("TZ", "UTC");
    command
}

/// Standard output of `dnf ARGS`, when the command ran and succeeded.
fn dnf(args: &[&str]) -> Option<String> {
    let run = dnf_command(args).output();
    match run {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        _ => None,
    }
}

/// Per-package entries of the transactions in `dnf history list`, newest
/// transaction first: `dnf history info ID` is run for each transaction at
/// or after `since`, until `limit` entries are known.
fn dnf_entries(
    list: &str,
    since: Option<u64>,
    limit: usize,
    unreadable: &mut Vec<String>,
) -> Vec<LogEntry> {
    let mut transactions = pkglog::parse_dnf_history(list);
    transactions.sort_by_key(|t| std::cmp::Reverse(t.id));
    let mut out = Vec::new();
    for t in transactions {
        if out.len() >= limit {
            break;
        }
        if since.is_some_and(|s| t.ts < s) {
            continue;
        }
        let id = t.id.to_string();
        match dnf(&["history", "info", &id]) {
            Some(info) => out.extend(pkglog::parse_dnf_history_info(&info)),
            None => unreadable.push(format!("dnf history info {id}")),
        }
    }
    out
}

/// What the sources gave: `read` counts the sources that were read,
/// `unreadable` names the ones that exist and could not be read.
#[derive(Default)]
struct Gathered {
    entries: Vec<LogEntry>,
    read: usize,
    looked_for: Vec<String>,
    unreadable: Vec<String>,
    /// times of the apk runs logged without package details (`apk --quiet`)
    apk_quiet: Vec<u64>,
}

impl Gathered {
    fn file(&mut self, path: &Path, parse: fn(&str) -> Vec<LogEntry>) {
        self.looked_for.push(path.display().to_string());
        match std::fs::read(path) {
            Ok(bytes) => {
                self.read += 1;
                self.entries.extend(parse(&String::from_utf8_lossy(&bytes)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => self.unreadable.push(path.display().to_string()),
        }
    }
}

impl Gathered {
    /// A rotated, gzip-compressed log (`dpkg.log.2.gz`), read through
    /// `gzip -dc` so that the core links no decompressor.
    fn gz_file(&mut self, path: &Path, parse: fn(&str) -> Vec<LogEntry>) {
        if !path.exists() {
            return;
        }
        self.looked_for.push(path.display().to_string());
        match Command::new("gzip").arg("-dc").arg("--").arg(path).output() {
            Ok(out) if out.status.success() => {
                self.read += 1;
                self.entries
                    .extend(parse(&String::from_utf8_lossy(&out.stdout)));
            }
            _ => self.unreadable.push(path.display().to_string()),
        }
    }
}

fn gather(since: Option<u64>, limit: usize) -> Gathered {
    let mut g = Gathered::default();
    g.file(Path::new("/var/log/pacman.log"), pkglog::parse_pacman_log);
    // oldest first, so that entries stay in log order: dpkg rotates to
    // dpkg.log.1 (plain) and dpkg.log.2.gz and older (compressed)
    for n in (2..=12).rev() {
        g.gz_file(
            Path::new(&format!("/var/log/dpkg.log.{n}.gz")),
            pkglog::parse_dpkg_log,
        );
    }
    g.file(Path::new("/var/log/dpkg.log.1"), pkglog::parse_dpkg_log);
    g.file(Path::new("/var/log/dpkg.log"), pkglog::parse_dpkg_log);
    // zypp history plus what logrotate has moved aside (history-DATE.gz)
    g.looked_for.push("/var/log/zypp/history".into());
    match crate::undo::read_zypp_history(Path::new("/var/log/zypp")) {
        Ok((text, skipped)) => {
            g.read += 1;
            g.entries.extend(pkglog::parse_zypp_history(&text));
            g.unreadable
                .extend(skipped.into_iter().map(|n| format!("/var/log/zypp/{n}")));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => g.unreadable.push("/var/log/zypp/history".into()),
    }
    g.file(Path::new("/var/log/apk.log"), pkglog::parse_apk_log);
    if let Ok(bytes) = std::fs::read("/var/log/apk.log") {
        g.apk_quiet = pkglog::apk_quiet_runs(&String::from_utf8_lossy(&bytes));
    }
    let (present, native) = crate::platform::native_manager();
    // /var/log/messages is every program's log: it is read for package
    // changes only where FreeBSD's pkg, which writes them there, is present
    if present.contains(&PkgManager::Pkg) {
        g.file(Path::new("/var/log/messages"), pkglog::parse_pkg_messages);
    }
    if native == Some(PkgManager::Dnf) {
        g.looked_for.push("dnf history list".into());
        match dnf(&["history", "list"]) {
            Some(list) => {
                g.read += 1;
                let found = dnf_entries(&list, since, limit, &mut g.unreadable);
                g.entries.extend(found);
            }
            None => g.unreadable.push("dnf history list".into()),
        }
    }
    for scope in [Scope::User, Scope::System] {
        let journal = ledger::paths_for(&ledger::default_root(&scope)).journal;
        g.file(&journal, ledger_entries);
    }
    g
}

/// What `yapacc timeline` was asked for.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `--since YYYY-MM-DD`
    pub since: Option<String>,
    /// `--limit N`
    pub limit: usize,
    /// `--since-boot`: only changes since the start of the previous boot
    pub since_boot: bool,
    /// `--html`: one self-contained HTML document instead of text
    pub html: bool,
}

/// The note about apk runs that were logged without their packages:
/// `count` runs, at or after the start of what is shown. Pure.
pub fn apk_quiet_note(count: usize) -> Option<String> {
    (count > 0).then(|| {
        format!(
            "{count} apk run(s) were logged without package details (apk --quiet): what they installed, upgraded or removed, if anything, is not in /var/log/apk.log and is not shown"
        )
    })
}

/// What is added to "no package log could be read". `native` is the native
/// package manager, when one was determined; `missing_tool` is the "this
/// system uses X but `X` is not on PATH" sentence, which only applies when
/// none was (a database whose tool is not on PATH says nothing about a
/// system whose own manager is present). Pure.
pub fn no_log_reason(native: Option<PkgManager>, missing_tool: Option<String>) -> String {
    match native {
        Some(PkgManager::Xbps) => {
            "; xbps keeps no transaction log of its own, so there is nothing to read".into()
        }
        Some(PkgManager::Apk) => {
            "; apk writes /var/log/apk.log only when run as root and that file is absent".into()
        }
        Some(_) => String::new(),
        None => missing_tool.map(|m| format!("; {m}")).unwrap_or_default(),
    }
}

/// The log of native manager `m`, where its history is one file that
/// `timeline` reads directly. apt's is several files (dpkg.log and what was
/// rotated out of it) and dnf's is behind `dnf history`: they have none
/// here. Pure.
pub fn own_log(native: Option<PkgManager>) -> Option<&'static str> {
    match native? {
        PkgManager::Zypper => Some("/var/log/zypp/history"),
        PkgManager::Pacman => Some("/var/log/pacman.log"),
        PkgManager::Apk => Some("/var/log/apk.log"),
        PkgManager::Pkg => Some("/var/log/messages"),
        _ => None,
    }
}

/// The whole error when the log of this system's own package manager is
/// there and this user may not read it (openSUSE keeps /var/log/zypp
/// root-only): that log and the remedy, and no list of the logs of other
/// package managers. `None` when that is not the case. Pure.
pub fn own_log_unreadable_message(
    native: Option<PkgManager>,
    unreadable: &[String],
) -> Option<String> {
    let own = own_log(native)?;
    unreadable.iter().any(|u| u == own).then(|| {
        format!(
            "no package log could be read: {own} exists but this user may not read it; run as root (`sudo yapacc timeline`)"
        )
    })
}

/// The error when no package log could be read. A log that exists but is
/// not readable is a permission problem, not a missing log: say so. Pure.
fn no_log_message(looked_for: &[String], unreadable: &[String]) -> String {
    if unreadable.is_empty() {
        format!(
            "no package log could be read on this machine; looked for: {}",
            looked_for.join(", ")
        )
    } else {
        format!(
            "no package log could be read: {} exist(s) but this user may not read them; run as root (sudo yapacc timeline). Also looked for: {}",
            unreadable.join(", "),
            looked_for.join(", ")
        )
    }
}

pub fn run(opts: &Options, json: bool) -> Result<()> {
    if opts.html && json {
        return Err(Error::usage(
            "--html and --json are two output formats; choose one",
        ));
    }
    let mut since = match opts.since.as_deref() {
        Some(s) => Some(parse_since(s).ok_or_else(|| {
            Error::usage(format!("--since '{s}' is not a date; write YYYY-MM-DD"))
        })?),
        None => None,
    };
    let mut notes: Vec<String> = Vec::new();
    let mut header: Option<String> = None;
    let mut boots: Vec<u64> = Vec::new();
    if opts.since_boot {
        let known = known_boots()?;
        let Some((instant, previous)) = since_boot_instant(&known.starts) else {
            return Err(Error::unavailable(format!(
                "--since-boot: {} named no boot",
                known.from
            )));
        };
        let which = if previous { "previous" } else { "current" };
        header = Some(format!(
            "Changes since the start of the {which} boot: {} (from {})",
            crate::util::fmt_utc(instant),
            known.from
        ));
        if !previous {
            notes.push(format!(
                "only the current boot is known (from {}): these are the changes since it started, not since the boot before it",
                known.from
            ));
        }
        since = Some(since.map_or(instant, |s| s.max(instant)));
        boots = known.starts;
    }
    let g = gather(since, opts.limit);
    if g.read == 0 {
        let native = crate::platform::native_manager().1;
        if let Some(message) = own_log_unreadable_message(native, &g.unreadable) {
            return Err(Error::unavailable(message));
        }
        let missing_tool = if native.is_none() {
            crate::platform::missing_native_tool()
                .map(|(m, tool)| crate::platform::missing_tool_message(m, tool))
        } else {
            None
        };
        return Err(Error::unavailable(format!(
            "{}{}",
            no_log_message(&g.looked_for, &g.unreadable),
            no_log_reason(native, missing_tool)
        )));
    }
    if opts.since_boot {
        if g.entries
            .iter()
            .any(|e| e.ts != 0 && matches!(e.source.as_str(), "apt" | "zypper"))
        {
            notes.push("apt and zypper log local time without a zone and yapacc reads it as UTC: their changes within the zone offset of the boot instant can be on the wrong side of it".into());
        }
        if g.entries.iter().any(|e| e.ts == 0) {
            notes.push("changes logged without a usable time are left out: they cannot be placed before or after a boot".into());
        }
    }
    let quiet = g
        .apk_quiet
        .iter()
        .filter(|t| since.is_none_or(|s| **t >= s))
        .count();
    notes.extend(apk_quiet_note(quiet));
    let entries = select(g.entries, since, opts.limit);
    notes.extend(
        g.unreadable
            .iter()
            .map(|what| format!("could not read {what}")),
    );
    if opts.html {
        let p = crate::platform::detect();
        let distribution = [p.os.pretty_name.as_str(), p.os.id.as_str()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("not determined")
            .to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let meta = HtmlMeta {
            distribution,
            pkg_manager: p
                .native_pkg_manager
                .map(|m| m.name().to_string())
                .unwrap_or_else(|| "not determined".into()),
            generated: crate::util::fmt_utc(now),
            scope: header.clone(),
            notes: notes.clone(),
        };
        print!("{}", render_html(&entries, &meta));
    } else if json {
        let text = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
        println!("{text}");
    } else {
        if let Some(h) = &header {
            println!("{h}");
        }
        print!("{}", render_with_boots(&entries, &boots));
    }
    for note in &notes {
        eprintln!("note: {note}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_log_is_a_permission_message() {
        let looked = vec!["/var/log/zypp/history".to_string()];
        let m = no_log_message(&looked, &looked);
        assert!(m.contains("run as root") && m.contains("/var/log/zypp/history"));
        let m = no_log_message(&looked, &[]);
        assert!(m.starts_with("no package log could be read on this machine"));
    }

    /// VM round of 2026-11-19, Tumbleweed as an ordinary user: the message
    /// ended "Also looked for: /var/log/pacman.log, /var/log/dpkg.log.1,
    /// ... /var/log/apk.log" on an openSUSE system.
    #[test]
    fn an_unreadable_own_log_is_named_alone_with_the_remedy() {
        let unreadable = vec!["/var/log/zypp/history".to_string()];
        let m = own_log_unreadable_message(Some(PkgManager::Zypper), &unreadable).unwrap();
        assert_eq!(
            m,
            "no package log could be read: /var/log/zypp/history exists but this user may not read it; run as root (`sudo yapacc timeline`)"
        );
        assert!(!m.contains("pacman") && !m.contains("dpkg") && !m.contains("apk"));
        // the log of another manager being unreadable is not this case
        assert_eq!(
            own_log_unreadable_message(Some(PkgManager::Pacman), &unreadable),
            None
        );
        assert_eq!(
            own_log_unreadable_message(Some(PkgManager::Zypper), &[]),
            None
        );
        assert_eq!(own_log_unreadable_message(None, &unreadable), None);
        // apt and dnf keep the general message: their history is not one file
        for m in [PkgManager::Apt, PkgManager::Dnf] {
            assert_eq!(own_log(Some(m)), None, "{m:?}");
            let dpkg = vec!["/var/log/dpkg.log".to_string()];
            assert_eq!(own_log_unreadable_message(Some(m), &dpkg), None);
        }
        assert_eq!(
            own_log(Some(PkgManager::Pacman)),
            Some("/var/log/pacman.log")
        );
    }

    /// Z3: on openSUSE, as an ordinary user, the message ended with "; this
    /// system uses dnf but `dnf` is not on PATH" although zypper is there.
    #[test]
    fn a_missing_tool_is_named_only_when_no_manager_was_determined() {
        let hint = || Some("this system uses dnf but `dnf` is not on PATH".to_string());
        assert_eq!(no_log_reason(Some(PkgManager::Zypper), hint()), "");
        assert_eq!(no_log_reason(Some(PkgManager::Apt), hint()), "");
        assert_eq!(
            no_log_reason(None, hint()),
            "; this system uses dnf but `dnf` is not on PATH"
        );
        assert_eq!(no_log_reason(None, None), "");
        assert!(no_log_reason(Some(PkgManager::Xbps), hint()).contains("xbps keeps no"));
        assert!(no_log_reason(Some(PkgManager::Apk), None).contains("only when run as root"));
    }

    /// E3: a transaction run with `apk --quiet` leaves only a header.
    #[test]
    fn quiet_apk_runs_get_a_note() {
        assert_eq!(apk_quiet_note(0), None);
        assert_eq!(
            apk_quiet_note(2).unwrap(),
            "2 apk run(s) were logged without package details (apk --quiet): what they installed, upgraded or removed, if anything, is not in /var/log/apk.log and is not shown"
        );
    }

    fn entry(ts: u64, source: &str, action: Action, name: &str, old: &str, new: &str) -> LogEntry {
        LogEntry {
            ts,
            source: source.into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    /// The text and the JSON form say the same thing about a dpkg
    /// downgrade and a dpkg reinstall (real dpkg.log lines, debian:stable).
    #[test]
    fn a_dpkg_downgrade_reads_the_same_in_text_and_json() {
        let log = "2026-11-19 12:31:49 upgrade zlib1g:amd64 1:1.3.dfsg+really1.3.1-1+deb13u1 1:1.3.dfsg+really1.3.1-1+b1\n\
                   2026-11-19 12:29:43 upgrade tree:amd64 2.2.1-1 2.2.1-1\n";
        let entries = select(pkglog::parse_dpkg_log(log), None, 10);
        assert_eq!(
            render(&entries),
            "2026-11-19 12:31 (local)  apt     downgrade  zlib1g  1:1.3.dfsg+really1.3.1-1+deb13u1 -> 1:1.3.dfsg+really1.3.1-1+b1\n\
             2026-11-19 12:29 (local)  apt     reinstall  tree  2.2.1-1\n"
        );
        let json = serde_json::to_value(&entries).unwrap();
        assert_eq!(json[0]["action"], "downgrade");
        assert_eq!(json[1]["action"], "reinstall");
        assert_eq!(json[1]["old_version"], "");
    }

    /// F10: on Rocky 9 (dnf 4) in a zone nine hours east of UTC, `timeline`
    /// showed `2026-11-19 21:29 UTC` for a change made at 12:29 UTC.
    #[test]
    fn dnf_is_asked_for_its_history_in_utc() {
        let command = dnf_command(&["history", "list"]);
        let env = |key: &str| {
            command
                .get_envs()
                .find(|(k, _)| *k == std::ffi::OsStr::new(key))
                .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        assert_eq!(env("TZ").as_deref(), Some("UTC"));
        assert_eq!(env("LC_ALL").as_deref(), Some("C"));
    }

    #[test]
    fn select_merges_newest_first_and_filters() {
        let all = vec![
            entry(100, "pacman", Action::Install, "a", "", "1"),
            entry(300, "pacman", Action::Install, "b", "", "1"),
            entry(300, "pacman", Action::Install, "c", "", "1"),
            entry(200, "apt", Action::Remove, "d", "1", ""),
            entry(0, "yapacc", Action::Install, "e", "", "1"),
        ];
        let names = |v: Vec<LogEntry>| v.into_iter().map(|e| e.name).collect::<Vec<_>>();
        let merged = names(select(all.clone(), None, 50));
        assert_eq!(merged, ["c", "b", "d", "a", "e"]);
        assert_eq!(names(select(all.clone(), Some(200), 50)), ["c", "b", "d"]);
        assert_eq!(names(select(all, None, 2)), ["c", "b"]);
    }

    #[test]
    fn since_is_a_strict_date() {
        assert_eq!(parse_since("2026-11-10"), Some(1_794_268_800));
        for bad in ["", "yesterday", "2026-13-01", "2026-11-00", "2026-11"] {
            assert_eq!(parse_since(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn render_lines() {
        assert_eq!(render(&[]), "(no package changes recorded)\n");
        let t = 1_794_771_081;
        let text = render(&[
            entry(t - 75, "apt", Action::Install, "tree", "", "2.2.1-1"),
            entry(t, "pacman", Action::Upgrade, "linux", "6.17-1", "6.18-1"),
            entry(t, "pacman", Action::Remove, "tree", "2.3.2-1", ""),
            entry(0, "dnf", Action::Install, "dnf -y install tree", "", ""),
        ]);
        let want = "\
2026-11-15 19:30 (local)  apt     install    tree  2.2.1-1
2026-11-15 19:31 UTC      pacman  upgrade    linux  6.17-1 -> 6.18-1
2026-11-15 19:31 UTC      pacman  remove     tree  2.3.2-1
(time unknown)            dnf     install    dnf -y install tree
";
        assert_eq!(text, want);
    }

    #[test]
    fn zypper_times_are_labelled_local() {
        let text = render(&[entry(
            1_794_771_006,
            "zypper",
            Action::Upgrade,
            "curl",
            "8.9.0-1.1",
            "8.10.0-1.1",
        )]);
        assert_eq!(
            text,
            "2026-11-15 19:30 (local)  zypper  upgrade    curl  8.9.0-1.1 -> 8.10.0-1.1\n"
        );
    }

    #[test]
    fn list_boots_json_array_lines_and_table() {
        let b1 = pkglog::utc_to_unix(2026, 11, 14, 8, 0, 1);
        let b2 = pkglog::utc_to_unix(2026, 11, 15, 7, 30, 0);
        // Written from the documented output of systemd's journalctl; there
        // is no fixture of it in fixtures/native yet.
        let array = format!(
            r#"[{{"index":-1,"boot_id":"0123456789abcdef0123456789abcdef","first_entry":{},"last_entry":{}}},{{"index":0,"boot_id":"fedcba9876543210fedcba9876543210","first_entry":{},"last_entry":{}}}]"#,
            b1 * 1_000_000 + 999_999,
            b1 * 1_000_000 + 5_000_000,
            b2 * 1_000_000,
            b2 * 1_000_000 + 7_000_000,
        );
        assert_eq!(parse_list_boots(&array), vec![b1, b2]);
        let lines = format!(
            "{{\"index\":0,\"first_entry\":\"{}\"}}\n{{\"index\":-1,\"first_entry\":{}}}\n{{\"index\":-2}}\n",
            b2 * 1_000_000,
            b1 * 1_000_000,
        );
        assert_eq!(parse_list_boots(&lines), vec![b1, b2]);
        let table = "\
IDX BOOT ID                          FIRST ENTRY                 LAST ENTRY
 -1 0123456789abcdef0123456789abcdef Sat 2026-11-14 08:00:01 UTC Sat 2026-11-14 23:10:00 UTC
  0 fedcba9876543210fedcba9876543210 Sun 2026-11-15 07:30:00 UTC Sun 2026-11-15 19:31:00 UTC
";
        assert_eq!(parse_list_boots(table), vec![b1, b2]);
        let old_table = "-1 0123456789abcdef0123456789abcdef Sat 2026-11-14 08:00:01 UTC—Sat 2026-11-14 23:10:00 UTC\n";
        assert_eq!(parse_list_boots(old_table), vec![b1]);
        for none in [
            "",
            "No journal files were found.\n",
            "[]",
            "-- No entries --\n",
        ] {
            assert!(parse_list_boots(none).is_empty(), "{none:?}");
        }
    }

    #[test]
    fn kern_boottime_is_parsed() {
        assert_eq!(
            parse_kern_boottime("{ sec = 1795007776, usec = 123 } Wed Nov 18 13:16:16 2026\n"),
            Some(1_795_007_776)
        );
        assert_eq!(parse_kern_boottime("garbage"), None);
    }

    #[test]
    fn btime_and_the_instant_since_boot_uses() {
        let stat = "cpu  1 2 3 4\nintr 5 6\nctxt 7\nbtime 1794771000\nprocesses 9\n";
        assert_eq!(parse_btime(stat), Some(1_794_771_000));
        assert_eq!(parse_btime("cpu  1 2 3 4\n"), None);
        assert_eq!(parse_btime("btime soon\n"), None);
        assert_eq!(since_boot_instant(&[]), None);
        assert_eq!(since_boot_instant(&[300]), Some((300, false)));
        assert_eq!(since_boot_instant(&[100, 200, 300]), Some((200, true)));
    }

    #[test]
    fn boot_separators_stand_between_entries() {
        let t = 1_794_771_081; // 2026-11-15 19:31:21 UTC
        let entries = [
            entry(t, "pacman", Action::Install, "c", "", "1"),
            entry(t - 3600, "pacman", Action::Install, "b", "", "1"),
            entry(t - 7200, "pacman", Action::Install, "a", "", "1"),
            entry(0, "pkg", Action::Install, "z", "", "1"),
        ];
        // one boot after everything, one between b and a, one before everything
        let text = render_with_boots(&entries, &[t - 90_000, t + 60, t - 5400]);
        let want = "\
── boot: 2026-11-15 19:32 UTC ──
2026-11-15 19:31 UTC      pacman  install    c  1
2026-11-15 18:31 UTC      pacman  install    b  1
── boot: 2026-11-15 18:01 UTC ──
2026-11-15 17:31 UTC      pacman  install    a  1
(time unknown)            pkg     install    z  1
";
        assert_eq!(text, want);
        assert_eq!(render_with_boots(&entries, &[]), render(&entries));
    }

    #[test]
    fn html_text_is_escaped() {
        assert_eq!(
            escape_html(r#"<script>alert("x")</script> & 'y'"#),
            "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;y&#39;"
        );
        assert_eq!(escape_html("libstdc++-13.2"), "libstdc++-13.2");
    }

    #[test]
    fn html_page_is_self_contained_and_escapes_log_text() {
        let meta = HtmlMeta {
            distribution: "Some <Linux> & Co".into(),
            pkg_manager: "pacman".into(),
            generated: "2026-11-16 10:00 UTC".into(),
            scope: Some("Changes since the start of the previous boot: <t>".into()),
            notes: vec!["could not read /var/log/<odd>".into()],
        };
        let page = render_html(
            &[
                entry(
                    1_794_771_081,
                    "pacman",
                    Action::Upgrade,
                    "<script>alert(1)</script>",
                    "1.0\"><img src=x>",
                    "2.0&",
                ),
                entry(0, "pkg", Action::Remove, "tree", "2.1.1", ""),
            ],
            &meta,
        );
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.trim_end().ends_with("</html>"));
        // nothing from the logs or the machine reaches the page as markup
        assert!(!page.contains("<script"), "{page}");
        assert!(!page.contains("<img"), "{page}");
        assert!(!page.contains("<odd>") && !page.contains("<Linux>") && !page.contains("<t>"));
        assert!(page.contains("<td>&lt;script&gt;alert(1)&lt;/script&gt;</td>"));
        assert!(page.contains("<td class=\"v\">1.0&quot;&gt;&lt;img src=x&gt;</td>"));
        assert!(page.contains("<td class=\"v\">2.0&amp;</td>"));
        assert!(page.contains("<dd>Some &lt;Linux&gt; &amp; Co</dd>"));
        assert!(page.contains("<li>note: could not read /var/log/&lt;odd&gt;</li>"));
        assert!(
            page.contains(
                "<td class=\"t\">2026-11-15 19:31 UTC</td><td>pacman</td><td>upgrade</td>"
            )
        );
        assert!(page.contains("<td class=\"t\">(time unknown)</td><td>pkg</td><td>remove</td>"));
        // self-contained: no address, no external asset, no script
        for outside in ["http://", "https://", "src=\"", "href=", "@import", "url("] {
            assert!(!page.contains(outside), "{outside}");
        }
        assert!(page.contains("prefers-color-scheme: dark"));
        let empty = render_html(&[], &HtmlMeta::default());
        assert!(empty.contains("(no package changes recorded)"));
        assert!(!empty.contains("<table>"));
    }

    #[test]
    fn ledger_journal_becomes_installs_and_removes() {
        let journal = r#"{"seq":1,"event_type":"init","timestamp":10,"payload":{}}
{"seq":2,"event_type":"upsert","timestamp":20,"payload":{"id":"pkg-a","name":"smoke-app","version":"1.0","status":"installed"}}
not json
{"seq":3,"event_type":"upsert","timestamp":30,"payload":{"id":"pkg-a","name":"smoke-app","version":"1.0","status":"removed"}}
{"seq":4,"event_type":"upsert","timestamp":40,"payload":{"id":"pkg-b","name":"other","version":"2.0","status":"installed"}}
{"seq":5,"event_type":"remove","timestamp":50,"payload":{"id":"pkg-b"}}
"#;
        let want = vec![
            entry(20, "yapacc", Action::Install, "smoke-app", "", "1.0"),
            entry(30, "yapacc", Action::Remove, "smoke-app", "1.0", ""),
            entry(40, "yapacc", Action::Install, "other", "", "2.0"),
            entry(50, "yapacc", Action::Remove, "other", "2.0", ""),
        ];
        assert_eq!(ledger_entries(journal), want);
    }
}
