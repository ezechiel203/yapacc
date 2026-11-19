// yapacc — `clean`: what can be reclaimed, and how
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! A report, not an action: every line names what takes the space, how
//! much, whether removing it is safe or needs a look, and the exact command
//! that reclaims it. Read-only; writes nothing.

use crate::error::Result;
use crate::native::NativeBackend;
use crate::platform::PkgManager;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    pub what: String,
    /// `None`: could not be measured (commonly: not readable as this user)
    pub size_bytes: Option<u64>,
    pub count: usize,
    /// "safe": nothing uses it. "review": look before removing.
    pub safety: &'static str,
    /// the command that reclaims it
    pub command: String,
    pub detail: Vec<String>,
    pub note: String,
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Bytes and file count under `root` (links are not followed), and whether
/// every directory could be read.
pub fn dir_usage(root: &Path) -> (u64, usize, bool) {
    dir_usage_of(root, |_| true)
}

/// [`dir_usage`], counting only the files `keep` accepts.
pub fn dir_usage_of(root: &Path, keep: impl Fn(&Path) -> bool) -> (u64, usize, bool) {
    let mut total = 0u64;
    let mut files = 0usize;
    let mut complete = true;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            complete = false;
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                complete = false;
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() && keep(&entry.path()) {
                total += meta.len();
                files += 1;
            }
        }
    }
    (total, files, complete)
}

/// Where each manager keeps downloaded packages, and how to empty it.
pub fn cache_of(m: PkgManager, have_paccache: bool) -> (&'static [&'static str], &'static str) {
    match m {
        PkgManager::Pacman => (
            &["/var/cache/pacman/pkg"],
            if have_paccache {
                "sudo paccache -rk2   (keeps the two newest versions of each package)"
            } else {
                "sudo pacman -Sc   (keeps installed versions; `paccache` from pacman-contrib is finer)"
            },
        ),
        PkgManager::Apt => (&["/var/cache/apt/archives"], "sudo apt-get clean"),
        // dnf keeps packages and repository metadata in the same tree:
        // only the `*.rpm` files there are downloaded packages (`collect`)
        PkgManager::Dnf => (
            &["/var/cache/libdnf5", "/var/cache/dnf"],
            "sudo dnf clean packages",
        ),
        PkgManager::Zypper => (&["/var/cache/zypp/packages"], "sudo zypper clean"),
        PkgManager::Apk => (&["/var/cache/apk"], "sudo apk cache clean"),
        PkgManager::Xbps => (&["/var/cache/xbps"], "sudo xbps-remove -O"),
        PkgManager::Pkg => (&["/var/cache/pkg"], "sudo pkg clean"),
        PkgManager::Portage => (&["/var/cache/distfiles"], "sudo eclean-dist"),
        PkgManager::Nix => (&[], "nix-collect-garbage"),
    }
}

/// Is `path` a downloaded package in dnf's cache tree, as opposed to the
/// repository metadata kept beside it? Pure.
pub fn is_rpm(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "rpm")
}

/// Is `path`, a file of manager `m`'s cache directory, a downloaded
/// package? The directory holds other things on three managers: apt keeps
/// a `lock` file there (and `partial/`), apk its repository indexes
/// (`APKINDEX.630cb808.tar.gz`), dnf its repository metadata. What the
/// reclaim command of [`cache_of`] removes is the packages. Pure.
pub fn is_cached_package(m: PkgManager, path: &Path) -> bool {
    let ext = |want: &str| path.extension().is_some_and(|x| x == want);
    match m {
        PkgManager::Apt => ext("deb"),
        PkgManager::Dnf => ext("rpm"),
        PkgManager::Apk => ext("apk"),
        _ => true,
    }
}

