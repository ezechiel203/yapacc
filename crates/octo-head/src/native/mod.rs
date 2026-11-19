// yapacc — native package facts
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Read-only facts about an installed native package, asked of whichever
//! package manager owns the system. The native database is the source of
//! truth: yapacc keeps no copy, so packages installed, upgraded or removed
//! outside yapacc are always reported correctly.
//!
//! One function per backend turns that manager's own query output into the
//! same [`NativeFact`]. Parsers are pure (text in, struct out) and unit
//! tested against captured output; the commands run with `LC_ALL=C`.

use crate::error::Error;
use crate::platform::PkgManager;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod apk;
pub mod apt;
pub mod dnf;
pub mod pacman;
pub mod pkgng;
pub mod xbps;
pub mod zypper;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// the user asked for it
    Explicit,
    /// pulled in to satisfy another package
    Dependency,
    /// this manager does not record it, or it could not be read
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NativeFact {
    pub manager: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub reason: Reason,
    /// installed packages that depend on this one
    pub required_by: Vec<String>,
    /// repository / origin as the manager reports it
    pub origin: String,
    /// `Some(true)`: no configured repository offers this package (AUR,
    /// local file, dropped). `Some(false)`: a repository offers it.
    /// `None`: cannot tell — no repository index is available locally.
    pub foreign: Option<bool>,
    /// part of the base system the manager protects (apt Essential /
    /// Priority: required); "nothing depends on it" does not make it an orphan
    pub essential: bool,
    /// why it counts as part of the base system: `essential` (apt marks it
    /// `Essential: yes` and refuses to remove it) or `priority required`
    /// (apt's `Priority: required`, which it does not refuse to remove);
    /// empty when `essential` is false
    pub base: String,
    pub installed: String,
    /// how the package was verified at install time, as recorded natively
    pub verified: String,
    pub packager: String,
}

/// pacman prints the install date in local time, as C's `%c` does
/// (`Fri Sep 25 18:37:59 2026`; other locales put the day first and add a
/// zone name). It is shown the way `timeline` shows a time without a zone:
/// `2026-09-25 18:37 (local)`. Text in any other form is kept as it is. Pure.
pub fn pacman_install_date(text: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let (mut month, mut day, mut year, mut time) = (None, None, None, None);
    for token in text.split_whitespace() {
        if let Some(m) = MONTHS.iter().position(|m| *m == token) {
            month = Some(m + 1);
        } else if digits(token) && token.len() == 4 {
            year = Some(token);
        } else if digits(token) && token.len() <= 2 {
            day = token.parse::<u32>().ok().filter(|d| (1..=31).contains(d));
        } else if token.len() == 8
            && token.split(':').count() == 3
            && token.split(':').all(|p| p.len() == 2 && digits(p))
        {
            time = Some(&token[..5]);
        }
    }
    match (year, month, day, time) {
        (Some(y), Some(m), Some(d), Some(t)) => format!("{y}-{m:02}-{d:02} {t} (local)"),
        _ => text.to_string(),
    }
}

/// Run `prog` with `LC_ALL=C`; return its exit status and what it printed
/// on stdout. A program that cannot be started is `Kind::Unavailable`.
/// `COLUMNS` is removed: pacman wraps its output to it even into a pipe.
fn run(prog: &str, args: &[&str]) -> Result<(i32, String), Error> {
    let out = Command::new(prog)
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("COLUMNS")
        .output()
        .map_err(|e| Error::unavailable(format!("cannot run {prog}: {e}")))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

/// Stdout of a command that exited with status 0, else `None`.
fn run_ok(prog: &str, args: &[&str]) -> Option<String> {
    match run(prog, args) {
        Ok((0, out)) => Some(out),
        _ => None,
    }
}

fn none_list(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() || s == "None" {
        vec![]
    } else {
        s.split_whitespace().map(|x| x.to_string()).collect()
    }
}

// ------------------------------------------------- repository metadata

/// Does directory `dir` hold an entry whose name passes `keep`?
fn dir_has(dir: &Path, keep: impl Fn(&str) -> bool) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| keep(&e.file_name().to_string_lossy()))
        })
        .unwrap_or(false)
}

/// Does `cache` hold the metadata of at least one dnf repository: a
/// directory with `repodata/repomd.xml` in it? dnf also keeps files there
/// that are no repository (`expired_repos.json`, `packages.db`,
/// `tempfiles.json` on dnf 4), which a fresh image has without any
/// metadata. Pure over the directory.
pub fn dnf_cache_has_metadata(cache: &Path) -> bool {
    std::fs::read_dir(cache)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().join("repodata").join("repomd.xml").is_file())
        })
        .unwrap_or(false)
}

/// The aliases of the enabled repositories in zypper's `repos.d`: the
/// section names of its `.repo` files, unless `enabled` is 0, false, no or
/// off (libzypp enables a repository that does not say). Sorted. Pure over
/// the directory.
pub fn zypp_enabled_repos(repos_d: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(repos_d) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "repo") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        out.extend(zypp_enabled_in(&text));
    }
    out.sort();
    out.dedup();
    out
}

/// The enabled sections of one `.repo` file. Pure.
pub fn zypp_enabled_in(text: &str) -> Vec<String> {
    let mut sections: Vec<(String, bool)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(alias) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((alias.trim().to_string(), true));
        } else if let Some((key, value)) = line.split_once('=')
            && key.trim() == "enabled"
            && let Some(last) = sections.last_mut()
        {
            last.1 = !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
        }
    }
    sections
        .into_iter()
        .filter(|(alias, on)| *on && !alias.is_empty())
        .map(|(alias, _)| alias)
        .collect()
}

/// The enabled zypper repositories whose metadata is not on disk: neither
/// a solv file under `cache/solv/ALIAS` nor anything under
/// `cache/raw/ALIAS` (libzypp writes `/` in an alias as `_`). `@System`
/// under `solv` is the installed packages, not a repository. zypper run as
/// root downloads what is missing here even with `--no-refresh`. Pure over
/// the directories.
pub fn zypp_repos_without_metadata(repos_d: &Path, cache: &Path) -> Vec<String> {
    zypp_enabled_repos(repos_d)
        .into_iter()
        .filter(|alias| {
            let dir = alias.replace('/', "_");
            let solv = cache.join("solv").join(&dir).join("solv").is_file();
            let raw = dir_has(&cache.join("raw").join(&dir), |_| true);
            !solv && !raw
        })
        .collect()
}

/// The command that downloads manager `m`'s repository metadata.
fn refresh_hint(m: PkgManager) -> &'static str {
    match m {
        PkgManager::Pacman => "pacman -Syu",
        PkgManager::Apt => "apt-get update",
        PkgManager::Dnf => "dnf makecache",
        PkgManager::Zypper => "zypper refresh",
        PkgManager::Apk => "apk update",
        PkgManager::Xbps => "xbps-install -S",
        PkgManager::Pkg => "pkg update",
        PkgManager::Portage => "emerge --sync",
        PkgManager::Nix => "nix-channel --update",
    }
}

