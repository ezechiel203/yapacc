// yapacc — native backend: xbps (Void Linux)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Everything is read by running `xbps-query`; pending updates come from
//! `xbps-install -un` (a dry run that changes nothing).

use super::{Candidate, Installed, NativeBackend, NativeFact, Reason, Update};
use crate::error::Error;
use crate::platform::PkgManager;

/// Split `name-1.2.3_4` into name and version (the version never holds a `-`).
pub fn split_pkgver(s: &str) -> Option<(&str, &str)> {
    let i = s.rfind('-')?;
    (i > 0 && i + 1 < s.len()).then(|| (&s[..i], &s[i + 1..]))
}

/// Parse `xbps-query -l`: `ii name-ver_rev   description`.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    let mut v: Vec<Installed> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(2, ' ');
            let state = it.next()?;
            let rest = it.next()?.trim_start();
            if state != "ii" {
                return None;
            }
            let (pkgver, desc) = match rest.split_once(char::is_whitespace) {
                Some((p, d)) => (p, d.trim()),
                None => (rest, ""),
            };
            let (name, version) = split_pkgver(pkgver)?;
            Some(Installed {
                name: name.into(),
                version: version.into(),
                arch: String::new(),
                size_bytes: 0,
                description: desc.into(),
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Parse a list of `name-ver_rev` lines (`-m`, `-O`, `-X`) into names.
pub fn parse_names(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| split_pkgver(l.trim()).map(|(n, _)| n.to_string()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The `key: value` properties printed by `xbps-query PKG` / `xbps-query -R PKG`.
pub fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        l.strip_prefix(key)
            .and_then(|r| r.strip_prefix(':'))
            .map(str::trim)
    })
}

pub fn parse_fact(text: &str, required_by: Vec<String>) -> Option<NativeFact> {
    let pkgver = property(text, "pkgver")?;
    let (name, version) = split_pkgver(pkgver)?;
    let repo = property(text, "repository").unwrap_or("");
    Some(NativeFact {
        manager: "xbps".into(),
        name: name.into(),
        version: version.into(),
        description: property(text, "short_desc").unwrap_or("").into(),
        reason: if property(text, "automatic-install") == Some("yes") {
            Reason::Dependency
        } else {
            Reason::Explicit
        },
        required_by,
        origin: if repo.is_empty() {
            "unknown (xbps recorded no repository)".into()
        } else {
            repo.into()
        },
        foreign: None,
        essential: false,
        base: String::new(),
        installed: property(text, "install-date").unwrap_or("").into(),
        verified: if repo.starts_with("http") {
            "repository signature (xbps verifies signed repository data)".into()
        } else {
            String::new()
        },
        packager: property(text, "maintainer").unwrap_or("").into(),
    })
}

pub fn parse_candidate(text: &str) -> Option<Candidate> {
    let (name, version) = split_pkgver(property(text, "pkgver")?)?;
    Some(Candidate {
        name: name.into(),
        version: version.into(),
        repo: property(text, "repository").unwrap_or("").into(),
        description: property(text, "short_desc").unwrap_or("").into(),
    })
}

/// Parse `xbps-query -Rs`: `[*] name-ver_rev   description`.
pub fn parse_search(text: &str) -> Vec<Candidate> {
    text.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix('[')?.split_once(']')?.1.trim_start();
            let (pkgver, desc) = match rest.split_once(char::is_whitespace) {
                Some((p, d)) => (p, d.trim()),
                None => (rest, ""),
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

/// Parse `xbps-query -o PATH`: `name-ver_rev: /path (regular file)`.
pub fn parse_owner(text: &str) -> Option<String> {
    let pkgver = text.lines().next()?.split_once(": ")?.0;
    split_pkgver(pkgver).map(|(n, _)| n.to_string())
}

/// Parse `xbps-query -f`: one path per line, links as `/a -> /b`.
pub fn parse_files(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split(" -> ").next().unwrap_or(l).trim().to_string())
        .filter(|l| l.starts_with('/'))
        .collect()
}

/// Parse `xbps-install -un`: `name-newver action arch repository size size`.
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 2 || f[1] != "update" {
                return None;
            }
            let (name, new) = split_pkgver(f[0])?;
            Some(Update {
                name: name.into(),
                old_version: String::new(),
                new_version: new.into(),
                repo: f.get(3).copied().unwrap_or("").into(),
            })
        })
        .collect()
}

