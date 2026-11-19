// yapacc — filesystem snapshots through the tool the machine already has
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! A thin provider layer over snapper, timeshift and zfs. yapacc does not
//! implement snapshots: it finds the snapshot manager that is configured
//! on this machine, lists what it holds, and can ask it for one more —
//! showing the exact command and what that snapshot does *not* cover
//! before anything runs.
//!
//! Detection is by evidence, per provider (STRATEGY C9): the tool on
//! `PATH`, plus a root config for snapper, plus `/` actually being on ZFS
//! for zfs. Plain btrfs without a snapshot manager is detection-only:
//! existing snapshot subvolumes are listed, but none is created, because
//! where a snapshot subvolume should live is a layout decision yapacc
//! would have to guess.
//!
//! `list` is read-only. `create` is gated like every other change.

use crate::error::{Error, Result};
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};

pub const DEFAULT_DESCRIPTION: &str = "yapacc: manual snapshot";

const SNAPPER_ROOT_CONFIG: &str = "/etc/snapper/configs/root";

const UNDO_WITHOUT_SNAPSHOTS: &str = "`yapacc undo` still works without one, within its limits: it restores package versions only, not configuration files or data.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub id: String,
    /// as the tool reports it; empty when the tool gives no date
    pub when: String,
    pub description: String,
    pub provider: String,
}

pub trait SnapshotProvider {
    fn name(&self) -> &'static str;
    /// Existing snapshots, newest first.
    fn list(&self) -> Result<Vec<Snapshot>>;
    /// The command that takes a snapshot, without `sudo`/`doas`. Empty
    /// when the provider cannot create one (plain btrfs).
    fn create_command(&self, description: &str) -> Vec<String>;
    /// What a snapshot from this provider covers and does not cover.
    fn guarantees(&self) -> &'static str;
}

// ---------------------------------------------------------------- running

fn strs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Run a read-only listing command and return its standard output.
fn capture(argv: &[&str]) -> Result<String> {
    let Some((prog, rest)) = argv.split_first() else {
        return Err(Error::from("empty command".to_string()));
    };
    let out = Command::new(prog)
        .args(rest)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::unavailable(format!("cannot run {prog}: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let said = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let is_root = crate::util::is_root();
    Err(Error::new(
        failure_kind(said, is_root),
        failure_message(&argv.join(" "), &out.status.to_string(), said, is_root),
    ))
}

/// The kind of a failed listing: a refusal for lack of privileges, as an
/// ordinary user, is "unavailable" (exit 6: something the command needs is
/// not available to this user, and the message says to run as root); any
/// other failure is a failure (exit 1). `snapshot list` is read-only and
/// never calls sudo or doas itself. Pure.
pub fn failure_kind(said: &str, is_root: bool) -> crate::error::Kind {
    if !is_root && looks_like_permission_error(said) {
        crate::error::Kind::Unavailable
    } else {
        crate::error::Kind::Failed
    }
}

fn looks_like_permission_error(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "permission",
        "not permitted",
        "access denied",
        "must be root",
        "as root",
        "admin access",
        "superuser",
        "privilege",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// What to say when a listing command fails. Pure.
pub fn failure_message(command: &str, status: &str, said: &str, is_root: bool) -> String {
    let said = if said.is_empty() {
        "(no message)"
    } else {
        said
    };
    if is_root {
        format!("`{command}` failed ({status}): {said}")
    } else if looks_like_permission_error(said) {
        format!(
            "`{command}` was refused ({status}): {said}\nListing these snapshots needs root: run as root."
        )
    } else {
        format!(
            "`{command}` failed ({status}): {said}\nIf this is a permission problem, run as root: listing snapshots commonly needs it."
        )
    }
}

// ---------------------------------------------------------------- snapper

/// Split one CSV record (RFC 4180 quoting: `"a, b"`, `""` for a quote).
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut cur)),
            other => cur.push(other),
        }
    }
    fields.push(cur);
    fields
}

/// snapper marks the active/default snapshot after its number (`3*`, `4-`, `5+`).
fn snapper_number(cell: &str) -> Option<String> {
    let n = cell.trim().trim_end_matches(['*', '-', '+']);
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
}

fn snapper_row(number: &str, date: &str, description: &str) -> Option<Snapshot> {
    let id = snapper_number(number)?;
    // number 0 is snapper's name for the live system, not a snapshot
    if id == "0" {
        return None;
    }
    Some(Snapshot {
        id,
        when: date.trim().to_string(),
        description: description.trim().to_string(),
        provider: "snapper".to_string(),
    })
}

/// Parse `snapper --csvout list --columns number,date,description`. The
/// header line names the columns; they are found by name. Order as given
/// (oldest first). Pure.
pub fn parse_snapper_csv(text: &str) -> Vec<Snapshot> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return vec![];
    };
    let header = csv_fields(header);
    let col = |name: &str| header.iter().position(|h| h.trim() == name);
    let (Some(n), Some(d), Some(desc)) = (col("number"), col("date"), col("description")) else {
        return vec![];
    };
    lines
        .filter_map(|l| {
            let f = csv_fields(l);
            snapper_row(f.get(n)?, f.get(d)?, f.get(desc)?)
        })
        .collect()
}

/// Parse the human table of `snapper list` (older snapper without
/// `--csvout`). Columns are separated by `|` or, on newer versions, `│`;
/// they are found by header name (`#`, `Date`, `Description`). Pure.
pub fn parse_snapper_table(text: &str) -> Vec<Snapshot> {
    let split =
        |l: &str| -> Vec<String> { l.split(['|', '│']).map(|c| c.trim().to_string()).collect() };
    let mut cols: Option<(usize, usize, usize)> = None;
    let mut out = Vec::new();
    for line in text.lines() {
        let cells = split(line);
        match cols {
            None => {
                let pos = |name: &str| cells.iter().position(|c| c == name);
                if let (Some(n), Some(d), Some(desc)) = (pos("#"), pos("Date"), pos("Description"))
                {
                    cols = Some((n, d, desc));
                }
            }
            Some((n, d, desc)) => {
                let (Some(num), Some(date), Some(description)) =
                    (cells.get(n), cells.get(d), cells.get(desc))
                else {
                    continue; // the rule under the header, or a short line
                };
                if let Some(s) = snapper_row(num, date, description) {
                    out.push(s);
                }
            }
        }
    }
    out
}

pub struct Snapper;

