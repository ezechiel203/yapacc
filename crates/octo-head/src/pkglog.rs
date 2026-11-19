//! Package change logs: one entry type for every package manager's history.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Install,
    Upgrade,
    Downgrade,
    Reinstall,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub ts: u64,        // unix seconds, UTC; 0 if the line has no usable time
    pub source: String, // "pacman" | "apt" | "dnf" | "zypper" | "pkg" | "yapacc"
    pub action: Action,
    pub name: String,
    pub old_version: String, // "" when not applicable
    pub new_version: String, // "" when not applicable
}

/// Unix seconds of a UTC date and time (days-from-civil). Before 1970: 0.
pub fn utc_to_unix(year: i64, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let yoe = y.rem_euclid(400);
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = y.div_euclid(400) * 146_097 + doe - 719_468;
    let secs = days * 86_400 + i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec);
    u64::try_from(secs).unwrap_or(0)
}

/// `2026-11-08T00:03:49+0000` -> unix seconds, UTC. 0 if it does not parse.
fn pacman_time(stamp: &str) -> u64 {
    let num = |from: usize, to: usize| {
        stamp
            .get(from..to)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(min), Some(sec)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
    ) else {
        return 0;
    };
    let (Some(zone_h), Some(zone_m)) = (num(20, 22), num(22, 24)) else {
        return 0;
    };
    let local = utc_to_unix(i64::from(year), month, day, hour, min, sec);
    let offset = u64::from(zone_h) * 3600 + u64::from(zone_m) * 60;
    match stamp.as_bytes()[19] {
        b'+' if stamp.len() == 24 => local.saturating_sub(offset),
        b'-' if stamp.len() == 24 => local + offset,
        _ => 0,
    }
}

/// Parse the text of `/var/log/pacman.log`. Only `[ALPM]` lines with the
/// verbs installed, upgraded, downgraded, reinstalled, removed are entries.
pub fn parse_pacman_log(text: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((stamp, rest)) = line
            .strip_prefix('[')
            .and_then(|l| l.split_once("] [ALPM] "))
        else {
            continue;
        };
        let Some((verb, rest)) = rest.split_once(' ') else {
            continue;
        };
        let action = match verb {
            "installed" => Action::Install,
            "upgraded" => Action::Upgrade,
            "downgraded" => Action::Downgrade,
            "reinstalled" => Action::Reinstall,
            "removed" => Action::Remove,
            _ => continue,
        };
        let Some((name, versions)) = rest.strip_suffix(')').and_then(|r| r.split_once(" (")) else {
            continue;
        };
        let (old_version, new_version) = match (action, versions.split_once(" -> ")) {
            (Action::Upgrade | Action::Downgrade, Some(pair)) => pair,
            (Action::Upgrade | Action::Downgrade, None) => continue,
            (Action::Remove, _) => (versions, ""),
            (_, _) => ("", versions),
        };
        if stamp.contains(']') || name.is_empty() {
            continue;
        }
        out.push(LogEntry {
            ts: pacman_time(stamp),
            source: "pacman".into(),
            action,
            name: name.into(),
            old_version: old_version.into(),
            new_version: new_version.into(),
        });
    }
    out
}

/// `2026-11-15` and `19:30:06` -> unix seconds, reading the pair as UTC.
pub fn date_time(date: &str, time: &str) -> Option<u64> {
    let num = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    Some(utc_to_unix(
        i64::from(num(d[0])?),
        num(d[1])?,
        num(d[2])?,
        num(t[0])?,
        num(t[1])?,
        num(t[2])?,
    ))
}

/// Parse the text of `/var/log/dpkg.log`. Only the lines whose third field
/// is install, upgrade or remove are entries. Times are local time in the
/// log and are read as if they were UTC.
///
/// dpkg writes `upgrade` for every replacement of one version by another:
///
/// ```text
/// 2026-11-19 12:31:49 upgrade zlib1g:amd64 1:1.3.dfsg+really1.3.1-1+deb13u1 1:1.3.dfsg+really1.3.1-1+b1
/// 2026-11-19 12:29:43 upgrade tree:amd64 2.2.1-1 2.2.1-1
/// ```
///
/// The two versions tell: the same one twice is a `Reinstall`, and a change
/// to a lower version, compared as dpkg compares them, is a `Downgrade`.
pub fn parse_dpkg_log(text: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 6 {
            continue;
        }
        let action = match f[2] {
            "install" => Action::Install,
            "upgrade" => Action::Upgrade,
            "remove" => Action::Remove,
            _ => continue,
        };
        let version = |v: &str| if v == "<none>" { "" } else { v }.to_string();
        let (mut old_version, new_version) = (version(f[4]), version(f[5]));
        let action = match action {
            Action::Upgrade if old_version == new_version => {
                // like the other logs: a reinstall has no old version
                old_version.clear();
                Action::Reinstall
            }
            Action::Upgrade
                if crate::vercmp::deb_vercmp(&old_version, &new_version)
                    == std::cmp::Ordering::Greater =>
            {
                Action::Downgrade
            }
            other => other,
        };
        out.push(LogEntry {
            ts: date_time(f[0], f[1]).unwrap_or(0),
            source: "apt".into(),
            action,
            name: f[3].split(':').next().unwrap_or(f[3]).to_string(),
            old_version,
            new_version,
        });
    }
    out
}

/// Parse `/var/log/apk.log` (apk-tools 3 writes it as root):
///
/// ```text
/// Running `apk add htop` at 2026-11-17 12:37:33
/// (1/2) Installing lsof (4.99.6-r0)
/// (1/1) Upgrading musl (1.2.6-r2 -> 1.2.6-r3)
/// (2/2) Purging htop (3.5.3-r0)
/// ```
///
/// Only the `Running ... at DATE TIME` header carries a time, so every
/// entry takes the time of the run it belongs to. That time is UTC:
/// apk-tools 3.0.9 run with `TZ=XXX-5` at 17:26 local time wrote `at
/// 2026-11-19 12:26:20`. Lines outside the numbered
/// `(n/m) Verb name (...)` shape (triggers, `OK:`, error blocks) are skipped.
pub fn parse_apk_log(text: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    let mut ts = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Running `") {
            ts = rest
                .rsplit_once("` at ")
                .and_then(|(_, when)| when.trim().split_once(' '))
                .and_then(|(d, t)| date_time(d, t))
                .unwrap_or(0);
            continue;
        }
        let Some(rest) = line.strip_prefix('(') else {
            continue;
        };
        let Some((counter, rest)) = rest.split_once(") ") else {
            continue;
        };
        let counter_ok = counter.split_once('/').is_some_and(|(a, b)| {
            let digits =
                |x: &str| !x.trim().is_empty() && x.trim().bytes().all(|c| c.is_ascii_digit());
            digits(a) && digits(b)
        });
        if !counter_ok {
            continue;
        }
        let Some((verb, rest)) = rest.split_once(' ') else {
            continue;
        };
        let action = match verb {
            "Installing" => Action::Install,
            "Upgrading" => Action::Upgrade,
            "Downgrading" => Action::Downgrade,
            "Reinstalling" => Action::Reinstall,
            "Purging" => Action::Remove,
            _ => continue,
        };
        let Some((name, versions)) = rest.split_once(" (") else {
            continue;
        };
        let Some(versions) = versions.trim_end().strip_suffix(')') else {
            continue;
        };
        if name.is_empty() || name.contains(' ') {
            continue;
        }
        let (old, new) = match (action, versions.split_once(" -> ")) {
            (Action::Upgrade | Action::Downgrade, Some((o, n))) => (o, n),
            (Action::Upgrade | Action::Downgrade, None) => continue,
            (Action::Remove, _) => (versions, ""),
            _ => ("", versions),
        };
        out.push(LogEntry {
            ts,
            source: "apk".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        });
    }
    out
}

