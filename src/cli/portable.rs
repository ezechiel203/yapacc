// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Read verbs answered through `NativeBackend`, for systems whose native
//! package manager is not pacman. What is printed is built by pure functions over the data, so it
//! is tested without apt or dnf.

use super::{InspectVerb, Verb, human_size, print_json, table};
use crate::error::{Error, Result};
use crate::native::{self, Candidate, Installed, NativeBackend, Update};
use crate::platform::PkgManager;
use serde::Serialize;
use std::collections::HashSet;

/// What a routed verb does once the backend is known.
type Answer<'a> = Box<dyn FnOnce(&dyn NativeBackend) -> Result<()> + 'a>;

/// Answer `verb` through the native backend when this system's package
/// manager is known and is not pacman. `None`: not handled here.
pub(super) fn route(verb: &Verb, json: bool) -> Option<Result<()>> {
    let answer: Answer = match verb {
        Verb::List {
            foreign,
            explicit,
            limit,
            include_external,
        } => Box::new(move |b| {
            if *foreign || *include_external {
                return Err(unsupported("list --foreign and --include-external", b));
            }
            list(b, *explicit, *limit, json)
        }),
        Verb::Search {
            query,
            aur,
            all,
            limit,
            category,
            kind,
            ptype,
        } => Box::new(move |b| {
            if *aur || *all || category.is_some() || kind.is_some() || ptype.is_some() {
                return Err(unsupported("the search filters and --aur/--all", b));
            }
            search(b, query, *limit, json)
        }),
        Verb::Check => Box::new(move |b| check(b, json)),
        Verb::Info { pkg } => Box::new(move |_| crate::why::run(pkg, json)),
        Verb::Inspect {
            cmd: InspectVerb::Owns { path },
        } => Box::new(move |b| owns(b, path, json)),
        Verb::Inspect {
            cmd: InspectVerb::Files { pkg },
        } => Box::new(move |b| lines(b.files(pkg)?, NO_FILES, json)),
        Verb::Inspect {
            cmd: InspectVerb::Orphans,
        } => Box::new(move |b| orphans(b, json)),
        _ => return None,
    };
    let (present, native) = crate::platform::native_manager();
    let Some(manager) = native else {
        // nothing usable on PATH: if a database says which manager this system
        // has, say that its tool is missing instead of falling through to pacman
        if present.is_empty()
            && let Some((m, tool)) = crate::platform::missing_native_tool()
        {
            return Some(Err(Error::unavailable(
                crate::platform::missing_tool_message(m, tool),
            )));
        }
        return None;
    };
    if manager == PkgManager::Pacman {
        return None;
    }
    Some(native::backend(manager).and_then(|b| answer(b.as_ref())))
}

fn unsupported(what: &str, b: &dyn NativeBackend) -> Error {
    Error::unavailable(format!(
        "{what} exist only for pacman; this system's package manager is {}",
        b.manager().name()
    ))
}

// ------------------------------------------------------------------ list

/// One row of `list`: an installed package, and whether the user asked for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListRow {
    #[serde(flatten)]
    pub package: Installed,
    pub explicit: bool,
}

/// Rows sorted by name; `only_explicit` keeps the explicit ones; at most `limit`.
pub fn list_rows(
    installed: Vec<Installed>,
    explicit: &[String],
    only_explicit: bool,
    limit: Option<usize>,
) -> Vec<ListRow> {
    let explicit: HashSet<&str> = explicit.iter().map(String::as_str).collect();
    let mut rows: Vec<ListRow> = installed
        .into_iter()
        .map(|package| ListRow {
            explicit: explicit.contains(package.name.as_str()),
            package,
        })
        .filter(|row| row.explicit || !only_explicit)
        .collect();
    rows.sort_by(|a, b| a.package.name.cmp(&b.package.name));
    rows.truncate(limit.unwrap_or(usize::MAX));
    rows
}

