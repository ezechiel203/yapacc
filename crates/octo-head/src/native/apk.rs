// yapacc — native backend: apk (Alpine and derivatives)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Installed state is read straight from apk's own text database
//! (`/lib/apk/db/installed`) and its world file (`/etc/apk/world`): both
//! are stable, documented formats shared by apk-tools 2 and 3, so no
//! human-oriented output has to be parsed for them. Repository questions
//! (`candidate`, `search`, `updates`) run `apk` itself.

use super::{Candidate, Installed, NativeBackend, NativeFact, Reason, Update};
use crate::error::Error;
use crate::platform::PkgManager;
use std::collections::BTreeSet;

const DB: &str = "/lib/apk/db/installed";
const WORLD: &str = "/etc/apk/world";

/// One record of the installed database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub size_bytes: u64,
    pub description: String,
    pub origin: String,
    pub maintainer: String,
    pub depends: Vec<String>,
    pub provides: Vec<String>,
    pub files: Vec<String>,
}

/// Strip a version constraint or repository pin from a dependency token:
/// `so:libc.musl-x86_64.so.1`, `busybox>=1.36`, `foo@testing`, `cmd:sh=1.2`.
pub fn dep_name(token: &str) -> &str {
    let end = token.find(['<', '>', '=', '~', '@']).unwrap_or(token.len());
    &token[..end]
}

/// Parse `/lib/apk/db/installed`.
pub fn parse_db(text: &str) -> Vec<Record> {
    let mut out = Vec::new();
    let mut cur = Record::default();
    let mut dir = String::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !cur.name.is_empty() {
                out.push(std::mem::take(&mut cur));
            } else {
                cur = Record::default();
            }
            dir.clear();
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key {
            "P" => cur.name = value.into(),
            "V" => cur.version = value.into(),
            "A" => cur.arch = value.into(),
            "I" => cur.size_bytes = value.parse().unwrap_or(0),
            "T" => cur.description = value.into(),
            "o" => cur.origin = value.into(),
            "m" => cur.maintainer = value.into(),
            "D" => cur.depends = value.split_whitespace().map(String::from).collect(),
            "p" => cur.provides = value.split_whitespace().map(String::from).collect(),
            "F" => dir = value.into(),
            "R" => cur.files.push(if dir.is_empty() {
                format!("/{value}")
            } else {
                format!("/{dir}/{value}")
            }),
            _ => {}
        }
    }
    out
}

pub fn parse_installed(text: &str) -> Vec<Installed> {
    let mut v: Vec<Installed> = parse_db(text)
        .into_iter()
        .map(|r| Installed {
            name: r.name,
            version: r.version,
            arch: r.arch,
            size_bytes: r.size_bytes,
            description: r.description,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Parse `/etc/apk/world`: one requested package per line, possibly with a
/// constraint or a repository pin.
pub fn parse_explicit(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| dep_name(l.trim()).to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('!'))
        .collect()
}

/// Installed packages with a dependency that `name` satisfies.
pub fn required_by(records: &[Record], name: &str) -> Vec<String> {
    let Some(target) = records.iter().find(|r| r.name == name) else {
        return vec![];
    };
    let mut offers: BTreeSet<&str> = target.provides.iter().map(|p| dep_name(p)).collect();
    offers.insert(name);
    let mut v: Vec<String> = records
        .iter()
        .filter(|r| r.name != name)
        .filter(|r| {
            r.depends
                .iter()
                .any(|d| !d.starts_with('!') && offers.contains(dep_name(d)))
        })
        .map(|r| r.name.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Split `name-1.2.3-r4` into name and version.
pub fn split_pkgver(s: &str) -> Option<(&str, &str)> {
    let rel = s.rfind('-')?;
    let ver = s[..rel].rfind('-')?;
    let release = &s[rel + 1..];
    (release.starts_with('r') && ver > 0).then(|| (&s[..ver], &s[ver + 1..]))
}

/// Parse `apk search -v`: `name-ver-rN - description`.
pub fn parse_search(text: &str) -> Vec<Candidate> {
    text.lines()
        .filter_map(|l| {
            let (pkgver, desc) = match l.split_once(" - ") {
                Some((p, d)) => (p.trim(), d.trim()),
                None => (l.trim(), ""),
            };
            let (name, version) = split_pkgver(pkgver)?;
            Some(Candidate {
                name: name.into(),
                version: version.into(),
                repo: String::new(),
                description: desc.into(),
            })
        })
        .collect()
}

pub fn parse_candidate(text: &str, name: &str) -> Option<Candidate> {
    parse_search(text).into_iter().find(|c| c.name == name)
}

/// Parse `apk version -l '<'`: `name-old-rN < new-rM` after a header line.
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" < ")?;
            let (name, old) = split_pkgver(left.trim())?;
            Some(Update {
                name: name.into(),
                old_version: old.into(),
                new_version: right.split_whitespace().next()?.into(),
                repo: String::new(),
            })
        })
        .collect()
}

pub fn owner(records: &[Record], path: &str) -> Option<String> {
    records
        .iter()
        .find(|r| r.files.iter().any(|f| f == path))
        .map(|r| r.name.clone())
}

