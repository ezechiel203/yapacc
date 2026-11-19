// yapacc — native backend: apt
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! apt and dpkg: pure parsers for the output of the commands the
//! backend runs. Text in, value out, no I/O.

use super::{Candidate, Installed, NativeBackend, NativeFact, Update};
use crate::error::Error;
use crate::platform::PkgManager;

/// One package name per line. Empty lines and lines that hold more than
/// one word are skipped.
fn names(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .map(str::to_string)
        .collect()
}

/// Parse the `dpkg-query -W` list: 7 fields separated by TAB; only rows
/// whose third field starts with `ii` are installed.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(7, '\t').collect();
            (f.len() == 7 && !f[0].is_empty() && f[2].starts_with("ii")).then(|| Installed {
                name: f[0].to_string(),
                version: f[1].to_string(),
                arch: String::new(),
                size_bytes: f[3].trim().parse::<u64>().unwrap_or(0) * 1024,
                description: f[6].trim().to_string(),
            })
        })
        .collect()
}

/// Parse `apt-mark showmanual`: one name per line. apt writes a package of
/// a foreign architecture as `libpng16-16t64:i386`; the architecture is
/// dropped, since every other list of this backend (`${Package}` of
/// dpkg-query) names packages without it.
pub fn parse_explicit(text: &str) -> Vec<String> {
    let mut v: Vec<String> = names(text)
        .into_iter()
        .map(|n| match n.split_once(':') {
            Some((name, _arch)) => name.to_string(),
            None => n,
        })
        .collect();
    v.dedup();
    v
}

/// Parse `apt-cache show --no-all-versions -- NAME`: the first stanza.
pub fn parse_candidate(text: &str) -> Option<Candidate> {
    let stanza = text.split("\n\n").find(|s| !s.trim().is_empty())?;
    let field = |key: &str| {
        stanza.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k == key).then(|| v.trim().to_string())
        })
    };
    Some(Candidate {
        name: field("Package")?,
        version: field("Version").unwrap_or_default(),
        repo: String::new(),
        description: field("Description")
            .or_else(|| field("Description-en"))
            .unwrap_or_default(),
    })
}

/// Parse `apt-cache search -- QUERY`: `name - description`.
pub fn parse_search(text: &str) -> Vec<Candidate> {
    text.lines()
        .filter_map(|l| {
            let (name, description) = l.split_once(" - ")?;
            (!name.is_empty() && !name.contains(' ')).then(|| Candidate {
                name: name.to_string(),
                version: String::new(),
                repo: String::new(),
                description: description.trim().to_string(),
            })
        })
        .collect()
}

/// Parse `dpkg-query -S -- PATH`: `NAME: PATH` or `NAME:ARCH: PATH`.
pub fn parse_owner(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (left, path) = l.rsplit_once(": ")?;
        let name = left.split([':', ',']).next()?.trim();
        (path.starts_with('/') && !name.is_empty() && !name.contains(' ')).then(|| name.to_string())
    })
}

/// Parse `dpkg-query -L -- NAME`: one path per line; `/.` is dropped.
pub fn parse_files(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with('/') && !l.ends_with('/') && *l != "/.")
        .map(str::to_string)
        .collect()
}

/// Parse `apt-get -s autoremove`: lines `Remv NAME [VERSION]`.
pub fn parse_orphans(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Remv ")?.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Parse `apt-get -s upgrade`: lines `Inst NAME [OLD] (NEW ORIGIN, ORIGIN
/// [ARCH])`. `[OLD]` is absent for a package that is not installed yet.
///
/// `repo` holds every origin apt names, separated by `, `: a security fix
/// is usually offered by the updates pocket too, and apt lists that one
/// first. Ubuntu 24.04, 2026-11-18:
///
/// ```text
/// Inst perl [5.38.2-3.2ubuntu0.6] (5.38.2-3.2ubuntu0.7 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64])
/// ```
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let (head, tail) = l.strip_prefix("Inst ")?.split_once('(')?;
            let mut head = head.split_whitespace();
            let name = head.next()?;
            let old = head.next().unwrap_or("").trim_matches(['[', ']']);
            let inside = tail.split(')').next()?;
            // the architecture, `[amd64]`, ends the list of origins
            let inside = inside.split(" [").next().unwrap_or(inside);
            let (new, origins) = match inside.trim().split_once(char::is_whitespace) {
                Some((new, origins)) => (new, origins),
                None => (inside.trim(), ""),
            };
            if new.is_empty() {
                return None;
            }
            let origins: Vec<&str> = origins
                .split(',')
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .collect();
            Some(Update {
                name: name.to_string(),
                old_version: old.to_string(),
                new_version: new.to_string(),
                repo: origins.join(", "),
            })
        })
        .collect()
}