/// What `inspect orphans` prints when the manager lists none.
pub const NO_ORPHANS: &str = "(no orphan: every package installed as a dependency is still needed)";
/// What `inspect files` prints for a package that installs no file (a
/// meta-package).
pub const NO_FILES: &str = "(the package installs no file)";
/// What `list` prints when no row is left.
pub const NO_PACKAGES: &str = "(no installed package to list)";
pub const NO_EXPLICIT_PACKAGES: &str = "(no package is marked as explicitly installed)";

/// The table of `list`. `width` is the terminal's; None prints whole lines.
pub fn render_list(rows: &[ListRow], width: Option<usize>) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let p = &row.package;
            vec![
                p.name.clone(),
                p.version.clone(),
                match p.size_bytes {
                    0 => "-".to_string(),
                    n => human_size(n as i64),
                },
                p.description.clone(),
            ]
        })
        .collect();
    table(&super::read::package_columns(false), &cells, width)
}

fn list(
    b: &dyn NativeBackend,
    only_explicit: bool,
    limit: Option<usize>,
    json: bool,
) -> Result<()> {
    let rows = list_rows(b.installed()?, &b.explicit()?, only_explicit, limit);
    if json {
        print_json(&rows);
    } else if rows.is_empty() {
        // an empty answer is said in words, not left as a lone header
        println!(
            "{}",
            if only_explicit {
                NO_EXPLICIT_PACKAGES
            } else {
                NO_PACKAGES
            }
        );
    } else {
        print!("{}", render_list(&rows, super::terminal_width()));
    }
    Ok(())
}

// ---------------------------------------------------------------- search

/// Best matches first: the exact name, then names that contain the query,
/// then the rest; by name inside each group. Identical rows are merged.
pub fn search_rows(mut found: Vec<Candidate>, query: &str, limit: usize) -> Vec<Candidate> {
    let q = query.to_lowercase();
    let rank = |c: &Candidate| -> u8 {
        let name = c.name.to_lowercase();
        if name == q {
            0
        } else if name.contains(&q) {
            1
        } else {
            2
        }
    };
    found.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.repo.cmp(&b.repo))
    });
    found.dedup();
    found.truncate(limit);
    found
}

pub fn render_search(found: &[Candidate]) -> String {
    if found.is_empty() {
        return "(no package matches in the repository indexes on disk)\n".to_string();
    }
    let mut s = String::new();
    for c in found {
        s.push_str(&c.name);
        if !c.version.is_empty() {
            s.push_str(&format!(" {}", c.version));
        }
        if !c.repo.is_empty() {
            s.push_str(&format!(" [{}]", c.repo));
        }
        s.push_str(&format!("  {}\n", c.description));
    }
    s
}

/// What a repository question prints when it found nothing: that nothing
/// matches, or, when no repository metadata is on disk, that sentence
/// instead, as a note (the question could not be asked). Pure.
pub fn nothing_or_note(nothing: &str, missing_metadata: Option<&str>) -> String {
    match missing_metadata {
        Some(why) => format!("note: {why}\n"),
        None => nothing.to_string(),
    }
}

/// With `--json` the answer on standard output stays the (empty) list, and
/// the reason goes to standard error.
fn note_missing_metadata(b: &dyn NativeBackend) {
    if let Some(why) = b.missing_metadata() {
        eprintln!("note: {why}");
    }
}

fn search(b: &dyn NativeBackend, query: &str, limit: usize, json: bool) -> Result<()> {
    let found = search_rows(b.search(query)?, query, limit);
    if json {
        print_json(&found);
        if found.is_empty() {
            note_missing_metadata(b);
        }
    } else if found.is_empty() {
        print!(
            "{}",
            nothing_or_note(&render_search(&found), b.missing_metadata().as_deref())
        );
    } else {
        print!("{}", render_search(&found));
    }
    Ok(())
}

// ----------------------------------------------------------------- check