/// The sentence for "no repository metadata on disk". `missing` names the
/// repositories when only some lack it. Pure.
pub fn no_metadata_message(m: PkgManager, missing: &[String]) -> String {
    let which = if missing.is_empty() {
        String::new()
    } else {
        format!(" for {}", missing.join(", "))
    };
    format!(
        "no repository metadata on disk{which}: run `{}` (yapacc does not refresh on a read command)",
        refresh_hint(m)
    )
}

/// `None` when manager `m`'s repository metadata is on disk, so that
/// questions about repositories (search, pending updates, where a package
/// comes from) can be answered without contacting anybody. Otherwise the
/// sentence that says so and names the command that downloads it: a read
/// command then answers with that note and does not call the manager for
/// repository data (zypper would download it; `dnf -C` fails).
///
/// What counts as metadata, per manager:
/// - pacman: a `*.db` under `/var/lib/pacman/sync`
/// - apt: a `*_Packages*` file under `/var/lib/apt/lists`
/// - dnf: a `repodata/repomd.xml` in a directory of `/var/cache/libdnf5`,
///   `/var/cache/dnf` or the user's `libdnf5` cache
/// - zypper: every enabled repository has a solv file or raw metadata
///   under `/var/cache/zypp`
/// - apk: an `APKINDEX*` under `/var/cache/apk` or `/var/lib/apk`
/// - xbps: a `*-repodata` in a directory of `/var/db/xbps`
/// - pkg: `/var/db/pkg/repo-*.sqlite`, or a `db` in a directory of
///   `/var/db/pkg/repos`
pub fn missing_metadata(m: PkgManager) -> Option<String> {
    let on_disk = match m {
        PkgManager::Pacman => dir_has(Path::new("/var/lib/pacman/sync"), |n| n.ends_with(".db")),
        PkgManager::Apt => dir_has(Path::new("/var/lib/apt/lists"), |n| n.contains("_Packages")),
        PkgManager::Dnf => {
            let user_cache = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .map(|c| c.join("libdnf5"));
            dnf_cache_has_metadata(Path::new("/var/cache/libdnf5"))
                || dnf_cache_has_metadata(Path::new("/var/cache/dnf"))
                || user_cache.is_some_and(|c| dnf_cache_has_metadata(&c))
        }
        PkgManager::Zypper => {
            let repos_d = Path::new("/etc/zypp/repos.d");
            let missing = zypp_repos_without_metadata(repos_d, Path::new("/var/cache/zypp"));
            if !missing.is_empty() {
                // all of them: say it without the list
                let all = missing.len() == zypp_enabled_repos(repos_d).len();
                return Some(no_metadata_message(m, if all { &[] } else { &missing }));
            }
            !zypp_enabled_repos(repos_d).is_empty()
        }
        PkgManager::Apk => {
            dir_has(Path::new("/var/cache/apk"), |n| n.starts_with("APKINDEX"))
                || dir_has(Path::new("/var/lib/apk"), |n| n.starts_with("APKINDEX"))
        }
        // xbps keeps one directory per repository, each holding `ARCH-repodata`
        PkgManager::Xbps => std::fs::read_dir("/var/db/xbps")
            .map(|d| {
                d.flatten()
                    .any(|e| dir_has(&e.path(), |n| n.ends_with("-repodata")))
            })
            .unwrap_or(false),
        // pkg 2.x keeps one directory per repository: repos/NAME/db
        PkgManager::Pkg => {
            dir_has(Path::new("/var/db/pkg"), |n| {
                n.starts_with("repo-") && n.ends_with(".sqlite")
            }) || std::fs::read_dir("/var/db/pkg/repos")
                .map(|d| d.flatten().any(|e| e.path().join("db").exists()))
                .unwrap_or(false)
        }
        PkgManager::Portage | PkgManager::Nix => return None,
    };
    (!on_disk).then(|| no_metadata_message(m, &[]))
}

// ---------------------------------------------------------------- pacman

/// Parse `pacman -Qi NAME` (C locale). `foreign`/`origin` are filled by the caller.
pub fn parse_pacman_qi(text: &str) -> Option<NativeFact> {
    let mut f = NativeFact {
        manager: "pacman".into(),
        name: String::new(),
        version: String::new(),
        description: String::new(),
        reason: Reason::Unknown,
        required_by: vec![],
        origin: String::new(),
        foreign: None,
        essential: false,
        base: String::new(),
        installed: String::new(),
        verified: String::new(),
        packager: String::new(),
    };
    let mut key = String::new();
    for line in text.lines() {
        let (k, v) = match line.split_once(" : ") {
            Some((k, v)) if !line.starts_with(' ') => (k.trim().to_string(), v.trim()),
            // A wrapped line continues the field above it. Only fields that
            // can wrap are continued; anything else would be overwritten by
            // its own last fragment.
            _ if line.starts_with(' ') => {
                match key.as_str() {
                    "Required By" => f.required_by.extend(none_list(line)),
                    "Description" => {
                        f.description.push(' ');
                        f.description.push_str(line.trim());
                    }
                    _ => {}
                }
                continue;
            }
            _ => continue,
        };
        match k.as_str() {
            "Name" => f.name = v.into(),
            "Version" => f.version = v.into(),
            "Description" => f.description = v.into(),
            "Required By" => f.required_by.extend(none_list(v)),
            "Install Date" => f.installed = pacman_install_date(v),
            "Validated By" => f.verified = v.into(),
            "Packager" => f.packager = v.into(),
            "Install Reason" => {
                f.reason = if v.starts_with("Explicitly") {
                    Reason::Explicit
                } else if v.contains("dependency") {
                    Reason::Dependency
                } else {
                    Reason::Unknown
                }
            }
            _ => {}
        }
        key = k;
    }
    (!f.name.is_empty()).then_some(f)
}

fn pacman_fact(name: &str) -> Option<NativeFact> {
    let mut f = parse_pacman_qi(&run_ok("pacman", &["-Qi", "--", name])?)?;
    // "foreign" = not offered by any sync repo (AUR, local file, dropped).
    // Only decidable when sync databases have actually been downloaded.
    let have_sync_dbs = std::fs::read_dir("/var/lib/pacman/sync")
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".db"))
        })
        .unwrap_or(false);
    if !have_sync_dbs {
        f.origin = "unknown — no repository database downloaded yet (refresh to find out)".into();
        return Some(f);
    }
    match run_ok("pacman", &["-Si", "--", &f.name]) {
        Some(si) => {
            f.foreign = Some(false);
            f.origin = si
                .lines()
                .find_map(|l| l.strip_prefix("Repository"))
                .and_then(|r| r.split_once(':'))
                .map(|(_, r)| format!("{} (sync repository)", r.trim()))
                .unwrap_or_else(|| "sync repository".into());
        }
        None => {
            f.foreign = Some(true);
            f.origin = "not in any configured repository (AUR, local file, or dropped)".into();
        }
    }
    Some(f)
}

