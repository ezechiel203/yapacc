// yapacc — `undo`: take the last package transaction back
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! The last transaction is read from the native manager's own log, reversed
//! (installed → removed, removed → installed again, upgraded → the version
//! before), and applied through the native manager after the usual gate.
//!
//! The old versions come from, in this order: the native package cache, the
//! configured repositories, and the distribution's public archive (Arch
//! Linux Archive, snapshot.debian.org, Launchpad). When one version cannot
//! be found anywhere the whole undo is refused and nothing is changed: a
//! half-undone transaction is worse than none.
//!
//! Planning writes nothing: an archive is only asked whether it has a file
//! (and how large it is), so a dry run downloads nothing. The files are
//! downloaded after the user agreed, into a directory made for that run,
//! checked (Arch: the detached signature, which is required; Debian: the
//! archive's hash), and deleted when the run ends. A file that is already
//! in that place is never trusted. `[safety] snapshot` applies as to any
//! other change.
//!
//! Does not: restore configuration files or data a package's scripts
//! changed, undo more than package versions, or replace a filesystem
//! snapshot. It says so before asking.

use crate::error::{Error, Result};
use crate::ops::{self, GateFlags};
use crate::pkglog::{self, Action, LogEntry};
use crate::platform::PkgManager;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One native transaction: when, by which command, and what it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub when: String,
    pub command: String,
    pub entries: Vec<LogEntry>,
}

/// What undoing one entry takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// it was installed: remove it
    Remove { name: String },
    /// it was removed, upgraded or downgraded: bring this version back
    Restore { name: String, version: String },
}

/// The steps that reverse `entries`. Reinstalls change nothing and are skipped.
pub fn reverse(entries: &[LogEntry]) -> Vec<Step> {
    entries
        .iter()
        .rev()
        .filter_map(|e| match e.action {
            Action::Install => Some(Step::Remove {
                name: e.name.clone(),
            }),
            Action::Remove | Action::Upgrade | Action::Downgrade if !e.old_version.is_empty() => {
                Some(Step::Restore {
                    name: e.name.clone(),
                    version: e.old_version.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------ log reading

/// The last transaction of a pacman.log that changed at least one package.
pub fn last_pacman_transaction(log: &str) -> Option<Transaction> {
    let lines: Vec<&str> = log.lines().collect();
    let mut end = lines.len();
    while let Some(start) = lines[..end]
        .iter()
        .rposition(|l| l.contains("[ALPM] transaction started"))
    {
        let block = lines[start..end].join("\n");
        let entries = pkglog::parse_pacman_log(&block);
        if !entries.is_empty() {
            let command = lines[..start]
                .iter()
                .rev()
                .find_map(|l| l.split_once("[PACMAN] Running '"))
                .map(|(_, c)| c.trim_end_matches('\'').to_string())
                .unwrap_or_default();
            let when = lines[start]
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .to_string();
            return Some(Transaction {
                when,
                command,
                entries,
            });
        }
        end = start;
    }
    None
}

/// The name and the architecture of a package as apt's history writes it
/// (`libpng16-16t64:i386`); the architecture is empty when none is given.
/// Pure.
pub fn apt_split(target: &str) -> (&str, &str) {
    target.split_once(':').unwrap_or((target, ""))
}

/// One `name:arch (v1[, v2])` item of an apt history line. The architecture
/// stays in the name: on a multiarch system `libfoo:i386` and `libfoo:amd64`
/// are two packages, and apt takes a bare `libfoo` as the native one.
fn apt_item(item: &str) -> Option<(String, String, String)> {
    let (name, rest) = item.trim().split_once(" (")?;
    let rest = rest.trim_end_matches(')');
    let mut parts = rest.split(", ");
    let first = parts.next()?.trim().to_string();
    let second = parts
        .next()
        .map(str::trim)
        .filter(|s| *s != "automatic")
        .unwrap_or("")
        .to_string();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some((name, first, second))
}

/// The last block of `/var/log/apt/history.log` that changed a package.
pub fn last_apt_transaction(history: &str) -> Option<Transaction> {
    let blocks: Vec<&str> = history.split("\nStart-Date:").collect();
    for (i, block) in blocks.iter().enumerate().rev() {
        let mut t = Transaction {
            when: String::new(),
            command: String::new(),
            entries: vec![],
        };
        for (n, line) in block.lines().enumerate() {
            // the first block still carries its own "Start-Date:" prefix
            let line = if n == 0 && i > 0 {
                format!("Start-Date:{line}")
            } else {
                line.to_string()
            };
            let Some((key, value)) = line.split_once(": ") else {
                continue;
            };
            let action = match key {
                "Start-Date" => {
                    t.when = value.trim().to_string();
                    continue;
                }
                "Commandline" => {
                    t.command = value.trim().to_string();
                    continue;
                }
                "Install" => Action::Install,
                "Upgrade" => Action::Upgrade,
                "Downgrade" => Action::Downgrade,
                "Remove" | "Purge" => Action::Remove,
                _ => continue,
            };
            for item in value.split("), ") {
                let Some((name, first, second)) = apt_item(item) else {
                    continue;
                };
                let (old, new) = match action {
                    Action::Install => (String::new(), first),
                    Action::Remove => (first, String::new()),
                    _ => (first, second),
                };
                t.entries.push(LogEntry {
                    ts: 0,
                    source: "apt".into(),
                    action,
                    name,
                    old_version: old,
                    new_version: new,
                });
            }
        }
        if !t.entries.is_empty() {
            return Some(t);
        }
    }
    None
}

/// The last run of `/var/log/zypp/history` that changed a package.
///
/// zypper writes a `command` record when it starts; the install and remove
/// records up to the next `command` record are that run's transaction, and
/// the command line is its `command`. Records that follow no `command`
/// record at all (a history written by another libzypp front end) have no
/// such boundary: of those, only the last group whose consecutive records
/// are in the same or the next minute is taken.
///
/// Limit: another front end (PackageKit, YaST) writes no `command` record,
/// so its changes made after a zypper run are listed with that run.
pub fn last_zypper_transaction(history: &str) -> Option<Transaction> {
    let run = pkglog::parse_zypp_runs(history)
        .into_iter()
        .rev()
        .find(|r| !r.entries.is_empty())?;
    let mut entries = run.entries;
    let mut when = run.when;
    if run.command.is_empty() {
        let same_run = |a: &LogEntry, b: &LogEntry| {
            a.ts != 0 && b.ts != 0 && (a.ts / 60).abs_diff(b.ts / 60) <= 1
        };
        let mut start = entries.len() - 1;
        while start > 0 && same_run(&entries[start - 1], &entries[start]) {
            start -= 1;
        }
        if start > 0 {
            when = crate::util::fmt_utc(entries[start].ts).replace(" UTC", " (local)");
            entries.drain(..start);
        }
    }
    Some(Transaction {
        when,
        command: run.command,
        entries,
    })
}

/// Zypper global options that take their value as the next word (the
/// `--opt=value` form is one word and needs no entry).
const ZYPPER_VALUE_OPTIONS: &[&str] = &[
    "-R",
    "--root",
    "-C",
    "--config",
    "-D",
    "--reposd-dir",
    "--userdata",
    "-c",
    "--cache-dir",
    "--raw-cache-dir",
    "--solv-cache-dir",
    "--pkg-cache-dir",
    "--plus-content",
    "-p",
    "--plus-repo",
];

/// The zypper verb of a recorded command line: the first word after the
/// program that is neither an option nor the value of one.
fn zypper_verb(command: &str) -> &str {
    let mut words = command.split_whitespace().skip(1);
    while let Some(w) = words.next() {
        if ZYPPER_VALUE_OPTIONS.contains(&w) {
            words.next();
        } else if !w.starts_with('-') {
            return w;
        }
    }
    ""
}

/// Names a zypper upgrade command (`update`, `dup`, `patch`, ...) is recorded
/// as having "installed" with no earlier record. libzypp writes an upgrade
/// as an install, so such a package may be an upgrade of one installed
/// before the history began; undoing it as an install would remove it.
/// Empty for any other command, whose install records are taken as installs.
///
/// Known gap: a transaction with no recorded command (PackageKit, YaST,
/// GNOME Software) gives an empty list here, so it is not refused even
/// though it may be an upgrade; the caller prints a note instead. The
/// command is unknown, so refusing every such install would block ordinary
/// installs too. STATUS.md states this.
pub fn zypper_uncertain_installs(command: &str, entries: &[LogEntry]) -> Vec<String> {
    let verb = zypper_verb(command);
    if !matches!(
        verb,
        "up" | "update" | "dup" | "dist-upgrade" | "patch" | "inr" | "install-new-recommends"
    ) {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|e| e.action == Action::Install)
        .map(|e| e.name.clone())
        .collect()
}

/// Whether the table of `zypper search -s --match-exact NAME` has a row for
/// the package `name` at `version` that comes from a repository (the row of
/// the installed copy, repository `(System Packages)`, does not count).
pub fn zypper_search_lists(output: &str, name: &str, version: &str) -> bool {
    output.lines().any(|line| {
        let c: Vec<&str> = line.split('|').map(str::trim).collect();
        c.len() >= 6
            && c[1] == name
            && c[2] == "package"
            && c[3] == version
            && !c[5].is_empty()
            && c[5] != "(System Packages)"
    })
}

/// From what `rpm -e --test NAME...` prints (`LC_ALL=C`): the installed
/// packages that need one of the named ones and are not named themselves,
/// each once, in rpm's order. Empty when rpm complained about nothing else.
pub fn rpm_erase_dependents(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let Some((_, pkg)) = line.split_once(" is needed by (installed) ") else {
            continue;
        };
        let pkg = pkg.trim();
        if !pkg.is_empty() && !out.iter().any(|p| p == pkg) {
            out.push(pkg.to_string());
        }
    }
    out
}

// ------------------------------------------------- where old versions are

/// The file of `name` at `version` among the names in a pacman cache.
pub fn pacman_cache_match(files: &[String], name: &str, version: &str) -> Option<String> {
    let prefix = format!("{name}-{version}-");
    files
        .iter()
        .find(|f| {
            f.starts_with(&prefix)
                && !f.ends_with(".sig")
                && f[prefix.len()..].contains(".pkg.tar")
                // what follows the version is exactly "<arch>.pkg.tar.<ext>"
                && !f[prefix.len()..].split(".pkg.tar").next().unwrap_or("").contains('-')
        })
        .cloned()
}

/// Arch Linux Archive addresses to try for a package file, most likely first.
pub fn arch_archive_urls(name: &str, version: &str, arch: &str) -> Vec<String> {
    let Some(first) = name.chars().next() else {
        return vec![];
    };
    [arch, "any"]
        .iter()
        .map(|a| {
            format!(
                "https://archive.archlinux.org/packages/{first}/{name}/{name}-{}-{a}.pkg.tar.zst",
                version.replace(':', "%3A")
            )
        })
        .collect()
}

/// The `.deb` file name apt gives a package in its cache.
pub fn deb_file_name(name: &str, version: &str, arch: &str) -> String {
    format!("{name}_{}_{arch}.deb", version.replace(':', "%3a"))
}

/// Launchpad addresses to try for an Ubuntu package (no epoch in file names).
pub fn launchpad_urls(name: &str, version: &str, arch: &str) -> Vec<String> {
    let bare = version.split_once(':').map(|(_, v)| v).unwrap_or(version);
    [arch, "all"]
        .iter()
        .map(|a| {
            format!("https://launchpad.net/ubuntu/+archive/primary/+files/{name}_{bare}_{a}.deb")
        })
        .collect()
}

/// From snapshot.debian.org's `binfiles` answer: the SHA-1 of the file for
/// `arch`, or for `all`.
pub fn snapshot_hash(json: &serde_json::Value, arch: &str) -> Option<String> {
    let files = json.get("result")?.as_array()?;
    let pick = |want: &str| {
        files.iter().find_map(|f| {
            (f.get("architecture")?.as_str()? == want)
                .then(|| f.get("hash")?.as_str().map(String::from))
                .flatten()
        })
    };
    pick(arch)
        .or_else(|| pick("all"))
        .filter(|h| h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// From the same answer asked with `?fileinfo=1`: the size in bytes of the
/// file with that hash. Pure.
pub fn snapshot_size(json: &serde_json::Value, hash: &str) -> Option<u64> {
    json.get("fileinfo")?
        .get(hash)?
        .as_array()?
        .iter()
        .find_map(|f| f.get("size")?.as_u64())
}

/// The name of the `.deb` the archive holds under `hash`, for the file the
/// undo downloads: the archive's own file name (`fileinfo`), when it is the
/// plain name of a `.deb` of this package; else the apt cache name with the
/// architecture the archive gave for that hash (an `Architecture: all`
/// package is `..._all.deb`, not `..._amd64.deb`). Pure.
pub fn snapshot_file_name(
    json: &serde_json::Value,
    hash: &str,
    name: &str,
    version: &str,
    arch: &str,
) -> String {
    let plain = |f: &str| {
        f.starts_with(&format!("{name}_"))
            && f.ends_with(".deb")
            && f.len() <= 255
            && f.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'~' | b'%' | b'-')
            })
    };
    let from_archive = json
        .get("fileinfo")
        .and_then(|i| i.get(hash))
        .and_then(|f| f.as_array())
        .and_then(|files| {
            files
                .iter()
                .filter_map(|f| f.get("name")?.as_str())
                .find(|f| plain(f))
        });
    if let Some(file) = from_archive {
        return file.to_string();
    }
    let archive_arch = json
        .get("result")
        .and_then(|r| r.as_array())
        .and_then(|files| {
            files.iter().find_map(|f| {
                (f.get("hash")?.as_str()? == hash)
                    .then(|| f.get("architecture")?.as_str())
                    .flatten()
            })
        })
        .filter(|a| ops::valid_deb_arch(a))
        .unwrap_or(arch);
    deb_file_name(name, version, archive_arch)
}

/// The directory undo downloads go under: `$XDG_CACHE_HOME/yapacc/undo`, or
/// `$HOME/.cache/yapacc/undo`. None when neither variable holds an absolute
/// path: a directory under `/tmp` could be prepared by any local user, so
/// there is no fallback. Pure.
pub fn download_parent(
    xdg_cache_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let absolute = |v: Option<std::ffi::OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg_cache_home).or_else(|| absolute(home).map(|h| h.join(".cache")))?;
    Some(base.join("yapacc").join("undo"))
}

/// How long a download directory left behind by a killed run is kept.
const STALE_RUN_SECS: u64 = 24 * 3600;

/// Delete the `run-*` directories under `parent` that are older than
/// `max_age_secs`: what a killed run could not delete itself.
fn sweep_stale_runs(parent: &Path, max_age_secs: u64) {
    let Ok(rd) = std::fs::read_dir(parent) else {
        return;
    };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with("run-") {
            continue;
        }
        // symlink_metadata: a link is never followed or deleted through
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() >= max_age_secs);
        if meta.is_dir() && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Create `dir` as a new, empty directory that only this user can read or
/// write. An existing path of that name is an error and is never reused, so
/// nothing in the directory was put there by anyone else. Its parent is
/// created when missing, must be this user's own directory, and is closed
/// to others.
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let cannot = |what: &Path, e: &dyn std::fmt::Display| {
        Error::from(format!(
            "cannot create {}: {e}. Nothing was changed.",
            what.display()
        ))
    };
    let parent = dir
        .parent()
        .ok_or_else(|| cannot(dir, &"it has no parent directory"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|e| cannot(parent, &e))?;
    // not recursive: fails if anything has that name already
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|e| cannot(dir, &e))?;
    // the parent must be this user's own, or the new directory could be
    // swapped for another one between the download and the install
    let ours = std::fs::symlink_metadata(dir).map_err(|e| cannot(dir, &e))?;
    let up = std::fs::symlink_metadata(parent).map_err(|e| cannot(parent, &e))?;
    if !up.is_dir() || up.uid() != ours.uid() {
        let _ = std::fs::remove_dir(dir);
        return Err(Error::refused(format!(
            "cannot undo: the download directory {} is not a directory owned by this user, so a file downloaded there could be replaced. Nothing was changed.",
            parent.display()
        )));
    }
    if up.mode() & 0o077 != 0 {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| cannot(parent, &e))?;
    }
    Ok(())
}

/// How a file from an archive is checked once it is downloaded.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Check {
    /// Arch Linux Archive: the detached signature at `URL.sig`, verified
    /// with `pacman-key --verify`. No signature, no install.
    PacmanSignature,
    /// snapshot.debian.org: the SHA-1 its index gave for the file
    Sha1(String),
    /// Launchpad: nothing beyond TLS
    TlsOnly,
}

impl Check {
    /// What the plan says about the file's origin and its check.
    fn label(&self) -> &'static str {
        match self {
            Check::PacmanSignature => {
                "Arch Linux Archive; its signature is verified with pacman-key before it is installed"
            }
            Check::Sha1(_) => {
                "snapshot.debian.org over TLS, archive hash checked; NOT signature-verified"
            }
            Check::TlsOnly => "Launchpad over TLS; NOT signature-verified",
        }
    }
}

/// An old version an archive still has. Planning only asks the archive
/// whether it is there; the file is downloaded after the user agreed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Remote {
    url: String,
    /// the name the file gets in the download directory (no `/`)
    file: String,
    /// size in bytes, when the archive said
    size: Option<u64>,
    check: Check,
}

/// The files one undo downloads. Planning (and so every dry run) only
/// chooses the name of a directory for this run and writes nothing; `fetch`
/// creates the directory, after consent, and it is deleted with everything
/// in it when the run ends. A file from an earlier run, or from anyone
/// else, is therefore never installed.
struct Downloads {
    /// `<cache>/yapacc/undo/run-<pid>-<time>`; None when no cache directory
    /// is known (see `download_parent`)
    dir: Option<PathBuf>,
    created: bool,
}

impl Downloads {
    fn plan() -> Self {
        Self::under(download_parent(
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        ))
    }

    fn under(parent: Option<PathBuf>) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self {
            dir: parent.map(|p| p.join(format!("run-{}-{stamp:x}", std::process::id()))),
            created: false,
        }
    }

    /// Where `file` will be once it is downloaded. Creates nothing.
    fn path(&self, file: &str) -> Result<PathBuf> {
        let dir = self.dir.as_ref().ok_or_else(|| {
            Error::refused(
                "cannot undo: an old version has to be downloaded, and neither XDG_CACHE_HOME nor HOME names a directory to download it into (yapacc does not use /tmp for packages it installs). Set HOME and run again. Nothing was changed.",
            )
        })?;
        if file.is_empty() || file.contains('/') || file.starts_with('.') {
            return Err(Error::refused(format!(
                "cannot undo: '{file}' is not a file name yapacc will download to. Nothing was changed."
            )));
        }
        Ok(dir.join(file))
    }

    /// Download and check every `Found::Remote` of `restores`. Call after
    /// consent and before the first native command: on any error nothing
    /// has been changed. Does nothing, and creates nothing, when no version
    /// comes from an archive.
    fn fetch(&mut self, restores: &[(String, String, Found)]) -> Result<()> {
        for (name, version, found) in restores {
            let Found::Remote(remote) = found else {
                continue;
            };
            let dest = self.path(&remote.file)?;
            if !self.created
                && let Some(dir) = &self.dir
            {
                create_private_dir(dir)?;
                self.created = true;
                if let Some(parent) = dir.parent() {
                    sweep_stale_runs(parent, STALE_RUN_SECS);
                }
            }
            println!("download: {}", remote.url);
            fetch_one(name, version, remote, &dest)?;
        }
        Ok(())
    }
}