/// The times of the apk runs in `/var/log/apk.log` that were asked to be
/// quiet (`-q`, `--quiet`), could change packages (`add`, `del`, `upgrade`,
/// `fix`) and left no package line and no error: with `--quiet` apk writes
/// only the `Running` header, so what such a run installed or removed, if
/// anything, is not in the log. Times as in [`parse_apk_log`] (0: unknown).
///
/// apk-tools 3.0.9 on Alpine 3.24 (2026-11-19) logged `apk --quiet add
/// less`, which installed three packages, as:
///
/// ```text
/// Running `apk --quiet add less` at 2026-11-19 11:22:15
/// apk-tools 3.0.9-r0, compiled for x86_64.
/// ```
pub fn apk_quiet_runs(text: &str) -> Vec<u64> {
    let mut out = Vec::new();
    // the run being read: its time, and whether it still counts
    let mut open: Option<u64> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Running `") {
            out.extend(open.take());
            let Some((command, when)) = rest.rsplit_once("` at ") else {
                continue;
            };
            let words: Vec<&str> = command.split_whitespace().collect();
            let quiet = words.iter().any(|w| {
                *w == "--quiet" || (w.starts_with('-') && !w.starts_with("--") && w.contains('q'))
            });
            let changes = words
                .iter()
                .skip(1)
                .find(|w| !w.starts_with('-'))
                .is_some_and(|verb| matches!(*verb, "add" | "del" | "upgrade" | "fix"));
            if quiet && changes {
                open = Some(
                    when.trim()
                        .split_once(' ')
                        .and_then(|(d, t)| date_time(d, t))
                        .unwrap_or(0),
                );
            }
        } else if line.starts_with('(') || line.starts_with("ERROR") {
            // a package line: the run has details; an error: it changed nothing
            open = None;
        }
    }
    out.extend(open);
    out
}

/// One row of `dnf history list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnfTransaction {
    pub id: u64,
    pub command: String,
    pub ts: u64,
    pub action: Action,
    pub altered: u64,
}

/// Parse the output of `dnf history list`. A row is read from the right:
/// the last field is the altered count, before it the action letters,
/// before those the date and the time; what is left is the command line.
pub fn parse_dnf_history(list: &str) -> Vec<DnfTransaction> {
    let mut out = Vec::new();
    for line in list.lines() {
        if line.contains('|') {
            out.extend(parse_dnf4_history_row(line));
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(id), Some(altered)) = (
            f.first().and_then(|s| s.parse::<u64>().ok()),
            f.last().and_then(|s| s.parse::<u64>().ok()),
        ) else {
            continue;
        };
        let Some((at, ts)) = (1..f.len().saturating_sub(2))
            .rev()
            .find_map(|i| date_time(f[i], f[i + 1]).map(|ts| (i, ts)))
        else {
            continue;
        };
        let letters = if at + 3 < f.len() { f[at + 2] } else { "" };
        let action = match letters.chars().next() {
            Some('I') => Action::Install,
            Some('E' | 'R') => Action::Remove,
            Some('D') => Action::Downgrade,
            _ => Action::Upgrade,
        };
        out.push(DnfTransaction {
            id,
            command: f[1..at].join(" "),
            ts,
            action,
            altered,
        });
    }
    out
}

/// A row of dnf 4's `dnf history list`: `ID | Command line | 2026-11-13
/// 00:27 | Removed | 5` (the date has no seconds; `EE` marks errors in the
/// altered column).
fn parse_dnf4_history_row(line: &str) -> Option<DnfTransaction> {
    let mut f: Vec<&str> = line.split('|').map(str::trim).collect();
    if f.len() < 5 {
        return None;
    }
    // the command line is the only free-text column and may contain '|':
    // fold the surplus middle fields back into it
    let command = f[1..f.len() - 3].join("|");
    let tail: Vec<&str> = f.split_off(f.len() - 3);
    let id = f[0].parse::<u64>().ok()?;
    let f = [f[0], command.as_str(), tail[0], tail[1], tail[2]];
    let (date, time) = f[2].split_once(' ')?;
    let ts = date_time(date, &format!("{}:00", time.trim()))?;
    let altered = f[4].split_whitespace().next()?.parse::<u64>().ok()?;
    let action = match f[3].chars().next() {
        Some('I') => Action::Install,
        Some('E' | 'R') => Action::Remove,
        Some('D') => Action::Downgrade,
        _ => Action::Upgrade,
    };
    Some(DnfTransaction {
        id,
        command,
        ts,
        action,
        altered,
    })
}

/// `Nov`, `13`, `00:25:28`, `2026` (dnf 4's `Begin time`, C locale) -> unix
/// seconds, reading the time as UTC: dnf 4 prints local time, so the caller
/// runs it with `TZ=UTC`.
fn dnf4_time(month: &str, day: &str, time: &str, year: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = MONTHS.iter().position(|n| *n == month)? + 1;
    let day: u32 = day.parse().ok()?;
    date_time(&format!("{year}-{m:02}-{day:02}"), time)
}

/// `tree-0:2.3.1-2.fc45.x86_64` -> (`tree`, `2.3.1-2.fc45`). An epoch
/// other than 0 stays in front of the version.
fn split_nevra(nevra: &str) -> Option<(&str, String)> {
    let (rest, _arch) = nevra.rsplit_once('.')?;
    let (rest, release) = rest.rsplit_once('-')?;
    let (name, version) = rest.rsplit_once('-')?;
    let version = version.strip_prefix("0:").unwrap_or(version);
    (!name.is_empty()).then(|| (name, format!("{version}-{release}")))
}