impl SnapshotProvider for Snapper {
    fn name(&self) -> &'static str {
        "snapper"
    }

    fn list(&self) -> Result<Vec<Snapshot>> {
        let mut snaps = match capture(&[
            "snapper",
            "--csvout",
            "list",
            "--columns",
            "number,date,description",
        ]) {
            Ok(text) => parse_snapper_csv(&text),
            // an older snapper has no --csvout; a permission problem will
            // fail the same way again and is reported from this second run
            Err(_) => parse_snapper_table(&capture(&["snapper", "list"])?),
        };
        snaps.reverse();
        Ok(snaps)
    }

    fn create_command(&self, description: &str) -> Vec<String> {
        strs(&[
            "snapper",
            "create",
            "--type",
            "single",
            "--cleanup-algorithm",
            "number",
            "--description",
            description,
        ])
    }

    fn guarantees(&self) -> &'static str {
        "Covers: the subvolume of snapper's `root` config (normally `/`), as it is at this instant.\n\
         Does NOT cover: /home and any other subvolume or filesystem mounted below it (nested subvolumes are not part of a btrfs snapshot); a separate /boot or EFI partition, so a kernel upgrade is only partly taken back.\n\
         Not a backup: the snapshot lives on the same disk as the data. With `--cleanup-algorithm number`, snapper's own cleanup may delete it later according to the limits in /etc/snapper/configs/root."
    }
}

// -------------------------------------------------------------- timeshift

fn is_timeshift_name(s: &str) -> bool {
    // 2026-11-01_10-00-01
    let b = s.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b'_',
            _ => c.is_ascii_digit(),
        })
}

/// Parse `timeshift --list`: lines of the form
/// `0    >  2026-11-01_10-00-01  O D   before upgrade`. Tags are the
/// single letters O B H D W M; the rest is the comment. Order as given
/// (oldest first). Pure.
pub fn parse_timeshift_list(text: &str) -> Vec<Snapshot> {
    let mut out = Vec::new();
    // The comment starts at the "Description" column of the header: a comment
    // that begins with a tag letter ("M to the max") is not part of the tags.
    let desc_col = text
        .lines()
        .find(|l| l.trim_start().starts_with("Num") && l.contains("Tags"))
        .and_then(|l| l.find("Description").map(|b| l[..b].chars().count()));
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(num), Some(arrow), Some(name)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        if !num.bytes().all(|b| b.is_ascii_digit()) || arrow != ">" || !is_timeshift_name(name) {
            continue;
        }
        let rest: Vec<&str> = words.collect();
        let tags = rest
            .iter()
            .take_while(|w| w.len() == 1 && "OBHDWM".contains(**w))
            .count();
        let (date, time) = name.split_once('_').unwrap_or((name, ""));
        let description = match desc_col {
            Some(c) if line.chars().count() >= c => line
                .chars()
                .skip(c)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            _ => rest[tags..].join(" "),
        };
        out.push(Snapshot {
            id: name.to_string(),
            when: format!("{date} {}", time.replace('-', ":"))
                .trim()
                .to_string(),
            description,
            provider: "timeshift".to_string(),
        });
    }
    out
}

pub struct Timeshift;

impl SnapshotProvider for Timeshift {
    fn name(&self) -> &'static str {
        "timeshift"
    }

    fn list(&self) -> Result<Vec<Snapshot>> {
        let mut snaps = parse_timeshift_list(&capture(&["timeshift", "--list"])?);
        snaps.reverse();
        Ok(snaps)
    }

    fn create_command(&self, description: &str) -> Vec<String> {
        // --scripted: without it a missing or stale snapshot device makes
        // timeshift prompt "Enter device name or number" on the terminal
        // (and give up with "Invalid input" without one); with it timeshift
        // fails at once with "Snapshot device not available".
        strs(&[
            "timeshift",
            "--create",
            "--comments",
            description,
            "--scripted",
        ])
    }

    fn guarantees(&self) -> &'static str {
        "Covers: the system files, in the mode timeshift is set up for (see /etc/timeshift/timeshift.json).\n\
         Does NOT cover, in rsync mode: /home and /root contents, which are excluded by default. In btrfs mode: anything outside the `@` subvolume (and `@home` unless it was enabled); other filesystems in either mode.\n\
         Not a backup unless timeshift's snapshot device is a different disk. If timeshift was never set up, it chooses a device itself on first use: check its settings first."
    }
}

// -------------------------------------------------------------------- zfs

/// The dataset mounted at `/`, from /proc/self/mountinfo content. `None`
/// when `/` is not on ZFS. The last mount over `/` wins. Pure.
pub fn zfs_root_dataset(mountinfo: &str) -> Option<String> {
    // "... <mount point> <opts> [optional fields] - <fstype> <source> <superopts>"
    let mut found = None;
    for line in mountinfo.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        if left.split_whitespace().nth(4) != Some("/") {
            continue;
        }
        let mut r = right.split_whitespace();
        found = match (r.next(), r.next()) {
            (Some("zfs"), Some(source)) => Some(source.to_string()),
            _ => None,
        };
    }
    found
}

/// `DATASET@yapacc-YYYYMMDDTHHMMSSZ` for a unix time. Pure.
pub fn zfs_snapshot_name(dataset: &str, unix_secs: u64) -> String {
    // "YYYY-MM-DD HH:MM UTC"
    let stamp = crate::util::fmt_utc(unix_secs);
    let digits: String = stamp.chars().filter(|c| c.is_ascii_digit()).collect();
    let (date, hm) = digits.split_at(digits.len().min(8));
    format!("{dataset}@yapacc-{date}T{hm}{:02}Z", unix_secs % 60)
}

/// Parse `zfs list -H -p -t snapshot -o name,creation,yapacc:description`:
/// tab-separated, creation in unix seconds (shown as it is if the tool
/// printed a date instead), `-` for an unset description. Order as given.
/// Pure.
pub fn parse_zfs_list(text: &str) -> Vec<Snapshot> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let name = f.next()?.trim();
            if name.is_empty() || !name.contains('@') {
                return None;
            }
            let creation = f.next().unwrap_or("").trim();
            let description = f.next().unwrap_or("").trim();
            Some(Snapshot {
                id: name.to_string(),
                when: match creation.parse::<u64>() {
                    Ok(secs) => crate::util::fmt_utc(secs),
                    Err(_) => creation.to_string(),
                },
                description: if description == "-" {
                    String::new()
                } else {
                    description.to_string()
                },
                provider: "zfs".to_string(),
            })
        })
        .collect()
}

pub struct Zfs {
    /// the dataset mounted at `/`
    pub dataset: String,
}

