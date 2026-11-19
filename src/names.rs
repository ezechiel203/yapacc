// yapacc — `names`: what is this package called elsewhere?
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! `yapacc names NAME` asks repology.org how the distributions name one
//! piece of software, and reduces the answer to one line per release a
//! user is likely to care about.
//!
//! **This command talks to the network.** It sends the package name given
//! on the command line (and a `yapacc/<version>` User-Agent) to
//! repology.org over HTTPS, through the `curl` binary. Nothing else about
//! the machine is sent. It writes nothing.
//!
//! Limits, stated in the output as well: Repology is a community-maintained
//! mapping; a project of the same name is not always the same software, and
//! the API does not say which numbered release of a distribution is the
//! current stable one, so rows are labelled with the release number and
//! nothing is claimed about "stable".

use crate::error::{Error, Result};
use crate::platform::OsRelease;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

const API: &str = "https://repology.org/api/v1";

const NETWORK_NOTICE: &str = "`yapacc names` sends the package name to repology.org over HTTPS (through curl) and reads the answer; it needs network access and the `curl` binary. Nothing else is sent and nothing is written.";

const SOURCE_LINE: &str = "Source: repology.org (community-maintained mapping; a same-named project is not always the same software)";

/// One package entry of the Repology API (`/api/v1/project/<name>`).
/// Only `repo` and `version` are always present in the real answer;
/// `binname` (one binary package) and `binnames` (all binary packages of a
/// source package) are alternatives, and some repositories give neither.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RepoPkg {
    pub repo: String,
    #[serde(default)]
    pub subrepo: Option<String>,
    #[serde(default)]
    pub srcname: Option<String>,
    #[serde(default)]
    pub binname: Option<String>,
    #[serde(default)]
    pub binnames: Option<Vec<String>>,
    #[serde(default)]
    pub visiblename: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    /// "Debian 13", "Fedora Rawhide", "Arch"...
    pub distribution: String,
    /// Repology's repository name ("debian_13", "fedora_rawhide", "arch")
    pub repo: String,
    /// binary package names; the source name when the repository lists none
    pub names: Vec<String>,
    pub version: String,
    /// Repology's own status word: newest, outdated, legacy, rolling...
    pub status: String,
    /// which install command applies ("debian", "fedora", "arch"...)
    pub family: &'static str,
    /// `None`: several packages and none is named like the request
    pub install: Option<String>,
    /// this row is the distribution release this machine runs
    pub local: bool,
}

/// Families with exactly one row: a matching `ID` is enough to mark it.
const SINGLE_ROW_FAMILIES: [&str; 6] =
    ["arch", "opensuse", "void", "freebsd", "gentoo", "homebrew"];

fn release_numbers(repo: &str, prefix: &str) -> Option<Vec<u32>> {
    let rest = repo.strip_prefix(prefix)?;
    let parts: Option<Vec<u32>> = rest.split('_').map(|p| p.parse::<u32>().ok()).collect();
    parts.filter(|v| !v.is_empty())
}