/// Reads the xbps database by running `xbps-query`.
pub struct XbpsBackend;

impl NativeBackend for XbpsBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Xbps
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        Ok(parse_installed(&super::output("xbps-query", &["-l"])?))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        Ok(parse_names(&super::output_or_empty("xbps-query", &["-m"])?))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::check_name(name)?;
        let Some(props) = super::output_if_found("xbps-query", &["--", name])? else {
            return Ok(None);
        };
        let (_, rdeps) = super::run("xbps-query", &["-X", name])?;
        Ok(parse_fact(&props, parse_names(&rdeps)))
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("xbps-query", &["-R", "--", name])?;
        Ok(out.and_then(|o| parse_candidate(&o)))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let (_, out) = super::run("xbps-query", &["-Rs", query])?;
        Ok(parse_search(&out))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let out = super::output_if_found("xbps-query", &["-o", path])?;
        Ok(out.and_then(|o| parse_owner(&o)))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("xbps-query", &["-f", name])?;
        out.map(|o| parse_files(&o))
            .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        Ok(parse_names(&super::output_or_empty("xbps-query", &["-O"])?))
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        let (_, out) = super::run("xbps-install", &["-un"])?;
        Ok(parse_updates(&out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_and_names() {
        let v = parse_installed(
            "ii xbps-0.59.2_3          XBPS package system utilities\nii base-files-0.145_1     Void Linux base system files\nuu half-1_1  unpacked only\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "base-files");
        assert_eq!(v[0].version, "0.145_1");
        assert_eq!(v[1].description, "XBPS package system utilities");
        assert_eq!(
            parse_names("tree-2.1.1_1\nbase-system-0.114_2\n\n"),
            vec!["base-system", "tree"]
        );
    }

    #[test]
    fn fact_and_candidate() {
        let props = "architecture: x86_64\nautomatic-install: yes\ninstall-date: 2026-11-01 10:00 UTC\ninstalled_size: 100KB\nmaintainer: Someone <a@b.c>\npkgname: libfoo\npkgver: libfoo-1.2_3\nrepository: https://repo-default.voidlinux.org/current\nshort_desc: A foo library\n";
        let f = parse_fact(props, vec!["bar".into()]).unwrap();
        assert_eq!(f.name, "libfoo");
        assert_eq!(f.version, "1.2_3");
        assert_eq!(f.reason, Reason::Dependency);
        assert_eq!(f.required_by, vec!["bar"]);
        assert!(f.origin.starts_with("https://"));
        let explicit = parse_fact("pkgver: tree-2.1.1_1\n", vec![]).unwrap();
        assert_eq!(explicit.reason, Reason::Explicit);
        let c = parse_candidate(props).unwrap();
        assert_eq!(c.description, "A foo library");
        assert!(parse_fact("nothing\n", vec![]).is_none());
    }

    #[test]
    fn search_owner_files_updates() {
        let s =
            parse_search("[*] tree-2.1.1_1      Recursive directory lister\n[-] treeline-1_1  x\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "tree");
        assert_eq!(
            parse_owner("bash-5.2.37_1: /usr/bin/bash (regular file)\n").as_deref(),
            Some("bash")
        );
        assert_eq!(
            parse_files("/usr/bin/bash\n/usr/bin/sh -> /usr/bin/bash\n"),
            vec!["/usr/bin/bash", "/usr/bin/sh"]
        );
        let u = parse_updates(
            "curl-8.10.0_1 update x86_64 https://repo-default.voidlinux.org/current 500 200\nnewdep-1_1 install x86_64 https://r 1 1\n",
        );
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].name, "curl");
        assert_eq!(u[0].new_version, "8.10.0_1");
    }
}