pub fn render_updates(updates: &[Update]) -> String {
    if updates.is_empty() {
        return "(no pending update in the repository indexes on disk)\n".to_string();
    }
    let mut s = String::new();
    for u in updates {
        s.push_str(&u.name);
        if !u.old_version.is_empty() {
            s.push_str(&format!(" {}", u.old_version));
        }
        s.push_str(&format!(" -> {}", u.new_version));
        if !u.repo.is_empty() {
            s.push_str(&format!(" [{}]", u.repo));
        }
        s.push('\n');
    }
    s
}

fn check(b: &dyn NativeBackend, json: bool) -> Result<()> {
    let updates = b.updates()?;
    if json {
        print_json(&updates);
        if updates.is_empty() {
            note_missing_metadata(b);
        }
    } else if updates.is_empty() {
        // "no pending update" is not said when the question was not asked
        print!(
            "{}",
            nothing_or_note(&render_updates(&updates), b.missing_metadata().as_deref())
        );
    } else {
        print!("{}", render_updates(&updates));
    }
    Ok(())
}

/// Whether manager `m` needs its repository metadata to tell which
/// installed packages nothing needs: zypper computes that with its
/// repositories loaded (and would download them); the others answer from
/// the installed packages alone. Pure.
pub fn orphans_need_metadata(m: PkgManager) -> bool {
    m == PkgManager::Zypper
}

/// The reason the orphan list of this backend cannot be worked out now, if
/// it cannot.
pub fn orphans_unknown(b: &dyn NativeBackend) -> Option<String> {
    orphans_need_metadata(b.manager())
        .then(|| b.missing_metadata())
        .flatten()
}

fn orphans(b: &dyn NativeBackend, json: bool) -> Result<()> {
    let items = b.orphans()?;
    match orphans_unknown(b) {
        Some(why) if json => {
            print_json(&items);
            eprintln!("note: {why}");
            Ok(())
        }
        Some(why) => {
            println!("note: {why}");
            Ok(())
        }
        None => lines(items, NO_ORPHANS, json),
    }
}

// ------------------------------------- inspect owns, files, orphans

pub fn render_owner(path: &str, owner: &str) -> String {
    format!("{path} is owned by {owner}\n")
}

pub fn owner_json(path: &str, owner: &str) -> serde_json::Value {
    serde_json::json!({ "path": path, "owner": owner })
}

/// One item per line.
pub fn render_lines(items: &[String]) -> String {
    items.iter().map(|item| format!("{item}\n")).collect()
}

/// [`render_lines`], or the one line `nothing` when there is no item: an
/// empty answer is said in words (as `check` and `search` say theirs), not
/// printed as nothing. Pure.
pub fn render_lines_or(items: &[String], nothing: &str) -> String {
    if items.is_empty() {
        format!("{nothing}\n")
    } else {
        render_lines(items)
    }
}

/// The canonical path `path` resolves to, when it is a symlink that points
/// somewhere else.
fn symlink_target_to_try(path: &str) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    let real = real.to_str()?.to_string();
    (real != path).then_some(real)
}

pub(super) fn owns(b: &dyn NativeBackend, path: &str, json: bool) -> Result<()> {
    let mut owner = b.owner_of(path)?;
    if owner.is_none()
        && let Some(target) = symlink_target_to_try(path)
    {
        // a symlink no package lists (Alpine's /usr/bin/env -> /bin/busybox):
        // the native tool answers for the file it points to
        owner = b.owner_of(&target)?;
    }
    let Some(owner) = owner else {
        return Err(Error::not_found(format!("no package owns {path}")));
    };
    if json {
        print_json(&owner_json(path, &owner));
    } else {
        print!("{}", render_owner(path, &owner));
    }
    Ok(())
}