/// Parse the output of `dnf history info ID`: one entry per row of the
/// "Packages altered" table whose action is Install, Upgrade, Downgrade,
/// Reinstall or Remove. The time is the transaction's begin time.
///
/// The version an upgrade or a downgrade replaced is on a row of its own:
/// `Replaced NEVRA` (dnf 5), `Upgraded NEVRA` or `Downgraded NEVRA`
/// (dnf 4). It becomes the old version of the entry of the same name in
/// the same transaction, and is not an entry itself.
pub fn parse_dnf_history_info(info: &str) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = Vec::new();
    let mut ts = 0;
    let mut in_table = false;
    // where the entries of the transaction being read start in `out`
    let mut first = 0;
    for line in info.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if line.starts_with("Begin time") {
            // dnf 5: `Begin time : 2026-11-15 19:41:04`; dnf 4 (LC_ALL=C):
            // `Begin time : Fri Nov 13 00:25:28 2026`
            ts = match f.len() {
                5 => date_time(f[3], f[4]),
                8 => dnf4_time(f[4], f[5], f[6], f[7]),
                _ => None,
            }
            .unwrap_or(0);
            in_table = false;
            first = out.len();
        } else if line.starts_with("Packages altered") || line.starts_with("Packages Altered") {
            in_table = true;
        } else if in_table && f.len() >= 2 {
            if matches!(f[0], "Replaced" | "Upgraded" | "Downgraded") {
                if let Some((name, version)) = split_nevra(f[1])
                    && let Some(e) = out[first..].iter_mut().find(|e| {
                        e.name == name
                            && e.old_version.is_empty()
                            && matches!(e.action, Action::Upgrade | Action::Downgrade)
                    })
                {
                    e.old_version = version;
                }
                continue;
            }
            let action = match f[0] {
                "Install" => Action::Install,
                "Upgrade" => Action::Upgrade,
                "Downgrade" => Action::Downgrade,
                "Reinstall" => Action::Reinstall,
                // dnf 4 writes `Removed`, older ones `Erase`
                "Remove" | "Removed" | "Erase" => Action::Remove,
                _ => continue,
            };
            let Some((name, version)) = split_nevra(f[1]) else {
                continue;
            };
            let (old_version, new_version) = match action {
                Action::Remove => (version, String::new()),
                _ => (String::new(), version),
            };
            out.push(LogEntry {
                ts,
                source: "dnf".into(),
                action,
                name: name.into(),
                old_version,
                new_version,
            });
        }
    }
    out
}

/// One run of zypper (or of another libzypp front end) in
/// `/var/log/zypp/history`: the `command` record that opened it and the
/// package changes recorded until the next `command` record. Changes
/// recorded before any `command` record form a run with an empty `command`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZyppRun {
    /// the `DATE TIME` text of the first record of the run, as logged (local time)
    pub when: String,
    /// the command line of the `command` record, quotes removed; "" if none
    pub command: String,
    pub entries: Vec<LogEntry>,
}

/// Parse the text of `/var/log/zypp/history` into runs.
///
/// Only `install` and `remove` records are entries; `command` records are
/// run boundaries; comment lines (`#`) and every other record type (radd,
/// rremove, patch, ...) are skipped. Times are local time in the log and
/// are read as if they were UTC, like dpkg.log.
///
/// libzypp records an upgrade as an `install` of the new version. Such a
/// record becomes:
/// - `Upgrade` with the old version, when a `remove` of the same name
///   stands just before it among the records of the same second (that
///   remove is then not an entry of its own), or when an earlier `install`
///   of the same name with another version is in this text and no `remove`
///   of it follows;
/// - `Reinstall`, when that earlier record has the same version;
/// - `Install` otherwise.
///
/// The history does not say whether a version change went up or down: the
/// two versions are compared as rpm compares them, and a change to a lower
/// version is a `Downgrade`.
///
/// Limits: a package whose earlier record is in a rotated history file,
/// not in this text, is reported as `Install`.
/// Packages that keep several versions installed (kernels) are reported as
/// upgraded from the version recorded before.
pub fn parse_zypp_runs(text: &str) -> Vec<ZyppRun> {
    let mut runs: Vec<ZyppRun> = Vec::new();
    let mut installed: HashMap<String, String> = HashMap::new();
    let mut block_stamp: &str = "";
    let mut block_start = 0usize;
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 4 {
            continue;
        }
        let stamp = f[0];
        let kind = f[1].trim();
        if kind == "command" {
            runs.push(ZyppRun {
                when: stamp.to_string(),
                command: f[3].replace('\'', ""),
                entries: Vec::new(),
            });
            block_stamp = "";
            block_start = 0;
            continue;
        }
        if kind != "install" && kind != "remove" {
            continue;
        }
        let (name, version) = (f[2].trim(), f[3].trim());
        if name.is_empty() {
            continue;
        }
        let ts = stamp
            .split_once(' ')
            .and_then(|(d, t)| date_time(d, t))
            .unwrap_or(0);
        if runs.is_empty() {
            runs.push(ZyppRun {
                when: stamp.to_string(),
                command: String::new(),
                entries: Vec::new(),
            });
        }
        let Some(run) = runs.last_mut() else {
            continue;
        };
        if stamp != block_stamp {
            block_stamp = stamp;
            block_start = run.entries.len();
        }
        if kind == "remove" {
            installed.remove(name);
            run.entries.push(LogEntry {
                ts,
                source: "zypper".into(),
                action: Action::Remove,
                name: name.to_string(),
                old_version: version.to_string(),
                new_version: String::new(),
            });
            continue;
        }
        let removed_just_before = run.entries[block_start..]
            .iter()
            .rposition(|e| e.action == Action::Remove && e.name == name)
            .map(|i| i + block_start);
        let (action, old_version) = match removed_just_before {
            Some(i) => (Action::Upgrade, run.entries.remove(i).old_version),
            None => match installed.get(name) {
                Some(before) if before == version => (Action::Reinstall, String::new()),
                Some(before) => (Action::Upgrade, before.clone()),
                None => (Action::Install, String::new()),
            },
        };
        // libzypp writes a downgrade like an upgrade: the versions tell
        let action = if action == Action::Upgrade
            && crate::vercmp::rpm_evr_cmp(&old_version, version) == std::cmp::Ordering::Greater
        {
            Action::Downgrade
        } else {
            action
        };
        installed.insert(name.to_string(), version.to_string());
        run.entries.push(LogEntry {
            ts,
            source: "zypper".into(),
            action,
            name: name.to_string(),
            old_version,
            new_version: version.to_string(),
        });
    }
    runs
}

/// Parse the text of `/var/log/zypp/history`: the entries of every run, in
/// log order. See [`parse_zypp_runs`] for what is and is not an entry.
pub fn parse_zypp_history(text: &str) -> Vec<LogEntry> {
    parse_zypp_runs(text)
        .into_iter()
        .flat_map(|r| r.entries)
        .collect()
}