impl SnapshotProvider for Zfs {
    fn name(&self) -> &'static str {
        "zfs"
    }

    fn list(&self) -> Result<Vec<Snapshot>> {
        let mut snaps = parse_zfs_list(&capture(&[
            "zfs",
            "list",
            "-H",
            "-p",
            "-t",
            "snapshot",
            "-o",
            "name,creation,yapacc:description",
            "-s",
            "creation",
        ])?);
        snaps.reverse();
        Ok(snaps)
    }

    fn create_command(&self, description: &str) -> Vec<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        vec![
            "zfs".to_string(),
            "snapshot".to_string(),
            "-o".to_string(),
            format!("yapacc:description={description}"),
            zfs_snapshot_name(&self.dataset, now),
        ]
    }

    fn guarantees(&self) -> &'static str {
        "Covers: the one dataset mounted at `/`, as it is at this instant.\n\
         Does NOT cover: its child datasets (the snapshot is not recursive) or any other dataset or pool, such as a separate /home, /var or /boot.\n\
         Not a backup: the snapshot lives in the same pool as the data. Nothing deletes it automatically."
    }
}

// ------------------------------------------------------------------ btrfs

/// Parse `btrfs subvolume list -s /`: lines of the form
/// `ID 260 gen 100 cgen 99 top level 5 otime 2026-11-01 10:00:01 path .snapshots/1/snapshot`.
/// Order as given. Pure.
pub fn parse_btrfs_snapshots(text: &str) -> Vec<Snapshot> {
    text.lines()
        .filter_map(|line| {
            let (left, path) = line.split_once(" path ")?;
            let words: Vec<&str> = left.split_whitespace().collect();
            if words.first() != Some(&"ID") {
                return None;
            }
            let id = words.get(1)?;
            let when = words
                .iter()
                .position(|w| *w == "otime")
                .map(|i| {
                    words[i + 1..]
                        .iter()
                        .take(2)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            Some(Snapshot {
                id: id.to_string(),
                when,
                description: path.trim().to_string(),
                provider: "btrfs".to_string(),
            })
        })
        .collect()
}

/// Plain btrfs with no snapshot manager: listing only.
pub struct Btrfs;

pub const BTRFS_NO_MANAGER: &str = "`/` is on btrfs but no snapshot manager is configured; install snapper or timeshift. yapacc does not create raw btrfs snapshots: where the snapshot subvolume should live is a layout decision it would have to guess.";

impl SnapshotProvider for Btrfs {
    fn name(&self) -> &'static str {
        "btrfs"
    }

    fn list(&self) -> Result<Vec<Snapshot>> {
        let mut snaps =
            parse_btrfs_snapshots(&capture(&["btrfs", "subvolume", "list", "-s", "/"])?);
        // creation time, newest first (the format sorts as text)
        snaps.sort_by(|a, b| b.when.cmp(&a.when));
        Ok(snaps)
    }

    /// Always empty: creation is not offered for plain btrfs.
    fn create_command(&self, _description: &str) -> Vec<String> {
        Vec::new()
    }

    fn guarantees(&self) -> &'static str {
        BTRFS_NO_MANAGER
    }
}

// -------------------------------------------------------------- detection

/// The evidence detection works from, gathered once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub snapper_tool: bool,
    pub snapper_root_config: bool,
    pub timeshift_tool: bool,
    pub zfs_tool: bool,
    pub btrfs_tool: bool,
    /// filesystem type of `/`
    pub root_fstype: String,
    /// dataset mounted at `/` when that is ZFS
    pub zfs_root: Option<String>,
}

fn evidence() -> Evidence {
    // FreeBSD has no /proc/self/mountinfo: ask `mount -p` instead
    let zfs_root = match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(text) => zfs_root_dataset(&text),
        Err(_) => crate::platform::root_mount_without_proc()
            .and_then(|(fs, source)| (fs == "zfs").then_some(source)),
    };
    Evidence {
        snapper_tool: crate::util::have("snapper"),
        snapper_root_config: Path::new(SNAPPER_ROOT_CONFIG).exists(),
        timeshift_tool: crate::util::have("timeshift"),
        zfs_tool: crate::util::have("zfs"),
        btrfs_tool: crate::util::have("btrfs"),
        root_fstype: crate::platform::root_fstype(),
        zfs_root,
    }
}

/// Providers that can take a snapshot here, in order of preference:
/// snapper (tool and root config), timeshift (tool), zfs (tool and `/` on
/// ZFS). Same evidence as `platform::detect()`'s `snapshot_providers`,
/// plus the snapper root config: an installed but unconfigured snapper
/// cannot snapshot anything. Pure.
pub fn creators(e: &Evidence) -> Vec<&'static str> {
    let mut v = Vec::new();
    if e.snapper_tool && e.snapper_root_config {
        v.push("snapper");
    }
    if e.timeshift_tool {
        v.push("timeshift");
    }
    if e.zfs_tool && e.zfs_root.is_some() {
        v.push("zfs");
    }
    v
}

/// Is this the plain-btrfs case: `/` on btrfs, the tool present?
pub fn plain_btrfs(e: &Evidence) -> bool {
    e.root_fstype == "btrfs" && e.btrfs_tool
}

/// Providers to list from: the creators, or plain btrfs when there is
/// none (with snapper or timeshift present, the btrfs listing would only
/// repeat their snapshots). Pure.
pub fn listers(e: &Evidence) -> Vec<&'static str> {
    let mut v = creators(e);
    if v.is_empty() && plain_btrfs(e) {
        v.push("btrfs");
    }
    v
}

fn provider(name: &str, e: &Evidence) -> Option<Box<dyn SnapshotProvider>> {
    match name {
        "snapper" => Some(Box::new(Snapper)),
        "timeshift" => Some(Box::new(Timeshift)),
        "zfs" => e
            .zfs_root
            .clone()
            .map(|dataset| Box::new(Zfs { dataset }) as Box<dyn SnapshotProvider>),
        "btrfs" => Some(Box::new(Btrfs)),
        _ => None,
    }
}

/// The provider `create` would use on this machine, if any.
pub fn preferred() -> Option<Box<dyn SnapshotProvider>> {
    let e = evidence();
    let name = creators(&e).into_iter().next()?;
    provider(name, &e)
}

/// What was looked for and not found. Pure.
pub fn none_message(e: &Evidence) -> String {
    let mut s = String::from(
        "no snapshot provider is usable on this machine. Looked for: snapper with a root config (/etc/snapper/configs/root), timeshift, zfs with `/` on ZFS, btrfs with `/` on btrfs.",
    );
    if e.snapper_tool && !e.snapper_root_config {
        s.push_str("\nsnapper is installed but has no root config: `snapper -c root create-config /` sets one up.");
    }
    if e.zfs_tool && e.zfs_root.is_none() {
        s.push_str(
            "\nzfs is installed but `/` is not on ZFS; yapacc only snapshots the root dataset.",
        );
    }
    s.push_str(&format!(
        "\nFilesystem of `/`: {}.\n{UNDO_WITHOUT_SNAPSHOTS}",
        e.root_fstype
    ));
    s
}

