// yapacc — native backend: dnf
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! dnf and rpm: pure parsers for the output of the commands the
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

/// Parse the `rpm -qa` list: 6 fields separated by TAB. The pseudo-package
/// `gpg-pubkey` (an imported key) is skipped.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(6, '\t').collect();
            (f.len() == 6 && !f[0].is_empty() && f[0] != "gpg-pubkey").then(|| Installed {
                name: f[0].to_string(),
                version: f[1].to_string(),
                arch: f[2].to_string(),
                size_bytes: f[3].trim().parse().unwrap_or(0),
                description: f[5].trim().to_string(),
            })
        })
        .collect()
}

/// Parse `dnf -q repoquery --userinstalled`: one name per line.
pub fn parse_explicit(text: &str) -> Vec<String> {
    names(text)
}

/// Parse the `dnf -q repoquery -- NAME` row: 6 fields separated by TAB
/// (name, evr, arch, repoid, installsize, summary). The first row wins.
pub fn parse_candidate(text: &str) -> Option<Candidate> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.splitn(6, '\t').collect();
        (f.len() == 6 && !f[0].is_empty()).then(|| Candidate {
            name: f[0].to_string(),
            version: f[1].to_string(),
            repo: f[3].to_string(),
            description: f[5].trim().to_string(),
        })
    })
}

/// Parse the `dnf -q repoquery -- '*QUERY*'` rows: 4 fields separated by
/// TAB (name, evr, repoid, summary).
pub fn parse_search(text: &str) -> Vec<Candidate> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(4, '\t').collect();
            (f.len() == 4 && !f[0].is_empty()).then(|| Candidate {
                name: f[0].to_string(),
                version: f[1].to_string(),
                repo: f[2].to_string(),
                description: f[3].trim().to_string(),
            })
        })
        .collect()
}

/// Parse `rpm -qf --qf '%{NAME}\n' -- PATH`: the name alone on a line.
pub fn parse_owner(text: &str) -> Option<String> {
    names(text).into_iter().next()
}

/// Parse `rpm -ql -- NAME`: one path per line.
pub fn parse_files(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with('/'))
        .map(str::to_string)
        .collect()
}

/// Parse `dnf -q repoquery --unneeded`: one name per line.
pub fn parse_orphans(text: &str) -> Vec<String> {
    names(text)
}

/// The installed version of each update, from the `rpm -qa` list
/// (`NAME<TAB>EVR` per line). dnf's `repoquery --upgrades` does not print
/// it. Of a package that keeps several versions installed (kernels) the
/// newest is the one an upgrade replaces. An update whose name is not in
/// the list keeps an empty old version. Pure.
pub fn with_installed_versions(mut updates: Vec<Update>, rpm_qa: &str) -> Vec<Update> {
    use std::collections::HashMap;
    let mut installed: HashMap<&str, &str> = HashMap::new();
    for line in rpm_qa.lines() {
        let Some((name, evr)) = line.split_once('\t') else {
            continue;
        };
        let evr = evr.trim();
        // rpm writes an absent epoch as `(none):` in %{EVR} on old versions
        let evr = evr.strip_prefix("(none):").unwrap_or(evr);
        installed
            .entry(name)
            .and_modify(|known| {
                if crate::vercmp::rpm_evr_cmp(evr, known) == std::cmp::Ordering::Greater {
                    *known = evr;
                }
            })
            .or_insert(evr);
    }
    for u in &mut updates {
        if u.old_version.is_empty()
            && let Some(v) = installed.get(u.name.as_str())
        {
            u.old_version = v.to_string();
        }
    }
    updates
}

/// Parse the `dnf -q repoquery --upgrades` rows: 3 fields separated by TAB
/// (name, evr, repoid). dnf does not print the installed version: see
/// [`with_installed_versions`].
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() == 3 && !f[0].is_empty()).then(|| Update {
                name: f[0].to_string(),
                old_version: String::new(),
                new_version: f[1].to_string(),
                repo: f[2].trim().to_string(),
            })
        })
        .collect()
}

/// The `--qf` of the installed list. rpm expands `\\t` and `\\n`.
const LIST_FORMAT: &str = "%{NAME}\\t%{EVR}\\t%{ARCH}\\t%{SIZE}\\t%{INSTALLTIME}\\t%{SUMMARY}\\n";
// dnf5 does not expand escapes: its `--qf` strings hold real TAB and
// newline characters.
const NAME_FORMAT: &str = "%{name}\n";
const CANDIDATE_FORMAT: &str = "%{name}\t%{evr}\t%{arch}\t%{repoid}\t%{installsize}\t%{summary}\n";
const SEARCH_FORMAT: &str = "%{name}\t%{evr}\t%{repoid}\t%{summary}\n";
const UPDATE_FORMAT: &str = "%{name}\t%{evr}\t%{repoid}\n";