// ------------------------------------------------------------------- apt

/// Parse `dpkg-query -W -f='${Package}\t${Version}\t${db:Status-Abbrev}\t${Essential}\t${Priority}\t${Maintainer}\t${binary:Summary}\n'`.
pub fn parse_dpkg_query(line: &str) -> Option<NativeFact> {
    let mut it = line.trim_end_matches('\n').splitn(7, '\t');
    let name = it.next()?.trim();
    let version = it.next()?.trim();
    let status = it.next()?.trim();
    let essential = it.next().unwrap_or("").trim() == "yes";
    let required = it.next().unwrap_or("").trim() == "required";
    let maintainer = it.next().unwrap_or("").trim();
    let summary = it.next().unwrap_or("").trim();
    // "ii " = desired install, status installed
    if name.is_empty() || !status.starts_with("ii") {
        return None;
    }
    Some(NativeFact {
        manager: "apt".into(),
        name: name.into(),
        version: version.into(),
        description: summary.into(),
        reason: Reason::Unknown,
        required_by: vec![],
        origin: String::new(),
        foreign: None,
        essential: essential || required,
        base: if essential {
            "essential".into()
        } else if required {
            "priority required".into()
        } else {
            String::new()
        },
        installed: String::new(),
        verified: String::new(),
        packager: maintainer.into(),
    })
}

/// Parse `apt-cache rdepends --installed NAME` into package names.
pub fn parse_apt_rdepends(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .skip_while(|l| !l.starts_with("Reverse Depends:"))
        .skip(1)
        .map(|l| l.trim().trim_start_matches('|').trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The name a dependency term refers to: `perl:any (>= 5.40)` is `perl`.
fn dep_term_name(term: &str) -> &str {
    let name = term.trim().split([' ', '(']).next().unwrap_or("");
    name.split(':').next().unwrap_or("")
}

/// Installed packages whose `Depends` or `Pre-Depends` name `name` or one of
/// the virtual packages it provides. `all` is one line per package:
/// `Package<TAB>Status-Abbrev<TAB>Depends<TAB>Pre-Depends`. A package that
/// accepts `name` as one alternative (`a | name`) counts, as it does in
/// `apt-cache rdepends`.
pub fn parse_dpkg_rdepends(all: &str, name: &str, provides: &str) -> Vec<String> {
    let mut wanted: Vec<&str> = vec![name];
    wanted.extend(
        provides
            .split(',')
            .map(dep_term_name)
            .filter(|p| !p.is_empty()),
    );
    let mut v: Vec<String> = all
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let pkg = cols.next()?.trim();
            let status = cols.next()?;
            // "ii ": wanted installed, fully installed
            if !status.starts_with("ii") || pkg.is_empty() || pkg == name {
                return None;
            }
            let needs = cols
                .flat_map(|field| field.split([',', '|']))
                .any(|term| wanted.contains(&dep_term_name(term)));
            needs.then(|| pkg.to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Where `apt-cache policy NAME` says the versions of a package come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AptPolicy {
    /// the first remote origin of the installed version (`***`), if it has one
    pub installed: Option<String>,
    /// the first remote origin of a version listed above the installed one
    /// (the table is sorted newest first)
    pub newer: Option<String>,
    /// the same for a version listed below it
    pub older: Option<String>,
}

/// Parse the version table of `apt-cache policy NAME`: for the installed
/// version and for the versions above and below it, the first origin that
/// is not the local dpkg status file. Pure.
///
/// A stock package whose installed version was superseded in the index
/// (Ubuntu 24.04, observed 2026-11-18) reads:
///
/// ```text
///      5.38.2-3.2ubuntu0.7 500
///         500 http://archive.ubuntu.com/ubuntu noble-updates/main amd64 Packages
///  *** 5.38.2-3.2ubuntu0.6 100
///         100 /var/lib/dpkg/status
/// ```
pub fn parse_apt_policy(text: &str) -> AptPolicy {
    #[derive(PartialEq)]
    enum At {
        Before,
        Installed,
        After,
    }
    let mut policy = AptPolicy::default();
    let mut at = At::Before;
    let mut in_table = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("Version table:") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        let indent = line.len() - t.len();
        let first_is_priority = t
            .split_whitespace()
            .next()
            .is_some_and(|p| p.parse::<i32>().is_ok());
        // origin lines are indented deeper than version lines and start
        // with a priority
        if indent >= 8 && first_is_priority {
            let origin = t.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
            if origin.starts_with("/var/lib/dpkg/status") {
                continue;
            }
            let slot = match at {
                At::Before => &mut policy.newer,
                At::Installed => &mut policy.installed,
                At::After => &mut policy.older,
            };
            if slot.is_none() {
                *slot = Some(origin);
            }
        } else if t.starts_with("*** ") {
            at = At::Installed;
        } else if at == At::Installed {
            at = At::After;
        }
    }
    policy
}

/// What `why` prints as "from" and "verified" for an apt package, and
/// whether no configured repository offers it. Pure.
pub fn apt_origin(policy: &AptPolicy) -> (String, Option<bool>, String) {
    match (&policy.installed, &policy.newer, &policy.older) {
        (Some(origin), _, _) => (
            origin.clone(),
            Some(false),
            "repository signature (apt verifies the signed Release index)".into(),
        ),
        // P5: the name is still in a configured repository; only the
        // installed version has left the index
        (None, Some(origin), _) => (
            format!(
                "the installed version is no longer in the repository index (a newer one is, from {origin})"
            ),
            Some(false),
            "unknown (the installed version is no longer in a repository index)".into(),
        ),
        (None, None, Some(origin)) => (
            format!(
                "the installed version is in no configured repository (an older one is, from {origin}): a local .deb, a removed PPA, or a withdrawn version"
            ),
            Some(true),
            "unknown (not from a configured repository)".into(),
        ),
        (None, None, None) => (
            "not in any configured repository (local .deb, removed PPA, or dropped)".into(),
            Some(true),
            "unknown (not from a configured repository)".into(),
        ),
    }
}

/// The name dpkg itself gives the installed package, from the output of
/// `dpkg-query -W -f='${binary:Package}\t${db:Status-Abbrev}\n' -- NAME`:
/// the first line whose status is `ii`. dpkg adds the architecture where
/// the bare name would be ambiguous or foreign (`libc6:amd64`,
/// `libdeflate0:i386`), and apt answers for the native architecture when
/// asked about a bare name. Pure.
pub fn parse_dpkg_qualified(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (name, status) = l.split_once('\t')?;
        (status.starts_with("ii") && !name.trim().is_empty()).then(|| name.trim().to_string())
    })
}

/// Does the output of `apt-mark showauto -- QUALIFIED` name the package?
/// apt writes a package of the native architecture without it (`libc6`)
/// and a foreign one with it (`libdeflate0:i386`). Pure.
pub fn apt_marked_auto(showauto: &str, name: &str, qualified: &str) -> bool {
    showauto
        .lines()
        .map(str::trim)
        .any(|l| l == name || l == qualified)
}

/// Installed packages that depend on apt package `name`, read from the
/// dependency fields of the installed packages (two `dpkg-query` runs, no
/// apt cache). `apt-cache rdepends` does not list a package that depends on
/// `name:any` (cowsay on perl:any), so it called such a dependency an
/// orphan.
fn apt_required_by(name: &str) -> Option<Vec<String>> {
    let provides =
        run_ok("dpkg-query", &["-W", "-f=${Provides}\n", "--", name]).unwrap_or_default();
    let all = run_ok(
        "dpkg-query",
        &[
            "-W",
            "-f=${Package}\t${db:Status-Abbrev}\t${Depends}\t${Pre-Depends}\n",
        ],
    )?;
    Some(parse_dpkg_rdepends(&all, name, &provides))
}

fn apt_fact(name: &str) -> Option<NativeFact> {
    let line = run_ok(
        "dpkg-query",
        &[
            "-W",
            "-f=${Package}\t${Version}\t${db:Status-Abbrev}\t${Essential}\t${Priority}\t${Maintainer}\t${binary:Summary}\n",
            "--",
            name,
        ],
    )?;
    // one line per architecture: the first that is installed
    let mut f = line.lines().find_map(parse_dpkg_query)?;
    // the name apt has to be asked about: with the architecture when the
    // package is a foreign one (`libdeflate0:i386`)
    let qualified = run_ok(
        "dpkg-query",
        &[
            "-W",
            "-f=${binary:Package}\t${db:Status-Abbrev}\n",
            "--",
            name,
        ],
    )
    .and_then(|out| parse_dpkg_qualified(&out))
    .unwrap_or_else(|| f.name.clone());
    // install reason: apt records "auto" for dependencies
    f.reason = match run_ok("apt-mark", &["showauto", "--", &qualified]) {
        Some(out) if apt_marked_auto(&out, &f.name, &qualified) => Reason::Dependency,
        Some(_) => Reason::Explicit,
        None => Reason::Unknown,
    };
    if let Some(required_by) = apt_required_by(&f.name) {
        f.required_by = required_by;
    }
    // Origin is only decidable when apt has repository indexes on disk
    // (fresh container images and minimal installs ship without them).
    let have_lists = std::fs::read_dir("/var/lib/apt/lists")
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().contains("_Packages"))
        })
        .unwrap_or(false);
    if !have_lists {
        f.origin = "unknown — no repository index on disk yet (apt update to find out)".into();
        f.verified = "unknown".into();
    } else if let Some(out) = run_ok("apt-cache", &["policy", "--", &qualified]) {
        (f.origin, f.foreign, f.verified) = apt_origin(&parse_apt_policy(&out));
    }
    // dpkg has no install-date field; the file list is rewritten on
    // install/upgrade. It is named after the qualified name for a
    // `Multi-Arch: same` package, after the bare name otherwise.
    f.installed = [qualified.as_str(), f.name.as_str()]
        .iter()
        .find_map(|n| std::fs::metadata(format!("/var/lib/dpkg/info/{n}.list")).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| {
            format!(
                "{} (last install or upgrade)",
                crate::util::fmt_utc(d.as_secs())
            )
        })
        .unwrap_or_default();
    Some(f)
}