/// Parse the FreeBSD `pkg` lines of `/var/log/messages`:
/// `pkg[PID]: NAME-VERSION installed`, `pkg[PID]: NAME-VERSION deinstalled`
/// and `pkg[PID]: NAME upgraded: OLD -> NEW` (also `downgraded:` and
/// `reinstalled:` in the same form). `pkg-static[PID]:` is read the same way.
///
/// The syslog stamp (`Nov 15 19:30:06`) carries no year and no zone. The year
/// is taken from `now` (a stamp more than a day ahead of `now` is from the
/// year before) and the time is kept as written, so it is local time, like
/// dpkg's. A line whose stamp does not parse gets `ts` 0 ("time unknown").
pub fn parse_pkg_messages(text: &str) -> Vec<LogEntry> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    parse_pkg_messages_at(text, now)
}

/// Unix seconds of a syslog stamp such as `Nov  5 09:01:02`, the year coming
/// from `now`. See [`parse_pkg_messages`].
pub fn syslog_time(stamp: &str, now: u64) -> Option<u64> {
    let mut f = stamp.split_whitespace();
    let (mon, day, time) = (f.next()?, f.next()?, f.next()?);
    if f.next().is_some() {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == mon)? as u32
        + 1;
    let day: u32 = day.parse().ok().filter(|d| (1..=31).contains(d))?;
    let t: Vec<u32> = time
        .split(':')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    if t.len() != 3 || t[0] > 23 || t[1] > 59 || t[2] > 60 {
        return None;
    }
    let z = (now / 86_400) as i64 + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let cur_month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + z.div_euclid(146_097) * 400 + i64::from(cur_month <= 2);
    let ts = utc_to_unix(year, month, day, t[0], t[1], t[2]);
    if ts > now + 86_400 {
        Some(utc_to_unix(year - 1, month, day, t[0], t[1], t[2]))
    } else {
        // A zone east of UTC can already be in the next year (stamp
        // "Jan  1 03:30:00" while UTC still says Dec 31): the stamp then
        // belongs to the coming year, not to eleven months ago.
        let next = utc_to_unix(year + 1, month, day, t[0], t[1], t[2]);
        if next <= now + 86_400 {
            Some(next)
        } else {
            Some(ts)
        }
    }
}