/// Reads the rpm database by running `rpm`, and the repository indexes
/// already on disk by running `dnf -q -C repoquery` (`-C`: cache only, so
/// dnf never downloads metadata).
///
/// Questions about installed packages only (`--userinstalled`,
/// `--unneeded`) add `--disablerepo=*`: with `-C` and no metadata on disk
/// dnf 4 otherwise fails on the first repository ("Cache-only enabled but
/// no cache for 'baseos'", Rocky 9). Questions about repositories are not
/// asked at all without metadata: `missing_metadata` says why.
pub struct DnfBackend;

/// Options in front of every installed-only `dnf repoquery`.
const INSTALLED_ONLY: [&str; 3] = ["-q", "-C", "--disablerepo=*"];

impl NativeBackend for DnfBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Dnf
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        let out = super::output("rpm", &["-qa", "--qf", LIST_FORMAT])?;
        Ok(parse_installed(&out))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        let mut args = INSTALLED_ONLY.to_vec();
        args.extend(["repoquery", "--userinstalled", "--qf", NAME_FORMAT]);
        Ok(parse_explicit(&super::output("dnf", &args)?))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::typed_fact(self.manager(), name)
    }
    fn required_by(&self, name: &str) -> Result<Vec<String>, Error> {
        super::typed_required_by(self.manager(), name)
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        if self.missing_metadata().is_some() {
            return Ok(None);
        }
        let args = [
            "-q",
            "-C",
            "repoquery",
            "--qf",
            CANDIDATE_FORMAT,
            "--",
            name,
        ];
        let out = super::output_if_found("dnf", &args)?;
        Ok(out.and_then(|o| parse_candidate(&o)))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        if self.missing_metadata().is_some() {
            return Ok(Vec::new());
        }
        let glob = format!("*{query}*");
        let args = ["-q", "-C", "repoquery", "--qf", SEARCH_FORMAT, "--", &glob];
        Ok(parse_search(&super::output_or_empty("dnf", &args)?))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let args = ["-qf", "--qf", "%{NAME}\\n", "--", path];
        let out = super::output_if_found("rpm", &args)?;
        Ok(out.and_then(|o| parse_owner(&o)))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("rpm", &["-ql", "--", name])?;
        out.map(|o| parse_files(&o))
            .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        let mut args = INSTALLED_ONLY.to_vec();
        args.extend(["repoquery", "--unneeded", "--qf", NAME_FORMAT]);
        Ok(parse_orphans(&super::output("dnf", &args)?))
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        if self.missing_metadata().is_some() {
            return Ok(Vec::new());
        }
        let args = ["-q", "-C", "repoquery", "--upgrades", "--qf", UPDATE_FORMAT];
        let updates = parse_updates(&super::output_or_empty("dnf", &args)?);
        if updates.is_empty() {
            return Ok(updates);
        }
        // the installed versions, which dnf does not print
        let installed = super::output("rpm", &["-qa", "--qf", "%{NAME}\\t%{EVR}\\n"])?;
        Ok(with_installed_versions(updates, &installed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::fixture;

    #[test]
    fn installed_list_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "list.txt");
        assert_eq!(exit, 0);
        let v = parse_installed(&body);
        assert_eq!(v.len(), 149);
        assert_eq!(
            v[0],
            Installed {
                name: "libgcc".into(),
                version: "16.2.1-3.fc45".into(),
                arch: "x86_64".into(),
                size_bytes: 277_212,
                description: "GCC version 16 shared support library".into()
            }
        );
        let last = &v[148];
        assert_eq!(
            (last.name.as_str(), last.version.as_str(), last.size_bytes),
            ("tree", "2.3.1-2.fc45", 123_789)
        );
        let bash = v.iter().find(|p| p.name == "bash").unwrap();
        assert_eq!(
            (bash.version.as_str(), bash.arch.as_str(), bash.size_bytes),
            ("5.3.15-2.fc45", "x86_64", 8_865_529)
        );
        assert_eq!(bash.description, "The GNU Bourne Again shell");
        assert!(!v.iter().any(|p| p.name == "gpg-pubkey"));
        assert!(parse_installed("only\tthree\tfields\n").is_empty());
    }

    #[test]
    fn explicit_set_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "explicit.txt");
        assert_eq!(exit, 0);
        let v = parse_explicit(&body);
        assert_eq!(v.len(), 21);
        assert_eq!((v[0].as_str(), v[20].as_str()), ("bash", "zstd"));
        assert!(v.iter().any(|n| n == "dnf5"));
        assert!(parse_explicit("").is_empty());
    }

    #[test]
    fn candidate_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "info.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_candidate(&body),
            Some(Candidate {
                name: "bash".into(),
                version: "5.3.15-2.fc45".into(),
                repo: "fedora".into(),
                description: "The GNU Bourne Again shell".into()
            })
        );
        assert_eq!(parse_candidate(""), None);
        let two = "a\t1-1\tx86_64\tfedora\t10\tfirst\na\t2-1\tx86_64\tupdates\t10\tsecond\n";
        assert_eq!(parse_candidate(two).unwrap().version, "1-1");
    }

    #[test]
    fn search_results_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "search.txt");
        assert_eq!(exit, 0);
        let v = parse_search(&body);
        assert_eq!(v.len(), 462);
        assert_eq!(
            v[0],
            Candidate {
                name: "NearTree".into(),
                version: "5.1.1-21.fc45".into(),
                repo: "fedora".into(),
                description: "An API for finding nearest neighbors".into()
            }
        );
        let last = &v[461];
        assert_eq!(
            (
                last.name.as_str(),
                last.version.as_str(),
                last.description.as_str()
            ),
            ("ytree", "2.13-3.fc45", "A file manager similar to XTree")
        );
        let tree = v.iter().find(|c| c.name == "tree").unwrap();
        assert_eq!(
            (
                tree.version.as_str(),
                tree.repo.as_str(),
                tree.description.as_str()
            ),
            ("2.3.1-2.fc45", "fedora", "File system tree viewer")
        );
        assert!(parse_search("").is_empty());
    }

    #[test]
    fn owner_from_the_fedora_fixtures() {
        let (body, exit) = fixture("fedora", "owner.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_owner(&body).as_deref(), Some("coreutils"));
        let (body, exit) = fixture("fedora", "owner-missing.txt");
        assert_eq!(exit, 1);
        assert_eq!(parse_owner(&body), None);
    }

    #[test]
    fn files_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "files.txt");
        assert_eq!(exit, 0);
        let v = parse_files(&body);
        assert_eq!(v.len(), 9);
        assert_eq!(
            (v[0].as_str(), v[8].as_str()),
            ("/usr/bin/tree", "/usr/share/man/man1/tree.1.gz")
        );
        assert!(v.iter().any(|p| p == "/usr/share/licenses/tree/LICENSE"));
        assert!(parse_files("(contains no files)\n").is_empty());
    }

    #[test]
    fn orphans_from_the_fedora_fixture() {
        let (body, exit) = fixture("fedora", "orphans.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_orphans(&body), vec!["tree"]);
        assert!(parse_orphans("").is_empty());
    }

    /// D5: `--json check` had an empty `old_version` on dnf.
    #[test]
    fn updates_get_the_installed_version_from_rpm() {
        let (body, _) = fixture("fedora", "updates-synthetic.txt");
        let rpm_qa = "bash\t5.3.15-2.fc45\nglibc\t2.44-1.fc45\nkernel-core\t7.3.3-300.fc45\nkernel-core\t7.3.4-300.fc45\nkernel-core\t7.2.9-200.fc45\ngpg-pubkey\t1-1\nbroken line\n";
        let v = with_installed_versions(parse_updates(&body), rpm_qa);
        let old = |name: &str| {
            v.iter()
                .find(|u| u.name == name)
                .map(|u| u.old_version.clone())
        };
        assert_eq!(old("bash").as_deref(), Some("5.3.15-2.fc45"));
        assert_eq!(old("glibc").as_deref(), Some("2.44-1.fc45"));
        // several versions installed: the newest is the one replaced
        assert_eq!(old("kernel-core").as_deref(), Some("7.3.4-300.fc45"));
        // the new version and the repository are untouched
        assert_eq!(v[0].new_version, "5.3.20-1.fc45");
        assert_eq!(v[0].repo, "updates");
        // a name rpm does not list keeps an empty old version
        let lone = with_installed_versions(parse_updates("x\t2-1\tupdates\n"), rpm_qa);
        assert_eq!(lone[0].old_version, "");
        // an epoch rpm writes as absent
        let epoch = with_installed_versions(
            parse_updates("mokutil\t2:0.7.2-6.el9_9\tbaseos\n"),
            "mokutil\t2:0.6.0-4.el9\nzlib\t(none):1.2.11-40.el9\n",
        );
        assert_eq!(epoch[0].old_version, "2:0.6.0-4.el9");
    }

    #[test]
    fn updates_from_the_fedora_fixtures() {
        let (body, exit) = fixture("fedora", "updates.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_updates(&body), vec![]);
        let (body, exit) = fixture("fedora", "updates-synthetic.txt");
        assert_eq!(exit, 0);
        let v = parse_updates(&body);
        assert_eq!(v.len(), 3);
        assert_eq!(
            v[0],
            Update {
                name: "bash".into(),
                old_version: String::new(),
                new_version: "5.3.20-1.fc45".into(),
                repo: "updates".into()
            }
        );
        assert_eq!(
            v[2],
            Update {
                name: "kernel-core".into(),
                old_version: String::new(),
                new_version: "1:6.20.3-200.fc45".into(),
                repo: "updates-testing".into()
            }
        );
        let glibc = v.iter().find(|u| u.name == "glibc").unwrap();
        assert_eq!(
            (glibc.new_version.as_str(), glibc.repo.as_str()),
            ("2.44-3.fc45", "updates")
        );
    }
}
