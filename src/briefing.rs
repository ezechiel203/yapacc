// yapacc — `briefing`: what the next upgrade will do to this machine
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Read-only. Before an upgrade it answers: how many packages, which of
//! them deserve attention (kernel, C library, init, boot loader, graphics,
//! the package manager itself, major version jumps), what the distribution
//! announced, which updates are security fixes, which configuration files
//! are still unmerged from last time, whether there is room, and what the
//! safety net is. Nothing is refreshed or downloaded except, when allowed,
//! the distribution's news feed.

use crate::error::Result;
use crate::native::{NativeBackend, Update};
use crate::platform::{PkgManager, Platform};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Flag {
    /// "reboot" | "care" | "info"
    pub level: &'static str,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Briefing {
    pub manager: String,
    pub updates: Vec<Update>,
    pub flags: Vec<Flag>,
    pub news: Vec<(String, String)>,
    pub security: Vec<String>,
    pub unmerged_config: Vec<String>,
    pub disk: Vec<String>,
    pub safety_net: String,
    pub notes: Vec<String>,
}

/// How the note starts that says the pending list could not be worked out.
const PENDING_UNKNOWN: &str = "pending updates are not known";

/// The leading number of a version, after any epoch: `2:9.1-3` → 9.
pub fn major(version: &str) -> Option<u64> {
    let v = version.split_once(':').map(|(_, v)| v).unwrap_or(version);
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Is `n` a package that holds a kernel image, on any of the supported
/// systems? Headers, sources, documentation, firmware and tools are not.
/// Pure.
///
/// - Arch: `linux`, `linux-lts`, `linux-zen`, `linux-hardened`, `linux-rt`,
///   `linux-rt-lts` (and `linux-cachyos` on derivatives)
/// - Debian, Ubuntu: `linux-image-*`, and Ubuntu's meta packages
///   `linux-generic`, `linux-virtual`, `linux-lowlatency`
/// - Fedora, RHEL: `kernel`, `kernel-core`, `kernel-rt`, `kernel-rt-core`,
///   `kernel-debug-core`, `kernel-64k`, `kernel-64k-core`, `kernel-uki-virt`
/// - openSUSE: `kernel-FLAVOUR` and `kernel-FLAVOUR-base` (the Minimal-VM
///   and Cloud images install `kernel-default-base`) for the flavours
///   default, preempt, rt, rt_debug, longterm, azure, kvmsmall, 64kb, pae,
///   lpae, vanilla, debug, zfcpdump
/// - Alpine: `linux-lts`, `linux-virt`, `linux-edge`, `linux-stable`,
///   `linux-rpi`
/// - Void: `linux`, `linux-lts`, `linux-mainline`, and `linuxN.M`
/// - Manjaro: `linuxNM` (`linux66`, `linux612`), and `linuxNM-rt`
/// - FreeBSD (pkgbase): `FreeBSD-kernel-*`, except the debug files (`-dbg`)
pub fn is_kernel(n: &str) -> bool {
    if matches!(
        n,
        "linux"
            | "linux-lts"
            | "linux-zen"
            | "linux-hardened"
            | "linux-rt"
            | "linux-rt-lts"
            | "linux-cachyos"
            | "linux-mainline"
            | "linux-generic"
            | "linux-virtual"
            | "linux-lowlatency"
            | "linux-lts-virt"
            | "linux-virt"
            | "linux-edge"
            | "linux-stable"
            | "linux-rpi"
            | "kernel"
    ) || n.starts_with("linux-image-")
    {
        return true;
    }
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    // Void: linux6.12, linux7.3 (not linux6.12-headers)
    if let Some(version) = n.strip_prefix("linux")
        && let Some((major, minor)) = version.split_once('.')
    {
        return digits(major) && digits(minor);
    }
    // Manjaro: linux66, linux612, linux612-rt (not linux612-headers, and
    // not linux612-nvidia, which is a module built for that kernel)
    if let Some(version) = n.strip_prefix("linux") {
        let version = version.strip_suffix("-rt").unwrap_or(version);
        if version.len() >= 2 && digits(version) {
            return true;
        }
    }
    if let Some(rest) = n.strip_prefix("FreeBSD-kernel-") {
        return !rest.is_empty() && !rest.ends_with("-dbg") && !rest.contains("-dbg-");
    }
    if let Some(flavour) = n.strip_prefix("kernel-") {
        // openSUSE's `-base` is the same kernel with fewer modules
        let flavour = flavour.strip_suffix("-base").unwrap_or(flavour);
        return matches!(
            flavour,
            "core"
                | "rt"
                | "rt-core"
                | "debug-core"
                | "64k"
                | "64k-core"
                | "uki-virt"
                | "default"
                | "preempt"
                | "rt_debug"
                | "longterm"
                | "azure"
                | "kvmsmall"
                | "64kb"
                | "pae"
                | "lpae"
                | "vanilla"
                | "debug"
                | "zfcpdump"
        );
    }
    false
}

fn named(updates: &[Update], pick: impl Fn(&str) -> bool) -> Vec<String> {
    updates
        .iter()
        .filter(|u| pick(&u.name))
        .map(|u| {
            if u.old_version.is_empty() {
                format!("{} {}", u.name, u.new_version)
            } else {
                format!("{} {} -> {}", u.name, u.old_version, u.new_version)
            }
        })
        .collect()
}

/// What deserves attention among pending updates. `removes_old_modules`:
/// the package manager replaces the running kernel's module directory when
/// it upgrades the kernel (pacman), which is worth a sentence there and
/// noise on a system that installs the new kernel beside the old one.
/// Pure.
pub fn classify(updates: &[Update], removes_old_modules: bool) -> Vec<Flag> {
    let mut flags = Vec::new();
    let mut flag = |level: &'static str, hits: Vec<String>, text: &str| {
        if !hits.is_empty() {
            flags.push(Flag {
                level,
                text: format!("{text}: {}", hits.join(", ")),
            });
        }
    };
    flag(
        "reboot",
        named(updates, is_kernel),
        if removes_old_modules {
            "new kernel — reboot to use it (the upgrade replaces the running kernel's modules: plugging in new hardware may fail until you reboot)"
        } else {
            "new kernel — reboot to use it"
        },
    );
    flag(
        "reboot",
        named(updates, |n| {
            matches!(n, "glibc" | "libc6" | "musl" | "libc" | "glibc-common")
        }),
        "C library — every running program keeps the old one until restarted; reboot when convenient",
    );
    flag(
        "reboot",
        named(updates, |n| {
            matches!(
                n,
                "systemd" | "systemd-libs" | "openrc" | "runit" | "dinit" | "s6" | "sysvinit"
            )
        }),
        "init system — reboot recommended",
    );
    flag(
        "care",
        named(updates, |n| {
            n == "grub"
                || n.starts_with("grub2")
                || n.starts_with("grub-pc")
                || n.starts_with("grub-efi")
                || matches!(
                    n,
                    "limine" | "shim" | "shim-signed" | "refind" | "systemd-boot" | "syslinux"
                )
        }),
        "boot loader package — some distributions update the package but NOT the copy installed on disk; check your distribution's notes before rebooting",
    );
    flag(
        "care",
        named(updates, |n| {
            n.starts_with("nvidia")
                || matches!(
                    n,
                    "mesa" | "xorg-server" | "libgl1-mesa-dri" | "mesa-dri-drivers"
                )
        }),
        "graphics stack — log out and in again; with a new nvidia driver, reboot",
    );
    flag(
        "care",
        named(updates, |n| {
            matches!(
                n,
                "pacman"
                    | "apt"
                    | "dpkg"
                    | "dnf"
                    | "dnf5"
                    | "rpm"
                    | "zypper"
                    | "libzypp"
                    | "apk-tools"
                    | "xbps"
                    | "pkg"
            )
        }),
        "the package manager itself — let it finish alone; do not interrupt",
    );
    flag(
        "care",
        named(updates, |n| {
            matches!(
                n,
                "openssh" | "openssh-server" | "openssl" | "libssl3" | "libssl3t64"
            )
        }),
        "remote access — if you are connected over ssh, keep a second session open until you have reconnected",
    );
    let jumps: Vec<String> = updates
        .iter()
        .filter(|u| match (major(&u.old_version), major(&u.new_version)) {
            (Some(a), Some(b)) => b > a,
            _ => false,
        })
        .filter(|u| !is_kernel(&u.name))
        .map(|u| format!("{} {} -> {}", u.name, u.old_version, u.new_version))
        .collect();
    if !jumps.is_empty() {
        let shown = jumps
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = jumps.len().saturating_sub(10);
        flags.push(Flag {
            level: "info",
            text: format!(
                "major version changes ({}): {shown}{}",
                jumps.len(),
                if more > 0 {
                    format!(", … and {more} more")
                } else {
                    String::new()
                }
            ),
        });
    }
    if updates.iter().any(|u| {
        u.name.starts_with("postgresql")
            && major(&u.old_version) < major(&u.new_version)
            && !u.old_version.is_empty()
    }) {
        flags.push(Flag {
            level: "care",
            text: "PostgreSQL major version — the data directory must be migrated (pg_upgrade) before the new server starts".into(),
        });
    }
    flags
}