// ------------------------------------------------------------- rpm / dnf

/// Parse `rpm -q --qf '%{NAME}\t%{EVR}\t%{INSTALLTIME}\t%{VENDOR}\t%{PACKAGER}\t%{SIGPGP:pgpsig}\t%{RSAHEADER:pgpsig}\t%{SUMMARY}\n'`.
pub fn parse_rpm_q(line: &str, manager: &str) -> Option<NativeFact> {
    let p: Vec<&str> = line.trim_end_matches('\n').splitn(8, '\t').collect();
    if p.len() < 8 || p[0].is_empty() {
        return None;
    }
    let clean = |s: &str| {
        if s == "(none)" {
            String::new()
        } else {
            s.trim().to_string()
        }
    };
    let sig = [clean(p[5]), clean(p[6])]
        .into_iter()
        .find(|s| !s.is_empty());
    Some(NativeFact {
        manager: manager.into(),
        name: p[0].into(),
        version: p[1].into(),
        description: clean(p[7]),
        reason: Reason::Unknown,
        required_by: vec![],
        origin: String::new(),
        foreign: None,
        essential: false,
        base: String::new(),
        installed: p[2]
            .parse::<u64>()
            .map(crate::util::fmt_utc)
            .unwrap_or_default(),
        verified: match sig {
            Some(s) => format!("package signature: {s}"),
            None => "NOT signed".into(),
        },
        packager: {
            let v = clean(p[3]);
            if v.is_empty() { clean(p[4]) } else { v }
        },
    })
}

/// Installed packages that require rpm package `name`.
///
/// dnf resolves "what requires" through provides (files, sonames, virtual
/// names); plain rpm only matches the package name. These are questions
/// about installed packages: `-C --disablerepo=*` keeps dnf from loading
/// (or, without `-C`, downloading) any repository for them, and makes them
/// work on a machine with no metadata on disk.
fn rpm_required_by(name: &str, manager: PkgManager) -> Option<Vec<String>> {
    let out = (manager == PkgManager::Dnf)
        .then(|| {
            run_ok(
                "dnf",
                &[
                    "-q",
                    "-C",
                    "--disablerepo=*",
                    "repoquery",
                    "--installed",
                    "--whatrequires",
                    name,
                    "--qf",
                    "%{name}\n",
                ],
            )
        })
        .flatten()
        .or_else(|| {
            run_ok(
                "rpm",
                &["-q", "--whatrequires", "--qf", "%{NAME}\n", "--", name],
            )
        })?;
    let mut v: Vec<String> = out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.contains(' ') && *l != name)
        .collect();
    v.sort();
    v.dedup();
    Some(v)
}