// --------------------------------------------------------------- commands

fn render(snaps: &[Snapshot]) -> String {
    let header = ["PROVIDER", "ID", "WHEN", "DESCRIPTION"];
    let mut width = header.map(|h| h.chars().count());
    for s in snaps {
        for (i, cell) in [&s.provider, &s.id, &s.when].iter().enumerate() {
            width[i] = width[i].max(cell.chars().count());
        }
    }
    let line = |c: [&str; 4]| {
        format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            c[0],
            c[1],
            c[2],
            c[3],
            w0 = width[0],
            w1 = width[1],
            w2 = width[2],
        )
        .trim_end()
        .to_string()
    };
    let mut out = line(header);
    out.push('\n');
    for s in snaps {
        out.push_str(&line([
            s.provider.as_str(),
            s.id.as_str(),
            s.when.as_str(),
            s.description.as_str(),
        ]));
        out.push('\n');
    }
    out
}

/// `yapacc snapshot list`: every usable provider's snapshots, newest first
/// within each provider. Read-only.
pub fn list(json: bool) -> Result<()> {
    let e = evidence();
    let names = listers(&e);
    if names.is_empty() {
        return Err(Error::unavailable(none_message(&e)));
    }
    let mut snaps: Vec<Snapshot> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    // every failure was "this user may not" (or a tool that is not there)
    let mut all_unavailable = true;
    let mut answered = 0usize;
    for name in &names {
        let Some(p) = provider(name, &e) else {
            continue;
        };
        match p.list() {
            Ok(mut found) => {
                answered += 1;
                snaps.append(&mut found);
            }
            Err(err) => {
                all_unavailable &= err.kind == crate::error::Kind::Unavailable;
                failures.push(format!("{name}: {err}"));
            }
        }
    }
    // a provider that could not be asked is an unknown, never "no snapshots"
    if answered == 0 {
        let message = format!(
            "no snapshot provider could be listed.\n{}",
            failures.join("\n")
        );
        return Err(if all_unavailable {
            Error::unavailable(message)
        } else {
            Error::from(message)
        });
    }
    for f in &failures {
        eprintln!("warning: {f}");
    }

    if json {
        return match serde_json::to_string_pretty(&snaps) {
            Ok(s) => {
                println!("{s}");
                Ok(())
            }
            Err(e) => Err(Error::from(format!("json serialization failed: {e}"))),
        };
    }

    println!("Snapshot providers: {}", names.join(", "));
    if snaps.is_empty() {
        println!("No snapshot exists yet.");
    } else {
        println!();
        print!("{}", render(&snaps));
    }
    if names == ["btrfs"] {
        println!("\nThese are the snapshot subvolumes btrfs itself reports. {BTRFS_NO_MANAGER}");
    } else if e.zfs_root.is_some() && names.contains(&"zfs") {
        println!(
            "\nzfs: snapshots of every dataset are listed; `yapacc snapshot create` snapshots only the dataset mounted at `/`."
        );
    }
    if !failures.is_empty() {
        println!(
            "\nIncomplete: {} provider(s) could not be listed (see the warnings).",
            failures.len()
        );
    }
    Ok(())
}

fn valid_description(d: &str) -> bool {
    !d.trim().is_empty() && d.len() <= 200 && !d.chars().any(char::is_control)
}

/// `yapacc snapshot create`: show the provider, the command and its
/// limits; then gate; then run the provider's own tool.
pub fn create(description: Option<&str>, g: crate::ops::GateFlags) -> Result<()> {
    let description = description.unwrap_or(DEFAULT_DESCRIPTION);
    if !valid_description(description) {
        return Err(Error::usage(
            "the description must be one non-empty line of at most 200 bytes, without control characters",
        ));
    }
    let e = evidence();
    let Some(p) = creators(&e)
        .into_iter()
        .next()
        .and_then(|n| provider(n, &e))
    else {
        return Err(Error::unavailable(if plain_btrfs(&e) {
            format!("{BTRFS_NO_MANAGER}\n{UNDO_WITHOUT_SNAPSHOTS}")
        } else {
            none_message(&e)
        }));
    };
    let cmd = p.create_command(description);
    if cmd.is_empty() {
        return Err(Error::unavailable(format!(
            "{} cannot take a snapshot through yapacc. {}",
            p.name(),
            p.guarantees()
        )));
    }
    let root = crate::ops::elevation_for(g);
    let shown_prefix = root
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| vec!["<root>".into()]);

    println!("Provider: {}", p.name());
    let others: Vec<&str> = creators(&e).into_iter().skip(1).collect();
    if !others.is_empty() {
        println!("  (also usable here, not used: {})", others.join(", "));
    }
    println!("Will run:\n  {}", crate::ops::shown(&shown_prefix, &cmd));
    println!("\n{}", p.guarantees());
    if g.dryrun {
        println!("\n{}", crate::prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    println!();
    crate::ops::gate("Take this snapshot", g)?;
    let prefix = root?;
    match crate::ops::run_attached(&prefix, &cmd, true)? {
        0 => {
            println!(
                "Snapshot requested through {}; `yapacc snapshot list` shows it.",
                p.name()
            );
            Ok(())
        }
        code => Err(Error::from(format!(
            "{} exited with status {code}; no snapshot should be assumed to exist{}",
            cmd[0],
            crate::ops::elevation_hint(&prefix, code)
        ))),
    }
}

// ------------------------------------------- snapshot before a change

/// `[safety] snapshot`: whether a change through yapacc is preceded by a
/// snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// snapshot when a tool is detected; a failed snapshot refuses the change
    Auto,
    /// like `Auto`, but a failed snapshot only warns
    AutoBestEffort,
}

impl Mode {
    pub fn parse(s: &str) -> std::result::Result<Mode, String> {
        match s {
            "off" => Ok(Mode::Off),
            "auto" => Ok(Mode::Auto),
            "auto-best-effort" => Ok(Mode::AutoBestEffort),
            other => Err(format!(
                "invalid value '{other}' (expected: off | auto | auto-best-effort)"
            )),
        }
    }
}

/// What to do before the native command. `creators` is `creators(&evidence)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// no snapshot: mode is off, or this is a dry run
    Skip,
    /// mode asks for one but no tool can take it: say so, continue
    NoTool,
    /// take one through this provider
    Take(&'static str),
}