/// Updates that come from a security repository (apt names them so).
/// `repo` holds every origin apt gave for the update, separated by `, `:
/// one of them being a security pocket is enough (Ubuntu lists
/// `noble-updates` before `noble-security`; Debian has
/// `Debian-Security:13/stable-security`).
pub fn security_from_repo(updates: &[Update]) -> Vec<String> {
    updates
        .iter()
        .filter(|u| u.repo.to_lowercase().contains("security"))
        .map(|u| u.name.clone())
        .collect()
}

/// Unmerged configuration files left under `root` by earlier upgrades: the
/// files `yapacc configs` reports (the same suffixes, `.rpmsave`,
/// `.dpkg-old` and `.ucf-dist` included, and the same walk), so that the
/// two commands cannot disagree.
pub fn unmerged_config(root: &Path, max_dirs: usize) -> Vec<String> {
    crate::configs::scan(root, max_dirs)
        .files
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// Parse `df -Pk PATH`: (available bytes, mount point).
pub fn parse_df(text: &str) -> Option<(u64, String)> {
    let line = text.lines().nth(1)?;
    let f: Vec<&str> = line.split_whitespace().collect();
    (f.len() >= 6).then(|| (f[3].parse::<u64>().unwrap_or(0) * 1024, f[5..].join(" ")))
}

fn free_space(path: &str) -> Option<(u64, String)> {
    let out = Command::new("df")
        .args(["-Pk", "--", path])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// Disk lines, with a warning when space is short. Pure over the readings.
pub fn disk_lines(readings: &[(&str, Option<(u64, String)>)]) -> Vec<String> {
    const GIB: u64 = 1024 * 1024 * 1024;
    let mut lines = Vec::new();
    for (path, reading) in readings {
        let Some((free, mount)) = reading else {
            continue;
        };
        // a path that is not its own mount point is covered by its parent
        if *path != "/" && mount != path {
            continue;
        }
        let low = if *path == "/boot" {
            *free < 150 * 1024 * 1024
        } else {
            *free < 2 * GIB
        };
        lines.push(format!(
            "{path}: {} free{}",
            crate::clean::human(*free),
            if low {
                "  — LOW: an upgrade that runs out of space can leave the system half-upgraded"
            } else {
                ""
            }
        ));
    }
    lines
}

fn pending(b: &dyn NativeBackend, notes: &mut Vec<String>, offline: bool) -> Vec<Update> {
    // Arch: `checkupdates` answers from a private copy of the databases, so
    // the answer is current without touching the system's own. It asks the
    // mirrors, so `--offline` leaves it out.
    if b.manager() == PkgManager::Pacman && !offline {
        match crate::backend::current_pending_updates() {
            crate::backend::CheckUpdates::Current(lines) => {
                notes.push("pending list is current (checkupdates)".into());
                return crate::native::pacman::parse_updates(&lines.join("\n"));
            }
            crate::backend::CheckUpdates::Failed(why) => notes.push(format!(
                "`checkupdates` is installed but failed ({why}): the list below is not current"
            )),
            crate::backend::CheckUpdates::Missing => {}
        }
    }
    // no metadata on disk: nothing was asked, so "0 pending" is not known
    if let Some(why) = b.missing_metadata() {
        notes.push(format!("{PENDING_UNKNOWN}: {why}"));
    } else {
        notes.push("pending list is as of the last repository refresh; yapacc does not refresh on a read command".into());
    }
    match b.updates() {
        Ok(u) => u,
        Err(e) => {
            notes.push(format!("pending updates could not be read: {}", e.message));
            vec![]
        }
    }
}

fn security(b: &dyn NativeBackend, updates: &[Update], notes: &mut Vec<String>) -> Vec<String> {
    let lines = |prog: &str, args: &[&str]| -> Option<Vec<String>> {
        let out = Command::new(prog)
            .args(args)
            .env("LC_ALL", "C")
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        )
    };
    // the two below ask the manager about its repositories: not without
    // metadata on disk (zypper would download it; `dnf -C` fails)
    if matches!(b.manager(), PkgManager::Dnf | PkgManager::Zypper) && b.missing_metadata().is_some()
    {
        return vec![];
    }
    match b.manager() {
        PkgManager::Apt => security_from_repo(updates),
        PkgManager::Dnf => lines("dnf", &["-q", "-C", "updateinfo", "list", "--security"])
            .unwrap_or_default()
            .into_iter()
            .filter(|l| {
                l.contains("FEDORA-")
                    || l.contains("RHSA-")
                    || l.contains("ALSA-")
                    || l.contains("RLSA-")
            })
            .collect(),
        PkgManager::Pacman if crate::util::have("arch-audit") => {
            lines("arch-audit", &["-u"]).unwrap_or_default()
        }
        PkgManager::Zypper => lines(
            "zypper",
            &[
                "--non-interactive",
                "--no-refresh",
                "list-patches",
                "--category",
                "security",
            ],
        )
        .unwrap_or_default()
        .into_iter()
        .filter(|l| l.contains('|') && l.contains("security") && !l.starts_with("Repository"))
        .collect(),
        m => {
            notes.push(match m {
                PkgManager::Pacman => "security: no source on this machine (install `arch-audit` to see which updates fix known vulnerabilities)".to_string(),
                other => format!("security: {} does not label security updates in a way yapacc reads yet", other.name()),
            });
            vec![]
        }
    }
}

pub fn collect(b: &dyn NativeBackend, p: &Platform, offline: bool) -> Briefing {
    let mut notes = Vec::new();
    let updates = pending(b, &mut notes, offline);
    let flags = classify(&updates, b.manager() == PkgManager::Pacman);
    let security = security(b, &updates, &mut notes);
    let news = if p.family == "arch" && !offline {
        match crate::backend::arch_news(4) {
            Ok(items) => items,
            Err(e) => {
                notes.push(format!("news: {e}"));
                vec![]
            }
        }
    } else {
        if p.family != "arch" {
            notes.push("news: this distribution has no machine-readable upgrade announcements yapacc reads; check its release notes for manual steps".into());
        }
        vec![]
    };
    let disk = disk_lines(&[
        ("/", free_space("/")),
        ("/boot", free_space("/boot")),
        ("/var", free_space("/var")),
    ]);
    Briefing {
        manager: b.manager().name().into(),
        flags,
        news,
        security,
        unmerged_config: unmerged_config(Path::new("/etc"), 4000),
        disk,
        safety_net: safety_net(
            &p.snapshot_providers,
            b.manager().name(),
            crate::ops::supports_undo(b.manager()),
        ),
        notes,
        updates,
    }
}

/// The "Safety net" sentence. `providers` is what `platform` detected.
///
/// A btrfs root with only the `btrfs` tool is detected as a provider, but
/// yapacc takes snapshots through a snapshot manager (snapper, timeshift)
/// or zfs, and `yapacc snapshot create` refuses plain btrfs. So plain btrfs
/// is not presented as "snapshots available — take one": it is named for
/// what it is, with what to install.
fn safety_net(providers: &[String], manager: &str, undo: bool) -> String {
    let usable: Vec<&str> = providers
        .iter()
        .map(String::as_str)
        .filter(|p| *p != "btrfs")
        .collect();
    let plain_btrfs = usable.is_empty() && providers.iter().any(|p| p == "btrfs");
    if !usable.is_empty() {
        let through = usable.join(", ");
        return if undo {
            format!(
                "snapshots available through {through} — take one before upgrading; `yapacc undo` restores package versions only"
            )
        } else {
            format!(
                "snapshots available through {through} — take one before upgrading (`yapacc undo` is not available with {manager})"
            )
        };
    }
    let found = if plain_btrfs {
        "btrfs is present but no snapshot manager is configured (install snapper or timeshift; yapacc does not create raw btrfs snapshots)"
    } else {
        "no snapshot tool detected"
    };
    if undo {
        format!("{found}: `yapacc undo` can restore package versions, not configuration or data")
    } else {
        format!(
            "{found}, and `yapacc undo` is not available with {manager}: nothing yapacc offers can take an upgrade back"
        )
    }
}

pub fn render(b: &Briefing) -> String {
    // not claimed when the question could not be asked (see the note)
    let unknown = b.updates.is_empty() && b.notes.iter().any(|n| n.starts_with(PENDING_UNKNOWN));
    let mut s = if unknown {
        format!(
            "Upgrade briefing — pending packages not known ({})\n",
            b.manager
        )
    } else {
        format!(
            "Upgrade briefing — {} package(s) pending ({})\n",
            b.updates.len(),
            b.manager
        )
    };
    if unknown {
        s.push_str("  The pending updates could not be worked out: see the note below.\n");
    } else if b.updates.is_empty() {
        s.push_str("  Nothing is pending.\n");
    }
    if !b.flags.is_empty() {
        s.push_str("\nNeeds attention\n");
        for f in &b.flags {
            s.push_str(&format!("  {:<7} {}\n", f.level, f.text));
        }
    } else if !b.updates.is_empty() {
        s.push_str("\nNeeds attention\n  nothing special: no kernel, C library, init, boot loader or package-manager update\n");
    }
    if !b.security.is_empty() {
        s.push_str(&format!("\nSecurity fixes ({})\n", b.security.len()));
        for line in b.security.iter().take(15) {
            s.push_str(&format!("  {line}\n"));
        }
        if b.security.len() > 15 {
            s.push_str(&format!("  … and {} more\n", b.security.len() - 15));
        }
    }
    if !b.news.is_empty() {
        s.push_str("\nDistribution news (newest first) — manual steps are announced here\n");
        for (title, date) in &b.news {
            let mark = if title.to_lowercase().contains("manual intervention")
                || title.to_lowercase().contains("requires")
            {
                "!"
            } else {
                " "
            };
            s.push_str(&format!("  {mark} {title}  ({date})\n"));
        }
    }
    if !b.unmerged_config.is_empty() {
        s.push_str(&format!(
            "\nConfiguration files still unmerged from earlier upgrades ({})\n",
            b.unmerged_config.len()
        ));
        for f in b.unmerged_config.iter().take(10) {
            // a file name must not move the cursor
            s.push_str(&format!("  {}\n", crate::configs::visible(f)));
        }
        if b.unmerged_config.len() > 10 {
            s.push_str(&format!("  … and {} more\n", b.unmerged_config.len() - 10));
        }
    }
    if !b.disk.is_empty() {
        s.push_str("\nDisk\n");
        for d in &b.disk {
            s.push_str(&format!("  {d}\n"));
        }
    }
    s.push_str(&format!("\nSafety net\n  {}\n", b.safety_net));
    for n in &b.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    if !b.updates.is_empty() {
        s.push_str("\nNext: `yapacc upgrade` shows the native manager's plan and asks before changing anything.\n");
    }
    s
}

pub fn run(json: bool, offline: bool) -> Result<()> {
    let backend = crate::native::native_backend()?;
    let platform = crate::platform::detect();
    let b = collect(backend.as_ref(), &platform, offline);
    if json {
        match serde_json::to_string_pretty(&b) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                return Err(crate::Error::from(format!(
                    "json serialization failed: {e}"
                )));
            }
        }
    } else {
        print!("{}", render(&b));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(name: &str, old: &str, new: &str, repo: &str) -> Update {
        Update {
            name: name.into(),
            old_version: old.into(),
            new_version: new.into(),
            repo: repo.into(),
        }
    }

    #[test]
    fn attention_flags() {
        let updates = vec![
            up("linux", "6.11.5.arch1-1", "6.12.1.arch1-1", "core"),
            up("glibc", "2.40-1", "2.41-1", "core"),
            up("grub", "2:2.12-2", "2:2.12-3", "core"),
            up("pacman", "7.0.0-1", "7.0.1-1", "core"),
            up("python", "3.12.7-1", "4.0.0-1", "core"),
            up("tree", "2.1.1-1", "2.1.3-1", "extra"),
        ];
        let flags = classify(&updates, true);
        let text: Vec<&str> = flags.iter().map(|f| f.text.as_str()).collect();
        assert!(
            text.iter()
                .any(|t| t.starts_with("new kernel") && t.contains("linux 6.11.5")),
            "{text:?}"
        );
        assert!(text.iter().any(|t| t.starts_with("C library")), "{text:?}");
        // the sentence about the modules is said where it is true (pacman)
        let kernel = |pacman: bool| {
            classify(&updates, pacman)
                .into_iter()
                .find(|f| f.text.starts_with("new kernel"))
                .map(|f| f.text)
                .unwrap_or_default()
        };
        assert!(kernel(true).contains("replaces the running kernel's modules"));
        assert!(kernel(false).starts_with("new kernel — reboot to use it: linux 6.11.5"));
        assert!(
            text.iter().any(|t| t.starts_with("boot loader")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|t| t.starts_with("the package manager itself")),
            "{text:?}"
        );
        let jumps = flags.iter().find(|f| f.level == "info").unwrap();
        assert!(
            jumps.text.contains("python 3.12.7-1 -> 4.0.0-1"),
            "{}",
            jumps.text
        );
        assert!(
            !jumps.text.contains("grub"),
            "epochs are not versions: {}",
            jumps.text
        );
        assert_eq!(flags.iter().filter(|f| f.level == "reboot").count(), 2);
        assert!(classify(&[up("tree", "2.1.1-1", "2.1.3-1", "extra")], true).is_empty());
    }

    #[test]
    fn majors_and_security() {
        assert_eq!(major("2:9.1-3"), Some(9));
        assert_eq!(major("17.2"), Some(17));
        assert_eq!(major("r123.abc"), None);
        assert_eq!(major(""), None);
        let updates = vec![
            up(
                "openssl",
                "3.0.1",
                "3.0.2",
                "Debian-Security:13/stable-security",
            ),
            up("tree", "1", "2", "Debian:13/stable"),
        ];
        assert_eq!(security_from_repo(&updates), vec!["openssl"]);
        // P1: Ubuntu 24.04 names the updates pocket first; `briefing`
        // labelled none of the 45 security updates of 57
        let noble = crate::native::apt::parse_updates(
            "Inst perl [5.38.2-3.2ubuntu0.6] (5.38.2-3.2ubuntu0.7 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64])\nInst tree [2.1.1-2] (2.1.1-2ubuntu3.24.04.2 Ubuntu:24.04/noble-updates [amd64])\n",
        );
        assert_eq!(noble.len(), 2);
        assert_eq!(security_from_repo(&noble), vec!["perl"]);
    }

    /// Z4: `kernel-default-base` (openSUSE Minimal-VM and Cloud images) was
    /// not flagged as a kernel.
    #[test]
    fn kernels_are_recognised_on_every_supported_system() {
        for kernel in [
            // Arch
            "linux",
            "linux-lts",
            "linux-zen",
            "linux-hardened",
            "linux-rt",
            // Debian, Ubuntu
            "linux-image-amd64",
            "linux-image-6.12.57+deb13-amd64",
            "linux-image-generic",
            "linux-image-6.8.0-88-generic",
            "linux-generic",
            // Fedora, RHEL
            "kernel",
            "kernel-core",
            "kernel-rt",
            "kernel-rt-core",
            // openSUSE
            "kernel-default",
            "kernel-default-base",
            "kernel-preempt",
            "kernel-longterm",
            "kernel-azure",
            "kernel-azure-base",
            "kernel-kvmsmall",
            "kernel-64kb",
            // Alpine
            "linux-virt",
            "linux-edge",
            // Void
            "linux6.12",
            "linux7.3",
            "linux-mainline",
            // Manjaro
            "linux66",
            "linux612",
            "linux61-rt",
            // FreeBSD pkgbase
            "FreeBSD-kernel-generic",
            "FreeBSD-kernel-generic-nodebug",
            "FreeBSD-kernel-minimal",
        ] {
            assert!(is_kernel(kernel), "{kernel}");
        }
        for other in [
            "linux-headers",
            "linux-lts-headers",
            "linux-firmware",
            "linux-api-headers",
            "linux-libc-dev",
            "linux-base",
            "linux-tools-common",
            "linux6.12-headers",
            "linux612-headers",
            "linux612-nvidia",
            "linux612-virtualbox-host-modules",
            "linux6",
            "linuxconsoletools",
            "linux-pam",
            "kernel-devel",
            "kernel-default-devel",
            "kernel-headers",
            "kernel-source",
            "kernel-syms",
            "kernel-macros",
            "kernel-firmware-all",
            "kernel-tools",
            "kernel-modules-extra",
            "kernel-install-tools",
            "kernel-srpm-macros",
            "FreeBSD-kernel-generic-dbg",
            "FreeBSD-runtime",
            "util-linux",
        ] {
            assert!(!is_kernel(other), "{other}");
        }
        let flags = classify(
            &[up(
                "kernel-default-base",
                "7.3.3-1.1",
                "7.3.4-1.1",
                "repo-oss",
            )],
            false,
        );
        assert!(
            flags
                .iter()
                .any(|f| f.level == "reboot" && f.text.contains("kernel-default-base 7.3.3-1.1")),
            "{flags:?}"
        );
    }

    #[test]
    fn disk_readings() {
        assert_eq!(
            parse_df(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda2 100000 50000 1048576 50% /\n"
            ),
            Some((1048576 * 1024, "/".to_string()))
        );
        assert_eq!(parse_df("garbage\n"), None);
        let gib = 1024 * 1024 * 1024;
        let lines = disk_lines(&[
            ("/", Some((gib, "/".into()))),
            ("/boot", Some((500 * 1024 * 1024, "/boot".into()))),
            ("/var", Some((gib, "/".into()))),
        ]);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("LOW"), "{lines:?}");
        assert!(!lines[1].contains("LOW"), "{lines:?}");
    }

    #[test]
    fn unmerged_config_files_are_found() {
        let dir = std::env::temp_dir().join(format!("yapacc-brief-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ssh")).unwrap();
        std::fs::write(dir.join("pacman.conf.pacnew"), "").unwrap();
        std::fs::write(dir.join("ssh").join("sshd_config.dpkg-dist"), "").unwrap();
        std::fs::write(dir.join("hosts"), "").unwrap();
        // the kinds `yapacc configs` reports and this list used to miss
        std::fs::write(dir.join("ssh").join("sshd_config.rpmsave"), "").unwrap();
        std::fs::write(dir.join("locale.gen.dpkg-old"), "").unwrap();
        std::fs::write(dir.join("nsswitch.conf.ucf-dist"), "").unwrap();
        let found = unmerged_config(&dir, 100);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.iter().any(|f| f.ends_with("pacman.conf.pacnew")));
        assert!(found.iter().any(|f| f.ends_with("sshd_config.rpmsave")));
        let reported: Vec<String> = crate::configs::scan(&dir, 100)
            .files
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert_eq!(found, reported, "briefing and configs agree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rendering_is_plain_and_complete() {
        let b = Briefing {
            manager: "apt".into(),
            updates: vec![up("openssl", "3.0.1", "3.0.2", "x-security")],
            flags: classify(&[up("openssl", "3.0.1", "3.0.2", "x-security")], false),
            security: vec!["openssl".into()],
            safety_net: "no snapshot tool detected".into(),
            ..Default::default()
        };
        let out = render(&b);
        assert!(
            out.starts_with("Upgrade briefing — 1 package(s) pending (apt)"),
            "{out}"
        );
        assert!(out.contains("remote access"), "{out}");
        assert!(out.contains("Security fixes (1)"), "{out}");
        assert!(out.contains("Safety net"), "{out}");
        let none = render(&Briefing::default());
        assert!(none.contains("Nothing is pending"), "{none}");
        // Z1, D2: without repository metadata "nothing is pending" is not
        // claimed, and the headline does not count
        let unknown = render(&Briefing {
            manager: "zypper".into(),
            notes: vec![format!(
                "{PENDING_UNKNOWN}: {}",
                crate::native::no_metadata_message(PkgManager::Zypper, &[])
            )],
            ..Default::default()
        });
        assert!(!unknown.contains("Nothing is pending"), "{unknown}");
        assert!(
            unknown.starts_with("Upgrade briefing — pending packages not known (zypper)\n"),
            "{unknown}"
        );
        assert!(!unknown.contains("0 package(s) pending"), "{unknown}");
        assert!(
            unknown.contains("note: pending updates are not known: no repository metadata on disk: run `zypper refresh`"),
            "{unknown}"
        );
    }

    /// Review finding 22: on a plain btrfs root `briefing` said "snapshots
    /// available through btrfs — take one", while `snapshot create` refuses
    /// plain btrfs.
    #[test]
    fn plain_btrfs_is_not_offered_as_a_way_to_take_a_snapshot() {
        let p = |names: &[&str]| -> Vec<String> { names.iter().map(|s| s.to_string()).collect() };
        let plain = safety_net(&p(&["btrfs"]), "pacman", true);
        assert_eq!(
            plain,
            "btrfs is present but no snapshot manager is configured (install snapper or timeshift; yapacc does not create raw btrfs snapshots): `yapacc undo` can restore package versions, not configuration or data"
        );
        assert!(!plain.contains("take one"), "{plain}");
        let plain_no_undo = safety_net(&p(&["btrfs"]), "xbps", false);
        assert!(
            plain_no_undo.starts_with("btrfs is present but no snapshot manager is configured")
                && plain_no_undo.contains("`yapacc undo` is not available with xbps"),
            "{plain_no_undo}"
        );
        // a snapshot manager on btrfs: it is the manager that is named
        assert_eq!(
            safety_net(&p(&["snapper", "btrfs"]), "zypper", true),
            "snapshots available through snapper — take one before upgrading; `yapacc undo` restores package versions only"
        );
        assert_eq!(
            safety_net(&p(&["timeshift", "zfs"]), "apt", false),
            "snapshots available through timeshift, zfs — take one before upgrading (`yapacc undo` is not available with apt)"
        );
        // nothing at all: as before
        assert_eq!(
            safety_net(&[], "pacman", true),
            "no snapshot tool detected: `yapacc undo` can restore package versions, not configuration or data"
        );
        assert_eq!(
            safety_net(&[], "apt", false),
            "no snapshot tool detected, and `yapacc undo` is not available with apt: nothing yapacc offers can take an upgrade back"
        );
    }
}