/// The items one per line, or `nothing` when there is none; with `--json`
/// the array alone, empty or not.
pub(super) fn lines(items: Vec<String>, nothing: &str, json: bool) -> Result<()> {
    if json {
        print_json(&items);
    } else {
        print!("{}", render_lines_or(&items, nothing));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn symlink_target_is_found_only_for_a_symlink() {
        let dir = std::env::temp_dir().join(format!("yapacc-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real");
        std::fs::write(&real, b"x").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let want = std::fs::canonicalize(&real)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(symlink_target_to_try(link.to_str().unwrap()), Some(want));
        assert_eq!(symlink_target_to_try(real.to_str().unwrap()), None);
        assert_eq!(symlink_target_to_try("/nonexistent/yapacc/x"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    fn installed(name: &str, version: &str, size_bytes: u64, description: &str) -> Installed {
        Installed {
            name: name.into(),
            version: version.into(),
            arch: "amd64".into(),
            size_bytes,
            description: description.into(),
        }
    }

    fn candidate(name: &str, version: &str, repo: &str, description: &str) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            repo: repo.into(),
            description: description.into(),
        }
    }

    #[test]
    fn list_rows_are_sorted_marked_filtered_and_limited() {
        let all = vec![
            installed("zlib1g", "1:1.3-1", 177_152, "compression library"),
            installed("bash", "5.2.37-2", 7_368_704, "GNU Bourne Again SHell"),
            installed("apt", "3.0.4", 0, "commandline package manager"),
        ];
        let explicit = vec!["bash".to_string(), "not-installed".to_string()];
        let rows = list_rows(all.clone(), &explicit, false, None);
        let names: Vec<&str> = rows.iter().map(|r| r.package.name.as_str()).collect();
        assert_eq!(names, ["apt", "bash", "zlib1g"]);
        let marks: Vec<bool> = rows.iter().map(|r| r.explicit).collect();
        assert_eq!(marks, [false, true, false]);
        assert_eq!(list_rows(all.clone(), &explicit, false, Some(2)).len(), 2);
        let only = list_rows(all, &explicit, true, None);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].package.name, "bash");
    }

    #[test]
    fn list_text_and_json() {
        let rows = list_rows(
            vec![
                installed("bash", "5.2.37-2", 7_368_704, "GNU Bourne Again SHell"),
                installed("apt", "3.0.4", 0, "commandline package manager"),
            ],
            &["bash".to_string()],
            false,
            None,
        );
        // columns are as wide as their longest cell; nothing is cut
        let text = render_list(&rows, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "NAME  VERSION       SIZE  DESCRIPTION",
                "apt   3.0.4            -  commandline package manager",
                "bash  5.2.37-2  7.03 MiB  GNU Bourne Again SHell",
            ]
        );
        // on a terminal only the description gives way, at a word
        let narrow = render_list(&rows, Some(40));
        let lines: Vec<&str> = narrow.lines().collect();
        assert_eq!(lines[1], "apt   3.0.4            -  commandline…");
        assert_eq!(lines[2], "bash  5.2.37-2  7.03 MiB  GNU Bourne…");
        assert!(lines.iter().all(|l| l.chars().count() <= 40), "{narrow}");
        // no room for a description worth reading: the row has none, and
        // the size is still whole
        let tiny = render_list(&rows, Some(30));
        assert_eq!(tiny.lines().nth(2), Some("bash  5.2.37-2  7.03 MiB"));
        assert_eq!(
            serde_json::to_string(&rows[1]).unwrap(),
            r#"{"name":"bash","version":"5.2.37-2","arch":"amd64","size_bytes":7368704,"description":"GNU Bourne Again SHell","explicit":true}"#
        );
    }

    #[test]
    fn search_rows_rank_exact_name_first_and_merge_duplicates() {
        let found = vec![
            candidate("ytree", "2.13-3", "fedora", "A file manager"),
            candidate("gcc", "16-1", "fedora", "builds a syntax tree"),
            candidate("tree", "2.3.1-2", "fedora", "File system tree viewer"),
            candidate("tree", "2.3.1-2", "fedora", "File system tree viewer"),
            candidate("ostree", "2026.1", "fedora", "Git for operating systems"),
        ];
        let rows = search_rows(found.clone(), "Tree", 10);
        let names: Vec<&str> = rows.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["tree", "ostree", "ytree", "gcc"]);
        assert_eq!(search_rows(found, "tree", 2).len(), 2);
    }

    #[test]
    fn search_text() {
        let found = vec![
            candidate("tree", "2.3.1-2.fc45", "fedora", "File system tree viewer"),
            candidate("tree", "", "", "displays an indented directory tree"),
        ];
        assert_eq!(
            render_search(&found),
            "tree 2.3.1-2.fc45 [fedora]  File system tree viewer\n\
             tree  displays an indented directory tree\n"
        );
        assert_eq!(
            render_search(&[]),
            "(no package matches in the repository indexes on disk)\n"
        );
    }

    #[test]
    fn check_text() {
        let update = |name: &str, old: &str, new: &str, repo: &str| Update {
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
            repo: repo.into(),
        };
        let updates = vec![
            update("zlib1g", "1:1.3-1", "1:1.3-2", "Debian-Security:13/stable"),
            update("bash", "", "5.3.20-1.fc45", "updates"),
            update("openssl", "3.6.5-1", "4.1.0-1", ""),
        ];
        assert_eq!(
            render_updates(&updates),
            "zlib1g 1:1.3-1 -> 1:1.3-2 [Debian-Security:13/stable]\n\
             bash -> 5.3.20-1.fc45 [updates]\n\
             openssl 3.6.5-1 -> 4.1.0-1\n"
        );
        assert_eq!(
            render_updates(&[]),
            "(no pending update in the repository indexes on disk)\n"
        );
    }

    /// D2 and Z1: without repository metadata on disk `check` printed "no
    /// pending update" and `search` "no package matches".
    #[test]
    fn an_empty_answer_without_metadata_says_that_instead() {
        let why = crate::native::no_metadata_message(PkgManager::Dnf, &[]);
        assert_eq!(
            nothing_or_note(&render_updates(&[]), Some(&why)),
            "note: no repository metadata on disk: run `dnf makecache` (yapacc does not refresh on a read command)\n"
        );
        assert_eq!(
            nothing_or_note(&render_updates(&[]), None),
            "(no pending update in the repository indexes on disk)\n"
        );
        assert_eq!(
            nothing_or_note(&render_search(&[]), None),
            "(no package matches in the repository indexes on disk)\n"
        );
        // only zypper needs its repositories to list orphans
        assert!(orphans_need_metadata(PkgManager::Zypper));
        for m in [PkgManager::Dnf, PkgManager::Apt, PkgManager::Apk] {
            assert!(!orphans_need_metadata(m), "{m:?}");
        }
    }

    #[test]
    fn owner_text_and_json() {
        assert_eq!(
            render_owner("/usr/bin/env", "coreutils"),
            "/usr/bin/env is owned by coreutils\n"
        );
        assert_eq!(
            owner_json("/usr/bin/env", "coreutils").to_string(),
            r#"{"owner":"coreutils","path":"/usr/bin/env"}"#
        );
    }

    #[test]
    fn files_and_orphans_are_one_item_per_line() {
        let items = vec![
            "/usr/bin/tree".to_string(),
            "/usr/share/man/man1/tree.1.gz".to_string(),
        ];
        assert_eq!(
            render_lines(&items),
            "/usr/bin/tree\n/usr/share/man/man1/tree.1.gz\n"
        );
        assert_eq!(render_lines(&[]), "");
        // Debian 13 with no orphan: `inspect orphans` printed nothing
        assert_eq!(
            render_lines_or(&[], NO_ORPHANS),
            "(no orphan: every package installed as a dependency is still needed)\n"
        );
        assert_eq!(
            render_lines_or(&[], NO_FILES),
            "(the package installs no file)\n"
        );
        assert_eq!(render_lines_or(&items, NO_ORPHANS), render_lines(&items));
        assert_eq!(
            serde_json::to_string(&items).unwrap(),
            r#"["/usr/bin/tree","/usr/share/man/man1/tree.1.gz"]"#
        );
    }
}