/// Numbered releases of one distribution present in the answer, oldest
/// first.
fn numbered<'a>(repos: &BTreeSet<&'a str>, prefix: &str) -> Vec<(Vec<u32>, &'a str)> {
    let mut v: Vec<(Vec<u32>, &'a str)> = repos
        .iter()
        .filter_map(|r| release_numbers(r, prefix).map(|n| (n, *r)))
        .collect();
    v.sort();
    v
}

fn release_label(name: &str, repo: &str, prefix: &str) -> String {
    format!(
        "{name} {}",
        repo.strip_prefix(prefix).unwrap_or(repo).replace('_', ".")
    )
}

/// Which repositories get a row, in display order: (family, label, repo).
/// Pure; works only from the repository names present in the answer.
pub fn selected_repos(repos: &BTreeSet<&str>) -> Vec<(&'static str, String, String)> {
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    let fixed = |family: &'static str,
                 label: &str,
                 repo: &str,
                 out: &mut Vec<(&'static str, String, String)>| {
        if repos.contains(repo) {
            out.push((family, label.to_string(), repo.to_string()));
        }
    };
    fixed("arch", "Arch", "arch", &mut out);

    // Debian: the API does not say which number is "stable" (the highest
    // is usually testing), so the two highest are shown under their number.
    let debian = numbered(repos, "debian_");
    for (_, repo) in debian.iter().skip(debian.len().saturating_sub(2)) {
        out.push((
            "debian",
            release_label("Debian", repo, "debian_"),
            repo.to_string(),
        ));
    }
    fixed("debian", "Debian unstable", "debian_unstable", &mut out);

    // Ubuntu: the highest LTS-numbered release (even year, .04), and the
    // highest release overall when that is a different one.
    let ubuntu = numbered(repos, "ubuntu_");
    let lts = ubuntu
        .iter()
        .rev()
        .find(|(n, _)| n.len() == 2 && n[0] % 2 == 0 && n[1] == 4)
        .map(|(_, r)| *r);
    let top = ubuntu.last().map(|(_, r)| *r);
    if let Some(repo) = lts {
        out.push((
            "ubuntu",
            format!("{} LTS", release_label("Ubuntu", repo, "ubuntu_")),
            repo.to_string(),
        ));
    }
    if let Some(repo) = top
        && Some(repo) != lts
    {
        out.push((
            "ubuntu",
            release_label("Ubuntu", repo, "ubuntu_"),
            repo.to_string(),
        ));
    }

    if let Some((_, repo)) = numbered(repos, "fedora_").last() {
        out.push((
            "fedora",
            release_label("Fedora", repo, "fedora_"),
            repo.to_string(),
        ));
    }
    fixed("fedora", "Fedora Rawhide", "fedora_rawhide", &mut out);
    fixed(
        "opensuse",
        "openSUSE Tumbleweed",
        "opensuse_tumbleweed",
        &mut out,
    );
    if let Some((_, repo)) = numbered(repos, "alpine_").last() {
        out.push((
            "alpine",
            release_label("Alpine", repo, "alpine_"),
            repo.to_string(),
        ));
    }
    fixed("alpine", "Alpine edge", "alpine_edge", &mut out);

    // Void is published per architecture; x86_64 when present, else the
    // first one in name order.
    let void = if repos.contains("void_x86_64") {
        Some("void_x86_64")
    } else {
        repos.iter().copied().find(|r| r.starts_with("void_"))
    };
    if let Some(repo) = void {
        out.push(("void", "Void".to_string(), repo.to_string()));
    }
    fixed("freebsd", "FreeBSD", "freebsd", &mut out);
    fixed("gentoo", "Gentoo", "gentoo", &mut out);
    fixed("nix", "Nixpkgs unstable", "nix_unstable", &mut out);
    fixed("homebrew", "Homebrew", "homebrew", &mut out);
    out
}

/// Repology's status words, best first. One repository often carries
/// several entries (Fedora `release` and `updates`, Gentoo slots, Homebrew
/// `HEAD`): the row shows the best one.
fn status_rank(status: &str) -> u8 {
    match status {
        "newest" => 6,
        "unique" => 5,
        "devel" => 4,
        "outdated" => 3,
        "legacy" => 2,
        // rolling, noscheme, incorrect, untrusted, ignored
        _ => 1,
    }
}

fn is_debug_name(n: &str) -> bool {
    ["-debuginfo", "-debugsource", "-dbgsym", "-dbg"]
        .iter()
        .any(|s| n.ends_with(s))
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !s.is_empty() && !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

fn version_of(p: &RepoPkg) -> String {
    p.version.clone().unwrap_or_default()
}

fn status_of(p: &RepoPkg) -> String {
    p.status.clone().unwrap_or_else(|| "unknown".to_string())
}

/// One row per selected repository. Pure. `install` and `local` are left
/// unset; see [`set_install`] and [`mark_local`].
pub fn summarize(packages: &[RepoPkg]) -> Vec<Row> {
    let repos: BTreeSet<&str> = packages.iter().map(|p| p.repo.as_str()).collect();
    let mut rows = Vec::new();
    for (family, label, repo) in selected_repos(&repos) {
        let entries: Vec<&RepoPkg> = packages.iter().filter(|p| p.repo == repo).collect();
        let Some(best) = entries.iter().copied().max_by(|a, b| {
            status_rank(&status_of(a))
                .cmp(&status_rank(&status_of(b)))
                .then_with(|| crate::vercmp::vercmp_full(&version_of(a), &version_of(b)))
        }) else {
            continue;
        };
        let version = version_of(best);
        let same: Vec<&RepoPkg> = entries
            .iter()
            .copied()
            .filter(|p| version_of(p) == version)
            .collect();
        let mut names: Vec<String> = Vec::new();
        for p in &same {
            if let Some(b) = &p.binname {
                push_unique(&mut names, b);
            }
            for b in p.binnames.iter().flatten() {
                push_unique(&mut names, b);
            }
        }
        if names.is_empty() {
            for p in &same {
                if let Some(s) = &p.srcname {
                    push_unique(&mut names, s);
                }
            }
        }
        if names.is_empty() {
            for p in &same {
                if let Some(s) = &p.visiblename {
                    push_unique(&mut names, s);
                }
            }
        }
        if names.iter().any(|n| !is_debug_name(n)) {
            names.retain(|n| !is_debug_name(n));
        }
        rows.push(Row {
            distribution: label,
            repo,
            names,
            version,
            status: status_of(best),
            family,
            install: None,
            local: false,
        });
    }
    rows
}

/// The install command of a family for one package name.
pub fn install_command(family: &str, name: &str) -> Option<String> {
    Some(match family {
        "arch" => format!("pacman -S {name}"),
        "debian" | "ubuntu" => format!("apt install {name}"),
        "fedora" => format!("dnf install {name}"),
        "opensuse" => format!("zypper install {name}"),
        "alpine" => format!("apk add {name}"),
        "void" => format!("xbps-install {name}"),
        "freebsd" => format!("pkg install {name}"),
        "gentoo" => format!("emerge {name}"),
        "nix" => format!("nix-env -iA nixpkgs.{name}"),
        "homebrew" => format!("brew install {name}"),
        _ => return None,
    })
}

/// Fill `install`, and move the chosen name to the front of `names`. The
/// name is the one equal to a wanted name (the request, then the Repology
/// project), or the only one. With several names and no match nothing is
/// chosen: picking one would be a guess.
pub fn set_install(rows: &mut [Row], wanted: &[&str]) {
    for row in rows.iter_mut() {
        let chosen = wanted
            .iter()
            .find_map(|w| row.names.iter().position(|n| n == w))
            .or(if row.names.len() == 1 { Some(0) } else { None });
        if let Some(i) = chosen {
            let name = row.names.remove(i);
            row.install = install_command(row.family, &name);
            row.names.insert(0, name);
        }
    }
}

/// Which row is this machine's: the family, and for families with several
/// rows the exact Repology repository of the running release. `None` in
/// the second place means the release could not be told from os-release
/// (Debian testing and unstable, for instance, look the same there).
pub fn local_target(os: &OsRelease) -> Option<(&'static str, Option<String>)> {
    let v = os.version_id.as_str();
    fn numeric(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    match os.id.as_str() {
        "arch" => Some(("arch", None)),
        "debian" => {
            let major = v.split('.').next().unwrap_or("");
            Some(("debian", numeric(major).then(|| format!("debian_{major}"))))
        }
        "ubuntu" => {
            let parts: Vec<&str> = v.split('.').collect();
            let ok = parts.len() == 2 && parts.iter().all(|p| numeric(p));
            Some((
                "ubuntu",
                ok.then(|| format!("ubuntu_{}_{}", parts[0], parts[1])),
            ))
        }
        "fedora" => {
            if os.pretty_name.to_lowercase().contains("rawhide") {
                Some(("fedora", Some("fedora_rawhide".to_string())))
            } else {
                Some(("fedora", numeric(v).then(|| format!("fedora_{v}"))))
            }
        }
        "opensuse-tumbleweed" => Some(("opensuse", None)),
        "alpine" => {
            if v.contains("alpha") || os.pretty_name.to_lowercase().contains("edge") {
                return Some(("alpine", Some("alpine_edge".to_string())));
            }
            let parts: Vec<&str> = v.split('.').collect();
            let ok = parts.len() >= 2 && numeric(parts[0]) && numeric(parts[1]);
            Some((
                "alpine",
                ok.then(|| format!("alpine_{}_{}", parts[0], parts[1])),
            ))
        }
        "void" => Some(("void", None)),
        "freebsd" => Some(("freebsd", None)),
        "gentoo" => Some(("gentoo", None)),
        // which nixpkgs channel a NixOS machine follows is not in os-release
        "nixos" => Some(("nix", None)),
        _ => None,
    }
}

/// Mark the machine's own row. Returns whether one was marked.
pub fn mark_local(rows: &mut [Row], os: &OsRelease) -> bool {
    let Some((family, repo)) = local_target(os) else {
        return false;
    };
    let mut any = false;
    for row in rows.iter_mut().filter(|r| r.family == family) {
        row.local = match &repo {
            Some(r) => row.repo == *r,
            None => SINGLE_ROW_FAMILIES.contains(&family),
        };
        any |= row.local;
    }
    any
}

/// Outcome of the search fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Project(String),
    Ambiguous(Vec<String>),
    Nothing,
}

fn carries_name(pkgs: &[RepoPkg], name: &str) -> bool {
    pkgs.iter().any(|p| {
        p.binname.as_deref() == Some(name)
            || p.srcname.as_deref() == Some(name)
            || p.binnames.iter().flatten().any(|b| b == name)
    })
}

/// Choose a project from `/api/v1/projects/?search=`: the only one, or the
/// only one that has a package named exactly `name`. Anything else is
/// ambiguous and is not decided here. Pure.
pub fn choose_project(name: &str, found: &BTreeMap<String, Vec<RepoPkg>>) -> Choice {
    if found.is_empty() {
        return Choice::Nothing;
    }
    if found.len() == 1 {
        return Choice::Project(found.keys().next().cloned().unwrap_or_default());
    }
    let exact: Vec<&String> = found
        .iter()
        .filter(|(_, pkgs)| carries_name(pkgs, name))
        .map(|(k, _)| k)
        .collect();
    match exact.as_slice() {
        [one] => Choice::Project((*one).clone()),
        [] => Choice::Ambiguous(found.keys().cloned().collect()),
        several => Choice::Ambiguous(several.iter().map(|s| (*s).clone()).collect()),
    }
}

/// Percent-encode one URL path or query component.
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    let agent = format!("yapacc/{}", env!("CARGO_PKG_VERSION"));
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "-A", agent.as_str(), url])
        .output()
        .map_err(|e| Error::unavailable(format!("cannot run curl: {e}\n{NETWORK_NOTICE}")))?;
    if !out.status.success() {
        return Err(Error::unavailable(format!(
            "repology.org could not be reached (curl: {}): {}\n{NETWORK_NOTICE}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// An answer of the API as a typed value. Every string of it is made
/// printable first (`aur_rpc::printable`): names, versions and status
/// words are printed on the terminal, and what a remote service sends must
/// not carry an escape sequence there. Pure.
fn parse_answer<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    let not_json = |e: serde_json::Error| {
        Error::from(format!(
            "repology.org answered something that is not the expected JSON: {e}"
        ))
    };
    let mut value: serde_json::Value = serde_json::from_slice(body).map_err(not_json)?;
    printable_json(&mut value);
    serde_json::from_value(value).map_err(not_json)
}

fn printable_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) if s.chars().any(char::is_control) => {
            *s = crate::aur_rpc::printable(s)
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(printable_json),
        serde_json::Value::Object(members) => {
            // project names are the keys of the search answer
            let cleaned: serde_json::Map<String, serde_json::Value> = std::mem::take(members)
                .into_iter()
                .map(|(k, mut v)| {
                    printable_json(&mut v);
                    (crate::aur_rpc::printable(&k), v)
                })
                .collect();
            *members = cleaned;
        }
        _ => {}
    }
}

fn fetch_project(project: &str) -> Result<Vec<RepoPkg>> {
    parse_answer(&http_get(&format!(
        "{API}/project/{}",
        url_encode(project)
    ))?)
}

fn fetch_search(name: &str) -> Result<BTreeMap<String, Vec<RepoPkg>>> {
    parse_answer(&http_get(&format!(
        "{API}/projects/?search={}",
        url_encode(name)
    ))?)
}

/// The Repology project for `name` and its packages.
fn lookup(name: &str) -> Result<(String, Vec<RepoPkg>)> {
    let direct = fetch_project(name)?;
    if !direct.is_empty() {
        return Ok((name.to_string(), direct));
    }
    match choose_project(name, &fetch_search(name)?) {
        Choice::Project(project) => {
            // the search answer is an abridged form; ask for the project itself
            let pkgs = fetch_project(&project)?;
            if pkgs.is_empty() {
                return Err(Error::not_found(format!(
                    "repology.org lists a project '{project}' for '{name}' but returned no package for it"
                )));
            }
            Ok((project, pkgs))
        }
        Choice::Ambiguous(candidates) => Err(Error::not_found(format!(
            "repology.org has no project named '{name}', and several that match it; nothing was chosen. Candidates:\n  {}\nRun `yapacc names <candidate>` with the one you mean.",
            candidates.join("\n  ")
        ))),
        Choice::Nothing => Err(Error::not_found(format!(
            "repology.org has no project named or matching '{name}'. Its project names are not always the local package name: try the upstream name."
        ))),
    }
}

fn names_cell(names: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut s = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        s.push_str(&format!(" (+{} more)", names.len() - SHOWN));
    }
    s
}