fn rpm_fact(name: &str, manager: PkgManager) -> Option<NativeFact> {
    let line = run_ok(
        "rpm",
        &[
            "-q",
            "--qf",
            "%{NAME}\t%{EVR}\t%{INSTALLTIME}\t%{VENDOR}\t%{PACKAGER}\t%{SIGPGP:pgpsig}\t%{RSAHEADER:pgpsig}\t%{SUMMARY}\n",
            "--",
            name,
        ],
    )?;
    let mut f = parse_rpm_q(line.lines().next()?, manager.name())?;
    if let Some(required_by) = rpm_required_by(&f.name, manager) {
        f.required_by = required_by;
    }
    if manager == PkgManager::Dnf {
        // reason and source repository live in the dnf history database
        if let Some(out) = run_ok(
            "dnf",
            &[
                "-q",
                "-C",
                "--disablerepo=*",
                "repoquery",
                "--installed",
                "--qf",
                "%{reason}\t%{from_repo}\n",
                "--",
                &f.name,
            ],
        ) && let Some((reason, repo)) = out.lines().next().and_then(|l| l.split_once('\t'))
        {
            f.reason = match reason.trim().to_lowercase().as_str() {
                "user" | "group" => Reason::Explicit,
                "dependency" | "weak-dependency" | "weak" => Reason::Dependency,
                _ => Reason::Unknown,
            };
            match repo.trim() {
                "@commandline" => {
                    f.foreign = Some(true);
                    f.origin = "local .rpm file (not from a repository)".into();
                }
                "" | "<unknown>" | "@System" => {
                    f.origin = "unknown (installed before dnf history, or by rpm directly)".into();
                }
                r => {
                    f.foreign = Some(false);
                    f.origin = format!("repository '{r}' (as recorded by dnf)");
                }
            }
        }
    }
    Some(f)
}

// ------------------------------------------------------------- dispatch

/// Facts about installed package `name` from manager `m`.
///
/// `Ok(None)` = that manager has no such installed package.
/// `Err` = yapacc cannot read this manager yet (it is still reported by
/// `yapacc platform`; see the support tiers in docs/STRATEGY.md).
pub fn fact(m: PkgManager, name: &str) -> Result<Option<NativeFact>, String> {
    if !crate::util::valid_pkg_name(name) {
        return Err(format!("'{name}' is not a valid package name"));
    }
    match m {
        PkgManager::Pacman => Ok(pacman_fact(name)),
        PkgManager::Apt => Ok(apt_fact(name)),
        PkgManager::Dnf | PkgManager::Zypper => Ok(rpm_fact(name, m)),
        other => Err(format!(
            "reading the {} database is not implemented (tier {}: detected only)",
            other.name(),
            other.tier()
        )),
    }
}

// -------------------------------------------- backends (YAPACC-002 §2.1)

/// One installed package, as the native manager lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub arch: String,
    /// 0 when the manager does not report it
    pub size_bytes: u64,
    pub description: String,
}

/// A package that a repository offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub name: String,
    pub version: String,
    pub repo: String,
    pub description: String,
}

/// A pending upgrade of an installed package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Update {
    pub name: String,
    pub old_version: String,
    pub new_version: String,
    pub repo: String,
}