pub fn fact(records: &[Record], world: &[String], name: &str) -> Option<NativeFact> {
    let r = records.iter().find(|r| r.name == name)?;
    Some(NativeFact {
        manager: "apk".into(),
        name: r.name.clone(),
        version: r.version.clone(),
        description: r.description.clone(),
        reason: if world.iter().any(|w| w == name) {
            Reason::Explicit
        } else {
            Reason::Dependency
        },
        required_by: required_by(records, name),
        origin: if r.origin.is_empty() || r.origin == r.name {
            "unknown — apk does not record which repository a package came from".into()
        } else {
            format!(
                "built from '{}' (apk does not record the repository)",
                r.origin
            )
        },
        foreign: None,
        essential: false,
        base: String::new(),
        installed: String::new(),
        verified: "repository index signature (apk verifies the signed APKINDEX)".into(),
        packager: r.maintainer.clone(),
    })
}

fn read(path: &str) -> Result<String, Error> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::unavailable(format!("cannot read {path}: {e}")))
}

fn records() -> Result<Vec<Record>, Error> {
    Ok(parse_db(&read(DB)?))
}

/// Reads apk's database files, and runs `apk` for repository questions.
pub struct ApkBackend;

impl NativeBackend for ApkBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Apk
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        Ok(parse_installed(&read(DB)?))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        Ok(parse_explicit(&read(WORLD)?))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::check_name(name)?;
        let world = parse_explicit(&std::fs::read_to_string(WORLD).unwrap_or_default());
        Ok(fact(&records()?, &world, name))
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let (_, out) = super::run("apk", &["search", "-v", "--exact", "--", name])?;
        Ok(parse_candidate(&out, name))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let (_, out) = super::run("apk", &["search", "-v", "-d", "--", query])?;
        Ok(parse_search(&out))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        Ok(owner(&records()?, path))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        records()?
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.files)
            .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        // apk keeps only what /etc/apk/world requires: a package nothing
        // needs is removed by apk itself, so there is never an orphan list.
        Ok(vec![])
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        let (_, out) = super::run("apk", &["version", "-l", "<"])?;
        Ok(parse_updates(&out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "C:Q1abc=\nP:musl\nV:1.2.5-r9\nA:x86_64\nS:411323\nI:654336\nT:the musl c library (libc) implementation\nU:https://musl.libc.org/\nL:MIT\no:musl\nm:Natanael Copa <ncopa@alpinelinux.org>\nt:1739467586\np:so:libc.musl-x86_64.so.1=1\nF:lib\nR:ld-musl-x86_64.so.1\nZ:Q1xyz=\n\nC:Q1def=\nP:busybox\nV:1.37.0-r12\nA:x86_64\nI:798720\nT:Size optimized toolbox of many common UNIX utilities\no:busybox\nD:so:libc.musl-x86_64.so.1\np:cmd:busybox=1.37.0-r12\nF:bin\nR:busybox\nF:etc\nR:securetty\n\nP:ssl_client\nV:1.37.0-r12\nA:x86_64\nI:20480\nT:External ssl_client for busybox wget\no:busybox\nD:musl>=1.2 !uclibc\n";

    #[test]
    fn database_records() {
        let recs = parse_db(SAMPLE);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].name, "musl");
        assert_eq!(recs[0].size_bytes, 654336);
        assert_eq!(recs[0].files, vec!["/lib/ld-musl-x86_64.so.1"]);
        assert_eq!(recs[1].files, vec!["/bin/busybox", "/etc/securetty"]);
        let listed = parse_installed(SAMPLE);
        assert_eq!(listed[0].name, "busybox");
        assert_eq!(listed[1].version, "1.2.5-r9");
    }

    #[test]
    fn reverse_dependencies_go_through_provides_and_constraints() {
        let recs = parse_db(SAMPLE);
        assert_eq!(required_by(&recs, "musl"), vec!["busybox", "ssl_client"]);
        assert!(required_by(&recs, "busybox").is_empty());
        assert_eq!(owner(&recs, "/bin/busybox").as_deref(), Some("busybox"));
        assert_eq!(owner(&recs, "/nope"), None);
    }

    #[test]
    fn world_and_fact() {
        let world = parse_explicit("alpine-base\nbusybox>=1.36\nfoo@testing\n\n");
        assert_eq!(world, vec!["alpine-base", "busybox", "foo"]);
        let recs = parse_db(SAMPLE);
        let f = fact(&recs, &world, "busybox").unwrap();
        assert_eq!(f.reason, Reason::Explicit);
        let m = fact(&recs, &world, "musl").unwrap();
        assert_eq!(m.reason, Reason::Dependency);
        assert_eq!(m.required_by.len(), 2);
        assert!(fact(&recs, &world, "nope").is_none());
    }

    #[test]
    fn search_and_updates() {
        assert_eq!(
            split_pkgver("font-dejavu-2.37-r5"),
            Some(("font-dejavu", "2.37-r5"))
        );
        assert_eq!(split_pkgver("garbage"), None);
        let s = parse_search(
            "tree-2.2.1-r0 - A recursive directory indented listing of files\ntree-doc-2.2.1-r0 - docs\n",
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "tree");
        assert_eq!(s[0].version, "2.2.1-r0");
        assert_eq!(
            parse_candidate("tree-doc-2.2.1-r0 - d\ntree-2.2.1-r0 - x\n", "tree")
                .unwrap()
                .description,
            "x"
        );
        let u = parse_updates(
            "Installed:                                Available:\nmusl-1.2.4-r0                           < 1.2.5-r9 \n",
        );
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].name, "musl");
        assert_eq!(u[0].old_version, "1.2.4-r0");
        assert_eq!(u[0].new_version, "1.2.5-r9");
    }
}