/// Decide whether to snapshot. Never snapshots on a dry run. Pure.
pub fn decide(mode: Mode, dryrun: bool, creators: &[&'static str]) -> Decision {
    if mode == Mode::Off || dryrun {
        return Decision::Skip;
    }
    match creators.first() {
        Some(name) => Decision::Take(name),
        None => Decision::NoTool,
    }
}

/// Only system-changing native managers are snapshotted: a root
/// filesystem snapshot says nothing about a user-level store (nix profiles
/// in the home directory, flatpak --user, brew). Pure.
pub fn manager_is_snapshotted(m: crate::platform::PkgManager) -> bool {
    !matches!(m, crate::platform::PkgManager::Nix)
}

/// Valid `[safety] snapshot_provider` values.
pub const PROVIDER_CHOICES: &str = "auto | btrfs | snapper | timeshift | zfs | none";

pub fn provider_pref_valid(s: &str) -> bool {
    matches!(
        s,
        "auto" | "btrfs" | "snapper" | "timeshift" | "zfs" | "none"
    )
}

/// `decide`, restricted by manager and by `[safety] snapshot_provider`:
/// `none` never snapshots; a named tool is used only if detected (plain
/// `btrfs` cannot create snapshots, so it yields `NoTool`). Pure.
pub fn decide_for(
    mode: Mode,
    dryrun: bool,
    eligible: bool,
    pref: &str,
    creators: &[&'static str],
) -> Decision {
    if !eligible || pref == "none" {
        return Decision::Skip;
    }
    let chosen: Vec<&'static str> = creators
        .iter()
        .copied()
        .filter(|c| pref == "auto" || *c == pref)
        .collect();
    decide(mode, dryrun, &chosen)
}

/// What a change does about its snapshot, decided before the gate so that
/// the plan can say it and the consent covers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outlook {
    /// nothing to say: `snapshot = "off"`, `snapshot_provider = "none"`, or
    /// a manager whose changes a root snapshot does not cover
    Skip,
    /// one is taken through this provider
    Take(&'static str),
    /// `snapshot_provider = "auto"` and no tool exists on this machine: the
    /// change runs without a snapshot, and the plan says so
    NoTool,
    /// `snapshot_provider` names a tool that is not usable here. Under
    /// `snapshot = "auto"` the change is refused; under `auto-best-effort`
    /// it goes on with a warning.
    Unusable { refuse: bool },
}

/// Decide the [`Outlook`]. `creators` is `creators(&evidence)`. Pure.
pub fn outlook(mode: Mode, eligible: bool, pref: &str, creators: &[&'static str]) -> Outlook {
    if mode == Mode::Off || !eligible || pref == "none" {
        return Outlook::Skip;
    }
    if pref == "auto" {
        return match creators.first() {
            Some(name) => Outlook::Take(name),
            None => Outlook::NoTool,
        };
    }
    match creators.iter().find(|c| **c == pref) {
        Some(name) => Outlook::Take(name),
        None => Outlook::Unusable {
            refuse: failure_refuses(mode),
        },
    }
}

/// Why the tool `snapshot_provider` names cannot take a snapshot here. Pure.
pub fn unusable_reason(pref: &str, e: &Evidence) -> String {
    match pref {
        "snapper" if !e.snapper_tool => "snapper is not installed".into(),
        "snapper" => {
            "snapper has no root config (`snapper -c root create-config /` sets one up)".into()
        }
        "timeshift" => "timeshift is not installed".into(),
        "zfs" if !e.zfs_tool => "zfs is not installed".into(),
        "zfs" => "`/` is not on ZFS".into(),
        "btrfs" => {
            "yapacc does not create plain btrfs snapshots (snapper or timeshift does)".into()
        }
        other => format!("{other} is not a snapshot tool yapacc drives"),
    }
}

/// The line of the plan when no snapshot tool exists (`Outlook::NoTool`).
pub const NO_TOOL_LINE: &str = "No snapshot will be taken: [safety] snapshot is set, but there is no snapshot tool on this machine: the change will run without a snapshot.";

/// The refusal when the named tool is not usable under `snapshot = "auto"`.
/// Pure.
pub fn unusable_refusal(pref: &str, why: &str) -> String {
    format!(
        "[safety] snapshot = \"auto\" requires a snapshot before the change, and snapshot_provider = \"{pref}\" cannot take one here: {why}. Nothing was changed. Make {pref} usable, set snapshot_provider = \"auto\", or set snapshot = \"auto-best-effort\" to go on without a snapshot."
    )
}

/// The warning for the same under `snapshot = "auto-best-effort"`. Pure.
pub fn unusable_warning(pref: &str, why: &str) -> String {
    format!(
        "No snapshot will be taken: snapshot_provider = \"{pref}\" cannot take one here ({why}); the change will run without a snapshot, as snapshot = \"auto-best-effort\" allows."
    )
}

/// Say, in the plan and before the gate, what happens about the snapshot:
/// the tool and the exact command when one will be taken; that none will
/// be when no tool exists; a refusal (exit 3, also under `--dryrun`) when
/// the tool `snapshot_provider` names is not usable and `snapshot = "auto"`.
/// Returns the tool when a snapshot will be taken. Runs nothing.
pub fn announce(
    mode: Mode,
    eligible: bool,
    root: &[String],
    what: &str,
    dryrun: bool,
) -> Result<Option<&'static str>> {
    let e = evidence();
    let pref = crate::config::get().safety.snapshot_provider.clone();
    match outlook(mode, eligible, &pref, &creators(&e)) {
        Outlook::Skip => Ok(None),
        Outlook::Take(name) => {
            let cmd = provider(name, &e)
                .map(|p| p.create_command(&snapshot_description(what)))
                .unwrap_or_default();
            if cmd.is_empty() {
                return Ok(None);
            }
            println!(
                "Before that, a snapshot will be taken through {name}:\n  {}{}",
                crate::ops::shown(root, &cmd),
                if dryrun { " (not under --dryrun)" } else { "" }
            );
            Ok(Some(name))
        }
        Outlook::NoTool => {
            println!("{NO_TOOL_LINE}");
            Ok(None)
        }
        Outlook::Unusable { refuse: true } => Err(Error::refused(unusable_refusal(
            &pref,
            &unusable_reason(&pref, &e),
        ))),
        Outlook::Unusable { refuse: false } => {
            println!(
                "warning: {}",
                unusable_warning(&pref, &unusable_reason(&pref, &e))
            );
            Ok(None)
        }
    }
}

fn snapshot_description(what: &str) -> String {
    format!("yapacc: before {what}")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

/// What a failed snapshot means for the change. Pure.
pub fn failure_refuses(mode: Mode) -> bool {
    mode == Mode::Auto
}

/// Take the pre-change snapshot. Call after the user consented and before
/// the native command. Err means the change must not run.
///
/// Only `decide` and the config parsing are unit-tested; the provider
/// commands and their failure handling ran for real against snapper, zfs and
/// timeshift (rsync mode) on VMs, see STATUS.md.
pub fn before_change(
    mode: Mode,
    dryrun: bool,
    eligible: bool,
    root: &[String],
    what: &str,
) -> Result<()> {
    if mode == Mode::Off || dryrun || !eligible {
        return Ok(());
    }
    let e = evidence();
    let found = creators(&e);
    let pref = crate::config::get().safety.snapshot_provider.clone();
    // the same decision the plan announced before the gate
    let name = match outlook(mode, eligible, &pref, &found) {
        Outlook::Skip => return Ok(()),
        Outlook::NoTool => {
            println!("snapshot: none taken (no snapshot tool on this machine), as the plan said.");
            return Ok(());
        }
        Outlook::Unusable { refuse: true } => {
            return Err(Error::refused(unusable_refusal(
                &pref,
                &unusable_reason(&pref, &e),
            )));
        }
        Outlook::Unusable { refuse: false } => {
            println!(
                "snapshot: none taken (snapshot_provider = \"{pref}\" cannot take one here), as the plan said."
            );
            return Ok(());
        }
        Outlook::Take(n) => n,
    };
    let description = snapshot_description(what);
    let outcome: Result<()> = (|| {
        let p =
            provider(name, &e).ok_or_else(|| Error::from(format!("{name} is not usable here")))?;
        let cmd = p.create_command(&description);
        if cmd.is_empty() {
            return Err(Error::from(format!("{name} cannot take a snapshot")));
        }
        println!("snapshot: {}", crate::ops::shown(root, &cmd));
        match crate::ops::run_attached(root, &cmd, true)? {
            0 => Ok(()),
            code => Err(Error::from(format!(
                "{} exited with status {code}{}",
                crate::ops::shown(root, &cmd),
                crate::ops::elevation_hint(root, code)
            ))),
        }
    })();
    match outcome {
        Ok(()) => {
            println!("snapshot: created through {name}: \"{description}\"");
            Ok(())
        }
        // a refusal (exit 3): the guard said no, and nothing was changed
        Err(err) if failure_refuses(mode) => Err(Error::refused(format!(
            "snapshot failed, so the change was not made: {err}\nNothing was changed. Set [safety] snapshot = \"auto-best-effort\" to continue without one."
        ))),
        Err(err) => {
            eprintln!("warning: snapshot failed ({err}); continuing as auto-best-effort allows.");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_mode_parses() {
        assert_eq!(Mode::parse("off"), Ok(Mode::Off));
        assert_eq!(Mode::parse("auto"), Ok(Mode::Auto));
        assert_eq!(Mode::parse("auto-best-effort"), Ok(Mode::AutoBestEffort));
        assert!(Mode::parse("always").is_err());
    }

    #[test]
    fn snapshot_decision() {
        let tools = ["snapper", "timeshift"];
        assert_eq!(decide(Mode::Off, false, &tools), Decision::Skip);
        assert_eq!(decide(Mode::Auto, true, &tools), Decision::Skip);
        assert_eq!(decide(Mode::AutoBestEffort, true, &tools), Decision::Skip);
        assert_eq!(decide(Mode::Auto, false, &[]), Decision::NoTool);
        assert_eq!(decide(Mode::Auto, false, &tools), Decision::Take("snapper"));
        assert_eq!(
            decide(Mode::AutoBestEffort, false, &["zfs"]),
            Decision::Take("zfs")
        );
        assert!(failure_refuses(Mode::Auto));
        assert!(!failure_refuses(Mode::AutoBestEffort));
    }

    #[test]
    fn provider_pref_and_manager_restrict_decision() {
        let tools = ["snapper", "timeshift"];
        let d = |dry, el, pref| decide_for(Mode::Auto, dry, el, pref, &tools);
        assert_eq!(d(false, true, "auto"), Decision::Take("snapper"));
        assert_eq!(d(false, true, "timeshift"), Decision::Take("timeshift"));
        assert_eq!(d(false, true, "zfs"), Decision::NoTool);
        assert_eq!(d(false, true, "btrfs"), Decision::NoTool);
        assert_eq!(d(false, true, "none"), Decision::Skip);
        assert_eq!(d(false, false, "auto"), Decision::Skip);
        // --dryrun never reaches a snapshot, whatever the tools
        for pref in ["auto", "snapper", "timeshift"] {
            assert_eq!(d(true, true, pref), Decision::Skip);
        }
        assert!(provider_pref_valid("none") && !provider_pref_valid("snap"));
        use crate::platform::PkgManager as M;
        assert!(manager_is_snapshotted(M::Pacman) && manager_is_snapshotted(M::Apt));
        assert!(!manager_is_snapshotted(M::Nix));
    }

    /// E1: with `snapshot = "auto"` and no usable tool the dry run said
    /// nothing and the real run went on with one line, even when
    /// `snapshot_provider` named a tool that was absent.
    #[test]
    fn a_named_tool_that_is_not_usable_refuses_under_auto() {
        let none: [&'static str; 0] = [];
        let tools = ["timeshift"];
        // snapshot_provider = "auto": take one if a tool is found
        assert_eq!(
            outlook(Mode::Auto, true, "auto", &tools),
            Outlook::Take("timeshift")
        );
        // ... and with no tool at all the plan says so, before the gate
        assert_eq!(outlook(Mode::Auto, true, "auto", &none), Outlook::NoTool);
        assert_eq!(
            outlook(Mode::AutoBestEffort, true, "auto", &none),
            Outlook::NoTool
        );
        // a named tool that is not usable: refuse for auto, warn otherwise
        assert_eq!(
            outlook(Mode::Auto, true, "snapper", &none),
            Outlook::Unusable { refuse: true }
        );
        assert_eq!(
            outlook(Mode::Auto, true, "snapper", &tools),
            Outlook::Unusable { refuse: true }
        );
        assert_eq!(
            outlook(Mode::AutoBestEffort, true, "snapper", &none),
            Outlook::Unusable { refuse: false }
        );
        assert_eq!(
            outlook(Mode::Auto, true, "timeshift", &tools),
            Outlook::Take("timeshift")
        );
        // plain btrfs never creates
        assert_eq!(
            outlook(Mode::Auto, true, "btrfs", &["snapper"]),
            Outlook::Unusable { refuse: true }
        );
        // off, none, or a manager that is not snapshotted: nothing to say
        assert_eq!(outlook(Mode::Off, true, "snapper", &none), Outlook::Skip);
        assert_eq!(outlook(Mode::Auto, true, "none", &tools), Outlook::Skip);
        assert_eq!(outlook(Mode::Auto, false, "snapper", &none), Outlook::Skip);
        // the words
        let absent = Evidence::default();
        assert_eq!(
            unusable_reason("snapper", &absent),
            "snapper is not installed"
        );
        let unconfigured = Evidence {
            snapper_tool: true,
            ..Default::default()
        };
        assert!(unusable_reason("snapper", &unconfigured).contains("no root config"));
        assert_eq!(unusable_reason("zfs", &absent), "zfs is not installed");
        let refusal = unusable_refusal("snapper", "snapper is not installed");
        assert_eq!(
            refusal,
            "[safety] snapshot = \"auto\" requires a snapshot before the change, and snapshot_provider = \"snapper\" cannot take one here: snapper is not installed. Nothing was changed. Make snapper usable, set snapshot_provider = \"auto\", or set snapshot = \"auto-best-effort\" to go on without a snapshot."
        );
        assert_eq!(crate::Error::refused(refusal).kind.exit_code(), 3);
        assert!(
            unusable_warning("snapper", "snapper is not installed")
                .starts_with("No snapshot will be taken: snapshot_provider = \"snapper\"")
        );
        assert!(NO_TOOL_LINE.contains(
            "there is no snapshot tool on this machine: the change will run without a snapshot"
        ));
    }

    #[test]
    fn dryrun_before_change_runs_nothing() {
        // returns before any detection or command, even with mode auto
        assert!(before_change(Mode::Auto, true, true, &[], "x").is_ok());
    }

    fn snap(provider: &str, id: &str, when: &str, description: &str) -> Snapshot {
        Snapshot {
            id: id.into(),
            when: when.into(),
            description: description.into(),
            provider: provider.into(),
        }
    }

    #[test]
    fn snapper_csv() {
        let text = "number,date,description\n\
                    0,,current\n\
                    1,2026-10-30 09:00:01,timeline\n\
                    2,2026-11-01 10:00:01,\"pacman -Syu, then \"\"reboot\"\"\"\n\
                    3,2026-11-02 08:15:42,yapacc: manual snapshot\n";
        assert_eq!(
            parse_snapper_csv(text),
            vec![
                snap("snapper", "1", "2026-10-30 09:00:01", "timeline"),
                snap(
                    "snapper",
                    "2",
                    "2026-11-01 10:00:01",
                    "pacman -Syu, then \"reboot\""
                ),
                snap(
                    "snapper",
                    "3",
                    "2026-11-02 08:15:42",
                    "yapacc: manual snapshot"
                ),
            ]
        );
        assert!(parse_snapper_csv("").is_empty());
        assert!(parse_snapper_csv("No permissions.\n").is_empty());
    }

    #[test]
    fn snapper_table_ascii_and_unicode() {
        let ascii = " # | Type   | Pre # | Date                     | User | Cleanup  | Description | Userdata\n\
                     ---+--------+-------+--------------------------+------+----------+-------------+---------\n\
                     0  | single |       |                          | root |          | current     |         \n\
                     1* | single |       | Fri Oct 30 09:00:01 2026 | root | timeline | timeline    |         \n\
                     2  | pre    |       | Sun Nov  1 10:00:01 2026 | root | number   | pacman -Syu | important=yes\n";
        assert_eq!(
            parse_snapper_table(ascii),
            vec![
                snap("snapper", "1", "Fri Oct 30 09:00:01 2026", "timeline"),
                snap("snapper", "2", "Sun Nov  1 10:00:01 2026", "pacman -Syu"),
            ]
        );
        let unicode = " # │ Type   │ Pre # │ Date                │ User │ Cleanup │ Description │ Userdata\n\
                       ───┼────────┼───────┼─────────────────────┼──────┼─────────┼─────────────┼─────────\n\
                       0  │ single │       │                     │ root │         │ current     │\n\
                       7  │ single │       │ 2026-11-02 08:15:42 │ root │ number  │ before dnf  │\n";
        assert_eq!(
            parse_snapper_table(unicode),
            vec![snap("snapper", "7", "2026-11-02 08:15:42", "before dnf")]
        );
        assert!(parse_snapper_table("No permissions.\n").is_empty());
    }

    #[test]
    fn timeshift_list() {
        let text = "\n/dev/sda2 is mounted at: /run/timeshift/backup, options: rw,relatime\n\n\
                    Device : /dev/sda2\nUUID   : 0a1b2c3d\nPath   : /run/timeshift/backup\nMode   : RSYNC\nStatus : OK\n\
                    3 snapshots, 120.5 GB free\n\n\
                    Num     Name                 Tags  Description  \n\
                    ------------------------------------------------------------------------------\n\
                    0    >  2026-10-01_10-00-01  O     before upgrade  \n\
                    1    >  2026-10-15_12-00-01  D W   \n\
                    2    >  2026-11-02_08-15-42  O     yapacc: manual snapshot\n";
        assert_eq!(
            parse_timeshift_list(text),
            vec![
                snap(
                    "timeshift",
                    "2026-10-01_10-00-01",
                    "2026-10-01 10:00:01",
                    "before upgrade"
                ),
                snap(
                    "timeshift",
                    "2026-10-15_12-00-01",
                    "2026-10-15 12:00:01",
                    ""
                ),
                snap(
                    "timeshift",
                    "2026-11-02_08-15-42",
                    "2026-11-02 08:15:42",
                    "yapacc: manual snapshot"
                ),
            ]
        );
        assert!(parse_timeshift_list("No snapshots found\n").is_empty());
        // real btrfs-mode output: comments that start with a tag letter
        let real = "Num     Name                 Tags  Description  \n\
                    ------------------------------------------------------------------------------\n\
                    9    >  2026-11-18_05-36-50  O     M to the max                   \n\
                    10   >  2026-11-18_05-36-51  W     H                              \n\
                    11   >  2026-11-18_05-36-52  D W   multi tag                      \n\
                    12   >  2026-11-18_05-36-53  O                                    \n";
        let v = parse_timeshift_list(real);
        let d: Vec<&str> = v.iter().map(|s| s.description.as_str()).collect();
        assert_eq!(d, ["M to the max", "H", "multi tag", ""]);
    }

    #[test]
    fn zfs_list_and_names() {
        let text = "rpool/ROOT/default@install\t1761991201\t-\n\
                    rpool/home@daily\t1762077601\t-\n\
                    rpool/ROOT/default@yapacc-20261102T081542Z\t1793607342\tyapacc: manual snapshot\n";
        let got = parse_zfs_list(text);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            snap(
                "zfs",
                "rpool/ROOT/default@install",
                &crate::util::fmt_utc(1_761_991_201),
                ""
            )
        );
        assert_eq!(got[2].description, "yapacc: manual snapshot");
        // without -p the tool prints a date: kept as it is
        let human = parse_zfs_list("tank/root@a\tSun Nov  1 10:00 2026\n");
        assert_eq!(
            human,
            vec![snap("zfs", "tank/root@a", "Sun Nov  1 10:00 2026", "")]
        );
        assert!(parse_zfs_list("no datasets available\n").is_empty());

        // 951782400 = 2000-02-29 00:00:00 UTC
        assert_eq!(
            zfs_snapshot_name("rpool/ROOT/default", 951_782_400 + 3_600 + 5 * 60 + 7),
            "rpool/ROOT/default@yapacc-20000229T010507Z"
        );
    }

    #[test]
    fn zfs_root_from_mountinfo() {
        let zfs = "22 1 0:20 / /proc rw,nosuid shared:5 - proc proc rw\n\
                   25 1 0:23 / / rw,relatime shared:1 - zfs rpool/ROOT/default rw,xattr,posixacl\n\
                   40 25 0:30 / /home rw,relatime shared:20 - zfs rpool/home rw,xattr,posixacl\n";
        assert_eq!(zfs_root_dataset(zfs).as_deref(), Some("rpool/ROOT/default"));
        let btrfs = "25 1 0:23 /@ / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw,subvol=/@\n\
                     40 25 0:30 / /data rw - zfs tank/data rw\n";
        assert_eq!(zfs_root_dataset(btrfs), None);
        // no optional fields; a later mount over / wins
        let over =
            "25 1 0:23 / / rw - zfs rpool/ROOT/old rw\n26 1 0:24 / / rw - ext4 /dev/sda1 rw\n";
        assert_eq!(zfs_root_dataset(over), None);
        assert_eq!(zfs_root_dataset(""), None);
    }

    #[test]
    fn btrfs_snapshot_subvolumes() {
        let text = "ID 260 gen 100 cgen 99 top level 258 otime 2026-10-30 09:00:01 path .snapshots/1/snapshot\n\
                    ID 271 gen 180 cgen 180 top level 258 otime 2026-11-02 08:15:42 path .snapshots/2/snapshot\n";
        assert_eq!(
            parse_btrfs_snapshots(text),
            vec![
                snap(
                    "btrfs",
                    "260",
                    "2026-10-30 09:00:01",
                    ".snapshots/1/snapshot"
                ),
                snap(
                    "btrfs",
                    "271",
                    "2026-11-02 08:15:42",
                    ".snapshots/2/snapshot"
                ),
            ]
        );
        assert!(
            parse_btrfs_snapshots("ERROR: can't perform the search: Operation not permitted\n")
                .is_empty()
        );
    }

    #[test]
    fn detection_and_preference() {
        let all = Evidence {
            snapper_tool: true,
            snapper_root_config: true,
            timeshift_tool: true,
            zfs_tool: true,
            btrfs_tool: true,
            root_fstype: "zfs".into(),
            zfs_root: Some("rpool/ROOT/default".into()),
        };
        assert_eq!(creators(&all), vec!["snapper", "timeshift", "zfs"]);

        // snapper without a root config cannot snapshot anything
        let unconfigured = Evidence {
            snapper_tool: true,
            btrfs_tool: true,
            root_fstype: "btrfs".into(),
            ..Evidence::default()
        };
        assert!(creators(&unconfigured).is_empty());
        assert_eq!(listers(&unconfigured), vec!["btrfs"]);
        assert!(none_message(&unconfigured).contains("create-config"));

        // zfs the tool, but / elsewhere
        let zfs_elsewhere = Evidence {
            zfs_tool: true,
            root_fstype: "ext4".into(),
            ..Evidence::default()
        };
        assert!(listers(&zfs_elsewhere).is_empty());
        assert!(none_message(&zfs_elsewhere).contains("package versions only"));

        // with a manager present, btrfs is not listed a second time
        let managed = Evidence {
            timeshift_tool: true,
            btrfs_tool: true,
            root_fstype: "btrfs".into(),
            ..Evidence::default()
        };
        assert_eq!(listers(&managed), vec!["timeshift"]);
    }

    #[test]
    fn create_commands() {
        assert_eq!(
            Snapper.create_command("before upgrade"),
            vec![
                "snapper",
                "create",
                "--type",
                "single",
                "--cleanup-algorithm",
                "number",
                "--description",
                "before upgrade"
            ]
        );
        assert_eq!(
            Timeshift.create_command("x"),
            vec!["timeshift", "--create", "--comments", "x", "--scripted"]
        );
        let z = Zfs {
            dataset: "rpool/ROOT/default".into(),
        }
        .create_command("x");
        assert_eq!(z[..4], ["zfs", "snapshot", "-o", "yapacc:description=x"]);
        assert!(z[4].starts_with("rpool/ROOT/default@yapacc-"));
        assert!(Btrfs.create_command("x").is_empty());
        // every provider states a limit
        for text in [
            Snapper.guarantees(),
            Timeshift.guarantees(),
            Zfs {
                dataset: String::new(),
            }
            .guarantees(),
        ] {
            assert!(text.contains("Does NOT cover"));
        }
    }

    /// D7: `snapshot list` as an ordinary user on a btrfs root exited 1.
    #[test]
    fn a_listing_refused_for_lack_of_privileges_is_unavailable() {
        use crate::error::Kind;
        // btrfs's own words on Fedora, as an ordinary user
        let refused = "ERROR: can't perform the search: Operation not permitted";
        assert_eq!(failure_kind(refused, false), Kind::Unavailable);
        assert_eq!(Kind::Unavailable.exit_code(), 6);
        assert!(
            failure_message(
                "btrfs subvolume list -s /",
                "exit status: 1",
                refused,
                false
            )
            .ends_with("Listing these snapshots needs root: run as root.")
        );
        // root was refused for another reason, or the tool failed otherwise
        assert_eq!(failure_kind(refused, true), Kind::Failed);
        assert_eq!(
            failure_kind("ERROR: not a btrfs filesystem: /", false),
            Kind::Failed
        );
    }

    #[test]
    fn failures_say_run_as_root() {
        let m = failure_message("snapper list", "exit status: 1", "No permissions.", false);
        assert!(m.contains("run as root"));
        assert!(m.contains("No permissions."));
        let m = failure_message("timeshift --list", "exit status: 1", "", false);
        assert!(m.contains("run as root"));
        let m = failure_message("snapper list", "exit status: 1", "Unknown config.", true);
        assert!(!m.contains("run as root"));
    }

    #[test]
    fn descriptions_are_one_plain_line() {
        assert!(valid_description(DEFAULT_DESCRIPTION));
        assert!(!valid_description(""));
        assert!(!valid_description("a\nb"));
        assert!(!valid_description(&"x".repeat(201)));
    }
}
