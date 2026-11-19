// yapacc — native backend: pacman
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! pacman: pure parsers for the output of the commands the
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

/// Value of the line `Key : Value` in one pacman record, or `""`.
fn field<'a>(record: &'a str, key: &str) -> &'a str {
    record
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(" : ")?;
            (!l.starts_with(' ') && k.trim() == key).then_some(v.trim())
        })
        .unwrap_or("")
}

/// `345.21 KiB` in bytes, rounded to the nearest integer; 0 when unreadable.
fn size_bytes(value: &str) -> u64 {
    let mut words = value.split_whitespace();
    let number: f64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
    let unit = match words.next() {
        Some("B") => 1.0,
        Some("KiB") => 1024.0,
        Some("MiB") => 1024.0 * 1024.0,
        Some("GiB") => 1024.0 * 1024.0 * 1024.0,
        _ => 0.0,
    };
    (number * unit).round() as u64
}

/// Parse `pacman -Qi`: records separated by blank lines.
pub fn parse_installed(text: &str) -> Vec<Installed> {
    text.split("\n\n")
        .filter(|r| !field(r, "Name").is_empty())
        .map(|r| Installed {
            name: field(r, "Name").to_string(),
            version: field(r, "Version").to_string(),
            arch: field(r, "Architecture").to_string(),
            size_bytes: size_bytes(field(r, "Installed Size")),
            description: field(r, "Description").to_string(),
        })
        .collect()
}

/// Parse `pacman -Qeq`: one name per line.
pub fn parse_explicit(text: &str) -> Vec<String> {
    names(text)
}

/// Parse `pacman -Si -- NAME`: the first record.
pub fn parse_candidate(text: &str) -> Option<Candidate> {
    text.split("\n\n")
        .find(|r| !field(r, "Name").is_empty())
        .map(|r| Candidate {
            name: field(r, "Name").to_string(),
            version: field(r, "Version").to_string(),
            repo: field(r, "Repository").to_string(),
            description: field(r, "Description").to_string(),
        })
}

/// Parse `pacman -Ss -- QUERY`: a line `repo/name version ...`, then an
/// indented line with the description.
pub fn parse_search(text: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut()
                && last.description.is_empty()
            {
                last.description = line.trim().to_string();
            }
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(id), Some(version)) = (words.next(), words.next()) else {
            continue;
        };
        let Some((repo, name)) = id.split_once('/') else {
            continue;
        };
        out.push(Candidate {
            name: name.to_string(),
            version: version.to_string(),
            repo: repo.to_string(),
            description: String::new(),
        });
    }
    out
}

/// Parse `pacman -Qo -- PATH`: `PATH is owned by NAME VERSION`.
pub fn parse_owner(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (_, rest) = l.rsplit_once(" is owned by ")?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

/// Parse `pacman -Qlq -- NAME`: one path per line, directories dropped.
pub fn parse_files(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with('/') && !l.ends_with('/') && *l != "/.")
        .map(str::to_string)
        .collect()
}

/// Parse `pacman -Qdtq`: one name per line.
pub fn parse_orphans(text: &str) -> Vec<String> {
    names(text)
}

/// Parse `pacman -Qu`: `NAME OLD -> NEW`.
pub fn parse_updates(text: &str) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 4 && w[2] == "->").then(|| Update {
                name: w[0].to_string(),
                old_version: w[1].to_string(),
                new_version: w[3].to_string(),
                repo: String::new(),
            })
        })
        .collect()
}

/// Reads pacman's databases by running `pacman`.
pub struct PacmanBackend;

