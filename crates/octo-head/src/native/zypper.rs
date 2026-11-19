// yapacc — native backend: zypper (openSUSE, SLE)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Installed state comes from the rpm database (the same queries as the
//! dnf backend). Repository questions run `zypper --no-refresh`, and only
//! when the metadata of every enabled repository is on disk:
//! `--no-refresh` skips refreshing metadata that exists, but zypper run as
//! root downloads metadata that does not (observed on Tumbleweed with an
//! empty /var/cache/zypp). Without it they answer with nothing and
//! `missing_metadata` says why, so nothing is ever downloaded by a read
//! command. zypper prints tables with `|` separators; [`parse_table`] reads
//! them by column title.

use super::{Candidate, Installed, NativeBackend, NativeFact, Reason, Update};
use crate::error::Error;
use crate::platform::PkgManager;

/// rpm expands `\\t` and `\\n` itself.
const LIST_FORMAT: &str = "%{NAME}\\t%{EVR}\\t%{ARCH}\\t%{SIZE}\\t%{INSTALLTIME}\\t%{SUMMARY}\\n";
/// Packages zypper installed to satisfy a dependency, one name per line.
const AUTO_INSTALLED: &str = "/var/lib/zypp/AutoInstalled";

/// Parse `rpm -qa --qf LIST_FORMAT`.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    let mut v: Vec<Installed> = text
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.splitn(6, '\t').collect();
            (p.len() == 6 && !p[0].is_empty() && p[0] != "gpg-pubkey").then(|| Installed {
                name: p[0].into(),
                version: p[1].into(),
                arch: p[2].into(),
                size_bytes: p[3].parse().unwrap_or(0),
                description: p[5].trim().into(),
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// A zypper table: the header row's titles, then the data rows.
pub fn parse_table(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut header: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut after_rule = false;
    let mut previous: Option<Vec<String>> = None;
    for line in text.lines() {
        let is_rule = line.starts_with("--") && line.contains('+');
        if is_rule {
            if let Some(h) = previous.take() {
                header = h;
            }
            after_rule = true;
            continue;
        }
        if !line.contains('|') {
            previous = None;
            continue;
        }
        let cells: Vec<String> = line.split('|').map(|c| c.trim().to_string()).collect();
        if after_rule {
            rows.push(cells);
        } else {
            previous = Some(cells);
        }
    }
    (header, rows)
}

fn column(header: &[String], title: &str) -> Option<usize> {
    header.iter().position(|h| h.eq_ignore_ascii_case(title))
}

fn cell(row: &[String], idx: Option<usize>) -> String {
    idx.and_then(|i| row.get(i)).cloned().unwrap_or_default()
}

/// Parse `zypper search -s`: `S | Name | Type | Version | Arch | Repository`,
/// or plain `zypper search`: `S | Name | Summary | Type`.
pub fn parse_search(text: &str) -> Vec<Candidate> {
    let (h, rows) = parse_table(text);
    let (name, version, repo, summary, kind) = (
        column(&h, "Name"),
        column(&h, "Version"),
        column(&h, "Repository"),
        column(&h, "Summary"),
        column(&h, "Type"),
    );
    rows.iter()
        .filter(|r| {
            let k = cell(r, kind);
            k.is_empty() || k == "package"
        })
        .map(|r| Candidate {
            name: cell(r, name),
            version: cell(r, version),
            repo: cell(r, repo),
            description: cell(r, summary),
        })
        .filter(|c| !c.name.is_empty())
        .collect()
}

pub fn parse_candidate(text: &str, name: &str) -> Option<Candidate> {
    let all = parse_search(text);
    // prefer a row from a real repository over the "(System Packages)" row
    all.iter()
        .find(|c| c.name == name && !c.repo.starts_with('('))
        .or_else(|| all.iter().find(|c| c.name == name))
        .cloned()
}

/// Parse `zypper packages --unneeded`: `S | Repository | Name | Version | Arch`.
pub fn parse_orphans(text: &str) -> Vec<String> {
    let (h, rows) = parse_table(text);
    let name = column(&h, "Name");
    let mut v: Vec<String> = rows
        .iter()
        .map(|r| cell(r, name))
        .filter(|n| !n.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Parse `zypper list-updates`:
/// `S | Repository | Name | Current Version | Available Version | Arch`.
pub fn parse_updates(text: &str) -> Vec<Update> {
    let (h, rows) = parse_table(text);
    let (name, old, new, repo) = (
        column(&h, "Name"),
        column(&h, "Current Version"),
        column(&h, "Available Version"),
        column(&h, "Repository"),
    );
    rows.iter()
        .map(|r| Update {
            name: cell(r, name),
            old_version: cell(r, old),
            new_version: cell(r, new),
            repo: cell(r, repo),
        })
        .filter(|u| !u.name.is_empty() && !u.new_version.is_empty())
        .collect()
}

/// Names in `/var/lib/zypp/AutoInstalled` (comment lines start with `#`).
pub fn parse_auto_installed(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

fn auto_installed() -> Option<Vec<String>> {
    std::fs::read_to_string(AUTO_INSTALLED)
        .ok()
        .map(|t| parse_auto_installed(&t))
}

/// Standard output of `zypper --non-interactive --no-refresh ARGS`, or
/// `None`, without running zypper, when the metadata of an enabled
/// repository is not on disk: zypper would download it.
fn zypper(args: &[&str]) -> Result<Option<String>, Error> {
    if super::missing_metadata(PkgManager::Zypper).is_some() {
        return Ok(None);
    }
    let mut full = vec!["--non-interactive", "--no-refresh"];
    full.extend_from_slice(args);
    // zypper exits 104 for "nothing found" and 1xx for informational
    // states; the table (or its absence) is the answer either way.
    let (_, out) = super::run("zypper", &full)?;
    Ok(Some(out))
}

/// The `Repository` line of `zypper info NAME`: the repository zypper
/// offers the package from now, or `@System` when no repository has it.
/// Pure.
pub fn parse_info_repository(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (key, value) = l.split_once(':')?;
        (key.trim() == "Repository" && !value.trim().is_empty()).then(|| value.trim().to_string())
    })
}

/// What `why` prints as "from" for a zypper package, and whether no
/// repository offers it. `repository` is `parse_info_repository`'s answer;
/// `missing` is `missing_metadata`'s. Never empty. Pure.
pub fn origin_text(repository: Option<&str>, missing: Option<&str>) -> (String, Option<bool>) {
    match (repository, missing) {
        (_, Some(why)) => (format!("unknown: {why}"), None),
        (Some("@System"), None) => (
            "not in any configured repository (installed from a file, or dropped)".into(),
            Some(true),
        ),
        (Some(repo), None) => (
            format!(
                "repository '{repo}' (where zypper offers it now; the zypp history does not record where the installed version came from)"
            ),
            Some(false),
        ),
        (None, None) => (
            "unknown: `zypper info` did not name a repository".into(),
            None,
        ),
    }
}

/// Where zypper offers installed package `name` from, for `why` (one
/// `zypper --no-refresh info`; not part of `fact`, which is asked once per
/// package of a dependency chain). The name must be a checked package name.
pub fn origin(name: &str) -> Result<(String, Option<bool>), Error> {
    super::check_name(name)?;
    let info = zypper(&["info", "--", name])?;
    Ok(origin_text(
        info.as_deref().and_then(parse_info_repository).as_deref(),
        super::missing_metadata(PkgManager::Zypper).as_deref(),
    ))
}

/// Reads the rpm database with `rpm` and the repository metadata already on
/// disk with `zypper --no-refresh`.
pub struct ZypperBackend;

impl NativeBackend for ZypperBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Zypper
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        let out = super::output("rpm", &["-qa", "--qf", LIST_FORMAT])?;
        Ok(parse_installed(&out))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        let auto = auto_installed().ok_or_else(|| {
            Error::unavailable(format!(
                "cannot read {AUTO_INSTALLED}: zypper's record of automatically installed packages"
            ))
        })?;
        Ok(self
            .installed()?
            .into_iter()
            .map(|p| p.name)
            .filter(|n| !auto.contains(n))
            .collect())
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        let mut f = super::typed_fact(self.manager(), name)?;
        if let (Some(f), Some(auto)) = (f.as_mut(), auto_installed()) {
            f.reason = if auto.contains(&f.name) {
                Reason::Dependency
            } else {
                Reason::Explicit
            };
        }
        Ok(f)
    }
    fn required_by(&self, name: &str) -> Result<Vec<String>, Error> {
        super::typed_required_by(self.manager(), name)
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let out = zypper(&["search", "-s", "--match-exact", "--", name])?;
        Ok(out.and_then(|o| parse_candidate(&o, name)))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let out = zypper(&["search", "--", query])?;
        Ok(out.map(|o| parse_search(&o)).unwrap_or_default())
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let out = super::output_if_found("rpm", &["-qf", "--qf", "%{NAME}\\n", "--", path])?;
        Ok(out.and_then(|o| o.lines().next().map(|l| l.trim().to_string())))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("rpm", &["-ql", "--", name])?;
        out.map(|o| {
            o.lines()
                .filter(|l| l.starts_with('/'))
                .map(String::from)
                .collect()
        })
        .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        let out = zypper(&["packages", "--unneeded"])?;
        Ok(out.map(|o| parse_orphans(&o)).unwrap_or_default())
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        let out = zypper(&["list-updates"])?;
        Ok(out.map(|o| parse_updates(&o)).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_list() {
        let v = parse_installed(
            "zlib\t1.3.1-2.1\tx86_64\t200000\t1700000000\tCompression library\ngpg-pubkey\t1-1\t(none)\t0\t1\tkey\nbash\t5.2-1\tx86_64\t7\t1\tThe GNU shell\nshort\tline\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "bash");
        assert_eq!(v[1].size_bytes, 200000);
    }

    #[test]
    fn search_tables() {
        let detailed = "Loading repository data...\nReading installed packages...\n\nS  | Name | Type    | Version   | Arch   | Repository\n---+------+---------+-----------+--------+------------------\ni+ | tree | package | 2.2.1-1.1 | x86_64 | (System Packages)\nv  | tree | package | 2.2.1-1.1 | x86_64 | repo-oss\n   | tree | srcpackage | 2.2.1-1.1 | noarch | repo-src\n";
        let c = parse_candidate(detailed, "tree").unwrap();
        assert_eq!(c.version, "2.2.1-1.1");
        assert_eq!(c.repo, "repo-oss");
        let plain = "S | Name     | Summary                 | Type\n--+----------+-------------------------+--------\n  | tree     | File listing as a tree  | package\ni | treeline | Something               | package\n";
        let s = parse_search(plain);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].description, "File listing as a tree");
        assert!(parse_search("No matching items found.\n").is_empty());
    }

    /// Z5: `why` printed an empty `from` line on zypper.
    #[test]
    fn the_origin_is_never_empty() {
        // `zypper --no-refresh info glibc` on Tumbleweed, 2026-11-19
        let info = "Loading repository data...\nReading installed packages...\n\n\nInformation for package glibc:\n------------------------------\nRepository     : openSUSE-Tumbleweed-Oss\nName           : glibc\nVersion        : 2.44-4.1\nArch           : x86_64\nVendor         : openSUSE\nInstalled Size : 7.2 MiB\nInstalled      : Yes\nStatus         : out-of-date (version 2.44-3.1 installed)\n";
        let repo = parse_info_repository(info);
        assert_eq!(repo.as_deref(), Some("openSUSE-Tumbleweed-Oss"));
        let (origin, foreign) = origin_text(repo.as_deref(), None);
        assert!(
            origin.starts_with("repository 'openSUSE-Tumbleweed-Oss' (where zypper offers it now"),
            "{origin}"
        );
        assert_eq!(foreign, Some(false));
        assert_eq!(
            origin_text(Some("@System"), None),
            (
                "not in any configured repository (installed from a file, or dropped)".into(),
                Some(true)
            )
        );
        // not found, or no table at all
        assert_eq!(
            parse_info_repository("package 'nosuch-xyz' not found.\nNo matching items found.\n"),
            None
        );
        let (origin, foreign) = origin_text(None, None);
        assert!(!origin.is_empty() && foreign.is_none(), "{origin}");
        // no metadata on disk: zypper was not asked, and the line says why
        let why = super::super::no_metadata_message(PkgManager::Zypper, &[]);
        let (origin, foreign) = origin_text(None, Some(&why));
        assert_eq!(
            origin,
            "unknown: no repository metadata on disk: run `zypper refresh` (yapacc does not refresh on a read command)"
        );
        assert_eq!(foreign, None);
    }

    #[test]
    fn updates_orphans_and_auto() {
        let up = "S | Repository | Name | Current Version | Available Version | Arch\n--+------------+------+-----------------+-------------------+-------\nv | repo-oss   | curl | 8.9.0-1.1       | 8.10.0-1.1        | x86_64\n";
        let u = parse_updates(up);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].old_version, "8.9.0-1.1");
        assert_eq!(u[0].new_version, "8.10.0-1.1");
        assert_eq!(u[0].repo, "repo-oss");
        assert!(parse_updates("No updates found.\n").is_empty());
        let or = "S | Repository | Name   | Version | Arch\n--+------------+--------+---------+-------\ni | repo-oss   | libfoo | 1-1.1   | x86_64\n";
        assert_eq!(parse_orphans(or), vec!["libfoo"]);
        assert_eq!(
            parse_auto_installed("# comment\nlibfoo\n\nlibbar\n"),
            vec!["libfoo", "libbar"]
        );
    }
}