/// What emptying the package cache of manager `m` costs. `yapacc undo`
/// reads old versions from the cache only on pacman and apt; on dnf and
/// zypper it asks the repositories, so nothing is said about undo there.
/// Pure.
pub fn cache_note(m: PkgManager) -> &'static str {
    match m {
        PkgManager::Pacman | PkgManager::Apt => {
            "safe, with one cost: `yapacc undo` looks here first for the previous version; an emptied cache means undo has to download it"
        }
        PkgManager::Dnf | PkgManager::Zypper => {
            "safe: a package is downloaded again when it is needed"
        }
        _ => {
            "safe, with one cost: the cached packages are the only local copy of the previous versions"
        }
    }
}

/// The hints are written with `sudo`; say `doas` where that is what exists.
pub fn with_available_elevation(command: &str, sudo: bool, doas: bool) -> String {
    match command.strip_prefix("sudo ") {
        Some(rest) if !sudo && doas => format!("doas {rest}"),
        _ => command.to_string(),
    }
}

/// Orphans with their sizes, largest first.
pub fn orphan_item(orphans: &[String], sizes: &HashMap<String, u64>) -> Option<Item> {
    if orphans.is_empty() {
        return None;
    }
    let mut sized: Vec<(&String, u64)> = orphans
        .iter()
        // apt names an orphan of a foreign architecture `name:i386`; the
        // sizes are listed by bare name
        .map(|n| {
            let bare = n.split_once(':').map_or(n.as_str(), |(bare, _)| bare);
            (
                n,
                sizes
                    .get(n)
                    .or_else(|| sizes.get(bare))
                    .copied()
                    .unwrap_or(0),
            )
        })
        .collect();
    sized.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let total: u64 = sized.iter().map(|(_, s)| s).sum();
    Some(Item {
        what: "packages nothing needs any more (installed as dependencies)".into(),
        size_bytes: (total > 0).then_some(total),
        count: orphans.len(),
        safety: "review",
        command: "yapacc remove orphans   (shows the list and asks)".into(),
        detail: sized
            .iter()
            .map(|(n, s)| {
                if *s > 0 {
                    format!("{n} ({})", human(*s))
                } else {
                    n.to_string()
                }
            })
            .collect(),
        note: "review: a package you use directly may have been installed as a dependency; `yapacc why NAME` tells".into(),
    })
}

/// Main flatpak runtimes no installed application declares. `apps` is the
/// `runtime` column of `flatpak list --app`; `runtimes` are
/// `application/arch/branch` of `flatpak list --runtime`.
pub fn unused_runtimes(apps: &[String], runtimes: &[String]) -> Vec<String> {
    runtimes
        .iter()
        .filter(|r| {
            let id = r.split('/').next().unwrap_or("");
            // only whole runtimes: extensions follow the runtime they extend
            (id.ends_with(".Platform") || id.ends_with(".Sdk")) && !apps.contains(r)
        })
        // an Sdk is kept when its Platform twin is used (build tools need it)
        .filter(|r| {
            let twin = r.replacen(".Sdk/", ".Platform/", 1);
            twin == **r || !apps.contains(&twin)
        })
        .cloned()
        .collect()
}

/// One listing of flatpak. `scope` comes from `why::flatpak_read_scope`:
/// flatpak is never asked about an installation that does not exist, since
/// asking creates it.
fn flatpak_column(args: &[&str], scope: &[&str]) -> Vec<String> {
    Command::new("flatpak")
        .args(args)
        .args(scope)
        .env("LC_ALL", "C")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.split('\t').map(str::trim).collect::<Vec<_>>().join("/"))
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn home_caches() -> Vec<PathBuf> {
    let Some(cache) = crate::config::xdg_base("XDG_CACHE_HOME", ".cache") else {
        return vec![];
    };
    ["yay", "paru", "pikaur", "yapacc"]
        .iter()
        .map(|n| cache.join(n))
        .filter(|p| p.is_dir())
        .collect()
}