/// The read side of one native package manager.
pub trait NativeBackend {
    fn manager(&self) -> PkgManager;
    fn installed(&self) -> Result<Vec<Installed>, Error>;
    /// names the user asked for
    fn explicit(&self) -> Result<Vec<String>, Error>;
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error>;
    /// Installed packages that depend on `name`: the `required_by` of
    /// [`NativeBackend::fact`] alone. `why` asks this of every ancestor of
    /// a dependency; a backend whose `fact` runs several commands answers
    /// it with fewer.
    fn required_by(&self, name: &str) -> Result<Vec<String>, Error> {
        Ok(self.fact(name)?.map(|f| f.required_by).unwrap_or_default())
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error>;
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error>;
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error>;
    fn files(&self, name: &str) -> Result<Vec<String>, Error>;
    fn orphans(&self) -> Result<Vec<String>, Error>;
    fn updates(&self) -> Result<Vec<Update>, Error>;
    /// `Some(why)` when this manager's repository metadata is not on disk:
    /// `candidate`, `search` and `updates` then answer with nothing (they
    /// do not ask the manager, which would download it or fail), and the
    /// caller prints this sentence instead of "nothing found".
    fn missing_metadata(&self) -> Option<String> {
        missing_metadata(self.manager())
    }
}

/// A package name taken from the user, checked before it reaches a command.
fn check_name(name: &str) -> Result<(), Error> {
    if crate::util::valid_pkg_name(name) {
        return Ok(());
    }
    Err(Error::usage(format!(
        "'{name}' is not a valid package name"
    )))
}

/// A search query: not empty, not an option, no control character.
fn check_query(query: &str) -> Result<(), Error> {
    let bad = query.is_empty()
        || query.len() > 128
        || query.starts_with('-')
        || query.chars().any(char::is_control);
    if bad {
        return Err(Error::usage(format!(
            "'{query}' is not a valid search query"
        )));
    }
    Ok(())
}

/// A path for `owner_of`: absolute (so it cannot be an option).
fn check_path(path: &str) -> Result<(), Error> {
    if path.starts_with('/') && !path.chars().any(char::is_control) {
        return Ok(());
    }
    Err(Error::usage(format!("'{path}' is not an absolute path")))
}

/// Stdout of a command when it exits with status 0, else `None`.
fn output_if_found(prog: &str, args: &[&str]) -> Result<Option<String>, Error> {
    let (code, out) = run(prog, args)?;
    Ok((code == 0).then_some(out))
}

/// Stdout of a command that must exit with status 0.
fn output(prog: &str, args: &[&str]) -> Result<String, Error> {
    match run(prog, args)? {
        (0, out) => Ok(out),
        (code, _) => Err(Error::from(format!("{prog} exited with status {code}"))),
    }
}

/// Like `output`, but a command that fails and prints nothing has nothing
/// to report: pacman exits 1 for an empty list, dnf for a missing index.
fn output_or_empty(prog: &str, args: &[&str]) -> Result<String, Error> {
    match run(prog, args)? {
        (code, out) if code != 0 && !out.trim().is_empty() => {
            Err(Error::from(format!("{prog} exited with status {code}")))
        }
        (_, out) => Ok(out),
    }
}

/// `fact` for the backends: the name is checked and the error is typed.
fn typed_fact(m: PkgManager, name: &str) -> Result<Option<NativeFact>, Error> {
    check_name(name)?;
    fact(m, name).map_err(Error::from)
}

/// `required_by` for the apt, dnf and zypper backends: the name is checked,
/// and only the commands that answer this one question are run.
fn typed_required_by(m: PkgManager, name: &str) -> Result<Vec<String>, Error> {
    check_name(name)?;
    Ok(match m {
        PkgManager::Apt => apt_required_by(name),
        _ => rpm_required_by(name, m),
    }
    .unwrap_or_default())
}

fn not_installed(name: &str) -> Error {
    Error::not_found(format!("package '{name}' is not installed"))
}

/// The backend that reads manager `m`.
pub fn backend(m: PkgManager) -> Result<Box<dyn NativeBackend>, Error> {
    match m {
        PkgManager::Pacman => Ok(Box::new(pacman::PacmanBackend)),
        PkgManager::Apt => Ok(Box::new(apt::AptBackend)),
        PkgManager::Dnf => Ok(Box::new(dnf::DnfBackend)),
        PkgManager::Zypper => Ok(Box::new(zypper::ZypperBackend)),
        PkgManager::Apk => Ok(Box::new(apk::ApkBackend)),
        PkgManager::Xbps => Ok(Box::new(xbps::XbpsBackend)),
        PkgManager::Pkg => Ok(Box::new(pkgng::PkgBackend)),
        other => Err(Error::unavailable(format!(
            "reading the {} database is not implemented (tier {}: detected only)",
            other.name(),
            other.tier()
        ))),
    }
}

/// The backend of the manager that owns this system.
pub fn native_backend() -> Result<Box<dyn NativeBackend>, Error> {
    match crate::platform::native_manager().1 {
        Some(m) => backend(m),
        None => Err(Error::unavailable(
            crate::platform::no_native_manager_message(),
        )),
    }
}

/// Test helper: the body and the exit status recorded in
/// `fixtures/native/<family>/<file>` (see `fixtures/README.md`).
#[cfg(test)]
fn fixture(family: &str, file: &str) -> (String, i32) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/native")
        .join(family)
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut rest = text.as_str();
    while rest.starts_with("# ") && !rest.starts_with("# exit: ") {
        rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
    }
    let (body, exit) = rest
        .rsplit_once("# exit: ")
        .unwrap_or_else(|| panic!("{} has no `# exit:` line", path.display()));
    let exit = exit
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("{}: bad exit status: {e}", path.display()));
    (body.to_string(), exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacman_install_date_is_shown_like_a_timeline_time() {
        assert_eq!(
            pacman_install_date("Fri Sep 25 18:37:59 2026"),
            "2026-09-25 18:37 (local)"
        );
        // `%c` pads a one-digit day with a space
        assert_eq!(
            pacman_install_date("Sun Nov  1 09:05:00 2026"),
            "2026-11-01 09:05 (local)"
        );
        // another locale's order, with a zone name
        assert_eq!(
            pacman_install_date("Sat 01 Nov 2026 10:00:00 CET"),
            "2026-11-01 10:00 (local)"
        );
        // not a date this function knows: unchanged
        assert_eq!(
            pacman_install_date("sam. 01 nov. 2026"),
            "sam. 01 nov. 2026"
        );
        assert_eq!(pacman_install_date(""), "");
    }

    #[test]
    fn dpkg_rdepends_see_any_qualified_and_virtual_dependencies() {
        let all = "cowsay\tii \tlibtext-charwidth-perl, perl:any\t\n\
                   perl-base\tii \t\tlibc6 (>= 2.34), dpkg (>= 1.17.17)\n\
                   exim4\tii \tdebconf (>= 0.5) | debconf-2.0, perl\t\n\
                   gone\trc \tperl\t\n\
                   mutt\tii \tlibc6, default-mta | mail-transport-agent\t\n\
                   perl\tii \tperl-base (= 5.40.1-6), perl-modules-5.40\t\n";
        assert_eq!(
            parse_dpkg_rdepends(all, "perl", ""),
            vec!["cowsay", "exim4"]
        );
        assert_eq!(
            parse_dpkg_rdepends(all, "libc6", ""),
            vec!["mutt", "perl-base"]
        );
        assert_eq!(
            parse_dpkg_rdepends(all, "postfix", "mail-transport-agent, default-mta (= 1)"),
            vec!["mutt"]
        );
        assert!(parse_dpkg_rdepends(all, "perl-doc", "").is_empty());
    }

    /// debian:stable with i386 added, `apt-get install libtiff6:i386`
    /// (2026-11-19): `dpkg-query -W libdeflate0` prints the qualified name,
    /// and `apt-mark showauto` lists `libdeflate0:i386`. `why` compared it
    /// with the bare name and called the package explicitly installed.
    #[test]
    fn apt_is_asked_about_a_foreign_package_by_its_qualified_name() {
        assert_eq!(
            parse_dpkg_qualified("libdeflate0:i386\tii \n").as_deref(),
            Some("libdeflate0:i386")
        );
        // both architectures of a `Multi-Arch: same` package: the first
        // installed one; a removed one (`rc`) is passed over
        assert_eq!(
            parse_dpkg_qualified("libc6:amd64\trc \nlibc6:i386\tii \n").as_deref(),
            Some("libc6:i386")
        );
        assert_eq!(parse_dpkg_qualified("tree\tii \n").as_deref(), Some("tree"));
        assert_eq!(parse_dpkg_qualified("tree\tun \n"), None);
        assert_eq!(parse_dpkg_qualified(""), None);
        // apt-mark's answer: foreign with the architecture, native without
        assert!(apt_marked_auto(
            "libdeflate0:i386\n",
            "libdeflate0",
            "libdeflate0:i386"
        ));
        assert!(apt_marked_auto("libc6\n", "libc6", "libc6:amd64"));
        assert!(!apt_marked_auto(
            "",
            "libpng16-16t64",
            "libpng16-16t64:i386"
        ));
        assert!(!apt_marked_auto("libc6-dev\n", "libc6", "libc6:amd64"));
    }

    #[test]
    fn pacman_qi_wrapped_scalar_fields_keep_their_first_line() {
        // `COLUMNS=60 pacman -Qi` wraps the description and the reason.
        let text = "Name            : ripgrep\n\
                    Version         : 15.1.0-1\n\
                    Description     : A search tool that combines the usability of\n\
                    \x20                 ag with the raw speed of grep\n\
                    Required By     : None\n\
                    Install Reason  : Installed as a dependency for another\n\
                    \x20                 package\n";
        let f = parse_pacman_qi(text).unwrap();
        assert_eq!(
            f.description,
            "A search tool that combines the usability of ag with the raw speed of grep"
        );
        assert_eq!(f.reason, Reason::Dependency);
        assert!(f.required_by.is_empty());
    }

    #[test]
    fn pacman_qi_explicit_with_wrapped_required_by() {
        let text = "Name            : glib2\nVersion         : 2.82.1-1\nDescription     : Low level core library\nRequired By     : at-spi2-core  avahi  cairo\n                  dconf  gtk3\nInstall Reason  : Installed as a dependency for another package\nInstall Date    : Sat 01 Nov 2026 10:00:00 CET\nPackager        : Someone <a@b.c>\nValidated By    : Signature\n";
        let f = parse_pacman_qi(text).unwrap();
        assert_eq!(f.name, "glib2");
        assert_eq!(f.reason, Reason::Dependency);
        assert_eq!(
            f.required_by,
            vec!["at-spi2-core", "avahi", "cairo", "dconf", "gtk3"]
        );
        assert_eq!(f.verified, "Signature");
        let explicit = parse_pacman_qi(
            "Name            : ripgrep\nVersion         : 14-1\nRequired By     : None\nInstall Reason  : Explicitly installed\n",
        )
        .unwrap();
        assert_eq!(explicit.reason, Reason::Explicit);
        assert!(explicit.required_by.is_empty());
        assert!(parse_pacman_qi("error: package 'x' was not found\n").is_none());
    }

    #[test]
    fn dpkg_query_line() {
        let f = parse_dpkg_query(
            "curl\t8.5.0-2ubuntu10\tii \tno\toptional\tUbuntu Developers <u@l.c>\tcommand line tool for transferring data\n",
        )
        .unwrap();
        assert_eq!(f.name, "curl");
        assert_eq!(f.version, "8.5.0-2ubuntu10");
        assert_eq!(f.manager, "apt");
        assert!(!f.essential);
        assert_eq!(f.packager, "Ubuntu Developers <u@l.c>");
        // Essential / Priority: required packages are base system, never "orphans"
        let bash =
            parse_dpkg_query("bash\t5.2\tii \tyes\trequired\tm\tGNU Bourne Again SHell\n").unwrap();
        assert!(bash.essential);
        assert_eq!(bash.base, "essential");
        // P2: `Priority: required` without `Essential: yes` (Debian 13)
        let charwidth = parse_dpkg_query(
            "libtext-charwidth-perl\t0.04-11+b4\tii \tno\trequired\tm\tget display widths of characters on the terminal\n",
        )
        .unwrap();
        assert!(charwidth.essential);
        assert_eq!(charwidth.base, "priority required");
        assert_eq!(f.base, "");
        // removed-but-config-remains ("rc") is not installed
        assert!(parse_dpkg_query("curl\t8\trc \tno\toptional\tm\ts\n").is_none());
    }

    #[test]
    fn apt_rdepends_and_policy() {
        let rd = "libssl3t64\nReverse Depends:\n  curl\n |openssh-client\n  curl\n";
        assert_eq!(parse_apt_rdepends(rd), vec!["curl", "openssh-client"]);

        let repo = "curl:\n  Installed: 8.5.0-2\n  Candidate: 8.5.0-2\n  Version table:\n *** 8.5.0-2 500\n        500 http://archive.ubuntu.com/ubuntu noble/main amd64 Packages\n        100 /var/lib/dpkg/status\n";
        let policy = parse_apt_policy(repo);
        let (origin, foreign, verified) = apt_origin(&policy);
        assert!(origin.starts_with("http://archive.ubuntu.com/ubuntu noble/main"));
        assert_eq!(foreign, Some(false));
        assert!(verified.starts_with("repository signature"));

        let local = "foo:\n  Installed: 1.0\n  Candidate: 1.0\n  Version table:\n *** 1.0 100\n        100 /var/lib/dpkg/status\n";
        assert_eq!(
            parse_apt_policy(local),
            AptPolicy::default(),
            "status-only means no remote origin"
        );
        let (origin, foreign, _) = apt_origin(&AptPolicy::default());
        assert_eq!(
            origin,
            "not in any configured repository (local .deb, removed PPA, or dropped)"
        );
        assert_eq!(foreign, Some(true));
    }

    /// P5: Ubuntu 24.04, a stock package whose installed version was
    /// superseded in the index: `why` said "not in any configured
    /// repository (local .deb, removed PPA, or dropped)".
    #[test]
    fn apt_policy_of_a_superseded_stock_package() {
        let text = "perl:\n  Installed: 5.38.2-3.2ubuntu0.6\n  Candidate: 5.38.2-3.2ubuntu0.7\n  Version table:\n     5.38.2-3.2ubuntu0.7 500\n        500 http://archive.ubuntu.com/ubuntu noble-updates/main amd64 Packages\n        500 http://security.ubuntu.com/ubuntu noble-security/main amd64 Packages\n *** 5.38.2-3.2ubuntu0.6 100\n        100 /var/lib/dpkg/status\n     5.38.2-3build1 500\n        500 http://archive.ubuntu.com/ubuntu noble/main amd64 Packages\n";
        let policy = parse_apt_policy(text);
        assert_eq!(policy.installed, None);
        assert_eq!(
            policy.newer.as_deref(),
            Some("http://archive.ubuntu.com/ubuntu noble-updates/main amd64 Packages")
        );
        assert_eq!(
            policy.older.as_deref(),
            Some("http://archive.ubuntu.com/ubuntu noble/main amd64 Packages")
        );
        let (origin, foreign, verified) = apt_origin(&policy);
        assert_eq!(
            origin,
            "the installed version is no longer in the repository index (a newer one is, from http://archive.ubuntu.com/ubuntu noble-updates/main amd64 Packages)"
        );
        // the package is offered by a configured repository
        assert_eq!(foreign, Some(false));
        assert!(verified.starts_with("unknown"), "{verified}");
        // newer than anything a repository has (a removed PPA): still foreign
        let ppa = "x:\n  Installed: 9.0\n  Candidate: 9.0\n  Version table:\n *** 9.0 100\n        100 /var/lib/dpkg/status\n     1.0 500\n        500 http://deb.debian.org/debian trixie/main amd64 Packages\n";
        let (origin, foreign, _) = apt_origin(&parse_apt_policy(ppa));
        assert!(origin.contains("an older one is, from http://deb.debian.org/debian trixie/main"));
        assert_eq!(foreign, Some(true));
    }

    /// D1: the dnf cache directory of a fresh Rocky 9 image holds
    /// `expired_repos.json`, `packages.db` and `tempfiles.json`, and that of
    /// a fresh Tumbleweed image holds `@System` under solv: no metadata.
    #[test]
    fn repository_metadata_is_told_apart_from_other_cache_entries() {
        let root = std::env::temp_dir().join(format!("yapacc-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // dnf
        let dnf = root.join("dnf");
        std::fs::create_dir_all(&dnf).unwrap();
        for f in ["expired_repos.json", "packages.db", "tempfiles.json"] {
            std::fs::write(dnf.join(f), "x").unwrap();
        }
        assert!(!dnf_cache_has_metadata(&dnf));
        assert!(!dnf_cache_has_metadata(&root.join("absent")));
        std::fs::create_dir_all(dnf.join("baseos-044cae74d71fe9ea/repodata")).unwrap();
        assert!(!dnf_cache_has_metadata(&dnf), "an empty repodata is none");
        std::fs::write(dnf.join("baseos-044cae74d71fe9ea/repodata/repomd.xml"), "x").unwrap();
        assert!(dnf_cache_has_metadata(&dnf));
        // zypper
        let repos_d = root.join("repos.d");
        let cache = root.join("zypp");
        std::fs::create_dir_all(&repos_d).unwrap();
        std::fs::create_dir_all(cache.join("solv/@System")).unwrap();
        std::fs::write(cache.join("solv/@System/solv"), "x").unwrap();
        std::fs::write(
            repos_d.join("repo-oss.repo"),
            "[repo-oss]\nname=openSUSE-Tumbleweed-Oss\nenabled=1\nautorefresh=1\nbaseurl=http://download.opensuse.org/tumbleweed/repo/oss/\n",
        )
        .unwrap();
        std::fs::write(
            repos_d.join("repo-debug.repo"),
            "[repo-debug]\nname=openSUSE-Tumbleweed-Debug\nenabled=0\n",
        )
        .unwrap();
        std::fs::write(
            repos_d.join("two.repo"),
            "[home:me/tools]\nbaseurl=http://x/\n\n[off]\nenabled = No\n",
        )
        .unwrap();
        std::fs::write(repos_d.join("notes.txt"), "[not-a-repo]\n").unwrap();
        assert_eq!(zypp_enabled_repos(&repos_d), ["home:me/tools", "repo-oss"]);
        // @System alone: every enabled repository lacks metadata
        assert_eq!(
            zypp_repos_without_metadata(&repos_d, &cache),
            ["home:me/tools", "repo-oss"]
        );
        // raw metadata of one, a solv file of the other (`/` becomes `_`)
        std::fs::create_dir_all(cache.join("raw/repo-oss/repodata")).unwrap();
        assert_eq!(
            zypp_repos_without_metadata(&repos_d, &cache),
            ["home:me/tools"]
        );
        std::fs::create_dir_all(cache.join("solv/home:me_tools")).unwrap();
        std::fs::write(cache.join("solv/home:me_tools/solv"), "x").unwrap();
        assert!(zypp_repos_without_metadata(&repos_d, &cache).is_empty());
        let _ = std::fs::remove_dir_all(&root);
        // the sentence
        assert_eq!(
            no_metadata_message(PkgManager::Dnf, &[]),
            "no repository metadata on disk: run `dnf makecache` (yapacc does not refresh on a read command)"
        );
        assert_eq!(
            no_metadata_message(PkgManager::Zypper, &["repo-update".to_string()]),
            "no repository metadata on disk for repo-update: run `zypper refresh` (yapacc does not refresh on a read command)"
        );
    }

    #[test]
    fn rpm_query_line() {
        let f = parse_rpm_q(
            "bash\t5.2.26-3.fc40\t1714000000\tFedora Project\tFedora Project\t(none)\tRSA/SHA256, Tue 01 Jan 2026, Key ID abcd\tThe GNU Bourne Again shell\n",
            "dnf",
        )
        .unwrap();
        assert_eq!(f.name, "bash");
        assert_eq!(f.version, "5.2.26-3.fc40");
        assert_eq!(f.installed, "2024-04-24 23:06 UTC");
        assert!(f.verified.contains("RSA/SHA256"));
        assert_eq!(f.packager, "Fedora Project");
        let unsigned =
            parse_rpm_q("x\t1-1\t1\t(none)\t(none)\t(none)\t(none)\ts\n", "dnf").unwrap();
        assert_eq!(unsigned.verified, "NOT signed");
    }

    #[test]
    fn hostile_names_never_reach_a_package_manager() {
        assert!(fact(PkgManager::Pacman, "--root=/tmp").is_err());
        assert!(fact(PkgManager::Apt, "a;b").is_err());
    }

    #[test]
    fn unsupported_backends_say_so() {
        let e = fact(PkgManager::Portage, "bash").unwrap_err();
        assert!(e.contains("not implemented"), "{e}");
    }

    #[test]
    fn runner_returns_status_and_stdout() {
        let (code, out) = run("sh", &["-c", "printf hi; exit 3"]).unwrap();
        assert_eq!((code, out.as_str()), (3, "hi"));
        let e = run("definitely-not-a-real-command-xyz", &[]).unwrap_err();
        assert_eq!(e.kind, crate::error::Kind::Unavailable);
        assert_eq!(run_ok("sh", &["-c", "printf hi"]).as_deref(), Some("hi"));
        assert_eq!(run_ok("sh", &["-c", "printf hi; exit 3"]), None);
    }

    #[test]
    fn backend_says_what_is_not_implemented() {
        let e = backend(PkgManager::Portage)
            .err()
            .expect("no portage backend");
        assert_eq!(e.kind, crate::error::Kind::Unavailable);
        assert_eq!(
            e.to_string(),
            "reading the portage database is not implemented (tier 3: detected only)"
        );
    }

    #[test]
    fn fixture_helper_strips_header_and_exit_line() {
        let (body, exit) = fixture("arch", "owner.txt");
        assert_eq!(body, "/usr/bin/env is owned by coreutils 9.12-2\n");
        assert_eq!(exit, 0);
        let (body, exit) = fixture("arch", "owner-missing.txt");
        assert_eq!(body, "error: No package owns /nonexistent/yapacc-fixture\n");
        assert_eq!(exit, 1);
        let (body, exit) = fixture("fedora", "updates-synthetic.txt");
        assert!(
            body.starts_with("bash\t5.3.20-1.fc45\tupdates\n"),
            "{body:?}"
        );
        assert_eq!(body.lines().count(), 3);
        assert_eq!(exit, 0);
        assert_eq!(fixture("debian", "explicit.txt"), (String::new(), 0));
    }

    #[test]
    fn tier_one_backends_refuse_hostile_input_before_any_command() {
        use crate::error::Kind;
        for m in [PkgManager::Pacman, PkgManager::Apt, PkgManager::Dnf] {
            let b = backend(m).expect("a tier-1 backend");
            assert_eq!(b.manager(), m);
            for bad in ["--root=/tmp", "a;b", ""] {
                assert_eq!(b.fact(bad).unwrap_err().kind, Kind::Usage);
                assert_eq!(b.candidate(bad).unwrap_err().kind, Kind::Usage);
                assert_eq!(b.files(bad).unwrap_err().kind, Kind::Usage);
            }
            for bad in ["-x", "", "a\nb"] {
                assert_eq!(b.search(bad).unwrap_err().kind, Kind::Usage);
            }
            for bad in ["relative/path", "-o", ""] {
                assert_eq!(b.owner_of(bad).unwrap_err().kind, Kind::Usage);
            }
        }
    }

    #[test]
    fn native_backend_reads_this_machine() {
        let Ok(b) = native_backend() else {
            eprintln!("skipping: no tier-1 native package manager on this machine");
            return;
        };
        let installed = b.installed().expect("the installed list");
        assert!(!installed.is_empty());
        assert!(installed.iter().all(|p| !p.name.is_empty()));
        assert_eq!(b.owner_of("/nonexistent/yapacc-test").unwrap(), None);
    }
}