/// The `-f` format of the installed list. dpkg-query expands `\\t` and `\\n`.
const LIST_FORMAT: &str = "${Package}\\t${Version}\\t${db:Status-Abbrev}\\t${Installed-Size}\\t${Essential}\\t${Priority}\\t${binary:Summary}\\n";

/// Reads dpkg's and apt's databases by running `dpkg-query`, `apt-mark`,
/// `apt-cache` and `apt-get -s` (simulation only).
pub struct AptBackend;

impl NativeBackend for AptBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Apt
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        let out = super::output("dpkg-query", &["-W", "-f", LIST_FORMAT])?;
        Ok(parse_installed(&out))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        Ok(parse_explicit(&super::output("apt-mark", &["showmanual"])?))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::typed_fact(self.manager(), name)
    }
    fn required_by(&self, name: &str) -> Result<Vec<String>, Error> {
        super::typed_required_by(self.manager(), name)
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let args = ["show", "--no-all-versions", "--", name];
        let out = super::output_if_found("apt-cache", &args)?;
        Ok(out.and_then(|o| parse_candidate(&o)))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let out = super::output("apt-cache", &["search", "--", query])?;
        Ok(parse_search(&out))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let out = super::output_if_found("dpkg-query", &["-S", "--", path])?;
        Ok(out.and_then(|o| parse_owner(&o)))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("dpkg-query", &["-L", "--", name])?;
        out.map(|o| parse_files(&o))
            .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        let out = super::output("apt-get", &["-s", "autoremove"])?;
        Ok(parse_orphans(&out))
    }
    // `--with-new-pkgs` matches what `yapacc upgrade` runs: a plain `upgrade`
    // keeps back a kernel meta-package whose new image is a new package, so
    // the pending list (and the briefing's kernel check) would miss it.
    fn updates(&self) -> Result<Vec<Update>, Error> {
        Ok(parse_updates(&super::output(
            "apt-get",
            &["-s", "upgrade", "--with-new-pkgs"],
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::fixture;

    #[test]
    fn installed_list_from_the_debian_fixture() {
        let (body, exit) = fixture("debian", "list.txt");
        assert_eq!(exit, 0);
        let v = parse_installed(&body);
        assert_eq!(v.len(), 79);
        assert_eq!(
            v[0],
            Installed {
                name: "apt".into(),
                version: "3.0.4".into(),
                arch: String::new(),
                size_bytes: 4_588_544,
                description: "commandline package manager".into()
            }
        );
        let last = &v[78];
        assert_eq!(
            (last.name.as_str(), last.version.as_str(), last.size_bytes),
            ("zlib1g", "1:1.3.dfsg+really1.3.1-1+b1", 177_152)
        );
        let bash = v.iter().find(|p| p.name == "bash").unwrap();
        assert_eq!(
            (bash.version.as_str(), bash.size_bytes),
            ("5.2.37-2+b10", 7_368_704)
        );
        assert_eq!(bash.description, "GNU Bourne Again SHell");
    }

    #[test]
    fn installed_list_from_the_ubuntu_fixture() {
        let (body, exit) = fixture("ubuntu", "list.txt");
        assert_eq!(exit, 0);
        let v = parse_installed(&body);
        assert_eq!(v.len(), 86);
        assert_eq!(
            v[0],
            Installed {
                name: "apt".into(),
                version: "3.3.3ubuntu0.1".into(),
                arch: String::new(),
                size_bytes: 4_744_192,
                description: "commandline package manager".into()
            }
        );
        let last = &v[85];
        assert_eq!(
            (last.name.as_str(), last.version.as_str(), last.size_bytes),
            ("zlib1g", "1:1.3.dfsg+really1.3.2-3ubuntu1.1", 174_080)
        );
        let bash = v.iter().find(|p| p.name == "bash").unwrap();
        assert_eq!(
            (bash.version.as_str(), bash.size_bytes),
            ("5.3-3ubuntu1", 2_068_480)
        );
    }

    #[test]
    fn installed_skips_rows_that_are_not_installed_or_not_rows() {
        assert!(parse_installed("curl\t8.5\trc \t500\tno\toptional\ttransfer tool\n").is_empty());
        assert!(parse_installed("only\tfour\tii \tfields\n").is_empty());
        assert!(parse_installed("").is_empty());
    }

    #[test]
    fn explicit_set_from_the_debian_and_ubuntu_fixtures() {
        let (body, exit) = fixture("debian", "explicit.txt");
        assert_eq!(exit, 0);
        assert!(parse_explicit(&body).is_empty());
        let (body, exit) = fixture("ubuntu", "explicit.txt");
        assert_eq!(exit, 0);
        let v = parse_explicit(&body);
        assert_eq!(v.len(), 85);
        assert_eq!((v[0].as_str(), v[84].as_str()), ("apt", "zlib1g"));
        assert!(v.iter().any(|n| n == "bash"));
    }

    /// debian:stable with i386 added (2026-11-19): `apt-mark showmanual`
    /// printed `libpng16-16t64:i386`, `tree`, `zlib1g`.
    #[test]
    fn explicit_names_lose_the_foreign_architecture() {
        assert_eq!(
            parse_explicit("libpng16-16t64:i386\ntree\nzlib1g\nzlib1g:i386\n"),
            ["libpng16-16t64", "tree", "zlib1g"]
        );
    }

    #[test]
    fn candidate_from_the_debian_and_ubuntu_fixtures() {
        let (body, exit) = fixture("debian", "info.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_candidate(&body),
            Some(Candidate {
                name: "bash".into(),
                version: "5.2.37-2+b10".into(),
                repo: String::new(),
                description: "GNU Bourne Again SHell".into()
            })
        );
        let (body, exit) = fixture("ubuntu", "info.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_candidate(&body),
            Some(Candidate {
                name: "bash".into(),
                version: "5.3-3ubuntu1".into(),
                repo: String::new(),
                description: "GNU Bourne Again SHell".into()
            })
        );
    }

    #[test]
    fn candidate_reads_the_older_description_field() {
        let old =
            "Package: hello\nVersion: 2.10-2\nDescription-en: example package\n A longer text.\n";
        let c = parse_candidate(old).unwrap();
        assert_eq!(
            (c.name.as_str(), c.version.as_str(), c.description.as_str()),
            ("hello", "2.10-2", "example package")
        );
        assert_eq!(parse_candidate("N: Unable to locate package nope\n"), None);
        assert_eq!(parse_candidate(""), None);
    }

    #[test]
    fn search_results_from_the_debian_fixture() {
        let (body, exit) = fixture("debian", "search.txt");
        assert_eq!(exit, 0);
        let v = parse_search(&body);
        assert_eq!(v.len(), 446);
        assert_eq!(
            v[0],
            Candidate {
                name: "libtreelayout-java".into(),
                version: String::new(),
                repo: String::new(),
                description: "Efficient and customizable TreeLayout Algorithm in Java".into()
            }
        );
        assert_eq!(
            (v[445].name.as_str(), v[445].description.as_str()),
            ("elpa-ztree", "text mode directory tree")
        );
        let tree = v.iter().find(|c| c.name == "tree").unwrap();
        assert_eq!(
            tree.description,
            "displays an indented directory tree, in color"
        );
    }

    #[test]
    fn search_results_from_the_ubuntu_fixture() {
        let (body, exit) = fixture("ubuntu", "search.txt");
        assert_eq!(exit, 0);
        let v = parse_search(&body);
        assert_eq!(v.len(), 510);
        assert_eq!(
            (v[0].name.as_str(), v[0].description.as_str()),
            (
                "device-tree-compiler",
                "Device Tree Compiler for Flat Device Trees"
            )
        );
        assert_eq!(
            (v[509].name.as_str(), v[509].description.as_str()),
            (
                "treeview",
                "Java re-implementation of Michael Eisen's TreeView"
            )
        );
        let fdt = v.iter().find(|c| c.name == "libfdt-dev").unwrap();
        assert_eq!(
            fdt.description,
            "Flat Device Trees manipulation library - development files"
        );
        assert!(parse_search("").is_empty());
    }

    #[test]
    fn owner_from_the_debian_and_ubuntu_fixtures() {
        let (body, exit) = fixture("debian", "owner.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_owner(&body).as_deref(), Some("coreutils"));
        let (body, exit) = fixture("ubuntu", "owner.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_owner(&body).as_deref(), Some("coreutils-from-uutils"));
        for family in ["debian", "ubuntu"] {
            let (body, exit) = fixture(family, "owner-missing.txt");
            assert_eq!(exit, 1);
            assert_eq!(parse_owner(&body), None);
        }
        let multiarch = "libc6:amd64: /lib/x86_64-linux-gnu/libc.so.6\n";
        assert_eq!(parse_owner(multiarch).as_deref(), Some("libc6"));
    }

    #[test]
    fn files_from_the_debian_and_ubuntu_fixtures() {
        let (body, exit) = fixture("debian", "files.txt");
        assert_eq!(exit, 0);
        let v = parse_files(&body);
        assert_eq!(v.len(), 14);
        assert_eq!(
            (v[0].as_str(), v[13].as_str()),
            ("/usr", "/usr/share/man/man1/tree.1.gz")
        );
        assert!(v.iter().any(|p| p == "/usr/bin/tree"));
        assert!(!v.iter().any(|p| p == "/."));
        let (body, exit) = fixture("ubuntu", "files.txt");
        assert_eq!(exit, 0);
        let v = parse_files(&body);
        assert_eq!(v.len(), 13);
        assert_eq!(
            (v[0].as_str(), v[12].as_str()),
            ("/usr", "/usr/share/man/man1/tree.1.gz")
        );
    }

    #[test]
    fn orphans_from_the_debian_and_ubuntu_fixtures() {
        for family in ["debian", "ubuntu"] {
            let (body, exit) = fixture(family, "orphans.txt");
            assert_eq!(exit, 0);
            assert_eq!(parse_orphans(&body), vec!["tree"]);
        }
        assert!(parse_orphans("Reading package lists...\n").is_empty());
    }

    #[test]
    fn updates_from_the_debian_fixture() {
        let (body, exit) = fixture("debian", "updates.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_updates(&body),
            vec![Update {
                name: "zlib1g".into(),
                old_version: "1:1.3.dfsg+really1.3.1-1+b1".into(),
                new_version: "1:1.3.dfsg+really1.3.1-1+deb13u1".into(),
                repo: "Debian-Security:13/stable-security".into()
            }]
        );
    }

    #[test]
    fn updates_from_the_ubuntu_fixture() {
        let (body, exit) = fixture("ubuntu", "updates.txt");
        assert_eq!(exit, 0);
        let v = parse_updates(&body);
        assert_eq!(v.len(), 4);
        assert_eq!(
            v[0],
            Update {
                name: "perl-base".into(),
                old_version: "5.42.3-1".into(),
                new_version: "5.42.3-1ubuntu0.1".into(),
                repo: "Ubuntu:26.10/stonking-updates, Ubuntu:26.10/stonking-security".into()
            }
        );
        assert_eq!(
            (
                v[3].name.as_str(),
                v[3].old_version.as_str(),
                v[3].new_version.as_str()
            ),
            ("libc-bin", "2.44-1ubuntu2", "2.44-1ubuntu2.1")
        );
        let libc6 = v.iter().find(|u| u.name == "libc6").unwrap();
        assert!(
            libc6.repo.starts_with("Ubuntu:26.10/stonking-updates"),
            "{}",
            libc6.repo
        );
        // P1: the line of Ubuntu 24.04 where the security pocket comes
        // second; the first origin alone hid 45 of 57 security updates
        let noble = "Inst perl [5.38.2-3.2ubuntu0.6] (5.38.2-3.2ubuntu0.7 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64])\n";
        assert_eq!(
            parse_updates(noble),
            vec![Update {
                name: "perl".into(),
                old_version: "5.38.2-3.2ubuntu0.6".into(),
                new_version: "5.38.2-3.2ubuntu0.7".into(),
                repo: "Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security".into()
            }]
        );
        // an `Inst` line with a trailing remark after the architecture
        let dep = "Inst libc6 [2.39-0ubuntu8.6] (2.39-0ubuntu8.7 Ubuntu:24.04/noble-security [amd64]) [libc-bin:amd64 ]\n";
        assert_eq!(parse_updates(dep)[0].repo, "Ubuntu:24.04/noble-security");
        let new_install = "Inst libfoo1 (1.2-3 Debian:13/stable [amd64])\n";
        assert_eq!(
            parse_updates(new_install),
            vec![Update {
                name: "libfoo1".into(),
                old_version: String::new(),
                new_version: "1.2-3".into(),
                repo: "Debian:13/stable".into()
            }]
        );
    }
}