/// The copy of the repository metadata that dnf5 makes for an ordinary
/// user (`~/.cache/libdnf5`) whenever that user asks it about the
/// repositories, which `check`, `search` and `briefing` do: 84 MiB on a
/// Fedora 45 VM after `sudo dnf makecache` (2026-11-19). `dnf clean` run as
/// root does not touch it. `usage` is [`dir_usage`] of `dir`. `None` when
/// the directory holds no file. Pure.
pub fn dnf_user_cache_item(dir: &Path, usage: (u64, usize, bool)) -> Option<Item> {
    let (bytes, files, complete) = usage;
    (files > 0).then(|| Item {
        what: format!(
            "repository metadata dnf copied for this user ({})",
            dir.display()
        ),
        size_bytes: complete.then_some(bytes),
        count: files,
        safety: "safe",
        command: format!("rm -r -- '{}'", dir.display()),
        detail: vec![],
        note: "safe: dnf copies it again from the system cache the next time this user asks it about the repositories (`check`, `search` and `briefing` do)".into(),
    })
}

/// The items, and what could not be worked out (each a sentence).
pub fn collect(backend: Option<&dyn NativeBackend>) -> (Vec<Item>, Vec<String>) {
    let mut items = Vec::new();
    let mut notes = Vec::new();
    if let Some(b) = backend {
        // an orphan list that could not be worked out is said, not left out
        match (crate::cli::orphans_unknown(b), b.orphans()) {
            (Some(why), _) => notes.push(format!("orphans are not known: {why}")),
            (None, Err(e)) => notes.push(format!("orphans are not known: {}", e.message)),
            (None, Ok(orphans)) => {
                let sizes: HashMap<String, u64> = b
                    .installed()
                    .map(|v| v.into_iter().map(|p| (p.name, p.size_bytes)).collect())
                    .unwrap_or_default();
                items.extend(orphan_item(&orphans, &sizes));
            }
        }
        let m = b.manager();
        let (dirs, command) = cache_of(m, crate::util::have("paccache"));
        let elevated = |command: &str| {
            with_available_elevation(
                command,
                crate::util::have("sudo"),
                crate::util::have("doas"),
            )
        };
        let bound = |complete: bool| {
            if complete {
                ""
            } else {
                "size is a lower bound (some directories are readable by root only); "
            }
        };
        for dir in dirs.iter().map(Path::new).filter(|d| d.is_dir()) {
            // dnf: the packages are the `*.rpm` files of the tree; the rest
            // is repository metadata, which `dnf clean packages` keeps
            let dnf = m == PkgManager::Dnf;
            let (bytes, files, complete) = dir_usage_of(dir, |p| is_cached_package(m, p));
            // apt's `partial/` is readable by root only on every system:
            // that alone is no reason to report an empty cache
            if files > 0 || (!complete && m != PkgManager::Apt) {
                items.push(Item {
                    what: format!(
                        "downloaded packages kept by {} ({})",
                        m.name(),
                        dir.display()
                    ),
                    size_bytes: Some(bytes),
                    count: files,
                    safety: "safe",
                    command: elevated(command),
                    detail: vec![],
                    note: format!("{}{}", bound(complete), cache_note(m)),
                });
            }
            if dnf {
                let (bytes, files, complete) = dir_usage_of(dir, |p| !is_rpm(p));
                if files > 0 {
                    items.push(Item {
                        what: format!("repository metadata kept by dnf ({})", dir.display()),
                        size_bytes: Some(bytes),
                        count: files,
                        safety: "safe",
                        command: elevated("sudo dnf clean metadata"),
                        detail: vec![],
                        note: format!(
                            "{}safe: rebuilt at the next refresh; until then `check`, `search` and `briefing` have no repository data to read",
                            bound(complete)
                        ),
                    });
                }
            }
        }
    }
    if let Some(scope) = crate::why::flatpak_read_scope() {
        let apps = flatpak_column(&["list", "--app", "--columns=runtime"], scope);
        let runtimes = flatpak_column(
            &["list", "--runtime", "--columns=application,arch,branch"],
            scope,
        );
        let unused = unused_runtimes(&apps, &runtimes);
        if !unused.is_empty() {
            items.push(Item {
                what: "flatpak runtimes no installed application declares".into(),
                size_bytes: None,
                count: unused.len(),
                safety: "review",
                command: "flatpak uninstall --unused   (flatpak lists them and asks)".into(),
                detail: unused,
                note: "flatpak's own check is the authority; this list is yapacc's reading of it"
                    .into(),
            });
        }
    }
    // dnf5's per-user copy of the metadata: listed only when it is there
    if let Some(dir) = crate::config::xdg_base("XDG_CACHE_HOME", ".cache")
        .map(|cache| cache.join("libdnf5"))
        .filter(|d| d.is_dir())
    {
        items.extend(dnf_user_cache_item(&dir, dir_usage(&dir)));
    }
    for dir in home_caches() {
        let (bytes, files, complete) = dir_usage(&dir);
        if files == 0 {
            continue;
        }
        items.push(Item {
            what: format!("build and download cache ({})", dir.display()),
            size_bytes: complete.then_some(bytes),
            count: files,
            safety: "safe",
            command: format!("rm -r -- '{}'", dir.display()),
            detail: vec![],
            note: "safe: rebuilt or downloaded again when needed".into(),
        });
    }
    (items, notes)
}