impl Drop for Downloads {
    fn drop(&mut self) {
        if let (true, Some(dir)) = (self.created, &self.dir) {
            let _ = std::fs::remove_dir_all(dir);
            remove_empty_parents(dir);
        }
    }
}

/// After a run directory is gone: remove `undo/` when this run leaves it
/// empty, and then `yapacc/` when that leaves it empty too, so that an undo
/// leaves no empty directory behind in the user's cache. `remove_dir` only
/// removes an empty directory, and only directories with these two names
/// are tried: the cache directory itself is never touched.
fn remove_empty_parents(run_dir: &Path) {
    let Some(undo) = run_dir.parent() else {
        return;
    };
    if undo.file_name().is_none_or(|n| n != "undo") || std::fs::remove_dir(undo).is_err() {
        return;
    }
    if let Some(yapacc) = undo.parent()
        && yapacc.file_name().is_some_and(|n| n == "yapacc")
    {
        let _ = std::fs::remove_dir(yapacc);
    }
}

/// Download one archive file to `dest` and check it as `remote.check` says.
/// A failed download is a refusal; a missing or rejected signature and a
/// wrong hash are integrity failures. Either way the file is deleted.
fn fetch_one(name: &str, version: &str, remote: &Remote, dest: &Path) -> Result<()> {
    if let Err(e) = curl_to(&remote.url, dest) {
        return Err(Error::refused(format!(
            "cannot undo: {name} {version} could not be downloaded from {} ({e}). Nothing was changed.",
            remote.url
        )));
    }
    match &remote.check {
        Check::PacmanSignature => {
            // the signature stays beside the file, so `pacman -U` checks it
            // again when it installs
            let mut sig = dest.as_os_str().to_owned();
            sig.push(".sig");
            let sig = PathBuf::from(sig);
            let refuse = |why: &str, said: &str| {
                let _ = std::fs::remove_file(dest);
                let _ = std::fs::remove_file(&sig);
                if !said.is_empty() {
                    eprintln!("{said}");
                }
                arch_signature_refusal(name, version, why)
            };
            if let Err(e) = curl_to(&format!("{}.sig", remote.url), &sig) {
                return Err(refuse("its signature could not be fetched", &e));
            }
            if let Err(said) = pacman_key_verifies(&sig, dest) {
                return Err(refuse(
                    "`pacman-key --verify` did not accept its signature",
                    &said,
                ));
            }
        }
        Check::Sha1(hash) => {
            let sum = Command::new("sha1sum")
                .arg("--")
                .arg(dest)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            if sum.split_whitespace().next() != Some(hash.as_str()) {
                let _ = std::fs::remove_file(dest);
                return Err(Error::integrity(format!(
                    "the file snapshot.debian.org returned for {name} {version} does not match its own hash; the file was deleted and nothing was changed"
                )));
            }
        }
        Check::TlsOnly => {}
    }
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Download to `dest.part` and rename on success, so `dest` is never a
/// truncated file. On failure returns curl's own message.
fn curl_to(url: &str, dest: &Path) -> std::result::Result<(), String> {
    let part = part_path(dest);
    let _ = std::fs::remove_file(&part);
    let out = Command::new("curl")
        .args(["-sSL", "--fail", "--max-time", "300", "-o"])
        .arg(&part)
        .arg("--")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if out.status.success() {
        std::fs::rename(&part, dest).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("cannot move {} into place: {e}", part.display())
        })
    } else {
        let _ = std::fs::remove_file(&part);
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn curl_missing() -> Error {
    Error::refused(
        "cannot undo: the old version is not in any local cache and curl is not installed, so yapacc cannot fetch it from an archive. Install curl and run again. Nothing was changed.",
    )
}

/// What an archive said when asked whether it has a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup<T> {
    /// it has it
    Found(T),
    /// it answered that it does not (HTTP 404 or 410)
    Absent,
    /// it did not answer, or answered with an error: curl's own message
    NoAnswer(String),
}

/// Read one `curl --fail` run: success is `Found`; "The requested URL
/// returned error: 404" (or 410) is the archive saying it does not have the
/// file; anything else (a timeout, a refused connection, a 5xx) is no
/// answer. Pure.
pub fn classify_lookup<T>(success: bool, found: T, stderr: &str) -> Lookup<T> {
    if success {
        return Lookup::Found(found);
    }
    let said = stderr.trim();
    if said.contains("returned error: 404") || said.contains("returned error: 410") {
        Lookup::Absent
    } else if said.is_empty() {
        Lookup::NoAnswer("curl failed without a message".into())
    } else {
        Lookup::NoAnswer(said.to_string())
    }
}

/// How long an archive lookup waits: 20 s for the connection, 60 s in all.
const LOOKUP_TIMEOUTS: [&str; 4] = ["--connect-timeout", "20", "--max-time", "60"];

/// Ask once, and once more when the archive did not answer: a slow
/// archive is asked twice at most, and "it does not have it" is never
/// asked again. Pure over `ask`.
pub fn ask_twice<T>(mut ask: impl FnMut() -> Lookup<T>) -> Lookup<T> {
    match ask() {
        Lookup::NoAnswer(_) => ask(),
        answered => answered,
    }
}

/// The text at `url`, when the archive has it.
fn curl_text(url: &str) -> Lookup<String> {
    ask_twice(|| {
        let run = Command::new("curl")
            .args(["-sSL", "--fail"])
            .args(LOOKUP_TIMEOUTS)
            .args(["--", url])
            .stdin(std::process::Stdio::null())
            .output();
        match run {
            Ok(out) => classify_lookup(
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).to_string(),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(e) => Lookup::NoAnswer(format!("cannot run curl: {e}")),
        }
    })
}

/// The refusal when an archive did not answer: said apart from "the archive
/// does not have it", because trying again later can succeed. Pure.
pub fn archive_silent(archive: &str, name: &str, version: &str, said: &str) -> Error {
    Error::refused(format!(
        "cannot undo: {archive} did not answer when asked for {name} {version} ({said}), so it is not known whether it has that version. Nothing was changed. Try again later."
    ))
}

/// The `Content-Length` of the last answer in the headers `curl -I -L`
/// printed (one block for each redirect; the last is the file's). Pure.
pub fn content_length(headers: &str) -> Option<u64> {
    let text = headers.replace("\r\n", "\n");
    let last = text.split("\n\n").filter(|b| !b.trim().is_empty()).last()?;
    last.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse().ok())
            .flatten()
    })
}

/// Ask whether `url` exists, without downloading it (a HEAD request).
/// `Found` carries the size when the server gave one.
fn curl_head(url: &str) -> Lookup<Option<u64>> {
    ask_twice(|| {
        let run = Command::new("curl")
            .args(["-sSIL", "--fail"])
            .args(LOOKUP_TIMEOUTS)
            .args(["--", url])
            .stdin(std::process::Stdio::null())
            .output();
        match run {
            Ok(out) => classify_lookup(
                out.status.success(),
                content_length(&String::from_utf8_lossy(&out.stdout)),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(e) => Lookup::NoAnswer(format!("cannot run curl: {e}")),
        }
    })
}

/// Where one old version was found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Found {
    /// a file already on disk (the native cache), and what it is
    File(PathBuf, &'static str),
    /// the configured repositories still offer it (apt and zypper `name=version`)
    Repo,
    /// an archive has it: downloaded and checked after consent
    Remote(Remote),
}

// ---------------------------------------------------------------- zypper

/// `Found::Repo` when the repository metadata already on this machine lists
/// `name` at `version`. Nothing is refreshed and nothing is downloaded.
fn zypper_find(name: &str, version: &str) -> Option<Found> {
    // without the metadata of every enabled repository on disk zypper would
    // download it, and a dry run downloads nothing: the version is then
    // "not found", and the undo is refused
    if crate::native::missing_metadata(PkgManager::Zypper).is_some() {
        return None;
    }
    let out = Command::new("zypper")
        .args([
            "--non-interactive",
            "--no-refresh",
            "search",
            "-s",
            "--match-exact",
            name,
        ])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    zypper_search_lists(&String::from_utf8_lossy(&out.stdout), name, version).then_some(Found::Repo)
}

// ---------------------------------------------------------------- pacman

/// Check a downloaded Arch package against its detached signature with the
/// system's pacman keyring (`pacman-key --verify`: the signing key must be
/// trusted there). Changes nothing and works as an ordinary user. `Err`
/// carries what pacman-key printed.
fn pacman_key_verifies(sig: &Path, file: &Path) -> std::result::Result<(), String> {
    let out = Command::new("pacman-key")
        .arg("--verify")
        .arg(sig)
        .arg(file)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run pacman-key: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .trim()
    .to_string())
}

/// Why a package downloaded from the Arch Linux Archive is not installed.
/// An integrity failure (exit 5): the file is never offered to pacman,
/// whose default `LocalFileSigLevel = Optional` installs a file that has no
/// signature beside it. Pure.
fn arch_signature_refusal(name: &str, version: &str, why: &str) -> Error {
    Error::integrity(format!(
        "cannot undo: {name} {version} was downloaded from the Arch Linux Archive but {why}, so yapacc will not install it. The downloaded file was deleted. Nothing was changed."
    ))
}

/// Find `name` at `version`: in pacman's cache, else (when `may_fetch`) in
/// the Arch Linux Archive, which is only asked whether it has the file and
/// its signature. Downloads nothing and writes nothing.
fn pacman_find(name: &str, version: &str, arch: &str, may_fetch: bool) -> Result<Option<Found>> {
    let cache = Path::new("/var/cache/pacman/pkg");
    let files: Vec<String> = std::fs::read_dir(cache)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    if let Some(f) = pacman_cache_match(&files, name, version) {
        return Ok(Some(Found::File(cache.join(f), "pacman cache")));
    }
    if !may_fetch {
        return Ok(None);
    }
    if !crate::util::have("curl") {
        return Err(curl_missing());
    }
    const ARCHIVE: &str = "archive.archlinux.org";
    for url in arch_archive_urls(name, version, arch) {
        let size = match curl_head(&url) {
            Lookup::Found(size) => size,
            Lookup::Absent => continue,
            Lookup::NoAnswer(said) => return Err(archive_silent(ARCHIVE, name, version, &said)),
        };
        // The detached signature is required: a package without one is
        // never handed to `pacman -U`, so say so now, not after the gate.
        match curl_head(&format!("{url}.sig")) {
            Lookup::Found(_) => {}
            Lookup::NoAnswer(said) => return Err(archive_silent(ARCHIVE, name, version, &said)),
            Lookup::Absent => {
                return Err(Error::integrity(format!(
                    "cannot undo: the Arch Linux Archive has {name} {version} but no signature for it, so yapacc will not install it. Nothing was changed."
                )));
            }
        }
        let file = url.rsplit('/').next().unwrap_or("pkg").replace("%3A", ":");
        return Ok(Some(Found::Remote(Remote {
            url,
            file,
            size,
            check: Check::PacmanSignature,
        })));
    }
    Ok(None)
}

// ------------------------------------------------------------------- apt

/// Whether what `apt-cache show NAME=VERSION` printed is the record of a
/// version a configured repository can deliver: only such a record has a
/// `Filename:` field. After a removal that kept the configuration files
/// (dpkg state `rc`) apt-cache still answers for the removed version, from
/// `/var/lib/dpkg/status`: that record has `Status:` and no `Filename:`, and
/// `apt-get install NAME=VERSION` cannot download it.
pub fn apt_show_is_downloadable(stdout: &str) -> bool {
    stdout.lines().any(|l| l.starts_with("Filename:"))
}

