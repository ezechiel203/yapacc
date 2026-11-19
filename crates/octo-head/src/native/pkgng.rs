// yapacc — native backend: pkg (FreeBSD, DragonFly BSD)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Reads through `pkg query` / `pkg rquery`, whose output format is chosen
//! by the caller, so nothing human-oriented is parsed except the two dry
//! runs (`pkg autoremove -n`, `pkg upgrade -n`).
//!
//! State of this provider: exercised on virtual machines of FreeBSD 14.5
//! and 15.1, not on real hardware; `yapacc doctor` says so there.

use super::{Candidate, Installed, NativeBackend, NativeFact, Reason, Update};
use crate::error::Error;
use crate::platform::PkgManager;

const LIST_FORMAT: &str = "%n\t%v\t%q\t%sb\t%c";
const FACT_FORMAT: &str = "%n\t%v\t%a\t%t\t%m\t%R\t%c";
const CANDIDATE_FORMAT: &str = "%n\t%v\t%R\t%c";

/// Parse `pkg query '%n\t%v\t%q\t%sb\t%c'`.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    let mut v: Vec<Installed> = text
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.splitn(5, '\t').collect();
            (p.len() == 5 && !p[0].is_empty()).then(|| Installed {
                name: p[0].into(),
                version: p[1].into(),
                arch: p[2].into(),
                size_bytes: p[3].parse().unwrap_or(0),
                description: p[4].trim().into(),
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn parse_lines(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Parse `pkg query '%n\t%v\t%a\t%t\t%m\t%R\t%c' NAME`.
pub fn parse_fact(line: &str, required_by: Vec<String>) -> Option<NativeFact> {
    let p: Vec<&str> = line.trim_end_matches('\n').splitn(7, '\t').collect();
    if p.len() < 7 || p[0].is_empty() {
        return None;
    }
    Some(NativeFact {
        manager: "pkg".into(),
        name: p[0].into(),
        version: p[1].into(),
        description: p[6].trim().into(),
        reason: match p[2] {
            "1" => Reason::Dependency,
            "0" => Reason::Explicit,
            _ => Reason::Unknown,
        },
        required_by,
        origin: match p[5] {
            "" | "unknown-repository" => {
                "unknown (built from ports, or installed from a file)".into()
            }
            r => format!("repository '{r}'"),
        },
        foreign: None,
        essential: false,
        base: String::new(),
        installed: p[3]
            .parse::<u64>()
            .map(crate::util::fmt_utc)
            .unwrap_or_default(),
        verified: String::new(),
        packager: p[4].into(),
    })
}

/// Parse `pkg rquery '%n\t%v\t%R\t%c'`.
pub fn parse_candidates(text: &str) -> Vec<Candidate> {
    text.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.splitn(4, '\t').collect();
            (p.len() == 4 && !p[0].is_empty()).then(|| Candidate {
                name: p[0].into(),
                version: p[1].into(),
                repo: p[2].into(),
                description: p[3].trim().into(),
            })
        })
        .collect()
}

/// Make `s` match literally inside a `pkg -g` (fnmatch) pattern: a search
/// for `*` or `[` must not act as a wildcard or a malformed class. Pure.
pub fn glob_literal(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' | '?' | '[' | ']' | '\\' => format!("[{c}]"),
            c => c.to_string(),
        })
        .collect()
}

/// Split `name-1.2.3_4,1` into name and version.
pub fn split_pkgver(s: &str) -> Option<(&str, &str)> {
    let i = s.rfind('-')?;
    (i > 0 && i + 1 < s.len()).then(|| (&s[..i], &s[i + 1..]))
}

/// Parse `pkg which -q PATH`: `name-version`.
pub fn parse_owner(text: &str) -> Option<String> {
    split_pkgver(text.lines().next()?.trim()).map(|(n, _)| n.to_string())
}