pub fn render(items: &[Item]) -> String {
    if items.is_empty() {
        return "Nothing to reclaim: no orphans, no package cache, no unused runtimes.\n".into();
    }
    let mut s = String::new();
    let mut total = 0u64;
    for item in items {
        let size = match item.size_bytes {
            Some(b) => {
                total += b;
                human(b)
            }
            None => "size unknown".into(),
        };
        s.push_str(&format!(
            "{:>10}  [{}] {} — {} item(s)\n",
            size, item.safety, item.what, item.count
        ));
        if !item.detail.is_empty() {
            let shown: Vec<&str> = item.detail.iter().take(8).map(String::as_str).collect();
            let more = item.detail.len().saturating_sub(shown.len());
            s.push_str(&format!(
                "            {}{}\n",
                shown.join(", "),
                if more > 0 {
                    format!(", … and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        s.push_str(&format!("            reclaim: {}\n", item.command));
        if !item.note.is_empty() {
            s.push_str(&format!("            {}\n", item.note));
        }
    }
    s.push_str(&format!(
        "\nMeasured total: {}. Nothing was changed: `clean` only reports.\n",
        human(total)
    ));
    s
}

pub fn run(json: bool) -> Result<()> {
    let backend = crate::native::native_backend().ok();
    let (items, notes) = collect(backend.as_deref());
    if json {
        match serde_json::to_string_pretty(&items) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                return Err(crate::Error::from(format!(
                    "json serialization failed: {e}"
                )));
            }
        }
        for note in &notes {
            eprintln!("note: {note}");
        }
    } else {
        for note in &notes {
            println!("note: {note}");
        }
        if !notes.is_empty() {
            println!();
        }
        if backend.is_none() {
            let why = match crate::platform::missing_native_tool() {
                Some((m, tool)) => crate::platform::missing_tool_message(m, tool),
                None => "no supported native package manager here".into(),
            };
            println!("note: {why}; orphans and the package cache are not covered.\n");
        }
        if items.is_empty() && !notes.is_empty() {
            // "no orphans" is not claimed when they could not be worked out
            println!(
                "Nothing else to reclaim: no package cache, no unused runtimes. Nothing was changed: `clean` only reports."
            );
        } else {
            print!("{}", render(&items));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// debian:stable after `apt-get clean` holds `lock` and `partial/` in
    /// /var/cache/apt/archives, and `clean` said "downloaded packages kept
    /// by apt — 1 item(s)"; Alpine after `apk update` holds two
    /// `APKINDEX.*.tar.gz` in /var/cache/apk, and they were "downloaded
    /// packages kept by apk — 2 item(s)" (2026-11-19).
    #[test]
    fn only_package_files_count_as_downloaded_packages() {
        let p = |name: &str| PathBuf::from("/var/cache/x").join(name);
        assert!(!is_cached_package(PkgManager::Apt, &p("lock")));
        assert!(is_cached_package(
            PkgManager::Apt,
            &p("tree_2.2.1-1_amd64.deb")
        ));
        assert!(!is_cached_package(
            PkgManager::Apk,
            &p("APKINDEX.630cb808.tar.gz")
        ));
        assert!(is_cached_package(
            PkgManager::Apk,
            &p("tree-2.3.2-r0.5a1b2c3d.apk")
        ));
        assert!(is_cached_package(
            PkgManager::Dnf,
            &p("tree-2.3.1-2.fc45.x86_64.rpm")
        ));
        assert!(!is_cached_package(PkgManager::Dnf, &p("repomd.xml")));
        // the others keep nothing else there: every file counts
        assert!(is_cached_package(
            PkgManager::Pacman,
            &p("tree-2.3.2-1-x86_64.pkg.tar.zst.sig")
        ));
    }

    #[test]
    fn hints_name_the_elevation_tool_that_exists() {
        assert_eq!(
            with_available_elevation("sudo apk cache clean", false, true),
            "doas apk cache clean"
        );
        assert_eq!(
            with_available_elevation("sudo apk cache clean", true, true),
            "sudo apk cache clean"
        );
        assert_eq!(
            with_available_elevation("sudo apk cache clean", false, false),
            "sudo apk cache clean"
        );
        assert_eq!(
            with_available_elevation("nix-collect-garbage", false, true),
            "nix-collect-garbage"
        );
    }

    #[test]
    fn sizes_read_like_people_write_them() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(6 * 1024 * 1024 * 1024), "6.0 GiB");
    }

    #[test]
    fn orphans_are_listed_largest_first() {
        let sizes: HashMap<String, u64> =
            [("big".to_string(), 5000u64), ("small".to_string(), 10)].into();
        let item = orphan_item(&["small".into(), "big".into(), "unknown".into()], &sizes).unwrap();
        assert_eq!(item.count, 3);
        assert_eq!(item.size_bytes, Some(5010));
        assert!(item.detail[0].starts_with("big"), "{:?}", item.detail);
        assert_eq!(item.detail[2], "unknown");
        assert_eq!(item.safety, "review");
        assert!(orphan_item(&[], &sizes).is_none());
        // apt's name of a foreign-architecture orphan still finds its size
        let foreign = orphan_item(&["big:i386".into()], &sizes).unwrap();
        assert_eq!(foreign.size_bytes, sizes.get("big").copied());
        assert!(foreign.detail[0].starts_with("big:i386 ("));
    }

    #[test]
    fn only_whole_unreferenced_runtimes_are_called_unused() {
        let apps: Vec<String> = ["org.freedesktop.Platform/x86_64/24.08"]
            .map(String::from)
            .into();
        let runtimes: Vec<String> = [
            "org.freedesktop.Platform/x86_64/24.08",
            "org.freedesktop.Platform/x86_64/22.08",
            "org.freedesktop.Platform.GL.default/x86_64/24.08",
            "org.freedesktop.Sdk/x86_64/24.08",
            "org.gnome.Sdk/x86_64/45",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            unused_runtimes(&apps, &runtimes),
            vec![
                "org.freedesktop.Platform/x86_64/22.08",
                "org.gnome.Sdk/x86_64/45"
            ]
        );
    }

    #[test]
    fn directory_usage_counts_files_not_links() {
        let dir = std::env::temp_dir().join(format!("yapacc-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a"), b"12345").unwrap();
        std::fs::write(dir.join("sub").join("b"), b"123").unwrap();
        let _ = std::os::unix::fs::symlink(dir.join("a"), dir.join("link"));
        let (bytes, files, complete) = dir_usage(&dir);
        assert_eq!((bytes, files, complete), (8, 2, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D3: `clean` on dnf reported the metadata cache as "downloaded
    /// packages", with `dnf clean packages`, which frees none of it, and the
    /// sentence about `undo` looking there.
    #[test]
    fn dnf_packages_and_metadata_are_measured_apart() {
        let dir = std::env::temp_dir().join(format!("yapacc-clean-dnf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("fedora-2d95c80a1fa0a67d");
        std::fs::create_dir_all(repo.join("repodata")).unwrap();
        std::fs::create_dir_all(repo.join("packages")).unwrap();
        std::fs::write(repo.join("repodata/repomd.xml"), b"1234567").unwrap();
        std::fs::write(repo.join("solv"), b"123").unwrap();
        std::fs::write(repo.join("packages/tree-2.3.1-2.fc45.x86_64.rpm"), b"12345").unwrap();
        assert_eq!(dir_usage_of(&dir, is_rpm), (5, 1, true));
        assert_eq!(dir_usage_of(&dir, |p| !is_rpm(p)), (10, 2, true));
        assert_eq!(dir_usage(&dir), (15, 3, true));
        let _ = std::fs::remove_dir_all(&dir);
        // undo reads the cache on pacman and apt only
        for m in [PkgManager::Pacman, PkgManager::Apt] {
            assert!(
                cache_note(m).contains("`yapacc undo` looks here first"),
                "{m:?}"
            );
        }
        for m in [PkgManager::Dnf, PkgManager::Zypper] {
            assert!(!cache_note(m).contains("undo"), "{m:?}");
            assert!(cache_note(m).starts_with("safe: "), "{m:?}");
        }
        assert!(cache_note(PkgManager::Apk).contains("only local copy"));
        assert_eq!(
            cache_of(PkgManager::Dnf, false).1,
            "sudo dnf clean packages"
        );
    }

    /// VM round of 2026-11-19, Fedora 45: after `sudo dnf makecache`,
    /// `check`, `search` and `briefing` run as a user each left 84 MiB in
    /// ~/.cache/libdnf5 (dnf5's copy), and `clean` never listed it.
    #[test]
    fn the_copy_dnf5_makes_for_a_user_is_listed_with_its_command() {
        let dir = Path::new("/home/tester/.cache/libdnf5");
        let item = dnf_user_cache_item(dir, (88_080_384, 19, true)).unwrap();
        assert_eq!(
            item.what,
            "repository metadata dnf copied for this user (/home/tester/.cache/libdnf5)"
        );
        assert_eq!(item.command, "rm -r -- '/home/tester/.cache/libdnf5'");
        assert_eq!((item.size_bytes, item.count), (Some(88_080_384), 19));
        assert_eq!(item.safety, "safe");
        assert!(item.note.starts_with("safe: dnf copies it again"));
        let out = render(&[item]);
        assert!(
            out.contains("84.0 MiB  [safe] repository metadata dnf copied for this user"),
            "{out}"
        );
        assert!(
            out.contains("reclaim: rm -r -- '/home/tester/.cache/libdnf5'"),
            "{out}"
        );
        // an empty directory is nothing to reclaim
        assert_eq!(dnf_user_cache_item(dir, (0, 0, true)), None);
        // a size that could not be measured in full is not given as one
        let partial = dnf_user_cache_item(dir, (10, 1, false)).unwrap();
        assert_eq!(partial.size_bytes, None);
    }

    #[test]
    fn report_says_nothing_changed() {
        let items = vec![Item {
            what: "cache".into(),
            size_bytes: Some(2048),
            count: 2,
            safety: "safe",
            command: "sudo apt-get clean".into(),
            detail: vec![],
            note: String::new(),
        }];
        let out = render(&items);
        assert!(out.contains("2.0 KiB"), "{out}");
        assert!(out.contains("reclaim: sudo apt-get clean"), "{out}");
        assert!(out.contains("Nothing was changed"), "{out}");
        assert!(render(&[]).starts_with("Nothing to reclaim"));
    }
}