/// Find the old version of `target` (`name:arch` as apt's history wrote it;
/// `native` is dpkg's own architecture, used when the log gave none). The
/// package's own architecture goes into every lookup: the apt query, the
/// `.deb` file name, snapshot.debian.org and Launchpad. An archive is only
/// asked whether it has the file: nothing is downloaded or written.
fn apt_find(
    target: &str,
    version: &str,
    native: &str,
    os_id: &str,
    may_fetch: bool,
) -> Result<Option<Found>> {
    let (name, arch) = apt_split(target);
    let arch = if arch.is_empty() { native } else { arch };
    let spec = format!("{target}={version}");
    let in_repo = Command::new("apt-cache")
        .args(["show", "--", &spec])
        .env("LC_ALL", "C")
        .output()
        .map(|o| {
            o.status.success() && apt_show_is_downloadable(&String::from_utf8_lossy(&o.stdout))
        })
        .unwrap_or(false);
    if in_repo {
        return Ok(Some(Found::Repo));
    }
    for a in [arch, "all"] {
        let cached = Path::new("/var/cache/apt/archives").join(deb_file_name(name, version, a));
        if cached.exists() {
            return Ok(Some(Found::File(cached, "apt cache")));
        }
    }
    if !may_fetch || !matches!(os_id, "debian" | "ubuntu") {
        return Ok(None);
    }
    if !crate::util::have("curl") {
        return Err(curl_missing());
    }
    match os_id {
        "debian" => {
            // the index answers with the file's hash and size: no download
            let api = format!(
                "https://snapshot.debian.org/mr/binary/{name}/{}/binfiles?fileinfo=1",
                version.replace(':', "%3A")
            );
            let text = match curl_text(&api) {
                Lookup::Found(text) => text,
                Lookup::Absent => return Ok(None),
                Lookup::NoAnswer(said) => {
                    return Err(archive_silent("snapshot.debian.org", name, version, &said));
                }
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                return Err(archive_silent(
                    "snapshot.debian.org",
                    name,
                    version,
                    "its answer could not be read",
                ));
            };
            let Some(hash) = snapshot_hash(&json, arch) else {
                return Ok(None);
            };
            Ok(Some(Found::Remote(Remote {
                url: format!("https://snapshot.debian.org/file/{hash}"),
                file: snapshot_file_name(&json, &hash, name, version, arch),
                size: snapshot_size(&json, &hash),
                check: Check::Sha1(hash),
            })))
        }
        "ubuntu" => {
            for url in launchpad_urls(name, version, arch) {
                match curl_head(&url) {
                    Lookup::Found(size) => {
                        let file = url.rsplit('/').next().unwrap_or("pkg.deb").to_string();
                        return Ok(Some(Found::Remote(Remote {
                            url,
                            file,
                            size,
                            check: Check::TlsOnly,
                        })));
                    }
                    // the other address (the `_all.deb` name) may have it
                    Lookup::Absent => {}
                    // asked twice already: the other address is not tried,
                    // and the undo is refused as "not known", at once
                    Lookup::NoAnswer(said) => {
                        return Err(archive_silent("launchpad.net", name, version, &said));
                    }
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

// ------------------------------------------------------------- the command

/// The time of a transaction the way `timeline` prints one: `YYYY-MM-DD
/// HH:MM`, then `UTC` when the log gave an offset (pacman) or `(local)`
/// when it gave none (apt's history, the zypp history). Text in any other
/// form is returned as it is. Pure.
pub fn when_label(raw: &str) -> String {
    let raw = raw.trim();
    if raw.ends_with("(local)") || raw.ends_with("UTC") {
        return raw.to_string();
    }
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let (Some(date), Some(rest)) = (raw.get(..10), raw.get(10..)) else {
        return raw.to_string();
    };
    let rest = rest.trim_start_matches(['T', ' ']);
    let (Some(clock), Some(tail)) = (rest.get(..5), rest.get(5..)) else {
        return raw.to_string();
    };
    let date_ok = date.split('-').map(str::len).eq([4, 2, 2]) && date.split('-').all(digits);
    let clock_ok = clock.split(':').map(str::len).eq([2, 2]) && clock.split(':').all(digits);
    if !date_ok || !clock_ok {
        return raw.to_string();
    }
    // seconds, when the log has them
    let (seconds, zone) = match tail.strip_prefix(':') {
        Some(t) if t.get(..2).is_some_and(digits) => (&t[..2], &t[2..]),
        Some(_) => return raw.to_string(),
        None => ("00", tail),
    };
    if zone.is_empty() {
        return format!("{date} {clock} (local)");
    }
    // +HHMM or -HHMM: the moment is known, so it is given in UTC
    let offset = zone
        .get(1..)
        .filter(|z| z.len() == 4 && digits(z))
        .and_then(|z| Some(z[..2].parse::<u64>().ok()? * 3600 + z[2..].parse::<u64>().ok()? * 60));
    let local = pkglog::date_time(date, &format!("{clock}:{seconds}"));
    match (zone.as_bytes()[0], offset, local) {
        (b'+', Some(off), Some(t)) => crate::util::fmt_utc(t.saturating_sub(off)),
        (b'-', Some(off), Some(t)) => crate::util::fmt_utc(t + off),
        _ => raw.to_string(),
    }
}

fn describe(t: &Transaction) -> String {
    let when = when_label(&t.when);
    let mut s = format!(
        "Last transaction: {}{}\n",
        if when.is_empty() {
            "(no time recorded)"
        } else {
            &when
        },
        if t.command.is_empty() {
            String::new()
        } else {
            format!("  ({})", t.command)
        }
    );
    for e in &t.entries {
        let what = match e.action {
            Action::Install => format!("installed   {} {}", e.name, e.new_version),
            Action::Remove => format!("removed     {} {}", e.name, e.old_version),
            Action::Upgrade => format!(
                "upgraded    {} {} -> {}",
                e.name, e.old_version, e.new_version
            ),
            Action::Downgrade => {
                format!(
                    "downgraded  {} {} -> {}",
                    e.name, e.old_version, e.new_version
                )
            }
            Action::Reinstall => format!("reinstalled {} {}", e.name, e.new_version),
        };
        s.push_str(&format!("  {what}\n"));
    }
    s
}

const LIMITS: &str = "Undo restores package versions only. It does not restore configuration files or data that package scripts changed; a filesystem snapshot does.";

/// Names to remove, and (name, version, where it was found) to restore.
type Resolved = (Vec<String>, Vec<(String, String, Found)>);

/// Resolve every step, or refuse naming what cannot be found.
fn resolve(
    steps: &[Step],
    mut find: impl FnMut(&str, &str) -> Result<Option<Found>>,
) -> Result<Resolved> {
    let mut removes = Vec::new();
    let mut restores = Vec::new();
    let mut missing = Vec::new();
    for step in steps {
        match step {
            Step::Remove { name } => removes.push(name.clone()),
            Step::Restore { name, version } => match find(name, version)? {
                Some(found) => restores.push((name.clone(), version.clone(), found)),
                None => missing.push(format!("{name} {version}")),
            },
        }
    }
    if !missing.is_empty() {
        return Err(Error::refused(format!(
            "cannot undo: the old version of {} is in no local cache and no configured repository, and the archive of this distribution, where yapacc asks one, does not have it. Nothing was changed. (A filesystem snapshot from before the transaction can still take it back.)",
            missing.join(", ")
        )));
    }
    Ok((removes, restores))
}

/// One line of the plan for a version an archive has. Pure.
fn remote_line(name: &str, version: &str, remote: &Remote) -> String {
    let size = match remote.size {
        Some(bytes) => crate::clean::human(bytes),
        None => "size not given".to_string(),
    };
    format!(
        "  restore {name} {version}  (to download, {size}: {}; {})",
        remote.url,
        remote.check.label()
    )
}

/// The versions of `restores` that come from an archive.
fn remotes(restores: &[(String, String, Found)]) -> Vec<&Remote> {
    restores
        .iter()
        .filter_map(|(_, _, found)| match found {
            Found::Remote(r) => Some(r),
            _ => None,
        })
        .collect()
}

/// What is said about the files to download: how many, how much, and that
/// nothing is fetched before the user agreed. None when there are none.
/// Pure.
fn downloads_note(remotes: &[&Remote]) -> Option<String> {
    if remotes.is_empty() {
        return None;
    }
    let known: u64 = remotes.iter().filter_map(|r| r.size).sum();
    let total = if remotes.iter().all(|r| r.size.is_some()) {
        format!(" ({})", crate::clean::human(known))
    } else {
        String::new()
    };
    Some(format!(
        "note: {} file{}{total} to download. Nothing has been downloaded: that happens after you agree and before anything is changed, into a directory made for that run; each file is checked as said above, a failed download or check stops the undo with nothing changed, and the files are deleted when the undo ends.",
        remotes.len(),
        if remotes.len() == 1 { "" } else { "s" },
    ))
}

fn show_sources(restores: &[(String, String, Found)]) {
    for (name, version, found) in restores {
        match found {
            Found::Repo => {
                println!("  restore {name} {version}  (from the configured repositories)")
            }
            Found::File(path, how) => {
                println!(
                    "  restore {name} {version}  (from {}: {how})",
                    path.display()
                )
            }
            Found::Remote(remote) => println!("{}", remote_line(name, version, remote)),
        }
    }
}

/// `[safety] snapshot` as a mode. A value that cannot be read is an error,
/// never "off": the undo must not run without the snapshot the user set.
fn snapshot_mode(value: &str) -> Result<crate::snapshot::Mode> {
    crate::snapshot::Mode::parse(value).map_err(|e| Error::usage(format!("[safety] snapshot: {e}")))
}

/// What a snapshot taken before the undo is described as.
const SNAPSHOT_WHAT: &str = "undo of the last package transaction";

/// The question of the gate; it names the snapshot when one is taken first.
/// Pure.
fn gate_question(snapshot_tool: Option<&str>) -> String {
    match snapshot_tool {
        Some(tool) => format!("Undo this transaction (a {tool} snapshot is taken first)"),
        None => "Undo this transaction".to_string(),
    }
}

// ------------------------------------------- the state an undo should leave

/// What an undo is expected to leave for one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expected {
    /// `name` installed at `version`
    Present { name: String, version: String },
    /// `name` not installed at `version`; with no version, not installed
    /// at all (a package that keeps several versions, like a kernel, loses
    /// only the one that was installed)
    Absent {
        name: String,
        version: Option<String>,
    },
}

/// The state the plan of an undo leads to: every restore present at its
/// version, every removal absent. A removal is `name`, `name:arch` (apt)
/// or `name=version` (zypper, one version of several). Pure.
pub fn expected_from_plan(removes: &[String], restores: &[(String, String)]) -> Vec<Expected> {
    let mut out: Vec<Expected> = restores
        .iter()
        .map(|(name, version)| Expected::Present {
            name: name.clone(),
            version: version.clone(),
        })
        .collect();
    out.extend(removes.iter().map(|r| match r.split_once('=') {
        Some((name, version)) => Expected::Absent {
            name: name.to_string(),
            version: Some(version.to_string()),
        },
        None => Expected::Absent {
            name: r.clone(),
            version: None,
        },
    }));
    out
}

/// The state the reverse of a transaction leads to, from its entries (dnf
/// undoes its own transactions, so there is no plan of yapacc's to read):
/// an installed version is gone, a removed, upgraded or downgraded package
/// is back at its old version. Entries without the version needed, and
/// reinstalls, expect nothing. Pure.
pub fn expected_from_entries(entries: &[LogEntry]) -> Vec<Expected> {
    entries
        .iter()
        .filter_map(|e| match e.action {
            Action::Install if !e.new_version.is_empty() => Some(Expected::Absent {
                name: e.name.clone(),
                version: Some(e.new_version.clone()),
            }),
            Action::Remove | Action::Upgrade | Action::Downgrade if !e.old_version.is_empty() => {
                Some(Expected::Present {
                    name: e.name.clone(),
                    version: e.old_version.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

/// The installed versions of every package, by name.
pub type Installed = std::collections::HashMap<String, Vec<String>>;

/// A version without the epoch rpm and dnf write when there is none.
fn bare_epoch(version: &str) -> &str {
    let v = version.strip_prefix("(none):").unwrap_or(version);
    v.strip_prefix("0:").unwrap_or(v)
}

/// Read the installed packages from what the manager's list command
/// printed. Pure.
///
/// - pacman, `pacman -Q`: `NAME VERSION`
/// - apt, `dpkg-query -W -f '${Package}\t${Architecture}\t${Version}\t${db:Status-Abbrev}\n'`:
///   rows whose state is `ii`, under `NAME` and under `NAME:ARCH`
/// - dnf and zypper, `rpm -qa --qf '%{NAME}\t%{EVR}\t%{VERSION}-%{RELEASE}\n'`:
///   both forms of the version, since the zypp history writes no epoch
pub fn parse_installed_state(m: PkgManager, text: &str) -> Installed {
    let mut state = Installed::new();
    let mut add = |name: &str, version: &str| {
        let versions = state.entry(name.to_string()).or_default();
        let version = bare_epoch(version).to_string();
        if !version.is_empty() && !versions.contains(&version) {
            versions.push(version);
        }
    };
    for line in text.lines() {
        match m {
            PkgManager::Pacman => {
                let mut w = line.split_whitespace();
                if let (Some(name), Some(version)) = (w.next(), w.next()) {
                    add(name, version);
                }
            }
            PkgManager::Apt => {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() == 4 && f[3].starts_with("ii") && !f[0].is_empty() {
                    add(f[0], f[2]);
                    add(&format!("{}:{}", f[0], f[1]), f[2]);
                }
            }
            _ => {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() == 3 && !f[0].is_empty() {
                    add(f[0], f[1]);
                    add(f[0], f[2]);
                }
            }
        }
    }
    state
}

/// apt's history writes an `Architecture: all` package with the system's
/// own architecture (`vim-common:amd64`), and dpkg lists it as `all`: make
/// such a package answer under `NAME:NATIVE` too. Pure.
pub fn alias_arch_all(state: &mut Installed, native: &str) {
    let aliases: Vec<(String, Vec<String>)> = state
        .iter()
        .filter_map(|(key, versions)| {
            let name = key.strip_suffix(":all")?;
            Some((format!("{name}:{native}"), versions.clone()))
        })
        .collect();
    for (key, versions) in aliases {
        let known = state.entry(key).or_default();
        for v in versions {
            if !known.contains(&v) {
                known.push(v);
            }
        }
    }
}

/// The installed packages of manager `m`, or `None` when they cannot be
/// listed. Reads only.
fn installed_state(m: PkgManager) -> Option<Installed> {
    let (prog, args): (&str, &[&str]) = match m {
        PkgManager::Pacman => ("pacman", &["-Q"]),
        PkgManager::Apt => (
            "dpkg-query",
            &[
                "-W",
                "-f=${Package}\t${Architecture}\t${Version}\t${db:Status-Abbrev}\n",
            ],
        ),
        PkgManager::Dnf | PkgManager::Zypper => (
            "rpm",
            &["-qa", "--qf", "%{NAME}\\t%{EVR}\\t%{VERSION}-%{RELEASE}\\n"],
        ),
        _ => return None,
    };
    let out = Command::new(prog)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut state = parse_installed_state(m, &String::from_utf8_lossy(&out.stdout));
    if m == PkgManager::Apt
        && let Ok(arch) = Command::new("dpkg").arg("--print-architecture").output()
    {
        alias_arch_all(&mut state, String::from_utf8_lossy(&arch.stdout).trim());
    }
    Some(state)
}

/// The versions of one package as they are shown: a version that is only
/// another one written without its epoch is left out. Pure.
fn shown_versions(have: &[String]) -> String {
    have.iter()
        .filter(|v| !have.iter().any(|other| other.ends_with(&format!(":{v}"))))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many of `expected` hold in `state`, and a sentence for each that
/// does not. Pure.
pub fn verify_state(expected: &[Expected], state: &Installed) -> (usize, Vec<String>) {
    let none: Vec<String> = Vec::new();
    let mut ok = 0;
    let mut wrong = Vec::new();
    for e in expected {
        match e {
            Expected::Present { name, version } => {
                let have = state.get(name).unwrap_or(&none);
                if have.iter().any(|v| v == bare_epoch(version)) {
                    ok += 1;
                } else if have.is_empty() {
                    wrong.push(format!("{name} is not installed (planned: {version})"));
                } else {
                    wrong.push(format!(
                        "{name} is at {} (planned: {version})",
                        shown_versions(have)
                    ));
                }
            }
            Expected::Absent { name, version } => {
                let have = state.get(name).unwrap_or(&none);
                match version {
                    Some(v) if have.iter().any(|h| h == bare_epoch(v)) => {
                        wrong.push(format!("{name} {v} is still installed"))
                    }
                    None if !have.is_empty() => wrong.push(format!(
                        "{name} is still installed ({})",
                        shown_versions(have)
                    )),
                    _ => ok += 1,
                }
            }
        }
    }
    (ok, wrong)
}

/// At most eight of `wrong`, then how many more. Pure.
fn listed(wrong: &[String]) -> String {
    let shown: Vec<&str> = wrong.iter().take(8).map(String::as_str).collect();
    match wrong.len() - shown.len() {
        0 => shown.join("; "),
        more => format!("{}; and {more} more", shown.join("; ")),
    }
}

/// What is said after an undo whose native command failed, from the state
/// the packages are in: `checked` is `verify_state`'s answer, or `None`
/// when the installed packages could not be listed. Pure.
pub fn failed_undo_outcome(manager: &str, checked: Option<&(usize, Vec<String>)>) -> String {
    match checked {
        Some((ok, wrong)) if wrong.is_empty() && *ok > 0 => format!(
            "The package versions were restored ({ok} of {ok} verified), but {manager} reported an error (often a package script that failed): see the {manager} output above."
        ),
        Some((ok, wrong)) if !wrong.is_empty() => format!(
            "{} of {} package(s) are not at the planned state: {}. `yapacc timeline` shows the current state.",
            wrong.len(),
            ok + wrong.len(),
            listed(wrong)
        ),
        _ => "`yapacc timeline` shows the current state.".to_string(),
    }
}

/// What is said after an undo whose commands all succeeded: `Ok` with the
/// "verified" line, or `Err` with what is not at the planned state (the
/// commands reported no error, but the undo is not done). Pure.
pub fn succeeded_undo_outcome(
    checked: Option<&(usize, Vec<String>)>,
) -> std::result::Result<String, String> {
    match checked {
        Some((ok, wrong)) if wrong.is_empty() => Ok(format!(
            "Verified: {ok} of {ok} package(s) are at the planned state."
        )),
        Some((ok, wrong)) => Err(format!(
            "the undo commands reported no error, but {} of {} package(s) are not at the planned state: {}. `yapacc timeline` shows the current state.",
            wrong.len(),
            ok + wrong.len(),
            listed(wrong)
        )),
        None => Ok(
            "note: the installed packages could not be listed afterwards, so the result was not verified."
                .to_string(),
        ),
    }
}

/// Show the commands, ask, and run them. `after_consent` is the one place
/// where files are downloaded: it runs after the gate and before the
/// snapshot and the first command, and never in a dry run, which returns
/// before the gate having written nothing.
///
/// `expected` is the state the undo should leave. It is compared with the
/// installed packages after the commands ran: a failed command whose
/// packages are all at the planned state says so (dnf exits 1 when a
/// scriptlet fails after every version was restored), and a success is
/// reported with the number of packages verified.
fn finish(
    m: PkgManager,
    commands: &[Vec<String>],
    expected: &[Expected],
    g: GateFlags,
    after_consent: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    // an undo is a change like any other: `[safety] snapshot` applies to it
    let mode = snapshot_mode(&crate::config::get().safety.snapshot)?;
    let eligible = crate::snapshot::manager_is_snapshotted(m);
    let root = ops::elevation_for(g);
    let shown_prefix = root
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| vec!["<root>".into()]);
    println!("\nWill run:");
    for c in commands {
        println!("  {}", ops::shown(&shown_prefix, c));
    }
    // what happens about the snapshot is part of the plan; a configured
    // tool that is not usable refuses here, before the gate
    let snap = crate::snapshot::announce(mode, eligible, &shown_prefix, SNAPSHOT_WHAT, g.dryrun)?;
    println!("note: {LIMITS}");
    if g.dryrun {
        println!("\n{}", crate::prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    ops::gate(&gate_question(snap), g)?;
    let prefix = root?;
    after_consent()?;
    // after consent, before the first command; a failed snapshot under
    // `snapshot = "auto"` stops the undo with nothing changed
    crate::snapshot::before_change(mode, g.dryrun, eligible, &prefix, SNAPSHOT_WHAT)?;
    let check = || installed_state(m).map(|state| verify_state(expected, &state));
    for c in commands {
        let code = ops::run_attached(&prefix, c, true)?;
        if code != 0 {
            return Err(Error::from(format!(
                "{} exited with status {code}; later steps were not run. {}{}",
                ops::shown(&prefix, c),
                failed_undo_outcome(m.name(), check().as_ref()),
                ops::elevation_hint(&prefix, code)
            )));
        }
    }
    match succeeded_undo_outcome(check().as_ref()) {
        Ok(verified) => {
            println!("\n{}\n{verified}", closing_line(commands.len()));
            Ok(())
        }
        Err(why) => Err(Error::from(why)),
    }
}

/// The plan of an undo as [`expected_from_plan`] takes it.
fn plan_state(removes: &[String], restores: &[(String, String, Found)]) -> Vec<Expected> {
    let restores: Vec<(String, String)> = restores
        .iter()
        .map(|(name, version, _)| (name.clone(), version.clone()))
        .collect();
    expected_from_plan(removes, &restores)
}

/// What is said once the undo has run. Each command is its own transaction in
/// the manager's log (pacman `-U` and `-R` included), and the next `undo`
/// reads only the last, whichever command that is.
pub fn closing_line(commands: usize) -> String {
    if commands <= 1 {
        "Undone. `yapacc timeline` shows it as a new transaction.".into()
    } else {
        format!(
            "Undone, as {commands} separate commands, which the log records as {commands} transactions. `yapacc timeline` shows them; a following `yapacc undo` would take back only the last one (the last command listed under \"Will run\"), not the whole undo."
        )
    }
}

/// A version as a package log may give it: the characters Debian, pacman
/// and rpm versions are made of (`1:2.41-12+deb13u4`, `1.0~rc1`,
/// `1.0^20260101`), and nothing else. Versions from the log go into file
/// names, archive URLs and `name=version` arguments. Pure.
pub fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 256
        && version.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'~' | b':' | b'_' | b'-' | b'^')
        })
}

/// Refuse a transaction whose log names a package or a version yapacc will
/// not pass to a command. Every entry is checked, the reinstalled ones too:
/// names and both versions are used for arguments, file names and URLs.
/// On apt a name may carry its architecture (`name:arch`).
fn valid(entries: &[LogEntry], m: PkgManager) -> Result<()> {
    for e in entries {
        if !ops::valid_target(m, &e.name) {
            return Err(Error::refused(format!(
                "the log names a package yapacc will not pass to a command: '{}'. Nothing was changed.",
                e.name
            )));
        }
        for v in [&e.old_version, &e.new_version] {
            if !v.is_empty() && !valid_version(v) {
                return Err(Error::refused(format!(
                    "the log gives {} a version yapacc will not pass to a command: '{v}'. Nothing was changed.",
                    e.name
                )));
            }
        }
    }
    Ok(())
}

/// The packages in `removes` that hold the kernel running now (`uname -r`
/// gives `release`). Debian's and Ubuntu's kernel image refuses its own
/// removal while it runs, which left an undo half applied (the removal
/// aborts after the downgrades were unpacked), so such an undo is refused.
/// A removal may carry its architecture (`linux-image-...:amd64`).
pub fn running_kernel_packages(removes: &[String], release: &str) -> Vec<String> {
    if release.is_empty() {
        return Vec::new();
    }
    let ours = [
        format!("linux-image-{release}"),
        format!("linux-image-unsigned-{release}"),
        format!("linux-image-{release}-unsigned"),
        format!("linux-modules-{release}"),
        format!("linux-modules-extra-{release}"),
    ];
    removes
        .iter()
        .filter(|r| ours.iter().any(|o| o == apt_split(r).0))
        .cloned()
        .collect()
}

/// The packages apt's simulation (`apt-get -s`) says it would remove: its
/// `Remv NAME [VERSION]` lines. Pure.
pub fn apt_simulated_removals(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("Remv ")?.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// A package name in the form apt's simulation prints: without the
/// architecture when it is the native one (apt's history also writes
/// `Architecture: all` packages with the native one), `name:arch` when
/// foreign. Pure.
fn apt_simulation_name(target: &str, native: &str) -> String {
    match apt_split(target) {
        (name, arch) if arch.is_empty() || arch == native || arch == "all" => name.to_string(),
        _ => target.to_string(),
    }
}

/// The packages apt's simulation would remove that the undo did not plan
/// to remove, each once, in apt's order. Empty when apt removes exactly
/// what was planned, or less. `planned` holds `name:arch` as the history
/// wrote it; `native` is `dpkg --print-architecture`. Pure.
pub fn apt_unplanned_removals(simulation: &str, planned: &[String], native: &str) -> Vec<String> {
    let planned: Vec<String> = planned
        .iter()
        .map(|p| apt_simulation_name(p, native))
        .collect();
    let mut out: Vec<String> = Vec::new();
    for r in apt_simulated_removals(simulation) {
        if !planned.contains(&apt_simulation_name(&r, native)) && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

fn nothing() -> Error {
    Error::not_found("no package transaction was found in the native log: nothing to undo")
}

fn pacman_undo(g: GateFlags, arch: &str, family: &str) -> Result<()> {
    let log = std::fs::read("/var/log/pacman.log")
        .map_err(|e| Error::unavailable(format!("cannot read /var/log/pacman.log: {e}")))?;
    let t = last_pacman_transaction(&String::from_utf8_lossy(&log)).ok_or_else(nothing)?;
    print!("{}", describe(&t));
    valid(&t.entries, PkgManager::Pacman)?;
    let steps = reverse(&t.entries);
    println!("\nTo undo it:");
    let (removes, restores) = resolve(&steps, |n, v| pacman_find(n, v, arch, family == "arch"))?;
    for r in &removes {
        println!("  remove  {r}");
    }
    show_sources(&restores);
    let mut downloads = Downloads::plan();
    let commands = pacman_commands(&removes, &restores, &downloads)?;
    if commands.is_empty() {
        println!("  (nothing: the transaction only reinstalled packages)");
        return Ok(());
    }
    if let Some(note) = downloads_note(&remotes(&restores)) {
        println!("{note}");
    }
    if !restores.is_empty() {
        println!(
            "note: the next full upgrade will upgrade restored packages again; add them to IgnorePkg in pacman.conf yourself if you need them held."
        );
    }
    let expected = plan_state(&removes, &restores);
    finish(PkgManager::Pacman, &commands, &expected, g, &mut || {
        downloads.fetch(&restores)
    })
}

/// The pacman commands of an undo: one `-U` with every old version (a
/// cache file, or the place its download will be in), then one `-R`. Pure.
fn pacman_commands(
    removes: &[String],
    restores: &[(String, String, Found)],
    downloads: &Downloads,
) -> Result<Vec<Vec<String>>> {
    let mut commands = Vec::new();
    if !restores.is_empty() {
        let mut c: Vec<String> = ["pacman", "-U", "--noconfirm", "--"]
            .map(String::from)
            .into();
        for (_, _, found) in restores {
            match found {
                Found::File(p, _) => c.push(p.display().to_string()),
                Found::Remote(r) => c.push(downloads.path(&r.file)?.display().to_string()),
                Found::Repo => {}
            }
        }
        commands.push(c);
    }
    if !removes.is_empty() {
        let mut c: Vec<String> = ["pacman", "-R", "--noconfirm", "--"]
            .map(String::from)
            .into();
        c.extend(removes.iter().cloned());
        commands.push(c);
    }
    Ok(commands)
}

/// The one apt command of an undo: `name:arch=version` or a `.deb` file
/// restores, `name:arch-` removes. The architecture is the package's own,
/// as the history wrote it. Pure.
fn apt_command(
    removes: &[String],
    restores: &[(String, String, Found)],
    downloads: &Downloads,
) -> Result<Vec<String>> {
    let mut c: Vec<String> = ["apt-get", "install", "-y", "--allow-downgrades", "--"]
        .map(String::from)
        .into();
    for (name, version, found) in restores {
        c.push(match found {
            Found::Repo => format!("{name}={version}"),
            Found::File(p, _) => p.display().to_string(),
            Found::Remote(r) => downloads.path(&r.file)?.display().to_string(),
        });
    }
    c.extend(removes.iter().map(|r| format!("{r}-")));
    Ok(c)
}

/// Why an undo must not run after apt's own simulation of it: the
/// simulation failed, or it would remove a package the undo did not plan
/// to remove (a dependent of a removed or downgraded package, or the
/// wrong architecture of one). None when the simulation agrees with the
/// plan. `planned` are the planned removals. Pure.
pub fn apt_simulation_refusal(
    code: i32,
    stdout: &str,
    planned: &[String],
    native: &str,
) -> Option<String> {
    if code != 0 {
        return Some(format!(
            "cannot undo: apt's simulation of the undo failed (status {code}), so the undo was not started. Nothing was changed."
        ));
    }
    let extra = apt_unplanned_removals(stdout, planned, native);
    if extra.is_empty() {
        return None;
    }
    Some(format!(
        "cannot undo: apt would also remove {}, which the transaction did not install. Nothing was changed.",
        extra.join(", ")
    ))
}

/// The packages apt's simulation lists under "WARNING: The following
/// essential packages will be removed." (C locale): the ones it does not
/// remove under `-y` without `--allow-remove-essential`. A `(due to X)`
/// remark is not a package. Pure.
pub fn apt_essential_removals(stdout: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut lines = stdout.lines().skip_while(|l| {
        !l.starts_with("WARNING: The following essential packages will be removed")
    });
    lines.next();
    let mut in_list = false;
    for line in lines {
        if !line.starts_with(' ') {
            if in_list {
                break;
            }
            // "This should NOT be done unless you know exactly ..."
            continue;
        }
        in_list = true;
        let mut remark = false;
        for word in line.split_whitespace() {
            remark |= word.starts_with('(');
            if !remark && !out.iter().any(|o| o == word) {
                out.push(word.to_string());
            }
            if word.ends_with(')') {
                remark = false;
            }
        }
    }
    out
}

/// Whether the undo may pass `--allow-remove-essential`: only when every
/// package apt counts as essential is one the undo planned to remove (so
/// the transaction installed it) and is of a foreign architecture (so it is
/// not one of this system's own essential packages: `libgcc-s1:i386` on
/// amd64 is the usual case, with the first i386 library ever installed).
/// `Err` names the packages that stand in the way. Pure.
pub fn apt_essential_removable(
    essential: &[String],
    planned: &[String],
    native: &str,
) -> std::result::Result<(), Vec<String>> {
    let planned: Vec<String> = planned
        .iter()
        .map(|p| apt_simulation_name(p, native))
        .collect();
    let blocked: Vec<String> = essential
        .iter()
        .filter(|e| {
            let foreign = apt_simulation_name(e, native).contains(':');
            !foreign || !planned.contains(&apt_simulation_name(e, native))
        })
        .cloned()
        .collect();
    if blocked.is_empty() && !essential.is_empty() {
        Ok(())
    } else {
        Err(blocked)
    }
}

/// `command` as apt simulates it: the same command with `-s`. `-y` stays,
/// so that apt fails in the simulation exactly where the real run would
/// (essential or held packages, observed with apt 2.6 and 3.0). Pure.
fn apt_simulation_of(command: &[String]) -> Vec<String> {
    let mut simulate = command.to_vec();
    simulate.insert(2, "-s".into());
    simulate
}

/// Run one apt simulation. Returns its exit status and what it printed on
/// stdout and stderr.
fn apt_run_simulation(simulate: &[String], c_locale: bool) -> Result<(i32, String, String)> {
    let mut cmd = Command::new(&simulate[0]);
    cmd.args(&simulate[1..]).stdin(std::process::Stdio::null());
    if c_locale {
        cmd.env("LC_ALL", "C");
    }
    let out = cmd
        .output()
        .map_err(|e| Error::unavailable(format!("cannot run {}: {e}", simulate[0])))?;
    Ok((
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Simulate the undo command with apt, show the simulation, and return the
/// command to run. Refuses (exit 3) when the simulation fails or would
/// remove a package the undo did not plan to remove. Changes nothing.
///
/// One failure is handled: apt refuses under `-y` to remove packages it
/// counts as essential. When those are all foreign-architecture packages
/// the transaction installed (`apt_essential_removable`), the command gets
/// `--allow-remove-essential`, and that command is simulated and shown.
fn apt_checked_command(
    command: Vec<String>,
    removes: &[String],
    native: &str,
) -> Result<Vec<String>> {
    let simulate = apt_simulation_of(&command);
    println!("\napt's own simulation ({}):", ops::shown(&[], &simulate));
    let (code, stdout, stderr) = apt_run_simulation(&simulate, false)?;
    print!("{stdout}");
    eprint!("{stderr}");
    if code == 0 {
        return match apt_simulation_refusal(code, &stdout, removes, native) {
            Some(why) => Err(Error::refused(why)),
            None => Ok(command),
        };
    }
    let failed = || {
        Error::refused(apt_simulation_refusal(code, &stdout, removes, native).unwrap_or_default())
    };
    // would it pass if apt were allowed to remove what it calls essential?
    let mut allowed = command;
    let Some(dashes) = allowed.iter().position(|a| a == "--") else {
        return Err(failed());
    };
    allowed.insert(dashes, "--allow-remove-essential".into());
    let simulate = apt_simulation_of(&allowed);
    let (code2, stdout2, _) = apt_run_simulation(&simulate, true)?;
    if code2 != 0 {
        return Err(failed());
    }
    let essential = apt_essential_removals(&stdout2);
    if let Err(blocked) = apt_essential_removable(&essential, removes, native) {
        if blocked.is_empty() {
            return Err(failed());
        }
        return Err(Error::refused(format!(
            "cannot undo: apt counts {} as essential, and does not remove essential packages under -y. yapacc passes --allow-remove-essential only for packages of a foreign architecture that the transaction itself installed. Nothing was changed.",
            blocked.join(", ")
        )));
    }
    if let Some(why) = apt_simulation_refusal(code2, &stdout2, removes, native) {
        return Err(Error::refused(why));
    }
    println!(
        "\nnote: apt counts {} as essential, and removes essential packages only with --allow-remove-essential. These are packages of a foreign architecture that this transaction installed, not this system's own ({native}) packages, so the undo passes that option.",
        essential.join(", ")
    );
    println!(
        "\napt's own simulation with that option ({}):",
        ops::shown(&[], &simulate)
    );
    print!("{stdout2}");
    Ok(allowed)
}

fn apt_undo(g: GateFlags, os_id: &str) -> Result<()> {
    let history = std::fs::read("/var/log/apt/history.log")
        .map_err(|e| Error::unavailable(format!("cannot read /var/log/apt/history.log: {e}")))?;
    let t = last_apt_transaction(&String::from_utf8_lossy(&history)).ok_or_else(nothing)?;
    print!("{}", describe(&t));
    valid(&t.entries, PkgManager::Apt)?;
    let steps = reverse(&t.entries);
    // dpkg's own architecture: the one apt means by a bare name, and leaves
    // out of its simulation
    let arch = Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if !ops::valid_deb_arch(&arch) {
        return Err(Error::unavailable(
            "cannot undo: `dpkg --print-architecture` did not give this system's architecture, which is needed to tell the packages of each architecture apart. Nothing was changed.",
        ));
    }
    println!("\nTo undo it:");
    let (removes, restores) = resolve(&steps, |n, v| apt_find(n, v, &arch, os_id, true))?;
    let release = Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let running = running_kernel_packages(&removes, &release);
    if !running.is_empty() {
        return Err(Error::refused(format!(
            "cannot undo: it would remove the running kernel ({}), which the package refuses and leaves the undo half applied. Nothing was changed. Reboot into the previous kernel first (pick it in the boot menu), then run `yapacc undo` again.",
            running.join(", ")
        )));
    }
    for r in &removes {
        println!("  remove  {r}");
    }
    show_sources(&restores);
    if removes.is_empty() && restores.is_empty() {
        println!("  (nothing to reverse)");
        return Ok(());
    }
    let expected = plan_state(&removes, &restores);
    // one apt transaction: restores and removals together
    let mut downloads = Downloads::plan();
    let c = apt_command(&removes, &restores, &downloads)?;
    let pending = remotes(&restores);
    let Some(note) = downloads_note(&pending) else {
        // nothing to download: simulate, then the gate
        let c = apt_checked_command(c, &removes, &arch)?;
        return finish(PkgManager::Apt, &[c], &expected, g, &mut || Ok(()));
    };
    println!("{note}");
    // apt can simulate the command only once the files it names exist
    if g.dryrun {
        println!(
            "note: apt's own simulation is not shown: it needs the files above, and a dry run downloads nothing. A real run asks before it downloads, then shows the simulation, refuses if it fails or would remove a package the transaction did not install, and asks again before anything is changed."
        );
        return finish(PkgManager::Apt, &[c], &expected, g, &mut || Ok(()));
    }
    ops::gate(&apt_download_question(pending.len()), g)?;
    downloads.fetch(&restores)?;
    let c = apt_checked_command(c, &removes, &arch)?;
    finish(PkgManager::Apt, &[c], &expected, g, &mut || Ok(()))
}

/// The question asked before an apt undo downloads anything; the undo
/// itself is asked about afterwards, once apt's simulation is shown. Pure.
fn apt_download_question(files: usize) -> String {
    format!(
        "Download the {files} file{} listed above (nothing else is changed yet)",
        if files == 1 { "" } else { "s" }
    )
}

/// Whether `dnf history info last` described a transaction. With an empty
/// history dnf 4 fails, but dnf 5 prints nothing and exits 0.
pub fn dnf_info_has_transaction(stdout: &str) -> bool {
    !stdout.trim().is_empty()
}

/// Whether a failing `dnf history info last` is dnf 4 saying the history is
/// empty, as opposed to dnf itself failing (lock, broken repo, bad config).
pub fn dnf_stderr_is_empty_history(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    // dnf 4 does not print "no transactions": with an empty history it
    // crashes with a Python AttributeError on `history.last().tid`.
    s.contains("no transaction")
        || (s.contains("attributeerror") && s.contains("'nonetype'") && s.contains("'tid'"))
}

/// The versions `dnf history undo` has to bring back, as the NEVRA text dnf
/// printed: the old versions an upgrade or downgrade replaced and the
/// packages a transaction removed. dnf 5 lists them as `Replaced`; dnf 4 as
/// `Upgraded` / `Downgraded` (dnf 4 wording seen on Rocky 9).
pub fn dnf_replaced_nevras(info: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for line in info.lines() {
        let mut w = line.split_whitespace();
        let (Some(action), Some(nevra)) = (w.next(), w.next()) else {
            continue;
        };
        // `Remove` (dnf 5) / `Removed` (dnf 4): undoing a removal reinstalls
        // that exact version, so it must be available just like an old one.
        if matches!(
            action,
            "Replaced" | "Upgraded" | "Downgraded" | "Remove" | "Removed"
        ) && nevra.contains('-')
            && !seen.contains(&dnf_norm_nevra(nevra))
        {
            seen.push(dnf_norm_nevra(nevra));
            out.push(nevra.to_string());
        }
    }
    out
}

/// A NEVRA with an epoch of 0 written or left out alike.
fn dnf_norm_nevra(s: &str) -> String {
    s.trim().replace("-0:", "-")
}

/// Whether the output of `dnf repoquery --available` lists that NEVRA (an
/// epoch of 0 is written or left out, on either side).
pub fn dnf_repoquery_lists(stdout: &str, nevra: &str) -> bool {
    let want = dnf_norm_nevra(nevra);
    stdout.lines().any(|l| dnf_norm_nevra(l) == want)
}

/// Which of `nevras` `dnf repoquery --available` does not list, run with
/// `prefix` (sudo/doas, or nothing). `Err` says why the check could not run.
fn dnf_repoquery_missing(
    prefix: &[String],
    nevras: &[String],
) -> std::result::Result<Vec<String>, String> {
    let mut argv: Vec<String> = prefix.to_vec();
    argv.extend(["dnf", "repoquery", "--available", "-q"].map(String::from));
    argv.extend(nevras.iter().cloned());
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).env("LC_ALL", "C");
    if prefix.is_empty() {
        cmd.stdin(std::process::Stdio::null());
    }
    let q = cmd
        .output()
        .map_err(|e| format!("cannot run dnf repoquery: {e}"))?;
    if !q.status.success() {
        return Err(format!("`dnf repoquery` failed: {}", q.status));
    }
    let listed = String::from_utf8_lossy(&q.stdout);
    Ok(nevras
        .iter()
        .filter(|n| !dnf_repoquery_lists(&listed, n))
        .cloned()
        .collect())
}

fn dnf_undo(g: GateFlags) -> Result<()> {
    println!("Last transaction, as dnf records it:");
    // the C locale: the transaction ID and the action words are read from
    // this text, and dnf translates them otherwise
    let out = Command::new("dnf")
        .args(["history", "info", "last"])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| Error::unavailable(format!("cannot run dnf: {e}")))?;
    let info = String::from_utf8_lossy(&out.stdout);
    print!("{info}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && !dnf_stderr_is_empty_history(&stderr) {
        eprint!("{stderr}");
        return Err(Error::from(format!(
            "dnf failed ({}); nothing was changed",
            out.status
        )));
    }
    if !out.status.success() || !dnf_info_has_transaction(&info) {
        return Err(nothing());
    }
    // The undo names this transaction by its ID. `dnf history undo last`
    // would undo whatever is last when it runs: another transaction
    // (dnf-automatic, PackageKit) committed while the question is open
    // would be reversed instead, unseen and unchecked.
    let Some(id) = dnf_transaction_id(&info) else {
        return Err(Error::refused(
            "cannot undo: yapacc could not read the transaction ID in what dnf printed above, and it does not ask dnf to undo \"last\" (that could be another transaction by the time the command runs). Nothing was changed. `dnf history list` shows the IDs; `dnf history undo ID` undoes one.",
        ));
    };
    println!("\nTransaction to undo: ID {id}");
    let nevras = dnf_replaced_nevras(&info);
    let mut missing = Vec::new();
    let mut checked_as_root = false;
    if !nevras.is_empty() {
        // dnf 4 keeps a separate metadata cache for each ordinary user, and
        // it can be stale in either direction (observed on Rocky 9: the
        // user's cache still listed a version deleted from the repository,
        // so the check passed and dnf then refused), while the undo itself
        // runs as root with the root cache: ask as root when possible.
        if crate::util::is_root() {
            match dnf_repoquery_missing(&[], &nevras) {
                Ok(m) => missing = m,
                // Offline or a broken repository must not block the undo:
                // dnf itself decides in the end.
                Err(why) => eprintln!(
                    "warning: could not check that the old versions are still available ({why}); continuing, dnf may still refuse."
                ),
            }
            checked_as_root = true;
        } else if let Ok(prefix) = ops::elevation_for(GateFlags {
            // a dry run never asks for a password just to look
            script: g.script || g.dryrun,
            ..g
        }) {
            println!(
                "note: checking availability with {} (dnf keeps a separate metadata cache for ordinary users)",
                ops::shown(&prefix, &[])
            );
            if let Ok(m) = dnf_repoquery_missing(&prefix, &nevras) {
                missing = m;
                checked_as_root = true;
            }
        }
        if !checked_as_root {
            // One repoquery for all old versions, as this user.
            match dnf_repoquery_missing(&[], &nevras) {
                Ok(m) => missing = m,
                // Offline or a broken repository must not block the undo:
                // dnf itself decides in the end.
                Err(why) => eprintln!(
                    "warning: could not check that the old versions are still available ({why}); continuing, dnf may still refuse."
                ),
            }
            if !missing.is_empty() && ops::elevation().is_ok() {
                eprintln!(
                    "warning: the repository check ran as {} with that user's metadata cache, which can be stale, and could not be repeated as root; continuing, dnf may still refuse.",
                    std::env::var("USER").unwrap_or_else(|_| "this user".into())
                );
                missing.clear();
            }
        }
    }
    if !missing.is_empty() {
        let user_only = if checked_as_root {
            ""
        } else {
            " This was checked as an ordinary user, with that user's own dnf metadata cache (neither sudo nor doas could repeat it as root); run `dnf makecache` or yapacc as root to confirm."
        };
        return Err(Error::refused(format!(
            "cannot undo: {} is in no enabled repository (undo would have to bring that version back), so dnf would refuse. Nothing was changed. (A filesystem snapshot from before the transaction can still take it back.){user_only}",
            missing.join(", ")
        )));
    }
    println!(
        "\ndnf undoes its own transactions; yapacc gates the command. Versions to bring back (old versions of upgrades, removed packages) were looked up with `dnf repoquery --available`; dnf can still refuse for other reasons."
    );
    // dnf works the reverse out itself: the state to expect is read from
    // the transaction it printed
    let expected = expected_from_entries(&pkglog::parse_dnf_history_info(&info));
    finish(
        PkgManager::Dnf,
        &[dnf_undo_command(&id)],
        &expected,
        g,
        &mut || Ok(()),
    )
}

/// The ID of the transaction `dnf history info` printed: its `Transaction
/// ID : N` line (dnf 4 and dnf 5 in the C locale). None when there is no
/// such line or it does not hold one number (dnf 4 writes `3..5` for a
/// range). Pure.
pub fn dnf_transaction_id(info: &str) -> Option<String> {
    info.lines()
        .find_map(|l| {
            let (key, value) = l.split_once(':')?;
            (key.trim() == "Transaction ID").then(|| value.trim().to_string())
        })
        .filter(|id| !id.is_empty() && id.len() <= 18 && id.bytes().all(|b| b.is_ascii_digit()))
}

/// The command that undoes exactly the transaction that was shown: by its
/// ID, never `last`, which names whatever transaction is the last one when
/// the command runs. Pure.
pub fn dnf_undo_command(id: &str) -> Vec<String> {
    ["dnf", "history", "undo", id, "-y"]
        .map(String::from)
        .into()
}

/// Multiversion packages (kernels) keep their old versions when "upgraded":
/// the new one is installed beside them. Undoing that is removing the new
/// version, not installing the old one (it is still there, so zypper would
/// do nothing and yapacc would report "Undone" with the new version left),
/// and removing an installed version must not take the other versions with
/// it (`zypper remove NAME` removes all of them). `installed` lists the
/// installed `version-release` strings of a name. Returns the removes
/// (`name` or `name=version`) and the restores that remain. Pure.
fn zypper_multiversion_fixup(
    entries: &[LogEntry],
    removes: Vec<String>,
    restores: Vec<(String, String, Found)>,
    installed: impl Fn(&str) -> Vec<String>,
) -> (Vec<String>, Vec<(String, String, Found)>) {
    let new_version = |name: &str| {
        entries
            .iter()
            .find(|e| e.name == name && !e.new_version.is_empty())
            .map(|e| e.new_version.clone())
    };
    let mut out_removes = Vec::new();
    for name in removes {
        let have = installed(&name);
        match new_version(&name) {
            Some(v) if have.len() > 1 && have.contains(&v) => {
                out_removes.push(format!("{name}={v}"))
            }
            _ => out_removes.push(name),
        }
    }
    let mut out_restores = Vec::new();
    for (name, old, found) in restores {
        let have = installed(&name);
        match new_version(&name) {
            Some(v) if have.contains(&old) && have.contains(&v) && old != v => {
                out_removes.push(format!("{name}={v}"))
            }
            _ => out_restores.push((name, old, found)),
        }
    }
    (out_removes, out_restores)
}

/// A package both installed and removed in one grouped transaction (history
/// with no command record to tell the runs apart) is a restore of its old
/// version: installing it replaces the installed one, and a following remove
/// would leave the package gone. Drops those removes. Pure.
fn drop_removes_of_restored(
    removes: Vec<String>,
    restores: &[(String, String, Found)],
) -> Vec<String> {
    removes
        .into_iter()
        .filter(|r| {
            let name = r.split_once('=').map_or(r.as_str(), |(n, _)| n);
            !restores.iter().any(|(n, _, _)| n == name)
        })
        .collect()
}

/// The running kernel's package (`name`, `version-release`) according to
/// rpm: the owner of `/boot/vmlinuz-RELEASE`. None when not found (not an
/// rpm kernel, no `uname`).
fn rpm_running_kernel_package() -> Option<(String, String)> {
    let rel = Command::new("uname").arg("-r").output().ok()?;
    let rel = String::from_utf8_lossy(&rel.stdout).trim().to_string();
    if rel.is_empty() {
        return None;
    }
    let out = Command::new("rpm")
        .args(["-qf", "--qf", "%{NAME} %{VERSION}-%{RELEASE}\\n"])
        .arg(format!("/boot/vmlinuz-{rel}"))
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (n, v) = text.lines().next()?.split_once(' ')?;
    Some((n.to_string(), v.to_string()))
}

/// Whether one of the removals (`name` or `name=version`) takes out the
/// package that holds the running kernel (`name`, `version-release`).
/// zypper itself removes a running kernel when asked to. Pure.
fn removes_running_kernel(removes: &[String], running: Option<&(String, String)>) -> bool {
    let Some((name, version)) = running else {
        return false;
    };
    removes.iter().any(|r| match r.split_once('=') {
        Some((n, v)) => n == name && v == version,
        None => r == name,
    })
}

/// The installed `version-release` strings of an rpm package name.
fn rpm_installed_versions(name: &str) -> Vec<String> {
    Command::new("rpm")
        .args(["-q", "--qf", "%{VERSION}-%{RELEASE}\\n", "--", name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The rotated predecessors of a zypp history file among `names` (file
/// names in its directory), oldest first: logrotate leaves
/// `history-20261117.gz` (openSUSE's own rule) or `history-202611.xz`.
/// Pure.
pub fn zypp_rotated_names(names: &[String]) -> Vec<String> {
    let mut v: Vec<String> = names
        .iter()
        .filter(|n| {
            n.strip_prefix("history-").is_some_and(|rest| {
                let (stamp, ext) = rest.split_once('.').unwrap_or((rest, ""));
                !stamp.is_empty()
                    && stamp.bytes().all(|b| b.is_ascii_digit())
                    && matches!(ext, "" | "gz" | "xz" | "bz2" | "zst")
            })
        })
        .cloned()
        .collect();
    v.sort();
    v
}

/// Text of a possibly compressed log, through the matching tool.
fn read_log_file(path: &Path) -> Option<String> {
    let tool = match path.extension().and_then(|e| e.to_str()) {
        Some("gz") => Some("gzip"),
        Some("xz") => Some("xz"),
        Some("bz2") => Some("bzip2"),
        Some("zst") => Some("zstd"),
        _ => None,
    };
    let bytes = match tool {
        None => std::fs::read(path).ok()?,
        Some(t) => {
            let o = Command::new(t)
                .arg("-dc")
                .arg("--")
                .arg(path)
                .output()
                .ok()?;
            if !o.status.success() {
                return None;
            }
            o.stdout
        }
    };
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `/var/log/zypp/history` preceded by its rotated predecessors, so that an
/// upgrade can find the version it replaced after logrotate has run.
/// Returns the text and the files that could not be read.
pub fn read_zypp_history(dir: &Path) -> std::io::Result<(String, Vec<String>)> {
    let current = dir.join("history");
    let mut text = String::new();
    let mut skipped = Vec::new();
    let names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    for n in zypp_rotated_names(&names) {
        match read_log_file(&dir.join(&n)) {
            Some(t) => {
                text.push_str(&t);
                if !t.ends_with('\n') {
                    text.push('\n');
                }
            }
            None => skipped.push(n),
        }
    }
    match std::fs::read(&current) {
        Ok(b) => text.push_str(&String::from_utf8_lossy(&b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !text.is_empty() => {}
        Err(e) => return Err(e),
    }
    Ok((text, skipped))
}

/// What to do about packages a zypper upgrade run is recorded as having
/// installed with no earlier record (see `zypper_uncertain_installs`).
#[derive(Debug, PartialEq, Eq)]
enum UnknownInstalls {
    /// Nothing is unknown.
    None,
    /// A dry run: say that a real run asks first.
    DryRunNote,
    /// Ask the user, on the terminal.
    Ask,
}

fn unknown_installs_decision(unknown: &[String], dryrun: bool) -> UnknownInstalls {
    if unknown.is_empty() {
        UnknownInstalls::None
    } else if dryrun {
        UnknownInstalls::DryRunNote
    } else {
        UnknownInstalls::Ask
    }
}

/// The text shown before the question about unknown installs.
fn unknown_installs_text(unknown: &[String], command: &str, path: &str) -> String {
    let one = unknown.len() == 1;
    format!(
        "`{command}` is recorded as having installed {}, with no earlier record in {path}. yapacc cannot tell whether {} new {} or {} already on this machine and upgraded: libzypp writes both the same way, and rpm keeps no trace of the replaced version. If new, undoing removes {}, which is right. If upgraded, it removes {} the system had before, and the version to go back to is not known. Packages that something else installed still needs are refused separately, by rpm, before anything runs.",
        unknown.join(", "),
        if one { "it is a" } else { "they are" },
        if one { "dependency" } else { "dependencies" },
        if one { "a package" } else { "packages" },
        if one { "it" } else { "them" },
        if one { "something" } else { "things" },
    )
}

/// Make the user decide about the packages yapacc cannot classify. `--yes`
/// does not answer this: only a yes typed at a terminal does.
fn confirm_unknown_installs(
    unknown: &[String],
    command: &str,
    path: &str,
    g: GateFlags,
) -> Result<()> {
    match unknown_installs_decision(unknown, g.dryrun) {
        UnknownInstalls::None => Ok(()),
        UnknownInstalls::DryRunNote => {
            println!("note: {}", unknown_installs_text(unknown, command, path));
            println!(
                "note: a real run asks you at the terminal whether to remove {}; --yes does not answer that.",
                unknown.join(", ")
            );
            Ok(())
        }
        UnknownInstalls::Ask => {
            println!("\n{}", unknown_installs_text(unknown, command, path));
            let question = format!("Remove {} as part of the undo", unknown.join(", "));
            match crate::prompt::confirm_on_terminal(&question) {
                Ok(true) => Ok(()),
                Ok(false) => Err(Error::refused("Aborted: nothing was changed.")),
                Err(e) => Err(Error::refused(format!("{e}. Nothing was changed."))),
            }
        }
    }
}

fn zypper_undo(g: GateFlags) -> Result<()> {
    let path = "/var/log/zypp/history";
    let (history, skipped) = read_zypp_history(Path::new("/var/log/zypp")).map_err(|e| {
        Error::unavailable(format!(
            "cannot read {path}: {e} (it is often readable by root only; `yapacc undo --dryrun` as root shows the plan and changes nothing)"
        ))
    })?;
    if !skipped.is_empty() {
        println!(
            "note: rotated history not read (decompression tool missing or failed): {}; an upgrade whose earlier record is in them is treated as uncertain.",
            skipped.join(", ")
        );
    }
    let t = last_zypper_transaction(&history).ok_or_else(nothing)?;
    print!("{}", describe(&t));
    println!(
        "note: zypper's history records an upgrade as an install of the new version. yapacc reads the version before it from the earlier records of {path}; a package whose earlier record has been rotated out of that file is shown above as installed, and undoing would remove it. Check the list."
    );
    if t.command.is_empty() {
        println!(
            "note: these changes follow no zypper command record; they were grouped by time only."
        );
    } else {
        println!(
            "note: changes made by another front end (PackageKit, YaST) after that zypper command are recorded without a boundary and are listed with it."
        );
    }
    valid(&t.entries, PkgManager::Zypper)?;
    let steps = reverse(&t.entries);
    let uncertain = zypper_uncertain_installs(&t.command, &t.entries);
    println!("\nTo undo it:");
    // An old version that is still installed beside the new one (a
    // multiversion kernel) needs no source: undoing is removing the new one,
    // and the repositories rarely still list a kernel that was replaced.
    let (removes, restores) = resolve(&steps, |n, v| {
        if rpm_installed_versions(n).iter().any(|i| i == v) {
            return Ok(Some(Found::Repo));
        }
        // the old versions are looked up in the metadata on this machine;
        // without it zypper would download it, which a look does not do
        if let Some(why) = crate::native::missing_metadata(PkgManager::Zypper) {
            return Err(Error::refused(format!(
                "cannot undo: whether a repository still has {n} {v} is not known: {why}. Nothing was changed."
            )));
        }
        Ok(zypper_find(n, v))
    })?;
    let (removes, restores) =
        zypper_multiversion_fixup(&t.entries, removes, restores, rpm_installed_versions);
    let removes = drop_removes_of_restored(removes, &restores);
    for r in &removes {
        println!("  remove  {r}");
    }
    show_sources(&restores);
    if removes_running_kernel(&removes, rpm_running_kernel_package().as_ref()) {
        return Err(Error::refused(format!(
            "cannot undo: removing {} would remove the kernel this machine is running (zypper does not stop that). Reboot into the kernel you want to keep (the older one) and run `yapacc undo` again. Nothing was changed.",
            removes.join(", ")
        )));
    }
    // `zypper --non-interactive remove` also removes whatever needs the
    // named packages. Undoing an install must remove what that run
    // installed and nothing else, so ask rpm first; it changes nothing.
    if !removes.is_empty() {
        let needed_by = Command::new("rpm")
            .args(["-e", "--test", "--"])
            .args(removes.iter().map(|r| r.replace('=', "-")))
            .env("LC_ALL", "C")
            .output()
            .map(|o| rpm_erase_dependents(&String::from_utf8_lossy(&o.stderr)))
            .unwrap_or_default();
        if !needed_by.is_empty() {
            let shown: Vec<&str> = needed_by.iter().take(8).map(String::as_str).collect();
            let more = match needed_by.len() - shown.len() {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            return Err(Error::refused(format!(
                "cannot undo: removing {} would also remove {}{more}, which that run did not install. Most likely a package listed above as installed was already there and was upgraded or reinstalled (its earlier record is not in {path}), so the version to go back to is not known. Nothing was changed.",
                removes.join(", "),
                shown.join(", ")
            )));
        }
    }
    confirm_unknown_installs(&uncertain, &t.command, path, g)?;
    let expected = plan_state(&removes, &restores);
    let mut commands = Vec::new();
    if !restores.is_empty() {
        let mut c: Vec<String> = [
            "zypper",
            "--non-interactive",
            "install",
            "--oldpackage",
            "--",
        ]
        .map(String::from)
        .into();
        c.extend(
            restores
                .iter()
                .map(|(name, version, _)| format!("{name}={version}")),
        );
        commands.push(c);
    }
    if !removes.is_empty() {
        let mut c: Vec<String> = ["zypper", "--non-interactive", "remove", "--"]
            .map(String::from)
            .into();
        c.extend(removes);
        commands.push(c);
    }
    if commands.is_empty() {
        println!("  (nothing: the transaction only reinstalled packages)");
        return Ok(());
    }
    if !restores.is_empty() {
        println!(
            "note: the old versions were looked up in the repository metadata already on this machine (no refresh); zypper refuses by itself if a repository no longer has one. The next `zypper update` will upgrade restored packages again; `zypper addlock` holds one."
        );
    }
    if commands.len() > 1 {
        println!(
            "note: this is {} zypper runs (restore, then remove); zypper's history will record one transaction for each, so a following `yapacc undo` would see only the last (the removal).",
            commands.len()
        );
    }
    if commands.iter().any(|c| c.iter().any(|a| a == "remove")) {
        println!(
            "note: `zypper remove NAME` removes every installed version of a package that keeps several (kernels)."
        );
    }
    finish(PkgManager::Zypper, &commands, &expected, g, &mut || Ok(()))
}

pub fn run(g: GateFlags) -> Result<()> {
    // fail closed, and first: an unreadable [safety] snapshot value must not
    // mean "off" (`finish` reads it again where it is used)
    snapshot_mode(&crate::config::get().safety.snapshot)?;
    let p = crate::platform::detect();
    let Some(m) = p.native_pkg_manager else {
        return Err(Error::unavailable(
            crate::platform::no_native_manager_message(),
        ));
    };
    match m {
        PkgManager::Pacman => pacman_undo(g, &p.arch, &p.family),
        PkgManager::Apt => apt_undo(g, &p.os.id),
        PkgManager::Dnf => dnf_undo(g),
        PkgManager::Zypper => zypper_undo(g),
        other => Err(Error::unavailable(format!(
            "undo is not available with {}: yapacc does not read its transaction log. `yapacc timeline` and the manager's own tools still apply.",
            other.name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACMAN: &str = "[2026-11-08T00:03:49+0000] [PACMAN] Running 'pacman -Syu'\n[2026-11-08T00:03:49+0000] [ALPM] transaction started\n[2026-11-08T00:03:49+0000] [ALPM] upgraded curl (8.9.0-1 -> 8.10.0-1)\n[2026-11-08T00:03:50+0000] [ALPM] removed oldlib (1.0-1)\n[2026-11-08T00:03:50+0000] [ALPM] installed newdep (2.0-1)\n[2026-11-08T00:03:50+0000] [ALPM] reinstalled bash (5.3-1)\n[2026-11-08T00:03:50+0000] [ALPM] transaction completed\n[2026-11-09T00:00:00+0000] [PACMAN] Running 'pacman -Sy'\n[2026-11-09T00:00:01+0000] [ALPM] transaction started\n[2026-11-09T00:00:01+0000] [ALPM] transaction completed\n";

    /// /var/log/apt/history.log of debian:stable after `apt-get install
    /// --allow-downgrades zlib1g=...` (2026-11-19): the listing says
    /// "downgraded", and undoing it brings the higher version back.
    #[test]
    fn an_apt_downgrade_is_listed_as_one_and_undone_upwards() {
        let history = "\nStart-Date: 2026-11-19  12:32:46\nCommandline: apt-get install -y --allow-downgrades zlib1g=1:1.3.dfsg+really1.3.1-1+b1\nDowngrade: zlib1g:amd64 (1:1.3.dfsg+really1.3.1-1+deb13u1, 1:1.3.dfsg+really1.3.1-1+b1)\nEnd-Date: 2026-11-19  12:32:46\n";
        let t = last_apt_transaction(history).expect("a transaction");
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].action, Action::Downgrade);
        assert_eq!(
            reverse(&t.entries),
            vec![Step::Restore {
                name: "zlib1g:amd64".into(),
                version: "1:1.3.dfsg+really1.3.1-1+deb13u1".into(),
            }]
        );
    }

    #[test]
    fn unknown_installs_are_asked_about_never_assumed() {
        let none: Vec<String> = vec![];
        let some = vec!["ucode-intel".to_string()];
        assert_eq!(
            unknown_installs_decision(&none, false),
            UnknownInstalls::None
        );
        assert_eq!(
            unknown_installs_decision(&none, true),
            UnknownInstalls::None
        );
        assert_eq!(
            unknown_installs_decision(&some, true),
            UnknownInstalls::DryRunNote
        );
        assert_eq!(
            unknown_installs_decision(&some, false),
            UnknownInstalls::Ask
        );
        let text = unknown_installs_text(&some, "zypper dup", "/var/log/zypp/history");
        assert!(text.contains("ucode-intel") && text.contains("new dependency or"));
    }

    #[test]
    fn the_time_of_a_transaction_is_printed_like_a_timeline_time() {
        // pacman: an offset is given, so the moment is shown in UTC
        assert_eq!(
            when_label("2026-11-08T00:03:49+0000"),
            "2026-11-08 00:03 UTC"
        );
        assert_eq!(
            when_label("2026-11-08T01:03:49+0100"),
            "2026-11-08 00:03 UTC"
        );
        assert_eq!(
            when_label("2026-11-07T19:03:49-0500"),
            "2026-11-08 00:03 UTC"
        );
        // apt's history (two spaces) and the zypp history: no zone
        assert_eq!(
            when_label("2026-11-15  19:30:06"),
            "2026-11-15 19:30 (local)"
        );
        assert_eq!(
            when_label("2026-11-15 19:30:00"),
            "2026-11-15 19:30 (local)"
        );
        // pacman before 5.2 wrote minutes only, without a zone
        assert_eq!(when_label("2019-05-01 12:00"), "2019-05-01 12:00 (local)");
        // already in that form, or not a time this function knows
        assert_eq!(
            when_label("2026-11-15 19:30 (local)"),
            "2026-11-15 19:30 (local)"
        );
        assert_eq!(when_label("yesterday"), "yesterday");
        assert_eq!(
            when_label("2026-11-15 19:30:00 CET"),
            "2026-11-15 19:30:00 CET"
        );
        assert_eq!(when_label(""), "");
        let t = Transaction {
            when: "2026-11-08T00:03:49+0000".into(),
            command: "pacman -Syu".into(),
            entries: vec![],
        };
        assert!(
            describe(&t).starts_with("Last transaction: 2026-11-08 00:03 UTC  (pacman -Syu)"),
            "{}",
            describe(&t)
        );
    }

    #[test]
    fn last_pacman_transaction_skips_empty_ones_and_is_reversed() {
        let t = last_pacman_transaction(PACMAN).unwrap();
        assert_eq!(t.command, "pacman -Syu");
        assert_eq!(t.when, "2026-11-08T00:03:49+0000");
        assert_eq!(t.entries.len(), 4);
        assert_eq!(
            reverse(&t.entries),
            vec![
                Step::Remove {
                    name: "newdep".into()
                },
                Step::Restore {
                    name: "oldlib".into(),
                    version: "1.0-1".into()
                },
                Step::Restore {
                    name: "curl".into(),
                    version: "8.9.0-1".into()
                },
            ]
        );
        assert!(last_pacman_transaction("nothing here\n").is_none());
    }

    #[test]
    fn last_apt_transaction_reads_every_action() {
        let h = "\nStart-Date: 2026-11-14  10:00:00\nCommandline: apt-get install -y tree\nInstall: tree:amd64 (2.2.1-1)\nEnd-Date: 2026-11-14  10:00:01\n\nStart-Date: 2026-11-15  19:30:06\nCommandline: apt upgrade\nInstall: libnew:amd64 (1.0, automatic), other:amd64 (2.0)\nUpgrade: curl:amd64 (8.9.0-1, 8.10.0-1), libc6:amd64 (2.41-1, 2.41-2)\nRemove: old:amd64 (0.9)\nEnd-Date: 2026-11-15  19:30:09\n";
        let t = last_apt_transaction(h).unwrap();
        assert_eq!(t.command, "apt upgrade");
        assert_eq!(t.when, "2026-11-15  19:30:06");
        let names: Vec<&str> = t.entries.iter().map(|e| e.name.as_str()).collect();
        // the architecture is part of the name: it is never dropped
        assert_eq!(
            names,
            [
                "libnew:amd64",
                "other:amd64",
                "curl:amd64",
                "libc6:amd64",
                "old:amd64"
            ]
        );
        assert_eq!(t.entries[0].new_version, "1.0");
        assert_eq!(t.entries[2].old_version, "8.9.0-1");
        assert_eq!(t.entries[2].new_version, "8.10.0-1");
        assert_eq!(t.entries[4].old_version, "0.9");
        let steps = reverse(&t.entries);
        assert_eq!(steps.len(), 5);
        assert_eq!(
            steps[0],
            Step::Restore {
                name: "old:amd64".into(),
                version: "0.9".into()
            }
        );
        let single = last_apt_transaction("Start-Date: 2026-11-15  19:30:06\nCommandline: apt-get install -y tree\nInstall: tree:amd64 (2.2.1-1)\nEnd-Date: x\n").unwrap();
        assert_eq!(single.entries.len(), 1);
        assert_eq!(single.when, "2026-11-15  19:30:06");
        assert!(last_apt_transaction("").is_none());
    }

    // Written from the documented format of /var/log/zypp/history and of
    // `zypper search -s`; there is no openSUSE fixture in fixtures/native yet.
    const ZYPP: &str = "\
# history
2026-11-14 10:00:00|command|root@host|'zypper' 'install' 'tree' 'curl' 'oldlib'|
2026-11-14 10:00:02|install|tree|2.1.1-1.2|x86_64|root@host|repo-oss|0123abcd|
2026-11-14 10:00:03|install|curl|8.9.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-14 10:00:04|install|oldlib|1.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:00|command|root@host|'zypper' 'dup'|
2026-11-15 19:30:06|install|curl|8.10.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:07|remove |oldlib|1.0-1.1|x86_64|root@host|
2026-11-15 19:30:08|install|newdep|2.0-1.1|x86_64|root@host|repo-oss|0123abcd|
2026-11-15 19:30:09|install|tree|2.1.1-1.2|x86_64|root@host|repo-oss|0123abcd|
2026-11-16 09:00:00|command|root@host|'zypper' 'refresh'|
";

    #[test]
    fn last_zypper_transaction_is_the_last_run_that_changed_packages() {
        let t = last_zypper_transaction(ZYPP).unwrap();
        assert_eq!(t.command, "zypper dup");
        assert_eq!(t.when, "2026-11-15 19:30:00");
        let seen: Vec<(Action, &str)> = t
            .entries
            .iter()
            .map(|e| (e.action, e.name.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                (Action::Upgrade, "curl"),
                (Action::Remove, "oldlib"),
                (Action::Install, "newdep"),
                (Action::Reinstall, "tree"),
            ]
        );
        assert_eq!(
            reverse(&t.entries),
            vec![
                Step::Remove {
                    name: "newdep".into()
                },
                Step::Restore {
                    name: "oldlib".into(),
                    version: "1.0-1.1".into()
                },
                Step::Restore {
                    name: "curl".into(),
                    version: "8.9.0-1.1".into()
                },
            ]
        );
        assert!(last_zypper_transaction("# nothing\n").is_none());
        let only_commands = "2026-11-16 09:00:00|command|root@host|'zypper' 'refresh'|\n";
        assert!(last_zypper_transaction(only_commands).is_none());
    }

    #[test]
    fn zypper_records_without_a_command_are_grouped_by_minute() {
        let h = "\
2026-11-14 10:00:02|install|tree|2.1.1-1.2|x86_64||repo-oss|0123abcd|
2026-11-15 19:30:58|install|curl|8.10.0-1.1|x86_64||repo-oss|0123abcd|
2026-11-15 19:31:02|install|newdep|2.0-1.1|x86_64||repo-oss|0123abcd|
";
        let t = last_zypper_transaction(h).unwrap();
        assert_eq!(t.command, "");
        assert_eq!(t.when, "2026-11-15 19:30 (local)");
        let names: Vec<&str> = t.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["curl", "newdep"]);
        let one = last_zypper_transaction(
            "2026-11-14 10:00:02|install|tree|2.1.1-1.2|x86_64||repo-oss|0123abcd|\n",
        )
        .unwrap();
        assert_eq!(one.when, "2026-11-14 10:00:02");
        assert_eq!(one.entries.len(), 1);
    }

    #[test]
    fn zypper_search_table_is_matched_on_name_type_version_and_repository() {
        let table = "\
Loading repository data...
Reading installed packages...

S  | Name      | Type       | Version    | Arch   | Repository
---+-----------+------------+------------+--------+------------------
i+ | curl      | package    | 8.10.0-1.1 | x86_64 | Main Repository
v  | curl      | package    | 8.9.0-1.1  | x86_64 | Main Repository
   | curl      | srcpackage | 8.8.0-1.1  | noarch | Source Repository
i  | curl      | package    | 8.7.0-1.1  | x86_64 | (System Packages)
   | curl-mini | package    | 8.6.0-1.1  | x86_64 | Main Repository
";
        assert!(zypper_search_lists(table, "curl", "8.9.0-1.1"));
        assert!(zypper_search_lists(table, "curl", "8.10.0-1.1"));
        assert!(!zypper_search_lists(table, "curl", "8.8.0-1.1"));
        assert!(!zypper_search_lists(table, "curl", "8.7.0-1.1"));
        assert!(!zypper_search_lists(table, "curl", "8.6.0-1.1"));
        assert!(!zypper_search_lists(table, "curl", "Version"));
        assert!(!zypper_search_lists(
            "No matching items found.\n",
            "curl",
            "8.9.0-1.1"
        ));
    }

    #[test]
    fn rpm_erase_test_names_the_packages_that_need_the_removed_ones() {
        // rpm 4.20.1 in openSUSE Tumbleweed 20261111, `rpm -e --test`
        let one = "error: Failed dependencies:\n\tbuild-key is needed by (installed) patterns-base-minimal_base-20241218-37.1.x86_64\n";
        assert_eq!(
            rpm_erase_dependents(one),
            ["patterns-base-minimal_base-20241218-37.1.x86_64"]
        );
        let twice = "error: Failed dependencies:\n\tlibzypp >= 17.38.17 is needed by (installed) zypper-1.14.102-1.1.x86_64\n\tlibzypp(econf) is needed by (installed) zypp-no-recommends-4.0+git31-1.1.x86_64\n\tlibzypp.so.1735()(64bit) is needed by (installed) zypper-1.14.102-1.1.x86_64\n";
        assert_eq!(
            rpm_erase_dependents(twice),
            [
                "zypper-1.14.102-1.1.x86_64",
                "zypp-no-recommends-4.0+git31-1.1.x86_64"
            ]
        );
        assert!(rpm_erase_dependents("").is_empty());
        assert!(rpm_erase_dependents("error: package tree is not installed\n").is_empty());
    }

    #[test]
    fn cache_and_archive_names() {
        let files: Vec<String> = [
            "curl-8.9.0-1-x86_64.pkg.tar.zst",
            "curl-8.9.0-1-x86_64.pkg.tar.zst.sig",
            "curl-git-8.9.0-1-x86_64.pkg.tar.zst",
            "lib-1-2-debug-1-any.pkg.tar.xz",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            pacman_cache_match(&files, "curl", "8.9.0-1").as_deref(),
            Some("curl-8.9.0-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(pacman_cache_match(&files, "curl", "8.8.0-1"), None);
        // "lib 1-2" must not match the package "lib-1-2-debug"
        assert_eq!(pacman_cache_match(&files, "lib", "1-2"), None);
        assert_eq!(
            arch_archive_urls("curl", "1:8.9.0-1", "x86_64")[0],
            "https://archive.archlinux.org/packages/c/curl/curl-1%3A8.9.0-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            deb_file_name("vim", "2:9.1-1", "amd64"),
            "vim_2%3a9.1-1_amd64.deb"
        );
        assert_eq!(
            launchpad_urls("vim", "2:9.1-1", "amd64")[1],
            "https://launchpad.net/ubuntu/+archive/primary/+files/vim_9.1-1_all.deb"
        );
    }

    #[test]
    fn apt_cache_show_from_dpkg_status_is_not_a_repository() {
        // apt 2.4.15 in Ubuntu 22.04, `apt-cache show vim-common=2:8.2.3995-1ubuntu2.15`
        // after `apt-get remove vim-common`: exit 0, answered from dpkg's status file
        let removed = "Package: vim-common\nStatus: deinstall ok config-files\nPriority: important\nSection: editors\nInstalled-Size: 382\nArchitecture: all\nMulti-Arch: foreign\nSource: vim\nVersion: 2:8.2.3995-1ubuntu2.15\nConfig-Version: 2:8.2.3995-1ubuntu2.15\nDepends: xxd\nConffiles:\n /etc/vim/vimrc e782ef3054004f773d556475cfad5870\nDescription: Vi IMproved - Common files\nDescription-md5: 30b64c3dceccc4faef229eafc38d0069\n\n";
        assert!(!apt_show_is_downloadable(removed));
        // the same command for the version the release pocket still has
        let in_repo = "Package: vim-common\nArchitecture: all\nVersion: 2:8.2.3995-1ubuntu2\nMulti-Arch: foreign\nPriority: important\nSection: editors\nSource: vim\nOrigin: Ubuntu\nInstalled-Size: 376\nDepends: xxd\nFilename: pool/main/v/vim/vim-common_8.2.3995-1ubuntu2_all.deb\nSize: 81500\nMD5sum: 4431c4babd9cde1c0e368326b965157c\nDescription: Vi IMproved - Common files\nDescription-md5: 30b64c3dceccc4faef229eafc38d0069\n\n";
        assert!(apt_show_is_downloadable(in_repo));
        // a version that is nowhere: apt-cache prints nothing
        assert!(!apt_show_is_downloadable(""));
        // the word inside another field does not count
        assert!(!apt_show_is_downloadable(
            "Package: x\nDescription: has a Filename: in it\n"
        ));
    }

    #[test]
    fn snapshot_answer() {
        let j: serde_json::Value = serde_json::from_str(
            r#"{"binary":"tree","result":[{"architecture":"arm64","hash":"00112233445566778899aabbccddeeff00112233"},{"architecture":"amd64","hash":"ffeeddccbbaa99887766554433221100ffeeddcc"}]}"#,
        )
        .unwrap();
        assert_eq!(
            snapshot_hash(&j, "amd64").as_deref(),
            Some("ffeeddccbbaa99887766554433221100ffeeddcc")
        );
        assert_eq!(snapshot_hash(&j, "riscv64"), None);
        let bad: serde_json::Value =
            serde_json::from_str(r#"{"result":[{"architecture":"amd64","hash":"../etc"}]}"#)
                .unwrap();
        assert_eq!(snapshot_hash(&bad, "amd64"), None);
    }

    #[test]
    fn dnf_failure_is_not_an_empty_history() {
        assert!(dnf_stderr_is_empty_history("Error: No transactions\n"));
        assert!(dnf_stderr_is_empty_history(
            "AttributeError: 'NoneType' object has no attribute 'tid'\n"
        ));
        assert!(!dnf_stderr_is_empty_history(
            "Error: Failed to download metadata for repo 'updates'\n"
        ));
        assert!(!dnf_stderr_is_empty_history(
            "Waiting for process with pid 123 to finish.\n"
        ));
    }

    /// A directory for one test, removed first so a failed earlier run does
    /// not matter.
    fn scratch(test: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yapacc-undo-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    // apt 3.0.3 in debian:stable (13), 2026-11-19, on a system with i386
    // added: `apt-get install -y libpng16-16t64:i386`. The foreign packages
    // came with an upgrade of the native zlib1g.
    const APT_I386: &str = "\nStart-Date: 2026-11-19  07:55:22\nCommandline: apt-get install -y libpng16-16t64:i386\nInstall: libunistring5:i386 (1.3-2, automatic), libidn2-0:i386 (2.3.8-2, automatic), libc6:i386 (2.41-12+deb13u4, automatic), zlib1g:i386 (1:1.3.dfsg+really1.3.1-1+deb13u1, automatic), libpng16-16t64:i386 (1.6.48-1+deb13u6), gcc-14-base:i386 (14.2.0-19, automatic), libgcc-s1:i386 (14.2.0-19, automatic)\nUpgrade: zlib1g:amd64 (1:1.3.dfsg+really1.3.1-1+b1, 1:1.3.dfsg+really1.3.1-1+deb13u1)\nEnd-Date: 2026-11-19  07:55:23\n";

    #[test]
    fn apt_undo_keeps_the_architecture_from_the_history_to_the_command() {
        let t = last_apt_transaction(APT_I386).unwrap();
        assert_eq!(t.entries.len(), 8);
        assert_eq!(t.entries[4].name, "libpng16-16t64:i386");
        assert_eq!(t.entries[7].name, "zlib1g:amd64");
        assert!(describe(&t).contains("installed   libpng16-16t64:i386 1.6.48-1+deb13u6"));
        valid(&t.entries, PkgManager::Apt).unwrap();
        let (removes, restores) =
            resolve(&reverse(&t.entries), |_, _| Ok(Some(Found::Repo))).unwrap();
        let c = apt_command(&removes, &restores, &Downloads::under(None)).unwrap();
        assert_eq!(
            c,
            [
                "apt-get",
                "install",
                "-y",
                "--allow-downgrades",
                "--",
                "zlib1g:amd64=1:1.3.dfsg+really1.3.1-1+b1",
                "libgcc-s1:i386-",
                "gcc-14-base:i386-",
                "libpng16-16t64:i386-",
                "zlib1g:i386-",
                "libc6:i386-",
                "libidn2-0:i386-",
                "libunistring5:i386-",
            ]
        );
        // the bug: a bare `libpng16-16t64-` is the amd64 package to apt
        assert!(!c.iter().any(|a| a == "libpng16-16t64-" || a == "zlib1g-"));
        // the package's own architecture names its file and its lookups
        assert_eq!(apt_split("libpng16-16t64:i386"), ("libpng16-16t64", "i386"));
        assert_eq!(apt_split("tree"), ("tree", ""));
        assert_eq!(
            deb_file_name("libpng16-16t64", "1.6.48-1+deb13u6", "i386"),
            "libpng16-16t64_1.6.48-1+deb13u6_i386.deb"
        );
        assert_eq!(
            launchpad_urls("libpng16-16t64", "1.6.43-5build1", "i386")[0],
            "https://launchpad.net/ubuntu/+archive/primary/+files/libpng16-16t64_1.6.43-5build1_i386.deb"
        );
        let j: serde_json::Value = serde_json::from_str(
            r#"{"result":[{"architecture":"amd64","hash":"ffeeddccbbaa99887766554433221100ffeeddcc"},{"architecture":"i386","hash":"00112233445566778899aabbccddeeff00112233"}],"fileinfo":{"00112233445566778899aabbccddeeff00112233":[{"name":"x_i386.deb","size":86876}]}}"#,
        )
        .unwrap();
        let hash = snapshot_hash(&j, "i386").unwrap();
        assert_eq!(hash, "00112233445566778899aabbccddeeff00112233");
        assert_eq!(snapshot_size(&j, &hash), Some(86876));
        assert_eq!(
            snapshot_size(&j, "ffeeddccbbaa99887766554433221100ffeeddcc"),
            None
        );
    }

    /// P7: an `Architecture: all` package was downloaded to a file named
    /// `..._amd64.deb`.
    #[test]
    fn a_downloaded_deb_is_named_as_the_archive_names_it() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let j: serde_json::Value = serde_json::from_str(
            r#"{"result":[{"architecture":"all","hash":"0123456789abcdef0123456789abcdef01234567"}],"fileinfo":{"0123456789abcdef0123456789abcdef01234567":[{"archive_name":"debian","name":"vim-common_9.1.1230-2_all.deb","path":"/pool/main/v/vim","size":412000}]}}"#,
        )
        .unwrap();
        assert_eq!(snapshot_hash(&j, "amd64").as_deref(), Some(hash));
        assert_eq!(
            snapshot_file_name(&j, hash, "vim-common", "2:9.1.1230-2", "amd64"),
            "vim-common_9.1.1230-2_all.deb"
        );
        // no usable name in the answer: the apt cache name, with the
        // architecture the archive gave for that hash
        let bare: serde_json::Value = serde_json::from_str(
            r#"{"result":[{"architecture":"all","hash":"0123456789abcdef0123456789abcdef01234567"}],"fileinfo":{"0123456789abcdef0123456789abcdef01234567":[{"name":"../../etc/x.deb"},{"name":"other_1_all.deb"},{"name":"vim-common_1_all.deb;id"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            snapshot_file_name(&bare, hash, "vim-common", "2:9.1.1230-2", "amd64"),
            "vim-common_2%3a9.1.1230-2_all.deb"
        );
        // nothing at all about the hash: the architecture that was asked for
        assert_eq!(
            snapshot_file_name(&serde_json::json!({}), hash, "tree", "2.2.1-1", "amd64"),
            "tree_2.2.1-1_amd64.deb"
        );
        // and whatever name comes out is one `Downloads` accepts
        let d = Downloads::under(Some(PathBuf::from("/x/cache/yapacc/undo")));
        assert!(d.path("vim-common_9.1.1230-2_all.deb").is_ok());
    }

    /// P8: a slow launchpad.net made an undo wait a minute for each
    /// lookup, without a retry, and then refuse with the words of "the
    /// archive does not have it".
    #[test]
    fn an_archive_that_does_not_answer_is_told_apart_from_one_that_has_not() {
        assert_eq!(
            classify_lookup(true, Some(81500u64), ""),
            Lookup::Found(Some(81500))
        );
        assert_eq!(
            classify_lookup(
                false,
                None::<u64>,
                "curl: (22) The requested URL returned error: 404\n"
            ),
            Lookup::Absent
        );
        for said in [
            "curl: (28) Connection timed out after 20002 milliseconds",
            "curl: (6) Could not resolve host: launchpad.net",
            "curl: (22) The requested URL returned error: 503",
        ] {
            assert_eq!(
                classify_lookup(false, None::<u64>, said),
                Lookup::NoAnswer(said.to_string())
            );
        }
        assert!(matches!(
            classify_lookup(false, None::<u64>, ""),
            Lookup::NoAnswer(_)
        ));
        // asked once more when there was no answer, and only then
        let mut calls = 0;
        let got = ask_twice(|| {
            calls += 1;
            if calls == 1 {
                Lookup::NoAnswer("timeout".into())
            } else {
                Lookup::Found(7)
            }
        });
        assert_eq!((got, calls), (Lookup::Found(7), 2));
        let mut calls = 0;
        let got: Lookup<u8> = ask_twice(|| {
            calls += 1;
            Lookup::NoAnswer(format!("timeout {calls}"))
        });
        assert_eq!((got, calls), (Lookup::NoAnswer("timeout 2".into()), 2));
        for answered in [Lookup::Absent, Lookup::Found(1)] {
            let mut calls = 0;
            let got = ask_twice(|| {
                calls += 1;
                answered.clone()
            });
            assert_eq!((got, calls), (answered.clone(), 1));
        }
        // 20 s to connect, 60 s in all
        assert_eq!(
            LOOKUP_TIMEOUTS,
            ["--connect-timeout", "20", "--max-time", "60"]
        );
        // the two refusals do not read alike
        let silent = archive_silent(
            "launchpad.net",
            "perl",
            "5.38.2-3.2ubuntu0.6",
            "curl: (28) Connection timed out after 20002 milliseconds",
        );
        assert_eq!(silent.kind.exit_code(), 3);
        assert_eq!(
            silent.to_string(),
            "cannot undo: launchpad.net did not answer when asked for perl 5.38.2-3.2ubuntu0.6 (curl: (28) Connection timed out after 20002 milliseconds), so it is not known whether it has that version. Nothing was changed. Try again later."
        );
        let absent = resolve(
            &[Step::Restore {
                name: "perl".into(),
                version: "5.38.2-3.2ubuntu0.6".into(),
            }],
            |_, _| Ok(None),
        )
        .unwrap_err();
        assert!(
            absent.to_string().contains("does not have it")
                && !absent.to_string().contains("did not answer"),
            "{absent}"
        );
        // an archive that did not answer stops the whole undo at once
        let stopped = resolve(
            &[
                Step::Restore {
                    name: "a".into(),
                    version: "1".into(),
                },
                Step::Restore {
                    name: "b".into(),
                    version: "1".into(),
                },
            ],
            |n, v| Err(archive_silent("launchpad.net", n, v, "timeout")),
        )
        .unwrap_err();
        assert!(stopped.to_string().contains("asked for a 1"), "{stopped}");
    }

    /// P6: after an undo an empty ~/.cache/yapacc directory remained.
    #[test]
    fn an_undo_leaves_no_empty_directory_in_the_cache() {
        let cache = scratch("parents");
        let run = cache.join("yapacc").join("undo").join("run-1-abc");
        create_private_dir(&run).unwrap();
        std::fs::remove_dir(&run).unwrap();
        remove_empty_parents(&run);
        assert!(!cache.join("yapacc").exists());
        assert!(
            cache.exists(),
            "the cache directory itself is never removed"
        );
        // something else lives in yapacc/: it stays, undo/ goes
        let run = cache.join("yapacc").join("undo").join("run-2-abc");
        create_private_dir(&run).unwrap();
        std::fs::write(cache.join("yapacc").join("repo-index.tsv"), "x").unwrap();
        std::fs::remove_dir(&run).unwrap();
        remove_empty_parents(&run);
        assert!(!cache.join("yapacc").join("undo").exists());
        assert!(cache.join("yapacc").join("repo-index.tsv").exists());
        // another run is still there: nothing is removed
        let other = cache.join("yapacc").join("undo").join("run-3-def");
        create_private_dir(&other).unwrap();
        let run = cache.join("yapacc").join("undo").join("run-4-abc");
        remove_empty_parents(&run);
        assert!(other.exists());
        // a directory of another name is not touched
        let odd = cache.join("elsewhere").join("run-5");
        std::fs::create_dir_all(&odd).unwrap();
        std::fs::remove_dir(&odd).unwrap();
        remove_empty_parents(&odd);
        assert!(cache.join("elsewhere").exists());
        // the whole thing through `Downloads`
        let home = scratch("parents-home");
        std::fs::create_dir_all(&home).unwrap();
        let mut d = Downloads::under(Some(home.join("yapacc").join("undo")));
        create_private_dir(d.dir.as_ref().unwrap()).unwrap();
        d.created = true;
        drop(d);
        assert!(!home.join("yapacc").exists());
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn apt_simulation_that_removes_more_than_planned_refuses() {
        let planned: Vec<String> = ["libpng16-16t64:i386", "ca-certificates:amd64", "tree:amd64"]
            .map(String::from)
            .into();
        // apt 3.0.3, debian:stable: a foreign package carries its
        // architecture, a native or `all` one does not
        let good = "Reading package lists...\nBuilding dependency tree...\nThe following packages will be REMOVED:\n  ca-certificates libpng16-16t64:i386 tree\n0 upgraded, 0 newly installed, 3 to remove and 0 not upgraded.\nRemv ca-certificates [20250419+deb13u1]\nRemv libpng16-16t64:i386 [1.6.48-1+deb13u6]\nRemv tree [2.2.1-1]\n";
        assert_eq!(
            apt_simulated_removals(good),
            ["ca-certificates", "libpng16-16t64:i386", "tree"]
        );
        assert_eq!(apt_simulation_refusal(0, good, &planned, "amd64"), None);
        // what the dropped architecture made apt do: remove the native
        // library and what needs it, and keep the i386 one
        let wrong = "Remv libfreetype6 [2.13.3+dfsg-1]\nRemv libpng16-16t64 [1.6.48-1+deb13u6]\n";
        let why = apt_simulation_refusal(0, wrong, &planned, "amd64").unwrap();
        assert!(
            why.contains("would also remove libfreetype6, libpng16-16t64,")
                && why.contains("Nothing was changed"),
            "{why}"
        );
        // on an i386 system the same lines are the planned package
        assert_eq!(
            apt_unplanned_removals(wrong, &planned, "i386"),
            ["libfreetype6"]
        );
        // a failed simulation refuses whatever it printed
        let failed = apt_simulation_refusal(100, "", &planned, "amd64").unwrap();
        assert!(failed.contains("simulation of the undo failed (status 100)"));
        // removing less than planned (already gone) is not a refusal
        assert_eq!(
            apt_simulation_refusal(0, "Remv tree [2.2.1-1]\n", &planned, "amd64"),
            None
        );
        // a planned name without an architecture is the native package
        assert!(
            apt_unplanned_removals("Remv tree [1]\n", &["tree".to_string()], "amd64").is_empty()
        );
        assert_eq!(
            apt_unplanned_removals("Remv tree:i386 [1]\n", &["tree".to_string()], "amd64"),
            ["tree:i386"]
        );
    }

    #[test]
    fn apt_essential_removals_are_allowed_only_for_foreign_packages_of_the_transaction() {
        // apt 3.0.4 (debian:stable) and 2.6.2 (debian:12), LC_ALL=C:
        // `apt-get install -s -y --allow-downgrades --allow-remove-essential -- ...`
        let sim = "The following packages will be REMOVED:\n  gcc-14-base:i386 libc6:i386 libgcc-s1:i386 libidn2-0:i386\n  libpng16-16t64:i386 libunistring5:i386 zlib1g:i386\nWARNING: The following essential packages will be removed.\nThis should NOT be done unless you know exactly what you are doing!\n  libgcc-s1:i386 gcc-14-base:i386 (due to libgcc-s1:i386)\n  libc6:i386 (due to libgcc-s1:i386)\n0 upgraded, 0 newly installed, 7 to remove and 0 not upgraded.\nRemv libpng16-16t64:i386 [1.6.48-1+deb13u6]\nRemv libc6:i386 [2.41-12+deb13u4] [libgcc-s1:i386 ]\n";
        let essential = apt_essential_removals(sim);
        assert_eq!(
            essential,
            ["libgcc-s1:i386", "gcc-14-base:i386", "libc6:i386"]
        );
        assert!(apt_essential_removals("Remv tree [2.2.1-1]\n").is_empty());
        let planned: Vec<String> = [
            "libgcc-s1:i386",
            "gcc-14-base:i386",
            "libpng16-16t64:i386",
            "zlib1g:i386",
            "libc6:i386",
        ]
        .map(String::from)
        .into();
        // foreign packages the transaction installed: allowed
        assert_eq!(
            apt_essential_removable(&essential, &planned, "amd64"),
            Ok(())
        );
        // the same names on a system where i386 is the native architecture
        assert_eq!(
            apt_essential_removable(&essential, &planned, "i386"),
            Err(essential.clone())
        );
        // a native essential package is never allowed, planned or not
        let native = vec!["libgcc-s1".to_string(), "libc6:i386".to_string()];
        let mut with_native = planned.clone();
        with_native.push("libgcc-s1:amd64".into());
        assert_eq!(
            apt_essential_removable(&native, &with_native, "amd64"),
            Err(vec!["libgcc-s1".to_string()])
        );
        // a foreign essential package the transaction did not install
        assert_eq!(
            apt_essential_removable(&essential, &planned[..2], "amd64"),
            Err(vec!["libc6:i386".to_string()])
        );
        // nothing essential listed: there is nothing this option would fix
        assert_eq!(apt_essential_removable(&[], &planned, "amd64"), Err(vec![]));
        // the simulation is the command itself plus `-s`
        let c: Vec<String> = [
            "apt-get",
            "install",
            "-y",
            "--allow-downgrades",
            "--",
            "tree:amd64-",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            apt_simulation_of(&c),
            [
                "apt-get",
                "install",
                "-s",
                "-y",
                "--allow-downgrades",
                "--",
                "tree:amd64-"
            ]
        );
    }

    fn entry(name: &str, old: &str, new: &str) -> LogEntry {
        LogEntry {
            ts: 0,
            source: "test".into(),
            action: Action::Upgrade,
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
        }
    }

    #[test]
    fn names_and_versions_from_the_log_are_checked_before_any_command() {
        for good in [
            "1:1.3.dfsg+really1.3.1-1+deb13u1",
            "2.41-12+deb13u4",
            "1.0~rc1-1",
            "8.9.0-1",
            "2.1.1-1.2",
            "1.0^20260101git1a2b3c4",
            "5.3_p3-r1",
        ] {
            assert!(valid_version(good), "{good}");
        }
        for bad in [
            "",
            "1.0/../../etc",
            "1.0 2",
            "1.0;id",
            "$(id)",
            "1.0\n2",
            "1.0%2f",
            "1.0?x=1",
            "1.0#frag",
            "é",
        ] {
            assert!(!valid_version(bad), "{bad:?}");
        }
        assert!(!valid_version(&"1".repeat(257)));
        for m in [PkgManager::Apt, PkgManager::Pacman, PkgManager::Zypper] {
            valid(&[entry("curl", "8.9.0-1", "8.10.0-1")], m).unwrap();
            let e = valid(&[entry("curl", "8.9.0-1/../x", "8.10.0-1")], m).unwrap_err();
            assert_eq!(e.kind, crate::error::Kind::Refused);
            assert!(e.message.contains("'8.9.0-1/../x'"), "{}", e.message);
            // the new version is used too (zypper `name=version`)
            let e = valid(&[entry("curl", "8.9.0-1", "8.10.0-1 --root=/x")], m).unwrap_err();
            assert_eq!(e.kind, crate::error::Kind::Refused);
            let e = valid(&[entry("--root=/tmp", "1", "2")], m).unwrap_err();
            assert_eq!(e.kind, crate::error::Kind::Refused);
        }
        // `name:arch` is a name on apt only, and only with a real architecture
        valid(&[entry("libc6:i386", "1", "2")], PkgManager::Apt).unwrap();
        assert!(valid(&[entry("libc6:i386", "1", "2")], PkgManager::Pacman).is_err());
        assert!(valid(&[entry("libc6:../x", "1", "2")], PkgManager::Apt).is_err());
        assert!(valid(&[entry("libc6:i386=9", "1", "2")], PkgManager::Apt).is_err());
    }

    #[test]
    fn dnf_undo_names_the_transaction_it_showed() {
        // dnf5 5.4.6 (Fedora) and dnf 4.14 (Rocky 9), LC_ALL=C
        let dnf5 = "Transaction ID : 7\nBegin time     : 2026-11-19 08:00:00\nUser           : 0 root\nDescription    : dnf install -y tree\n";
        let dnf4 = "Transaction ID : 12\nBegin time     : Thu Nov 19 08:00:00 2026\nCommand Line   : install -y tree\n";
        assert_eq!(dnf_transaction_id(dnf5).as_deref(), Some("7"));
        assert_eq!(dnf_transaction_id(dnf4).as_deref(), Some("12"));
        assert_eq!(dnf_undo_command("7"), ["dnf", "history", "undo", "7", "-y"]);
        assert!(!dnf_undo_command("7").iter().any(|a| a == "last"));
        // no ID, a range, or anything that is not one number: no command
        for bad in [
            "",
            "Description : Transaction ID : 9\n",
            "Transaction ID : 3..5\n",
            "Transaction ID : last\n",
            "Transaction ID : -1\n",
            "Transaction ID :\n",
            "Transaction ID : 7 --setopt=x\n",
        ] {
            assert_eq!(dnf_transaction_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn downloads_go_under_the_users_cache_and_never_under_tmp() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            download_parent(os("/x/cache"), os("/home/u")),
            Some(PathBuf::from("/x/cache/yapacc/undo"))
        );
        assert_eq!(
            download_parent(None, os("/home/u")),
            Some(PathBuf::from("/home/u/.cache/yapacc/undo"))
        );
        // a relative or empty value is not a place; nothing set: no place
        assert_eq!(
            download_parent(os("cache"), os("/home/u")),
            Some(PathBuf::from("/home/u/.cache/yapacc/undo"))
        );
        assert_eq!(download_parent(os(""), os("")), None);
        assert_eq!(download_parent(None, None), None);
        // with no place, a version that must be downloaded is a refusal
        let none = Downloads::under(None);
        let e = none.path("x.deb").unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Refused);
        assert!(
            e.message.contains("neither XDG_CACHE_HOME nor HOME") && e.message.contains("/tmp"),
            "{}",
            e.message
        );
        // a file name from a URL never leaves the directory
        let some = Downloads::under(Some(PathBuf::from("/x/cache/yapacc/undo")));
        assert!(some.path("../x.deb").is_err());
        assert!(some.path("a/b.deb").is_err());
        assert!(some.path("").is_err());
        let p = some.path("x_1_i386.deb").unwrap();
        assert!(p.starts_with("/x/cache/yapacc/undo") && p.ends_with("x_1_i386.deb"));
    }

    fn arch_remote(url: &str) -> Remote {
        Remote {
            url: url.to_string(),
            file: "curl-8.9.0-1-x86_64.pkg.tar.zst".into(),
            size: Some(1536),
            check: Check::PacmanSignature,
        }
    }

    #[test]
    fn planning_and_a_dry_run_write_nothing() {
        let parent = scratch("dryrun").join("yapacc").join("undo");
        let mut downloads = Downloads::under(Some(parent.clone()));
        let restores = vec![(
            "curl".to_string(),
            "8.9.0-1".to_string(),
            Found::Remote(arch_remote(
                "https://archive.archlinux.org/packages/c/curl/curl-8.9.0-1-x86_64.pkg.tar.zst",
            )),
        )];
        // the plan names the place the file will be in, and creates nothing
        let commands = pacman_commands(&["newdep".to_string()], &restores, &downloads).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0][..4], ["pacman", "-U", "--noconfirm", "--"]);
        let planned = PathBuf::from(&commands[0][4]);
        assert!(
            planned.starts_with(&parent) && planned.ends_with("curl-8.9.0-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(commands[1], ["pacman", "-R", "--noconfirm", "--", "newdep"]);
        let c = apt_command(&[], &restores, &downloads).unwrap();
        assert_eq!(c[5], planned.display().to_string());
        // what the plan says about it
        let line = remote_line("curl", "8.9.0-1", &arch_remote("https://a/x"));
        assert!(
            line.contains("to download, 1.5 KiB: https://a/x")
                && line.contains("signature is verified with pacman-key"),
            "{line}"
        );
        let note = downloads_note(&remotes(&restores)).unwrap();
        assert!(
            note.contains("1 file (1.5 KiB) to download")
                && note.contains("Nothing has been downloaded"),
            "{note}"
        );
        assert_eq!(downloads_note(&[]), None);
        // a dry run returns before the gate: the download step never runs
        let mut fetched = false;
        let dry = GateFlags {
            dryrun: true,
            yes: true,
            ..Default::default()
        };
        finish(PkgManager::Pacman, &commands, &[], dry, &mut || {
            fetched = true;
            downloads.fetch(&restores)
        })
        .unwrap();
        assert!(!fetched, "a dry run must not download");
        drop(downloads);
        assert!(!parent.exists(), "a dry run must not create {parent:?}");
        assert!(!scratch("dryrun").exists());
    }

    /// D4: Fedora, the undo of a full upgrade: `dnf history undo N -y`
    /// exited 1 because two scriptlets failed, with every package version
    /// restored. The message said only "exited with status 1".
    #[test]
    fn a_failed_undo_says_whether_the_versions_were_restored() {
        // the transaction, as `dnf history info` printed it (dnf 5)
        let info = "Transaction ID : 12\nBegin time     : 2026-11-18 10:00:00\nPackages altered:\n  Action   Package                               Reason     Repository\n  Install  kernel-core-0:7.3.4-300.fc45.x86_64   User       updates\n  Upgrade  7zip-0:26.06-1.fc45.x86_64            Dependency updates\n  Upgrade  dbus-1:1.16.4-1.fc45.x86_64           Dependency updates\n  Remove   oldlib-0:1.0-1.fc45.x86_64            Dependency @System\n  Replaced 7zip-0:26.03-1.fc45.x86_64            Dependency @System\n  Replaced dbus-1:1.16.2-6.fc45.x86_64           Dependency @System\n";
        let expected = expected_from_entries(&pkglog::parse_dnf_history_info(info));
        assert_eq!(
            expected,
            vec![
                Expected::Absent {
                    name: "kernel-core".into(),
                    version: Some("7.3.4-300.fc45".into())
                },
                Expected::Present {
                    name: "7zip".into(),
                    version: "26.03-1.fc45".into()
                },
                Expected::Present {
                    name: "dbus".into(),
                    version: "1:1.16.2-6.fc45".into()
                },
                Expected::Present {
                    name: "oldlib".into(),
                    version: "1.0-1.fc45".into()
                },
            ]
        );
        // every version restored: `rpm -qa` after the undo (an older
        // kernel stays installed beside the one that was removed)
        let restored = parse_installed_state(
            PkgManager::Dnf,
            "kernel-core\t7.3.3-300.fc45\t7.3.3-300.fc45\n7zip\t26.03-1.fc45\t26.03-1.fc45\ndbus\t1:1.16.2-6.fc45\t1.16.2-6.fc45\noldlib\t1.0-1.fc45\t1.0-1.fc45\n",
        );
        let checked = verify_state(&expected, &restored);
        assert_eq!(checked, (4, vec![]));
        assert_eq!(
            failed_undo_outcome("dnf", Some(&checked)),
            "The package versions were restored (4 of 4 verified), but dnf reported an error (often a package script that failed): see the dnf output above."
        );
        assert_eq!(
            succeeded_undo_outcome(Some(&checked)).unwrap(),
            "Verified: 4 of 4 package(s) are at the planned state."
        );
        // nothing was restored
        let untouched = parse_installed_state(
            PkgManager::Dnf,
            "kernel-core\t7.3.3-300.fc45\t7.3.3-300.fc45\nkernel-core\t7.3.4-300.fc45\t7.3.4-300.fc45\n7zip\t26.06-1.fc45\t26.06-1.fc45\ndbus\t1:1.16.4-1.fc45\t1.16.4-1.fc45\n",
        );
        let checked = verify_state(&expected, &untouched);
        assert_eq!(checked.0, 0);
        assert_eq!(
            checked.1,
            [
                "kernel-core 7.3.4-300.fc45 is still installed",
                "7zip is at 26.06-1.fc45 (planned: 26.03-1.fc45)",
                "dbus is at 1:1.16.4-1.fc45 (planned: 1:1.16.2-6.fc45)",
                "oldlib is not installed (planned: 1.0-1.fc45)",
            ]
        );
        let said = failed_undo_outcome("dnf", Some(&checked));
        assert!(
            said.starts_with("4 of 4 package(s) are not at the planned state: kernel-core 7.3.4-300.fc45 is still installed; "),
            "{said}"
        );
        assert!(
            said.ends_with("`yapacc timeline` shows the current state."),
            "{said}"
        );
        // commands that reported no error and did not do it: an error
        let e = succeeded_undo_outcome(Some(&checked)).unwrap_err();
        assert!(
            e.starts_with("the undo commands reported no error, but 4 of 4"),
            "{e}"
        );
        // the packages could not be listed: nothing is claimed
        assert_eq!(
            failed_undo_outcome("apt", None),
            "`yapacc timeline` shows the current state."
        );
        assert!(
            succeeded_undo_outcome(None)
                .unwrap()
                .contains("was not verified")
        );
    }

    #[test]
    fn the_planned_state_is_read_for_every_manager() {
        // pacman: `pacman -Q`
        let plan = expected_from_plan(
            &["newdep".to_string()],
            &[("curl".to_string(), "8.9.0-1".to_string())],
        );
        let state = parse_installed_state(PkgManager::Pacman, "curl 8.9.0-1\nbash 5.3.20-1\n");
        assert_eq!(verify_state(&plan, &state), (2, vec![]));
        let state = parse_installed_state(PkgManager::Pacman, "curl 8.10.0-1\nnewdep 2.0-1\n");
        assert_eq!(
            verify_state(&plan, &state).1,
            [
                "curl is at 8.10.0-1 (planned: 8.9.0-1)",
                "newdep is still installed (2.0-1)"
            ]
        );
        // apt: names carry their architecture, and an arch-all package is
        // written with the native one in apt's history
        let plan = expected_from_plan(
            &["libpng16-16t64:i386".to_string()],
            &[
                ("vim-common:amd64".to_string(), "2:9.1.0016-1".to_string()),
                ("tree".to_string(), "2.2.1-1".to_string()),
            ],
        );
        let mut state = parse_installed_state(
            PkgManager::Apt,
            "vim-common\tall\t2:9.1.0016-1\tii \ntree\tamd64\t2.2.1-1\tii \nlibpng16-16t64\tamd64\t1.6.48-1\tii \nlibpng16-16t64\ti386\t1.6.48-1\trc \n",
        );
        alias_arch_all(&mut state, "amd64");
        assert_eq!(verify_state(&plan, &state), (3, vec![]));
        // zypper: one version of several goes, and the history has no epoch
        let plan = expected_from_plan(
            &["kernel-default=7.3.4-1.1".to_string()],
            &[("hello".to_string(), "1.0-68.1".to_string())],
        );
        let state = parse_installed_state(
            PkgManager::Zypper,
            "kernel-default\t7.3.3-1.1\t7.3.3-1.1\nhello\t1:1.0-68.1\t1.0-68.1\n",
        );
        assert_eq!(verify_state(&plan, &state), (2, vec![]));
        // more than eight: the rest is counted
        let many: Vec<String> = (0..11).map(|i| format!("p{i} is not installed")).collect();
        assert!(listed(&many).ends_with("p7 is not installed; and 3 more"));
    }

    #[test]
    fn the_download_directory_is_new_private_and_deleted_afterwards() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("private");
        let parent = root.join("yapacc").join("undo");
        let dir = parent.join("run-1-abc");
        create_private_dir(&dir).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&parent), 0o700);
        // a directory that is already there is never reused, whatever is in it
        std::fs::write(dir.join("curl-8.9.0-1-x86_64.pkg.tar.zst"), "planted").unwrap();
        let e = create_private_dir(&dir).unwrap_err();
        assert!(e.message.contains("cannot create"), "{}", e.message);
        // nor is a symlink of that name followed
        let link = parent.join("run-2-abc");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(create_private_dir(&link).is_err());
        // a parent left open by an earlier version is closed
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o775)).unwrap();
        create_private_dir(&parent.join("run-3-abc")).unwrap();
        assert_eq!(mode(&parent), 0o700);
        // the run's directory goes away with the run; others stay
        let mut downloads = Downloads::under(Some(parent.clone()));
        let own = downloads.dir.clone().unwrap();
        create_private_dir(&own).unwrap();
        downloads.created = true;
        std::fs::write(own.join("x.deb"), "x").unwrap();
        drop(downloads);
        assert!(!own.exists());
        assert!(dir.exists());
        // stale directories of killed runs are swept, by name, links untouched
        std::fs::create_dir(parent.join("keep")).unwrap();
        sweep_stale_runs(&parent, STALE_RUN_SECS);
        assert!(dir.exists(), "a fresh run directory is kept");
        sweep_stale_runs(&parent, 0);
        assert!(!dir.exists() && !parent.join("run-3-abc").exists());
        assert!(parent.join("keep").exists());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_arch_archive_package_without_a_signature_is_an_integrity_failure() {
        let e = arch_signature_refusal("curl", "8.9.0-1", "its signature could not be fetched");
        assert_eq!(e.kind, crate::error::Kind::Integrity);
        assert_eq!(e.kind.exit_code(), 5);
        assert!(
            e.message.contains("curl 8.9.0-1") && e.message.contains("Nothing was changed"),
            "{}",
            e.message
        );
        // the real path, with curl reading local files: the package is
        // there, its `.sig` is not
        if !crate::util::have("curl") {
            return;
        }
        let root = scratch("nosig");
        std::fs::create_dir_all(root.join("dl")).unwrap();
        let source = root.join("curl-8.9.0-1-x86_64.pkg.tar.zst");
        std::fs::write(&source, "not a package").unwrap();
        let dest = root.join("dl").join("curl-8.9.0-1-x86_64.pkg.tar.zst");
        let remote = arch_remote(&format!("file://{}", source.display()));
        let e = fetch_one("curl", "8.9.0-1", &remote, &dest).unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Integrity);
        assert!(
            e.message.contains("signature could not be fetched"),
            "{}",
            e.message
        );
        assert!(!dest.exists(), "the unsigned file must not be kept");
        // a file that cannot be downloaded at all is a refusal
        let gone = arch_remote(&format!("file://{}", root.join("absent").display()));
        let e = fetch_one("curl", "8.9.0-1", &gone, &dest).unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Refused);
        assert!(!dest.exists() && !part_path(&dest).exists());
        // a wrong hash (snapshot.debian.org) is an integrity failure too
        if crate::util::have("sha1sum") {
            let deb = Remote {
                url: format!("file://{}", source.display()),
                file: "x_1_amd64.deb".into(),
                size: None,
                check: Check::Sha1("00112233445566778899aabbccddeeff00112233".into()),
            };
            let dest = root.join("dl").join("x_1_amd64.deb");
            let e = fetch_one("x", "1", &deb, &dest).unwrap_err();
            assert_eq!(e.kind, crate::error::Kind::Integrity);
            assert!(!dest.exists());
            // sha1 of "not a package"
            let right = Remote {
                check: Check::Sha1(sha1_of(&source)),
                ..deb
            };
            fetch_one("x", "1", &right, &dest).unwrap();
            assert!(dest.exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    fn sha1_of(path: &Path) -> String {
        let out = Command::new("sha1sum")
            .arg("--")
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }

    #[test]
    fn head_answers_give_the_size_of_the_last_response() {
        // curl -sSIL: one header block for the redirect, one for the file
        let h = "HTTP/2 303 \r\nlocation: https://launchpadlibrarian.net/1/x.deb\r\ncontent-length: 0\r\n\r\nHTTP/2 200 \r\ncontent-type: application/x-debian-package\r\nContent-Length: 81500\r\n\r\n";
        assert_eq!(content_length(h), Some(81500));
        assert_eq!(
            content_length("HTTP/1.1 200 OK\ncontent-length: 12\n\n"),
            Some(12)
        );
        assert_eq!(content_length("HTTP/1.1 200 OK\r\n\r\n"), None);
        assert_eq!(content_length(""), None);
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\ncontent-length: big\r\n\r\n"),
            None
        );
    }

    #[test]
    fn undo_follows_the_safety_snapshot_setting() {
        use crate::snapshot::Mode;
        assert_eq!(snapshot_mode("off").unwrap(), Mode::Off);
        assert_eq!(snapshot_mode("auto").unwrap(), Mode::Auto);
        assert_eq!(
            snapshot_mode("auto-best-effort").unwrap(),
            Mode::AutoBestEffort
        );
        // fail closed: a value that cannot be read is not "off"
        let e = snapshot_mode("always").unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Usage);
        assert!(e.message.contains("[safety] snapshot"), "{}", e.message);
        assert_eq!(gate_question(None), "Undo this transaction");
        assert_eq!(
            gate_question(Some("snapper")),
            "Undo this transaction (a snapper snapshot is taken first)"
        );
        assert!(apt_download_question(1).starts_with("Download the 1 file listed above"));
        assert!(apt_download_question(3).starts_with("Download the 3 files listed above"));
    }

    #[test]
    fn dnf5_empty_history_is_no_transaction() {
        // dnf5 5.4.6 with no history: exit 0, nothing on stdout
        assert!(!dnf_info_has_transaction(""));
        assert!(!dnf_info_has_transaction("\n"));
        assert!(dnf_info_has_transaction(
            "Transaction ID : 3\nDescription    : dnf install -y tree\n"
        ));
    }

    #[test]
    fn the_running_kernel_is_not_removable() {
        let removes: Vec<String> = [
            "linux-image-7.2.12+deb13-amd64",
            "linux-modules-7.2.12+deb13-amd64",
            "linux-image-6.8.0-100-generic",
            "linux-base-amd64",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            running_kernel_packages(&removes, "7.2.12+deb13-amd64"),
            vec![
                "linux-image-7.2.12+deb13-amd64".to_string(),
                "linux-modules-7.2.12+deb13-amd64".to_string()
            ]
        );
        assert!(running_kernel_packages(&removes, "6.1.0-1-amd64").is_empty());
        assert!(running_kernel_packages(&removes, "").is_empty());
        // apt's history names it with its architecture
        let qualified = vec!["linux-image-7.2.12+deb13-amd64:amd64".to_string()];
        assert_eq!(
            running_kernel_packages(&qualified, "7.2.12+deb13-amd64"),
            qualified
        );
    }

    #[test]
    fn one_missing_version_refuses_the_whole_undo() {
        let steps = vec![
            Step::Remove { name: "a".into() },
            Step::Restore {
                name: "b".into(),
                version: "1".into(),
            },
            Step::Restore {
                name: "c".into(),
                version: "2".into(),
            },
        ];
        let e = resolve(&steps, |n, _| Ok((n == "b").then_some(Found::Repo))).unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Refused);
        assert!(e.message.contains("c 2"), "{}", e.message);
        let (removes, restores) = resolve(&steps, |_, _| Ok(Some(Found::Repo))).unwrap();
        assert_eq!(removes, vec!["a"]);
        assert_eq!(restores.len(), 2);
    }

    #[test]
    fn closing_line_counts_the_transactions() {
        assert!(closing_line(1).contains("a new transaction"));
        let two = closing_line(2);
        assert!(two.contains("2 transactions") && two.contains("only the last"));
        let three = closing_line(3);
        assert!(three.contains("3 separate commands") && three.contains("3 transactions"));
    }

    fn inst(name: &str) -> LogEntry {
        LogEntry {
            ts: 0,
            source: "zypper".into(),
            action: Action::Install,
            name: name.into(),
            old_version: String::new(),
            new_version: "1".into(),
        }
    }

    #[test]
    fn zypp_rotated_history_names_are_found_oldest_first() {
        let names: Vec<String> = [
            "history",
            "history-20261117.gz",
            "history-20260101.xz",
            "history-202605.xz",
            "history-old",
            "history-.gz",
            "other.log",
            "history-20260101.sh",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            zypp_rotated_names(&names),
            [
                "history-20260101.xz",
                "history-202605.xz",
                "history-20261117.gz"
            ]
        );
    }

    #[test]
    fn multiversion_upgrade_is_undone_by_removing_the_new_version() {
        let up = LogEntry {
            action: Action::Upgrade,
            old_version: "2.0-1".into(),
            new_version: "3.0-1".into(),
            ..inst("kern")
        };
        let installed = |n: &str| match n {
            "kern" => vec!["1.0-1".into(), "2.0-1".into(), "3.0-1".into()],
            "plain" => vec!["2.0-1".to_string()],
            _ => vec![],
        };
        // old version still installed: remove only the new one, no restore
        let restores = vec![("kern".to_string(), "2.0-1".to_string(), Found::Repo)];
        let (rm, re) =
            zypper_multiversion_fixup(std::slice::from_ref(&up), vec![], restores, installed);
        assert_eq!(rm, ["kern=3.0-1"]);
        assert!(re.is_empty());
        // old version gone (a normal upgrade): the restore stays
        let up2 = LogEntry {
            name: "plain".into(),
            old_version: "1.0-1".into(),
            new_version: "2.0-1".into(),
            ..up.clone()
        };
        let restores = vec![("plain".to_string(), "1.0-1".to_string(), Found::Repo)];
        let (rm, re) = zypper_multiversion_fixup(&[up2], vec![], restores, installed);
        assert!(rm.is_empty() && re.len() == 1);
        // an install of one of several installed versions removes just that one
        let ins = LogEntry {
            new_version: "3.0-1".into(),
            ..inst("kern")
        };
        let (rm, _) = zypper_multiversion_fixup(
            std::slice::from_ref(&ins),
            vec!["kern".into()],
            vec![],
            installed,
        );
        assert_eq!(rm, ["kern=3.0-1"]);
        // a single installed version keeps the plain name
        let (rm, _) = zypper_multiversion_fixup(&[ins], vec!["plain".into()], vec![], installed);
        assert_eq!(rm, ["plain"]);
    }

    #[test]
    fn a_removed_then_installed_package_is_restored_not_removed() {
        let restores = vec![("tree".to_string(), "2.3-1".to_string(), Found::Repo)];
        let removes = vec![
            "tree".to_string(),
            "other".to_string(),
            "tree=3.0-1".to_string(),
        ];
        assert_eq!(drop_removes_of_restored(removes, &restores), ["other"]);
    }

    #[test]
    fn removing_the_running_kernel_is_detected() {
        let run = ("kernel-default".to_string(), "7.3.4-1.1".to_string());
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(removes_running_kernel(
            &r(&["kernel-default=7.3.4-1.1"]),
            Some(&run)
        ));
        assert!(removes_running_kernel(
            &r(&["x", "kernel-default"]),
            Some(&run)
        ));
        assert!(!removes_running_kernel(
            &r(&["kernel-default=7.3.3-1.1"]),
            Some(&run)
        ));
        assert!(!removes_running_kernel(
            &r(&["kernel-default-base"]),
            Some(&run)
        ));
        assert!(!removes_running_kernel(&r(&["kernel-default"]), None));
    }

    #[test]
    fn zypper_upgrade_commands_make_bare_installs_uncertain() {
        let e = [inst("a"), inst("b")];
        assert_eq!(zypper_uncertain_installs("zypper dup", &e), ["a", "b"]);
        assert_eq!(zypper_uncertain_installs("zypper -n update", &e).len(), 2);
        assert!(zypper_uncertain_installs("zypper install a", &e).is_empty());
        assert!(zypper_uncertain_installs("", &e).is_empty());
        let mut up = inst("c");
        up.action = Action::Upgrade;
        up.old_version = "0".into();
        assert!(zypper_uncertain_installs("zypper up", &[up]).is_empty());
    }

    #[test]
    fn dnf_old_versions_are_read_from_history_info() {
        let info = "Packages altered:\n  Action   Package                    Reason Repository\n  Upgrade  tree-0:2.3.1-2.fc45.x86_64 User   fedora\n  Replaced tree-0:2.2.1-4.fc44.x86_64 User   @System\n";
        assert_eq!(dnf_replaced_nevras(info), ["tree-0:2.2.1-4.fc44.x86_64"]);
        assert!(
            dnf_replaced_nevras("Action Package\n Install tree-1-1.x86_64 User fedora\n")
                .is_empty()
        );
        assert_eq!(
            dnf_replaced_nevras("Upgraded tree-1-1.x86_64 @@System\n"),
            ["tree-1-1.x86_64"]
        );
    }

    #[test]
    fn dnf_removed_packages_must_be_available_too() {
        // dnf 5 says `Remove`, dnf 4 says `Removed`; epochs are kept
        assert_eq!(
            dnf_replaced_nevras("  Remove  tree-1:2.2.1-4.fc44.x86_64 User @System\n"),
            ["tree-1:2.2.1-4.fc44.x86_64"]
        );
        assert_eq!(
            dnf_replaced_nevras("    Removed     tree-1:2.2.1-4.el9.noarch @@System\n"),
            ["tree-1:2.2.1-4.el9.noarch"]
        );
        // a package gone from every repository is reported as missing
        assert!(!dnf_repoquery_lists(
            "other-1-1.noarch\n",
            "tree-1:2.2.1-4.el9.noarch"
        ));
    }

    #[test]
    fn dnf_repoquery_output_is_matched_on_the_nevra() {
        let out = "tree-0:2.3.1-2.fc45.x86_64\n";
        assert!(dnf_repoquery_lists(out, "tree-0:2.3.1-2.fc45.x86_64"));
        assert!(dnf_repoquery_lists(out, "tree-2.3.1-2.fc45.x86_64"));
        assert!(!dnf_repoquery_lists(out, "tree-0:2.2.1-4.fc44.x86_64"));
        assert!(!dnf_repoquery_lists("", "tree-0:2.2.1-4.fc44.x86_64"));
        // epoch 0 present or absent on either side
        let with = "tree-0:2.3.1-2.fc45.x86_64\n";
        assert!(dnf_repoquery_lists(with, "tree-2.3.1-2.fc45.x86_64"));
        assert!(dnf_repoquery_lists(with, "tree-0:2.3.1-2.fc45.x86_64"));
        // a non-zero epoch is not dropped
        assert!(!dnf_repoquery_lists(with, "tree-1:2.3.1-2.fc45.x86_64"));
    }

    #[test]
    fn dnf_replaced_nevras_dedupes_across_epoch_forms() {
        let info = "Replaced tree-0:1-1.x86_64 @@System\nReplaced tree-1-1.x86_64 @@System\n";
        assert_eq!(dnf_replaced_nevras(info), ["tree-0:1-1.x86_64"]);
    }

    #[test]
    fn zypper_global_options_with_values_are_skipped_for_the_verb() {
        let e = [inst("a")];
        assert_eq!(
            zypper_uncertain_installs("zypper --root /mnt up", &e),
            ["a"]
        );
        assert_eq!(
            zypper_uncertain_installs("zypper -n -C /etc/z.conf --userdata x dup", &e),
            ["a"]
        );
        // the value is not mistaken for the verb
        assert!(zypper_uncertain_installs("zypper --root /mnt install a", &e).is_empty());
        assert_eq!(
            zypper_uncertain_installs("zypper --root=/mnt up", &e),
            ["a"]
        );
    }
}