/// Parse `pkg autoremove -n`: indented `name: version` lines.
pub fn parse_orphans(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with('\t') || l.starts_with("    "))
        .filter_map(|l| l.trim().split_once(": ").map(|(n, _)| n.to_string()))
        .filter(|n| !n.is_empty() && !n.contains(' '))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Parse `pkg upgrade -n`: indented `name: old -> new [repo]` lines.
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let (name, rest) = l.trim().split_once(": ")?;
            let (old, new) = rest.split_once(" -> ")?;
            let mut new_parts = new.split_whitespace();
            Some(Update {
                name: name.into(),
                old_version: old.trim().into(),
                new_version: new_parts.next()?.into(),
                repo: new_parts
                    .next()
                    .map(|r| r.trim_matches(['[', ']']).to_string())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// What one `pkg rquery -U %n NAME` says about `NAME` being in the
/// repository catalogue on disk: `Some(true)` when it printed a name,
/// `Some(false)` when it failed and printed nothing at all (pkg-rquery(8)
/// exits non-zero when no package matched; src/rquery.c), `None` when it
/// cannot be told (it printed an error: no catalogue, no access, a broken
/// repository). Pure.
pub fn catalogue_answer(code: i32, stdout: &str, stderr: &str) -> Option<bool> {
    if stdout.lines().any(|l| !l.trim().is_empty()) {
        return Some(true);
    }
    (code != 0 && stderr.trim().is_empty()).then_some(false)
}

/// Whether package `name` is in the repository catalogue on disk. `-U`
/// keeps pkg from updating the catalogue, so nothing is contacted. The name
/// must be a checked package name.
pub fn in_catalogue(name: &str) -> Option<bool> {
    super::check_name(name).ok()?;
    let out = std::process::Command::new("pkg")
        .args(["rquery", "-U", "%n", "--", name])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    catalogue_answer(
        out.status.code().unwrap_or(-1),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Reads the pkg database by running `pkg`.
pub struct PkgBackend;

impl NativeBackend for PkgBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Pkg
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        Ok(parse_installed(&super::output(
            "pkg",
            &["query", LIST_FORMAT],
        )?))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        let (_, out) = super::run("pkg", &["query", "-e", "%a = 0", "%n"])?;
        Ok(parse_lines(&out))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::check_name(name)?;
        let Some(line) = super::output_if_found("pkg", &["query", FACT_FORMAT, name])? else {
            return Ok(None);
        };
        let (_, rdeps) = super::run("pkg", &["query", "%rn", name])?;
        Ok(line
            .lines()
            .next()
            .and_then(|l| parse_fact(l, parse_lines(&rdeps))))
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let (_, out) = super::run("pkg", &["rquery", "-U", CANDIDATE_FORMAT, name])?;
        Ok(parse_candidates(&out).into_iter().next())
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let pattern = format!("*{}*", glob_literal(query));
        let (_, out) = super::run("pkg", &["rquery", "-U", "-g", CANDIDATE_FORMAT, &pattern])?;
        Ok(parse_candidates(&out))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let out = super::output_if_found("pkg", &["which", "-q", path])?;
        Ok(out.and_then(|o| parse_owner(&o)))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("pkg", &["query", "%Fp", name])?;
        out.map(|o| {
            o.lines()
                .filter(|l| l.starts_with('/'))
                .map(String::from)
                .collect()
        })
        .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        let (_, out) = super::run("pkg", &["autoremove", "-n"])?;
        Ok(parse_orphans(&out))
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        let (_, out) = super::run("pkg", &["upgrade", "-n", "-U"])?;
        Ok(parse_updates(&out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E2: `install repo nosuchpackage-xyz --yes` ran `pkg install -y` on
    /// FreeBSD after a simulation that had printed pkg's error. Written
    /// from pkg-rquery(8) and src/rquery.c; not run on FreeBSD.
    #[test]
    fn the_catalogue_answers_known_unknown_or_cannot_tell() {
        assert_eq!(catalogue_answer(0, "tree\n", ""), Some(true));
        // several repositories offer it
        assert_eq!(catalogue_answer(0, "tree\ntree\n", ""), Some(true));
        // no match: a failure status and not a word
        assert_eq!(catalogue_answer(1, "", ""), Some(false));
        assert_eq!(catalogue_answer(1, "\n", ""), Some(false));
        // an error message: the catalogue could not be asked
        assert_eq!(
            catalogue_answer(1, "", "pkg: No active remote repositories configured.\n"),
            None
        );
        assert_eq!(
            catalogue_answer(
                77,
                "",
                "pkg: Insufficient privileges to query the package database\n"
            ),
            None
        );
        // success without a line (an older pkg): cannot tell, never "unknown"
        assert_eq!(catalogue_answer(0, "", ""), None);
        // a name that is not one is not asked about
        assert_eq!(in_catalogue("a;b"), None);
    }

    #[test]
    fn installed_and_fact() {
        let v = parse_installed(
            "pkg\t1.21.3\tFreeBSD:14:amd64\t38000000\tPackage manager\ncurl\t8.10.1\tFreeBSD:14:amd64\t4000000\tCommand line tool for transferring data\nbad line\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "curl");
        assert_eq!(v[1].size_bytes, 38000000);
        let f = parse_fact(
            "curl\t8.10.1\t0\t1714000000\tsunpoet@FreeBSD.org\tFreeBSD\tCommand line tool\n",
            vec!["git".into()],
        )
        .unwrap();
        assert_eq!(f.reason, Reason::Explicit);
        assert_eq!(f.origin, "repository 'FreeBSD'");
        assert_eq!(f.installed, "2024-04-24 23:06 UTC");
        assert_eq!(f.required_by, vec!["git"]);
        assert!(parse_fact("short\tline\n", vec![]).is_none());
    }

    #[test]
    fn glob_metacharacters_are_literal() {
        assert_eq!(glob_literal("bonnie++"), "bonnie++");
        assert_eq!(glob_literal("a*b?"), "a[*]b[?]");
        assert_eq!(glob_literal("["), "[[]");
        assert_eq!(glob_literal("]"), "[]]");
        assert_eq!(glob_literal("a\\b"), "a[\\]b");
    }

    #[test]
    fn candidates_owner_and_dry_runs() {
        let c = parse_candidates("tree\t2.1.1\tFreeBSD\tDisplay a tree-view of directories\n");
        assert_eq!(c[0].repo, "FreeBSD");
        assert_eq!(parse_owner("bash-5.2.37\n").as_deref(), Some("bash"));
        let o = parse_orphans(
            "Checking integrity... done\nDeinstallation has been requested for the following 2 packages:\n\nInstalled packages to be REMOVED:\n\tlibfoo: 1.0\n\tlibbar: 2.0_1\n\nNumber of packages to be removed: 2\n",
        );
        assert_eq!(o, vec!["libbar", "libfoo"]);
        let u = parse_updates(
            "Installed packages to be UPGRADED:\n\tcurl: 8.9.0 -> 8.10.1 [FreeBSD]\n\tgit: 2.46.0 -> 2.47.0\n\nNumber of packages to be upgraded: 2\n",
        );
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].repo, "FreeBSD");
        assert_eq!(u[1].new_version, "2.47.0");
    }
}