/// The aligned table. Pure.
pub fn render(rows: &[Row]) -> String {
    let header = ["DISTRIBUTION", "PACKAGE", "VERSION", "STATUS", "INSTALL"];
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|r| {
            [
                r.distribution.clone(),
                names_cell(&r.names),
                r.version.clone(),
                r.status.clone(),
                r.install
                    .clone()
                    .unwrap_or_else(|| "(several packages: choose one)".to_string()),
            ]
        })
        .collect();
    let mut width = header.map(|h| h.chars().count());
    for c in &cells {
        for (i, cell) in c.iter().enumerate() {
            width[i] = width[i].max(cell.chars().count());
        }
    }
    let line = |mark: &str, c: [&str; 5]| {
        format!(
            "{mark} {:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            c[0],
            c[1],
            c[2],
            c[3],
            c[4],
            w0 = width[0],
            w1 = width[1],
            w2 = width[2],
            w3 = width[3],
        )
        .trim_end()
        .to_string()
    };
    let mut out = line(" ", header);
    out.push('\n');
    for (row, c) in rows.iter().zip(&cells) {
        out.push_str(&line(
            if row.local { "*" } else { " " },
            [
                c[0].as_str(),
                c[1].as_str(),
                c[2].as_str(),
                c[3].as_str(),
                c[4].as_str(),
            ],
        ));
        out.push('\n');
    }
    out
}