impl NativeBackend for PacmanBackend {
    fn manager(&self) -> PkgManager {
        PkgManager::Pacman
    }
    fn installed(&self) -> Result<Vec<Installed>, Error> {
        Ok(parse_installed(&super::output("pacman", &["-Qi"])?))
    }
    fn explicit(&self) -> Result<Vec<String>, Error> {
        Ok(parse_explicit(&super::output_or_empty(
            "pacman",
            &["-Qeq"],
        )?))
    }
    fn fact(&self, name: &str) -> Result<Option<NativeFact>, Error> {
        super::typed_fact(self.manager(), name)
    }
    fn candidate(&self, name: &str) -> Result<Option<Candidate>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("pacman", &["-Si", "--", name])?;
        Ok(out.and_then(|o| parse_candidate(&o)))
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, Error> {
        super::check_query(query)?;
        let out = super::output_or_empty("pacman", &["-Ss", "--", query])?;
        Ok(parse_search(&out))
    }
    fn owner_of(&self, path: &str) -> Result<Option<String>, Error> {
        super::check_path(path)?;
        let out = super::output_if_found("pacman", &["-Qo", "--", path])?;
        Ok(out.and_then(|o| parse_owner(&o)))
    }
    fn files(&self, name: &str) -> Result<Vec<String>, Error> {
        super::check_name(name)?;
        let out = super::output_if_found("pacman", &["-Qlq", "--", name])?;
        out.map(|o| parse_files(&o))
            .ok_or_else(|| super::not_installed(name))
    }
    fn orphans(&self) -> Result<Vec<String>, Error> {
        Ok(parse_orphans(&super::output_or_empty(
            "pacman",
            &["-Qdtq"],
        )?))
    }
    fn updates(&self) -> Result<Vec<Update>, Error> {
        Ok(parse_updates(&super::output_or_empty("pacman", &["-Qu"])?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::fixture;

    #[test]
    fn installed_list_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "list.txt");
        assert_eq!(exit, 0);
        let v = parse_installed(&body);
        assert_eq!(v.len(), 138);
        assert_eq!(
            v[0],
            Installed {
                name: "acl".into(),
                version: "2.4.0-1".into(),
                arch: "x86_64".into(),
                size_bytes: 353_495,
                description: "Access control list utilities, libraries and headers".into()
            }
        );
        let last = &v[137];
        assert_eq!(
            (last.name.as_str(), last.version.as_str(), last.size_bytes),
            ("zstd", "1.5.7-3", 1_610_353)
        );
        let bash = v.iter().find(|p| p.name == "bash").unwrap();
        assert_eq!(
            (bash.version.as_str(), bash.size_bytes),
            ("5.3.20-1", 10_055_844)
        );
        assert_eq!(bash.description, "The GNU Bourne Again shell");
        assert!(parse_installed("error: failed to init transaction\n").is_empty());
    }

    #[test]
    fn installed_size_units() {
        assert_eq!(size_bytes("12.00 B"), 12);
        assert_eq!(size_bytes("345.21 KiB"), 353_495);
        assert_eq!(size_bytes("2.50 MiB"), 2_621_440);
        assert_eq!(size_bytes("1.00 GiB"), 1_073_741_824);
        assert_eq!(size_bytes("many"), 0);
        assert_eq!(size_bytes(""), 0);
    }

    #[test]
    fn explicit_set_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "explicit.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_explicit(&body), vec!["base"]);
        assert_eq!(
            parse_explicit("a\n\nwarning: database file is odd\nb\n"),
            vec!["a", "b"]
        );
        assert!(parse_explicit("").is_empty());
    }

    #[test]
    fn candidate_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "info.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_candidate(&body),
            Some(Candidate {
                name: "bash".into(),
                version: "5.3.20-1".into(),
                repo: "core".into(),
                description: "The GNU Bourne Again shell".into()
            })
        );
        assert_eq!(
            parse_candidate("error: package 'nope' was not found\n"),
            None
        );
    }

    #[test]
    fn search_results_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "search.txt");
        assert_eq!(exit, 0);
        let v = parse_search(&body);
        assert_eq!(v.len(), 95);
        assert_eq!(
            v[0],
            Candidate {
                name: "accessibility-inspector".into(),
                version: "26.08.3-1".into(),
                repo: "extra".into(),
                description: "Inspect your application accessibility tree".into()
            }
        );
        let last = &v[94];
        assert_eq!(
            (
                last.name.as_str(),
                last.version.as_str(),
                last.repo.as_str()
            ),
            ("zed", "1.28.1-1", "extra")
        );
        assert_eq!(
            last.description,
            "A high-performance, multiplayer code editor from the creators of Atom and Tree-sitter"
        );
        let tree = v.iter().find(|c| c.name == "tree").unwrap();
        assert_eq!(
            (tree.version.as_str(), tree.repo.as_str()),
            ("2.3.2-1", "extra")
        );
        assert_eq!(
            tree.description,
            "A directory listing program displaying a depth indented list of files"
        );
        assert!(parse_search("").is_empty());
    }

    #[test]
    fn owner_from_the_arch_fixtures() {
        let (body, exit) = fixture("arch", "owner.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_owner(&body).as_deref(), Some("coreutils"));
        let (body, exit) = fixture("arch", "owner-missing.txt");
        assert_eq!(exit, 1);
        assert_eq!(parse_owner(&body), None);
    }

    #[test]
    fn files_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "files.txt");
        assert_eq!(exit, 0);
        assert_eq!(
            parse_files(&body),
            vec!["/usr/bin/tree", "/usr/share/man/man1/tree.1.gz"]
        );
        assert!(parse_files("").is_empty());
    }

    #[test]
    fn orphans_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "orphans.txt");
        assert_eq!(exit, 0);
        assert_eq!(parse_orphans(&body), vec!["tree"]);
        assert!(parse_orphans("").is_empty());
    }

    #[test]
    fn updates_from_the_arch_fixture() {
        let (body, exit) = fixture("arch", "updates.txt");
        assert_eq!(exit, 0);
        let v = parse_updates(&body);
        assert_eq!(v.len(), 23);
        assert_eq!(
            v[0],
            Update {
                name: "coreutils".into(),
                old_version: "9.12-2".into(),
                new_version: "9.12-3".into(),
                repo: String::new()
            }
        );
        assert_eq!(
            v[22],
            Update {
                name: "zlib".into(),
                old_version: "1:1.3.2-4".into(),
                new_version: "1:1.4-1".into(),
                repo: String::new()
            }
        );
        let openssl = v.iter().find(|u| u.name == "openssl").unwrap();
        assert_eq!(
            (openssl.old_version.as_str(), openssl.new_version.as_str()),
            ("3.6.5-1", "4.1.0-1")
        );
        assert!(parse_updates("").is_empty());
        assert!(parse_updates(":: Synchronizing package databases...\n").is_empty());
    }
}