/// [`parse_pkg_messages`] with the current time given.
pub fn parse_pkg_messages_at(text: &str, now: u64) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((at, tag_at)) = [" pkg[", " pkg-static["]
            .iter()
            .find_map(|tag| line.find(*tag).map(|i| (i + tag.len(), i)))
        else {
            continue;
        };
        let Some((pid, msg)) = line[at..].split_once("]: ") else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let msg = msg.trim_end();
        // "Nov 15 19:30:06 host": drop the host name, parse the stamp
        let ts = line[..tag_at]
            .rsplit_once(' ')
            .and_then(|(stamp, _host)| syslog_time(stamp, now))
            .unwrap_or(0);
        let entry = |action: Action, name: &str, old: &str, new: &str| LogEntry {
            ts,
            source: "pkg".into(),
            action,
            name: name.to_string(),
            old_version: old.to_string(),
            new_version: new.to_string(),
        };
        let changed = [
            (" upgraded: ", Action::Upgrade),
            (" downgraded: ", Action::Downgrade),
            (" reinstalled: ", Action::Reinstall),
        ]
        .iter()
        .find_map(|(verb, action)| msg.split_once(*verb).map(|(n, v)| (*action, n, v)));
        if let Some((action, name, versions)) = changed {
            let Some((old, new)) = versions.split_once(" -> ") else {
                continue;
            };
            if name.is_empty() || name.contains(' ') || old.is_empty() || new.contains(' ') {
                continue;
            }
            out.push(match action {
                Action::Reinstall => entry(action, name, "", new),
                _ => entry(action, name, old, new),
            });
            continue;
        }
        let (action, name_version) = if let Some(nv) = msg.strip_suffix(" deinstalled") {
            (Action::Remove, nv)
        } else if let Some(nv) = msg.strip_suffix(" installed") {
            (Action::Install, nv)
        } else {
            continue;
        };
        // FreeBSD versions never contain '-': the last one ends the name
        let Some((name, version)) = name_version.rsplit_once('-') else {
            continue;
        };
        if name.is_empty() || version.is_empty() || name_version.contains(' ') {
            continue;
        }
        out.push(match action {
            Action::Remove => entry(action, name, version, ""),
            _ => entry(action, name, "", version),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn dnf4_history_row_keeps_a_command_containing_pipes() {
        let t = parse_dnf4_history_row(
            "    12 | install foo | bar | 2026-11-13 00:25 | Install        |    2",
        )
        .expect("row parses");
        assert_eq!(t.id, 12);
        assert_eq!(t.command, "install foo|bar");
        assert_eq!(t.altered, 2);
    }

    use super::*;

    #[test]
    fn apk_log_entries_take_the_time_of_their_run() {
        let log = "\nRunning `apk --quiet update` at 2026-11-17 12:19:53\napk-tools 3.0.6-r0, compiled for x86_64.\n\
Running `apk add htop` at 2026-11-17 12:37:33\napk-tools 3.0.6-r0, compiled for x86_64.\n\
(1/2) Installing lsof (4.99.6-r0)\n(2/2) Installing htop (3.5.3-r0)\nExecuting busybox-1.37.0-r31.trigger\nOK: 133.3 MiB in 203 packages\n\
Running `apk upgrade musl` at 2026-11-17 12:37:36\n(1/1) Upgrading musl (1.2.6-r2 -> 1.2.6-r3)\n\
Running `apk del htop` at 2026-11-17 12:37:37\n(1/2) Purging htop (3.5.3-r0)\n( 2/12) Purging lsof (4.99.6-r0)\n\
  htop-3.5.3-r0:\n    breaks: world[htop=999]\n(1/1) Frobnicating x (1)\n(1/1) Upgrading broken (1)\n(x/y) Installing z (1)\n";
        let e = parse_apk_log(log);
        let t1 = utc_to_unix(2026, 11, 17, 12, 37, 33);
        let t2 = utc_to_unix(2026, 11, 17, 12, 37, 36);
        let t3 = utc_to_unix(2026, 11, 17, 12, 37, 37);
        let mk = |ts, action, name: &str, old: &str, new: &str| LogEntry {
            ts,
            source: "apk".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        };
        assert_eq!(
            e,
            vec![
                mk(t1, Action::Install, "lsof", "", "4.99.6-r0"),
                mk(t1, Action::Install, "htop", "", "3.5.3-r0"),
                mk(t2, Action::Upgrade, "musl", "1.2.6-r2", "1.2.6-r3"),
                mk(t3, Action::Remove, "htop", "3.5.3-r0", ""),
                mk(t3, Action::Remove, "lsof", "4.99.6-r0", ""),
            ]
        );
        assert!(parse_apk_log("").is_empty());
    }

    /// E3: apk-tools 3.0.9 on Alpine 3.24, 2026-11-19: the log of `apk
    /// update`, an install, a quiet install (three packages were installed),
    /// an install with nothing to do, a quiet removal, a quiet upgrade and
    /// a quiet run that failed.
    #[test]
    fn apk_runs_logged_without_package_details_are_counted() {
        let log = "\nRunning `apk update` at 2026-11-19 11:22:14\napk-tools 3.0.9-r0, compiled for x86_64.\nv3.24.4-127-gced0c71f993 [https://dl-cdn.alpinelinux.org/alpine/v3.24/main]\nOK: 28673 distinct packages available\n\n\
Running `apk add tree` at 2026-11-19 11:22:14\napk-tools 3.0.9-r0, compiled for x86_64.\n(1/1) Installing tree (2.3.2-r0)\nExecuting busybox-1.37.0-r31.trigger\nOK: 8283 KiB in 17 packages\n\n\
Running `apk --quiet add less` at 2026-11-19 11:22:15\napk-tools 3.0.9-r0, compiled for x86_64.\n\n\
Running `apk add tree` at 2026-11-19 11:22:15\napk-tools 3.0.9-r0, compiled for x86_64.\nOK: 8953 KiB in 20 packages\n\n\
Running `apk -q del less` at 2026-11-19 11:22:16\napk-tools 3.0.9-r0, compiled for x86_64.\n\n\
Running `apk -q upgrade` at 2026-11-19 11:22:17\napk-tools 3.0.9-r0, compiled for x86_64.\n\n\
Running `apk -q add nosuchpkg-xyz` at 2026-11-19 11:22:17\napk-tools 3.0.9-r0, compiled for x86_64.\nERROR: unable to select packages:\n  nosuchpkg-xyz (no such package):\n    required by: world[nosuchpkg-xyz]\n";
        let at = |s| utc_to_unix(2026, 11, 19, 11, 22, s);
        // the three quiet runs that could have changed packages; not the
        // update, not the run that says OK, not the one that failed
        assert_eq!(apk_quiet_runs(log), vec![at(15), at(16), at(17)]);
        // and the entries of the log are the one install that was not quiet
        let entries = parse_apk_log(log);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "tree");
        // a quiet update, a quiet query, a long option that only holds a q
        let other = "Running `apk --quiet update` at 2026-11-17 12:19:53\nRunning `apk -q info` at 2026-11-17 12:19:54\nRunning `apk --query add x` at 2026-11-17 12:19:55\nOK: 1 MiB in 2 packages\n";
        assert!(apk_quiet_runs(other).is_empty());
        // a quiet run that still printed its packages has details
        let detailed = "Running `apk -q add x` at 2026-11-17 12:19:55\n(1/1) Installing x (1-r0)\n";
        assert!(apk_quiet_runs(detailed).is_empty());
        assert!(apk_quiet_runs("").is_empty());
    }

    fn zypp_entry(action: Action, name: &str, old: &str, new: &str, ts: u64) -> LogEntry {
        LogEntry {
            ts,
            source: "zypper".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    // Written from the documented format of /var/log/zypp/history; there is
    // no openSUSE fixture in fixtures/native yet.
    const ZYPP: &str = "\
# 2026-11-14 08:00:00 history created
2026-11-14 10:00:00|command|root@host|'zypper' 'install' 'tree' 'curl'|
2026-11-14 10:00:01|radd   |repo-oss|http://download.opensuse.org/oss|
2026-11-14 10:00:02|install|tree|2.1.1-1.2|x86_64|root@host|repo-oss|0123abcd|
2026-11-14 10:00:03|install|curl|8.9.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:00|command|root@host|'zypper' 'dup'|
2026-11-15 19:30:06|install|curl|8.10.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:06|remove |libold|1.0-1.1|x86_64|root@host|
2026-11-15 19:30:06|install|libold|2.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:07|remove |tree|2.1.1-1.2|x86_64|root@host|
2026-11-15 19:30:08|install|tree|2.1.1-1.2|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:09|install|curl|8.10.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:09|patch  |openSUSE-2026-1|1|noarch|repo-update|moderate|security|needed|applied|
not a record
";

    #[test]
    fn zypp_history_records() {
        let t1 = utc_to_unix(2026, 11, 14, 10, 0, 0);
        let t2 = utc_to_unix(2026, 11, 15, 19, 30, 0);
        assert_eq!(t2, 1_794_771_000);
        let want = vec![
            zypp_entry(Action::Install, "tree", "", "2.1.1-1.2", t1 + 2),
            zypp_entry(Action::Install, "curl", "", "8.9.0-1.1", t1 + 3),
            // an install over an earlier install of another version
            zypp_entry(Action::Upgrade, "curl", "8.9.0-1.1", "8.10.0-1.1", t2 + 6),
            // a remove just before, in the same second: one upgrade entry
            zypp_entry(Action::Upgrade, "libold", "1.0-1.1", "2.0-1.1", t2 + 6),
            // a remove in another second stays a remove, and the install an install
            zypp_entry(Action::Remove, "tree", "2.1.1-1.2", "", t2 + 7),
            zypp_entry(Action::Install, "tree", "", "2.1.1-1.2", t2 + 8),
            zypp_entry(Action::Reinstall, "curl", "", "8.10.0-1.1", t2 + 9),
        ];
        assert_eq!(parse_zypp_history(ZYPP), want);
    }

    /// Z6: `timeline` labelled a downgrade `upgrade 1.0-69.1 -> 1.0-68.1`.
    #[test]
    fn a_zypp_version_change_downwards_is_a_downgrade() {
        let text = "\
2026-11-17 09:00:00|command|root@host|'zypper' 'install' 'hello'|
2026-11-17 09:00:01|install|hello|1.0-69.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-18 09:00:00|command|root@host|'zypper' 'install' '--oldpackage' 'hello=1.0-68.1'|
2026-11-18 09:00:01|install|hello|1.0-68.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-18 10:00:00|command|root@host|'zypper' 'install' '--oldpackage' 'libold=1.9-1.1'|
2026-11-18 10:00:01|remove |libold|2.0-1.1|x86_64|root@host|
2026-11-18 10:00:01|install|libold|1.9-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-18 11:00:00|command|root@host|'zypper' 'update'|
2026-11-18 11:00:01|install|hello|1.0-70.1|x86_64|root@host|repo-oss|0123abcd|
";
        let got: Vec<(Action, String, String, String)> = parse_zypp_history(text)
            .into_iter()
            .map(|e| (e.action, e.name, e.old_version, e.new_version))
            .collect();
        let row = |a, n: &str, o: &str, v: &str| (a, n.to_string(), o.to_string(), v.to_string());
        assert_eq!(
            got,
            vec![
                row(Action::Install, "hello", "", "1.0-69.1"),
                row(Action::Downgrade, "hello", "1.0-69.1", "1.0-68.1"),
                // the remove-then-install form of the same thing
                row(Action::Downgrade, "libold", "2.0-1.1", "1.9-1.1"),
                row(Action::Upgrade, "hello", "1.0-68.1", "1.0-70.1"),
            ]
        );
    }

    #[test]
    fn zypp_runs_are_cut_at_command_records() {
        let runs = parse_zypp_runs(ZYPP);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].when, "2026-11-14 10:00:00");
        assert_eq!(runs[0].command, "zypper install tree curl");
        assert_eq!(runs[0].entries.len(), 2);
        assert_eq!(runs[1].command, "zypper dup");
        assert_eq!(runs[1].entries.len(), 5);
        let bare =
            parse_zypp_runs("2026-11-14 10:00:02|install|tree|2.1.1-1.2|x86_64||repo|sum|\n");
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].command, "");
        assert_eq!(bare[0].when, "2026-11-14 10:00:02");
        assert!(parse_zypp_runs("# only a comment\n\n").is_empty());
    }

    fn pkg_entry(action: Action, name: &str, old: &str, new: &str) -> LogEntry {
        LogEntry {
            ts: 0,
            source: "pkg".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    #[test]
    fn freebsd_pkg_messages() {
        // Same form as real lines seen in /var/log/messages on FreeBSD 14.5 (pkg 2.9.2).
        let text = "\
Nov 15 19:30:06 beastie pkg[1234]: tree-2.1.1 installed
Nov 15 19:30:07 beastie pkg[1235]: py311-setuptools-63.1.0_1,1 installed
Nov 15 19:31:00 beastie pkg[1300]: curl upgraded: 8.9.0 -> 8.10.0
Nov 15 19:31:01 beastie pkg-static[1301]: pkg downgraded: 2.0.0 -> 1.21.3
Nov 15 19:31:02 beastie pkg[1302]: bash reinstalled: 5.2.37 -> 5.2.37
Nov 15 19:32:00 beastie pkg[1400]: tree-2.1.1 deinstalled
Nov 15 19:32:01 beastie sshd[1500]: something-1.0 installed
Nov 15 19:32:02 beastie mypkg[1501]: other-1.0 installed
Nov 15 19:32:03 beastie pkg[1502]: noversion installed
Nov 15 19:32:04 beastie pkg[1503]: curl upgraded: 8.9.0
Nov 15 19:32:05 beastie pkg[abc]: tree-2.1.1 installed
Nov 15 19:32:06 beastie pkg[1504]: Newer FreeBSD version for package tree
";
        let want = vec![
            pkg_entry(Action::Install, "tree", "", "2.1.1"),
            pkg_entry(Action::Install, "py311-setuptools", "", "63.1.0_1,1"),
            pkg_entry(Action::Upgrade, "curl", "8.9.0", "8.10.0"),
            pkg_entry(Action::Downgrade, "pkg", "2.0.0", "1.21.3"),
            pkg_entry(Action::Reinstall, "bash", "", "5.2.37"),
            pkg_entry(Action::Remove, "tree", "2.1.1", ""),
        ];
        let now = utc_to_unix(2026, 11, 17, 12, 0, 0);
        let got = parse_pkg_messages_at(text, now);
        // same entries, now carrying the time of their syslog stamp
        let stamps = [(30, 6), (30, 7), (31, 0), (31, 1), (31, 2), (32, 0)];
        let want: Vec<LogEntry> = want
            .into_iter()
            .zip(stamps)
            .map(|(mut e, (m, s))| {
                e.ts = utc_to_unix(2026, 11, 15, 19, m, s);
                e
            })
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn syslog_stamps_get_their_year_from_now() {
        let now = utc_to_unix(2026, 1, 3, 10, 0, 0);
        // a December stamp read on 3 January is last year's
        assert_eq!(
            syslog_time("Dec 31 23:59:58", now),
            Some(utc_to_unix(2025, 12, 31, 23, 59, 58))
        );
        assert_eq!(
            syslog_time("Jan  2 04:05:06", now),
            Some(utc_to_unix(2026, 1, 2, 4, 5, 6))
        );
        // a stamp only hours ahead (clock skew, zone) stays in this year
        assert_eq!(
            syslog_time("Jan  3 15:00:00", now),
            Some(utc_to_unix(2026, 1, 3, 15, 0, 0))
        );
        // east of UTC the new year starts first: Dec 31 22:00 UTC is Jan 1
        // 03:30 in the log
        let ny = utc_to_unix(2026, 12, 31, 22, 0, 0);
        assert_eq!(
            syslog_time("Jan  1 03:30:00", ny),
            Some(utc_to_unix(2027, 1, 1, 3, 30, 0))
        );
        assert_eq!(
            syslog_time("Dec 31 21:00:00", ny),
            Some(utc_to_unix(2026, 12, 31, 21, 0, 0))
        );
        for bad in [
            "",
            "Nov 15",
            "Foo 15 10:00:00",
            "Nov 32 10:00:00",
            "Nov 15 25:00:00",
            "Nov 15 10:00",
            "Nov 15 10:00:00 extra",
            "Nov x 10:00:00",
        ] {
            assert_eq!(syslog_time(bad, now), None, "{bad}");
        }
    }

    #[test]
    fn a_pkg_line_without_a_stamp_has_no_time() {
        let got = parse_pkg_messages_at("host pkg[12]: tree-2.1.1 installed\n", 1_700_000_000);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ts, 0);
    }

    #[test]
    fn utc_to_unix_matches_known_dates() {
        assert_eq!(utc_to_unix(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(utc_to_unix(2000, 2, 29, 0, 0, 0), 951_782_400);
        assert_eq!(utc_to_unix(2024, 4, 24, 23, 6, 40), 1_714_000_000);
    }

    fn pacman_entry(action: Action, name: &str, old: &str, new: &str, ts: u64) -> LogEntry {
        LogEntry {
            ts,
            source: "pacman".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    #[test]
    fn pacman_fixture_gives_the_installed_packages() {
        let raw = include_str!("../../../fixtures/native/arch/log-pacman.txt");
        let text: String = raw
            .lines()
            .filter(|l| !l.starts_with("# cmd:") && !l.starts_with("# exit:"))
            .map(|l| format!("{l}\n"))
            .collect();
        let got = parse_pacman_log(&text);
        assert_eq!(got.len(), 33);
        let first = pacman_entry(Action::Install, "gpgme", "", "2.2.0-1", 1_794_096_229);
        assert_eq!(got[0], first);
        let last = pacman_entry(Action::Install, "tree", "", "2.3.2-1", 1_794_771_081);
        assert_eq!(got[32], last);
    }

    #[test]
    fn pacman_verbs_and_time_zones() {
        let text = "[2026-11-15T19:31:21+0000] [ALPM] upgraded linux (6.17.7-1 -> 6.17.8-1)\n\
                    [2026-11-15T19:31:21+0000] [ALPM] downgraded mesa (1:25.3-1 -> 1:25.2-2)\n\
                    [2026-11-15T19:31:21+0000] [ALPM] reinstalled tree (2.3.2-1)\n\
                    [2026-11-15T19:31:21+0000] [ALPM] removed tree (2.3.2-1)\n\
                    [2026-11-15T14:31:21-0500] [ALPM] installed zlib (1.3.1-2)\n\
                    [2026-11-15T21:01:21+0130] [ALPM] installed xz (5.8.1-1)\n";
        let ts = 1_794_771_081;
        let want = vec![
            pacman_entry(Action::Upgrade, "linux", "6.17.7-1", "6.17.8-1", ts),
            pacman_entry(Action::Downgrade, "mesa", "1:25.3-1", "1:25.2-2", ts),
            pacman_entry(Action::Reinstall, "tree", "", "2.3.2-1", ts),
            pacman_entry(Action::Remove, "tree", "2.3.2-1", "", ts),
            pacman_entry(Action::Install, "zlib", "", "1.3.1-2", ts),
            pacman_entry(Action::Install, "xz", "", "5.8.1-1", ts),
        ];
        assert_eq!(parse_pacman_log(text), want);
    }

    #[test]
    fn pacman_other_lines_are_not_entries() {
        let text = "[2026-11-15T19:31:21+0000] [ALPM] transaction started\n\
                    [2026-11-15T19:31:20+0000] [PACMAN] Running 'pacman -S tree'\n\
                    [2026-11-15T19:31:21+0000] [ALPM-SCRIPTLET] installed x (1)\n\
                    [2026-11-15T19:31:21+0000] [ALPM] upgraded broken (1.0-1)\n\
                    [2026-11-15T19:31:21+0000] [ALPM] installed noversion\n\
                    not a log line\n";
        assert!(parse_pacman_log(text).is_empty());
        let old_style = parse_pacman_log("[2019-01-01 12:00] [ALPM] installed zlib (1.2.11-3)\n");
        assert_eq!(old_style.len(), 1);
        assert_eq!(old_style[0].ts, 0);
    }

    fn dpkg_entry(action: Action, name: &str, old: &str, new: &str, ts: u64) -> LogEntry {
        LogEntry {
            ts,
            source: "apt".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    fn dpkg_fixture(raw: &str) -> String {
        raw.lines()
            .filter(|l| !l.starts_with("# cmd:") && !l.starts_with("# exit:"))
            .map(|l| format!("{l}\n"))
            .collect()
    }

    #[test]
    fn dpkg_debian_fixture_gives_one_install() {
        let text = dpkg_fixture(include_str!("../../../fixtures/native/debian/log-dpkg.txt"));
        let want = vec![dpkg_entry(
            Action::Install,
            "tree",
            "",
            "2.2.1-1",
            1_794_771_006,
        )];
        assert_eq!(parse_dpkg_log(&text), want);
    }

    #[test]
    fn dpkg_ubuntu_fixture_gives_upgrades_and_an_install() {
        let text = dpkg_fixture(include_str!("../../../fixtures/native/ubuntu/log-dpkg.txt"));
        let got = parse_dpkg_log(&text);
        assert_eq!(got.len(), 3);
        let first = dpkg_entry(
            Action::Upgrade,
            "apt",
            "3.3.3",
            "3.3.3ubuntu0.1",
            1_793_892_326,
        );
        assert_eq!(got[0], first);
        let last = dpkg_entry(Action::Install, "tree", "", "2.3.2-1", 1_794_771_032);
        assert_eq!(got[2], last);
    }

    #[test]
    fn dpkg_verbs_and_other_lines() {
        let text = "2026-11-15 19:30:06 upgrade libc6:amd64 2.41-12 2.41-13\n\
                    2026-11-15 19:30:06 remove tree:amd64 2.2.1-1 <none>\n\
                    2026-11-15 19:30:06 install hello:all <none> 2.10-5\n\
                    2026-11-15 19:30:06 status installed tree:amd64 2.2.1-1\n\
                    2026-11-15 19:30:06 configure tree:amd64 2.2.1-1 <none>\n\
                    2026-11-15 19:30:06 startup archives unpack\n\
                    yesterday noon install tree:amd64 <none> 2.2.1-1\n";
        let ts = 1_794_771_006;
        let want = vec![
            dpkg_entry(Action::Upgrade, "libc6", "2.41-12", "2.41-13", ts),
            dpkg_entry(Action::Remove, "tree", "2.2.1-1", "", ts),
            dpkg_entry(Action::Install, "hello", "", "2.10-5", ts),
            dpkg_entry(Action::Install, "tree", "", "2.2.1-1", 0),
        ];
        assert_eq!(parse_dpkg_log(text), want);
    }

    /// dpkg.log lines of debian:stable, 2026-11-19: an upgrade, the
    /// downgrade that took it back (`apt-get install --allow-downgrades
    /// zlib1g=...`), and `apt-get install --reinstall tree`. `timeline`
    /// showed all three as `upgrade`, the last one as `2.2.1-1 -> 2.2.1-1`.
    #[test]
    fn a_dpkg_version_change_downwards_is_a_downgrade_and_the_same_version_a_reinstall() {
        let text = "2026-11-19 12:31:47 upgrade zlib1g:amd64 1:1.3.dfsg+really1.3.1-1+b1 1:1.3.dfsg+really1.3.1-1+deb13u1\n\
                    2026-11-19 12:31:49 upgrade zlib1g:amd64 1:1.3.dfsg+really1.3.1-1+deb13u1 1:1.3.dfsg+really1.3.1-1+b1\n\
                    2026-11-19 12:29:43 upgrade tree:amd64 2.2.1-1 2.2.1-1\n";
        let got: Vec<(Action, String, String, String)> = parse_dpkg_log(text)
            .into_iter()
            .map(|e| (e.action, e.name, e.old_version, e.new_version))
            .collect();
        let row = |a, n: &str, o: &str, v: &str| (a, n.to_string(), o.to_string(), v.to_string());
        let (low, high) = (
            "1:1.3.dfsg+really1.3.1-1+b1",
            "1:1.3.dfsg+really1.3.1-1+deb13u1",
        );
        assert_eq!(
            got,
            vec![
                row(Action::Upgrade, "zlib1g", low, high),
                row(Action::Downgrade, "zlib1g", high, low),
                row(Action::Reinstall, "tree", "", "2.2.1-1"),
            ]
        );
    }

    #[test]
    fn dnf_history_list_fixture_gives_four_transactions() {
        let text = dpkg_fixture(include_str!(
            "../../../fixtures/native/fedora/log-history-list.txt"
        ));
        let got = parse_dnf_history(&text);
        let row = |i: usize| {
            let t: &DnfTransaction = &got[i];
            (t.id, t.command.as_str(), t.ts, t.action, t.altered)
        };
        assert_eq!(got.len(), 4);
        let first = "dnf -q -y mark dependency tree";
        assert_eq!(row(0), (4, first, 1_794_771_132, Action::Upgrade, 1));
        assert_eq!(row(1).1, "dnf -q -y install tree");
        assert_eq!(row(1).3, Action::Install);
        assert_eq!(row(2).3, Action::Remove);
        let last = "dnf5 --config /builddir/result/image/bu";
        assert_eq!(row(3), (1, last, 1_794_725_264, Action::Install, 149));
    }

    #[test]
    fn dnf_history_list_other_rows() {
        let text = " 7 dnf downgrade tree   2026-11-15 19:32:12 D   1\n\
                     6 dnf upgrade          2026-11-15 19:32:12 U  12\n\
                     no id here             2026-11-15 19:32:12 I   1\n\
                     3 no date in this row                      I   1\n";
        let got = parse_dnf_history(text);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].action, got[0].altered), (Action::Downgrade, 1));
        assert_eq!((got[1].action, got[1].altered), (Action::Upgrade, 12));
    }

    fn dnf_entry(action: Action, name: &str, old: &str, new: &str, ts: u64) -> LogEntry {
        LogEntry {
            ts,
            source: "dnf".into(),
            action,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    #[test]
    fn dnf_info_fixtures_give_one_install_and_one_remove() {
        let install = dpkg_fixture(include_str!(
            "../../../fixtures/native/fedora/log-history-info-install.txt"
        ));
        let want = vec![dnf_entry(
            Action::Install,
            "tree",
            "",
            "2.3.1-2.fc45",
            1_794_771_664,
        )];
        assert_eq!(parse_dnf_history_info(&install), want);
        let remove = dpkg_fixture(include_str!(
            "../../../fixtures/native/fedora/log-history-info-remove.txt"
        ));
        let want = vec![dnf_entry(
            Action::Remove,
            "tree",
            "2.3.1-2.fc45",
            "",
            1_794_771_664,
        )];
        assert_eq!(parse_dnf_history_info(&remove), want);
    }

    #[test]
    fn dnf4_history_list_real_wording() {
        // from `LC_ALL=C dnf history list` on Rocky Linux 9.9 (dnf 4.14.0)
        let text = "ID     | Command line             | Date and time    | Action(s)      | Altered\n\
                    -------------------------------------------------------------------------------\n\
                    \x20   13 | history undo last -y     | 2026-11-17 12:21 | Removed        |    1   \n\
                    \x20    4 | -C -y remove langpacks-* | 2026-11-13 00:27 | Removed        |    5   \n\
                    \x20    3 | -C -y remove firewalld - | 2026-11-13 00:27 | Removed        |   10 EE\n\
                    \x20    2 | --config /kiwi_dnf4.conf | 2026-11-13 00:25 | Install        |  259 EE\n\
                    \x20    1 | -y upgrade nano          | 2026-11-13 00:25 | Upgrade        |    2   \n";
        let got = parse_dnf_history(text);
        assert_eq!(got.len(), 5);
        assert_eq!(
            (
                got[0].id,
                got[0].command.as_str(),
                got[0].ts,
                got[0].action,
                got[0].altered
            ),
            (13, "history undo last -y", 1_794_918_060, Action::Remove, 1)
        );
        assert_eq!((got[2].action, got[2].altered), (Action::Remove, 10));
        assert_eq!((got[3].action, got[3].altered), (Action::Install, 259));
        assert_eq!(got[4].action, Action::Upgrade);
    }

    #[test]
    fn dnf4_info_real_wording() {
        // from `LC_ALL=C dnf history info` on Rocky Linux 9.9 (dnf 4.14.0)
        let text = "Transaction ID : 9\n\
                    Begin time     : Tue Nov 17 12:16:20 2026\n\
                    Packages Altered:\n\
                    \x20   Upgrade  nano-5.6.1-7.el9.x86_64  @baseos\n\
                    \x20   Upgraded nano-5.6.1-5.el9.x86_64  @@System\n\
                    \x20   Removed  strace-6.12-1.el9.x86_64 @@System\n\
                    \x20   Downgrade  vim-minimal-2:8.2.2637-20.el9_1.x86_64 @@commandline\n\
                    \x20   Reinstalled nano-5.6.1-7.el9.x86_64 @@System\n";
        let ts = 1_794_917_780;
        let want = vec![
            // D5: the old version, from the `Upgraded` row
            dnf_entry(Action::Upgrade, "nano", "5.6.1-5.el9", "5.6.1-7.el9", ts),
            dnf_entry(Action::Remove, "strace", "6.12-1.el9", "", ts),
            dnf_entry(
                Action::Downgrade,
                "vim-minimal",
                "",
                "2:8.2.2637-20.el9_1",
                ts,
            ),
        ];
        assert_eq!(parse_dnf_history_info(text), want);
    }

    #[test]
    fn dnf_info_names_epochs_and_other_rows() {
        let text = "Begin time     : 2026-11-15 19:41:04\n\
                    Description    : Install not-in-the-table-0:1-1.fc45.noarch\n\
                    Packages altered:\n\
                    \x20 Action        Package                             Reason Repository\n\
                    \x20 Upgrade       python3-dnf-0:5.4.0-1.fc45.noarch   User   updates\n\
                    \x20 Downgrade     bind-utils-32:9.18.1-1.fc45.x86_64  User   fedora\n\
                    \x20 Reinstall     xz-1:5.8.1-4.fc45.x86_64            User   fedora\n\
                    \x20 Replaced      python3-dnf-0:5.3.0-1.fc45.noarch   User   @System\n\
                    \x20 Reason Change tree-0:2.3.1-2.fc45.x86_64          Dep    @System\n\
                    \x20 Install       broken                              User   fedora\n";
        let ts = 1_794_771_664;
        let want = vec![
            // D5: the old version, from the `Replaced` row
            dnf_entry(
                Action::Upgrade,
                "python3-dnf",
                "5.3.0-1.fc45",
                "5.4.0-1.fc45",
                ts,
            ),
            dnf_entry(Action::Downgrade, "bind-utils", "", "32:9.18.1-1.fc45", ts),
            dnf_entry(Action::Reinstall, "xz", "", "1:5.8.1-4.fc45", ts),
        ];
        assert_eq!(parse_dnf_history_info(text), want);
    }
}