pub fn run(name: &str, json: bool) -> Result<()> {
    if !crate::util::valid_pkg_name(name) {
        return Err(Error::usage(format!(
            "'{name}' is not a valid package name"
        )));
    }
    let (project, packages) = lookup(name)?;
    let mut rows = summarize(&packages);
    set_install(&mut rows, &[name, project.as_str()]);
    let os = crate::platform::detect().os;
    let marked = mark_local(&mut rows, &os);

    if json {
        return match serde_json::to_string_pretty(&rows) {
            Ok(s) => {
                println!("{s}");
                Ok(())
            }
            Err(e) => Err(Error::from(format!("json serialization failed: {e}"))),
        };
    }

    if project == name {
        println!("Repology project: {project}");
    } else {
        println!("Repology project: {project} (no project is named '{name}'; found by search)");
    }
    let all_repos: BTreeSet<&str> = packages.iter().map(|p| p.repo.as_str()).collect();
    if rows.is_empty() {
        println!(
            "None of the distributions yapacc lists carries it ({} other repositories do: https://repology.org/project/{}/versions).",
            all_repos.len(),
            url_encode(&project)
        );
    } else {
        println!();
        print!("{}", render(&rows));
        println!();
        if marked {
            println!("* = the distribution release this machine runs");
        } else {
            println!(
                "No row is marked: this machine reports ID={}{} and none of the rows above is that release.",
                os.id,
                if os.version_id.is_empty() {
                    String::new()
                } else {
                    format!(" VERSION_ID={}", os.version_id)
                }
            );
        }
        println!(
            "Version and status are the best entry of each repository; status is Repology's word (newest, outdated, legacy...). Numbered releases are shown by number: Repology does not say which one is the current stable."
        );
        if rows.len() < all_repos.len() {
            println!(
                "{} more repositories carry it: https://repology.org/project/{}/versions",
                all_repos.len() - rows.len(),
                url_encode(&project)
            );
        }
    }
    println!("{SOURCE_LINE}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the shape of https://repology.org/api/v1/project/ripgrep as
    // observed on 2026-11-16: `binname` on some repositories, `binnames` on
    // others, neither on Gentoo/Nix/Homebrew; `origversion` may be null;
    // several entries per repository.
    const SAMPLE: &str = r#"[
      {"repo":"homebrew","srcname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":null,"status":"newest","summary":"Search tool"},
      {"repo":"homebrew","srcname":"ripgrep","visiblename":"ripgrep","version":"HEAD","origversion":null,"status":"rolling"},
      {"repo":"alpine_3_23","subrepo":"community","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.1.0","origversion":"15.1.0-r0","status":"outdated","maintainers":["a@example.org"],"licenses":["MIT"]},
      {"repo":"alpine_3_24","subrepo":"community","srcname":"ripgrep","binname":"ripgrep-doc","visiblename":"ripgrep-doc","version":"15.1.0","origversion":"15.1.0-r1","status":"outdated"},
      {"repo":"alpine_3_24","subrepo":"community","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.1.0","origversion":"15.1.0-r1","status":"outdated"},
      {"repo":"alpine_edge","subrepo":"community","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":"15.2.0-r0","status":"newest"},
      {"repo":"arch","subrepo":"extra","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":"15.2.0-1","status":"newest"},
      {"repo":"aur","srcname":"ripgrep-git","binname":"ripgrep-git","visiblename":"ripgrep-git","version":"15.2.0","origversion":null,"status":"rolling"},
      {"repo":"debian_12","subrepo":"bookworm/main","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"13.0.0","origversion":"13.0.0-4","status":"outdated"},
      {"repo":"debian_13","subrepo":"trixie/main","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"14.1.1","origversion":"14.1.1-1","status":"outdated"},
      {"repo":"debian_14","subrepo":"forky/main","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":"15.2.0-1","status":"newest"},
      {"repo":"debian_unstable","subrepo":"sid/main","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":"15.2.0-1","status":"newest"},
      {"repo":"fedora_43","subrepo":"updates","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"fedora_44","subrepo":"release","srcname":"rust-ripgrep","binnames":["ripgrep","rust-ripgrep-debuginfo","rust-ripgrep-debugsource"],"visiblename":"rust-ripgrep","version":"14.1.1","origversion":null,"status":"legacy"},
      {"repo":"fedora_44","subrepo":"updates","srcname":"rust-ripgrep","binnames":["ripgrep","rust-ripgrep-debuginfo","rust-ripgrep-debugsource"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"fedora_rawhide","subrepo":"development","srcname":"rust-ripgrep","binnames":["ripgrep","rust-ripgrep-debuginfo","rust-ripgrep-debugsource"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"freebsd","srcname":"textproc/ripgrep","binname":"ripgrep","visiblename":"textproc/ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"gentoo","srcname":"sys-apps/ripgrep","visiblename":"sys-apps/ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"gentoo","srcname":"sys-apps/ripgrep","visiblename":"sys-apps/ripgrep","version":"15.1.0","origversion":null,"status":"legacy"},
      {"repo":"nix_stable_26_05","srcname":"ripgrep","visiblename":"ripgrep","version":"15.1.0","origversion":null,"status":"outdated"},
      {"repo":"nix_unstable","srcname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"opensuse_leap_15_6","subrepo":"main/oss","srcname":"ripgrep","visiblename":"ripgrep","version":"13.0.0","origversion":null,"status":"outdated"},
      {"repo":"opensuse_tumbleweed","subrepo":"src-oss","srcname":"ripgrep","binnames":["ripgrep","ripgrep-debuginfo","ripgrep-bash-completion","ripgrep-zsh-completion","ripgrep-fish-completion"],"visiblename":"ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"ubuntu_24_04","subrepo":"noble/universe","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"14.1.0","origversion":null,"status":"outdated"},
      {"repo":"ubuntu_26_04","subrepo":"resolute/universe","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"15.1.0","origversion":null,"status":"outdated"},
      {"repo":"ubuntu_26_10","subrepo":"stonking/universe","srcname":"rust-ripgrep","binnames":["ripgrep"],"visiblename":"rust-ripgrep","version":"15.2.0","origversion":null,"status":"newest"},
      {"repo":"void_x86_64","subrepo":"x86_64","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":null,"status":"newest"}
    ]"#;

    fn sample() -> Vec<RepoPkg> {
        serde_json::from_str(SAMPLE).expect("sample parses")
    }

    fn os(id: &str, version_id: &str, pretty: &str) -> OsRelease {
        OsRelease {
            id: id.to_string(),
            id_like: vec![],
            pretty_name: pretty.to_string(),
            version_id: version_id.to_string(),
        }
    }

    /// What repology.org sends is printed in a table: a control character
    /// in a name, a version or a project name does not reach the terminal.
    #[test]
    fn control_characters_of_an_answer_are_not_kept() {
        let body = br#"{"tree\u001b[2J":[{"repo":"arch","binname":"tree","version":"2.3.2\u001b[31m","status":"newest"}]}"#;
        let found: BTreeMap<String, Vec<RepoPkg>> = parse_answer(body).unwrap();
        let (project, pkgs) = found.iter().next().unwrap();
        assert_eq!(project, "tree\u{fffd}[2J");
        assert_eq!(pkgs[0].version.as_deref(), Some("2.3.2\u{fffd}[31m"));
        let plain: Vec<RepoPkg> =
            parse_answer(br#"[{"repo":"arch","binname":"tree","version":"2.3.2"}]"#).unwrap();
        assert_eq!(plain[0].version.as_deref(), Some("2.3.2"));
        assert!(parse_answer::<Vec<RepoPkg>>(b"<html>").is_err());
    }

    #[test]
    fn one_row_per_release_in_order() {
        let rows = summarize(&sample());
        let got: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.distribution.as_str(), r.repo.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Arch", "arch"),
                ("Debian 13", "debian_13"),
                ("Debian 14", "debian_14"),
                ("Debian unstable", "debian_unstable"),
                ("Ubuntu 26.04 LTS", "ubuntu_26_04"),
                ("Ubuntu 26.10", "ubuntu_26_10"),
                ("Fedora 44", "fedora_44"),
                ("Fedora Rawhide", "fedora_rawhide"),
                ("openSUSE Tumbleweed", "opensuse_tumbleweed"),
                ("Alpine 3.24", "alpine_3_24"),
                ("Alpine edge", "alpine_edge"),
                ("Void", "void_x86_64"),
                ("FreeBSD", "freebsd"),
                ("Gentoo", "gentoo"),
                ("Nixpkgs unstable", "nix_unstable"),
                ("Homebrew", "homebrew"),
            ]
        );
    }

    #[test]
    fn best_entry_and_names_of_each_repository() {
        let rows = summarize(&sample());
        let row = |repo: &str| rows.iter().find(|r| r.repo == repo).expect(repo);
        // `updates` (newest) wins over `release` (legacy); debug packages dropped
        assert_eq!(row("fedora_44").version, "15.2.0");
        assert_eq!(row("fedora_44").status, "newest");
        assert_eq!(row("fedora_44").names, vec!["ripgrep"]);
        // binary name, not the source name
        assert_eq!(row("debian_13").names, vec!["ripgrep"]);
        assert_eq!(row("debian_13").status, "outdated");
        // no binary name given: the source name
        assert_eq!(row("gentoo").names, vec!["sys-apps/ripgrep"]);
        assert_eq!(row("gentoo").version, "15.2.0");
        // a rolling HEAD entry does not hide the release
        assert_eq!(row("homebrew").version, "15.2.0");
        assert_eq!(row("freebsd").names, vec!["ripgrep"]);
        assert_eq!(row("alpine_3_24").names, vec!["ripgrep-doc", "ripgrep"]);
        assert_eq!(row("opensuse_tumbleweed").names.len(), 4);
    }

    #[test]
    fn install_command_uses_the_matching_name_or_none() {
        let mut rows = summarize(&sample());
        set_install(&mut rows, &["ripgrep", "ripgrep"]);
        let row = |repo: &str| rows.iter().find(|r| r.repo == repo).expect(repo);
        assert_eq!(row("arch").install.as_deref(), Some("pacman -S ripgrep"));
        assert_eq!(
            row("debian_13").install.as_deref(),
            Some("apt install ripgrep")
        );
        assert_eq!(
            row("ubuntu_26_04").install.as_deref(),
            Some("apt install ripgrep")
        );
        assert_eq!(
            row("fedora_44").install.as_deref(),
            Some("dnf install ripgrep")
        );
        assert_eq!(
            row("opensuse_tumbleweed").install.as_deref(),
            Some("zypper install ripgrep")
        );
        assert_eq!(
            row("alpine_3_24").install.as_deref(),
            Some("apk add ripgrep")
        );
        assert_eq!(row("alpine_3_24").names[0], "ripgrep");
        assert_eq!(
            row("void_x86_64").install.as_deref(),
            Some("xbps-install ripgrep")
        );
        assert_eq!(
            row("freebsd").install.as_deref(),
            Some("pkg install ripgrep")
        );
        assert_eq!(
            row("gentoo").install.as_deref(),
            Some("emerge sys-apps/ripgrep")
        );
        assert_eq!(
            row("nix_unstable").install.as_deref(),
            Some("nix-env -iA nixpkgs.ripgrep")
        );
        assert_eq!(
            row("homebrew").install.as_deref(),
            Some("brew install ripgrep")
        );

        // several names, none like the request: nothing is chosen
        let mut rows = summarize(&sample());
        set_install(&mut rows, &["rg"]);
        let alpine = rows.iter().find(|r| r.repo == "alpine_3_24").unwrap();
        assert_eq!(alpine.install, None);
    }

    #[test]
    fn local_row_is_marked_only_when_the_release_is_known() {
        let mut rows = summarize(&sample());
        assert!(mark_local(&mut rows, &os("arch", "", "Arch Linux")));
        assert_eq!(
            rows.iter()
                .filter(|r| r.local)
                .map(|r| r.repo.as_str())
                .collect::<Vec<_>>(),
            vec!["arch"]
        );

        let mut rows = summarize(&sample());
        assert!(mark_local(
            &mut rows,
            &os("debian", "13", "Debian GNU/Linux 13 (trixie)")
        ));
        assert_eq!(
            rows.iter()
                .filter(|r| r.local)
                .map(|r| r.repo.as_str())
                .collect::<Vec<_>>(),
            vec!["debian_13"]
        );

        // testing and unstable carry no VERSION_ID: not guessed
        let mut rows = summarize(&sample());
        assert!(!mark_local(
            &mut rows,
            &os("debian", "", "Debian GNU/Linux forky/sid")
        ));
        // a release that has no row
        let mut rows = summarize(&sample());
        assert!(!mark_local(
            &mut rows,
            &os("ubuntu", "24.04", "Ubuntu 24.04.3 LTS")
        ));
        // a derivative is not the distribution it derives from
        let mut rows = summarize(&sample());
        assert!(!mark_local(
            &mut rows,
            &os("linuxmint", "22", "Linux Mint 22")
        ));

        assert_eq!(
            local_target(&os("ubuntu", "26.04", "")),
            Some(("ubuntu", Some("ubuntu_26_04".to_string())))
        );
        assert_eq!(
            local_target(&os("alpine", "3.24.1", "Alpine Linux v3.24")),
            Some(("alpine", Some("alpine_3_24".to_string())))
        );
        assert_eq!(
            local_target(&os("alpine", "3.25_alpha20261101", "Alpine Linux edge")),
            Some(("alpine", Some("alpine_edge".to_string())))
        );
        assert_eq!(
            local_target(&os("fedora", "46", "Fedora Linux 46 (Rawhide Prerelease)")),
            Some(("fedora", Some("fedora_rawhide".to_string())))
        );
    }

    #[test]
    fn search_fallback_chooses_only_when_unambiguous() {
        // shape of /api/v1/projects/?search=NAME: object of project -> packages
        let found: BTreeMap<String, Vec<RepoPkg>> = serde_json::from_str(
            r#"{
              "ripgrep":[{"repo":"arch","srcname":"ripgrep","binname":"ripgrep","visiblename":"ripgrep","version":"15.2.0","origversion":null,"status":"newest"}],
              "ripgrep-all":[{"repo":"arch","srcname":"ripgrep-all","binname":"ripgrep-all","visiblename":"ripgrep-all","version":"0.10.9","origversion":null,"status":"newest"}],
              "emacs:ripgrep":[{"repo":"nix_unstable","srcname":"emacsPackages.ripgrep","visiblename":"emacs-ripgrep","version":"0.4.0","origversion":null,"status":"newest"}]
            }"#,
        )
        .unwrap();
        assert_eq!(
            choose_project("ripgrep", &found),
            Choice::Project("ripgrep".into())
        );
        assert_eq!(
            choose_project("rip", &found),
            Choice::Ambiguous(vec![
                "emacs:ripgrep".into(),
                "ripgrep".into(),
                "ripgrep-all".into()
            ])
        );
        let one: BTreeMap<String, Vec<RepoPkg>> = found
            .iter()
            .filter(|(k, _)| *k == "ripgrep-all")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            choose_project("rga", &one),
            Choice::Project("ripgrep-all".into())
        );
        assert_eq!(choose_project("x", &BTreeMap::new()), Choice::Nothing);
    }

    #[test]
    fn table_is_aligned_and_marks_the_local_row() {
        let mut rows = summarize(&sample());
        set_install(&mut rows, &["ripgrep"]);
        mark_local(&mut rows, &os("arch", "", ""));
        let text = render(&rows);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("  DISTRIBUTION"));
        assert!(lines[1].starts_with("* Arch "));
        assert!(lines[1].ends_with("pacman -S ripgrep"));
        let col = lines[0].find("INSTALL").unwrap();
        assert_eq!(lines[1].find("pacman -S"), Some(col));
        assert!(text.contains("(+1 more)"));
    }

    #[test]
    fn names_are_encoded_before_they_enter_a_url() {
        assert_eq!(url_encode("c++utilities"), "c%2B%2Butilities");
        assert_eq!(url_encode("emacs:ripgrep"), "emacs%3Aripgrep");
        assert_eq!(url_encode("foo@1.2"), "foo%401.2");
    }
}
