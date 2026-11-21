// yapacc — platform probe
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! What kind of machine is this? Each axis is detected on its own, from
//! evidence on the running system, and no axis is ever inferred from
//! another: yapacc must work on any combination (Debian with runit, Arch
//! with OpenRC and Limine, Fedora in a container with no init at all).
//!
//! Axes: distribution, native package manager, init system, firmware +
//! boot managers + bootloaders, log systems, snapshot providers, libc.
//!
//! The distribution name is used for the support tier shown to the user,
//! and for one decision: which of dnf and zypper is native when both are
//! installed and both have repositories (see [`rpm_front_end`]). Everything
//! else is keyed on the detected tools.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: Vec<String>,
    pub pretty_name: String,
    pub version_id: String,
}

/// Parse os-release(5) content. Unknown keys are ignored; quotes stripped.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut os = OsRelease {
        id: "unknown".into(),
        id_like: vec![],
        pretty_name: String::new(),
        version_id: String::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches(['"', '\'']).to_string();
        match k.trim() {
            "ID" => os.id = v.to_lowercase(),
            "ID_LIKE" => os.id_like = v.split_whitespace().map(|s| s.to_lowercase()).collect(),
            "PRETTY_NAME" => os.pretty_name = v,
            "VERSION_ID" => os.version_id = v,
            _ => {}
        }
    }
    os
}

fn read_os_release() -> OsRelease {
    ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_os_release(&t))
        .unwrap_or_else(|| parse_os_release(""))
}

/// Distribution family, for the support tier only.
pub fn family(os: &OsRelease) -> &'static str {
    let is = |names: &[&str]| {
        names.contains(&os.id.as_str()) || os.id_like.iter().any(|l| names.contains(&l.as_str()))
    };
    if is(&["debian", "ubuntu"]) {
        "debian"
    } else if is(&["fedora", "rhel", "centos"]) {
        "redhat"
    } else if is(&["arch"]) {
        "arch"
    } else if is(&["suse", "opensuse"]) {
        "suse"
    } else if is(&["void"]) {
        "void"
    } else if is(&["gentoo"]) {
        "gentoo"
    } else if is(&["alpine"]) {
        "alpine"
    } else if is(&["nixos"]) {
        "nixos"
    } else if is(&["freebsd", "dragonfly", "openbsd", "netbsd"]) {
        "bsd"
    } else {
        "other"
    }
}

/// A native package manager yapacc knows how to recognise.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PkgManager {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Xbps,
    Portage,
    Apk,
    Nix,
    /// FreeBSD / DragonFly `pkg`
    Pkg,
}

impl PkgManager {
    pub const ALL: [PkgManager; 9] = [
        PkgManager::Pacman,
        PkgManager::Apt,
        PkgManager::Dnf,
        PkgManager::Zypper,
        PkgManager::Xbps,
        PkgManager::Portage,
        PkgManager::Apk,
        PkgManager::Nix,
        PkgManager::Pkg,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PkgManager::Pacman => "pacman",
            PkgManager::Apt => "apt",
            PkgManager::Dnf => "dnf",
            PkgManager::Zypper => "zypper",
            PkgManager::Xbps => "xbps",
            PkgManager::Portage => "portage",
            PkgManager::Apk => "apk",
            PkgManager::Nix => "nix",
            PkgManager::Pkg => "pkg",
        }
    }

    /// (database path, tool) pairs: the manager counts as present when its
    /// database exists AND its query tool is on PATH.
    fn evidence(self) -> (&'static [&'static str], &'static str) {
        match self {
            PkgManager::Pacman => (&["/var/lib/pacman/local"], "pacman"),
            PkgManager::Apt => (&["/var/lib/dpkg/status"], "dpkg-query"),
            PkgManager::Dnf => (&["/var/lib/rpm", "/usr/lib/sysimage/rpm"], "dnf"),
            PkgManager::Zypper => (&["/var/lib/rpm", "/usr/lib/sysimage/rpm"], "zypper"),
            PkgManager::Xbps => (&["/var/db/xbps"], "xbps-query"),
            PkgManager::Portage => (&["/var/db/pkg"], "emerge"),
            PkgManager::Apk => (&["/lib/apk/db", "/usr/lib/apk/db"], "apk"),
            PkgManager::Nix => (&["/nix/var/nix/db"], "nix-env"),
            PkgManager::Pkg => (&["/var/db/pkg/local.sqlite"], "pkg"),
        }
    }

    fn present(self) -> bool {
        self.has_database() && crate::util::have(self.evidence().1)
    }

    /// Is this manager's package database on the machine? Portage keeps its
    /// database in `/var/db/pkg` too, the directory FreeBSD's `pkg` keeps
    /// `local.sqlite` in: a directory holding that file is not Portage's.
    fn has_database(self) -> bool {
        let found = self.evidence().0.iter().any(|d| Path::new(d).exists());
        found && !(self == PkgManager::Portage && Path::new("/var/db/pkg/local.sqlite").exists())
    }

    /// Does this manager's database own `path`? This is how the NATIVE
    /// manager is told apart from one that merely happens to be installed
    /// (pacman on Ubuntu, rpm on Debian).
    fn owns(self, path: &str) -> bool {
        let (prog, args): (&str, Vec<&str>) = match self {
            PkgManager::Pacman => ("pacman", vec!["-Qo", "--", path]),
            PkgManager::Apt => ("dpkg-query", vec!["-S", "--", path]),
            PkgManager::Dnf | PkgManager::Zypper => ("rpm", vec!["-qf", "--", path]),
            PkgManager::Xbps => ("xbps-query", vec!["-o", path]),
            PkgManager::Portage => ("qfile", vec!["-q", path]),
            PkgManager::Apk => ("apk", vec!["info", "--who-owns", path]),
            PkgManager::Nix => return false,
            PkgManager::Pkg => ("pkg", vec!["which", "-q", path]),
        };
        Command::new(prog)
            .args(args)
            .env("LC_ALL", "C")
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    }

    /// Which support tier this backend is in (docs/STRATEGY.md).
    pub fn tier(self) -> u8 {
        match self {
            PkgManager::Pacman | PkgManager::Apt | PkgManager::Dnf => 1,
            PkgManager::Zypper | PkgManager::Xbps | PkgManager::Apk | PkgManager::Pkg => 2,
            // detected only: no backend, changes refused with the reason
            PkgManager::Portage | PkgManager::Nix => 3,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BootInfo {
    /// "uefi" | "bios" | "unknown"
    pub firmware: String,
    /// boot managers that present a menu / pick an entry
    pub managers: Vec<String>,
    /// loaders that load the kernel
    pub loaders: Vec<String>,
    /// evidence could not be read (commonly: /boot is root-only)
    pub incomplete: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Platform {
    pub os: OsRelease,
    pub family: String,
    pub arch: String,
    pub libc: String,
    /// the manager that owns the base system, if one could be determined
    pub native_pkg_manager: Option<PkgManager>,
    /// every recognised manager present (native first)
    pub pkg_managers: Vec<PkgManager>,
    /// extra package sources present: flatpak, snap, nix, brew, distrobox...
    pub extra_sources: Vec<String>,
    pub init: String,
    pub boot: BootInfo,
    pub log_systems: Vec<String>,
    /// native package-manager logs that exist on this machine
    pub package_logs: Vec<String>,
    /// those that are there, or lie in a directory closed to this user,
    /// and that this user may not read (openSUSE keeps /var/log/zypp
    /// root-only); left out of the JSON when there is none
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub package_logs_unreadable: Vec<String>,
    pub snapshot_providers: Vec<String>,
    pub root_fstype: String,
    pub container: bool,
    pub image_based: bool,
    /// 1 = main target, 2 = supported with some features missing, 3 = detected only, 0 = unknown
    pub support_tier: u8,
}

/// Filesystem type and source of the mount over `/`, from the content of
/// `mount -p` (fstab form: `source mountpoint fstype options dump pass`),
/// which is what FreeBSD has instead of /proc/self/mountinfo. The last
/// mount over `/` wins. Pure.
pub fn parse_mount_p_root(text: &str) -> Option<(String, String)> {
    let mut found = None;
    for line in text.lines() {
        let mut f = line.split_whitespace();
        if let (Some(source), Some("/"), Some(fstype)) = (f.next(), f.next(), f.next()) {
            found = Some((fstype.to_string(), source.to_string()));
        }
    }
    found
}

/// `mount -p` output, for systems without /proc/self/mountinfo.
fn mount_p() -> String {
    std::process::Command::new("mount")
        .arg("-p")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Filesystem type and source of `/` where /proc/self/mountinfo does not
/// exist (FreeBSD): asks `mount -p`.
pub fn root_mount_without_proc() -> Option<(String, String)> {
    parse_mount_p_root(&mount_p())
}

/// Filesystem type of `/` from /proc/self/mountinfo (no findmnt, no shell);
/// without /proc (FreeBSD), from `mount -p`.
pub fn root_fstype() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return root_mount_without_proc()
            .map(|(fs, _)| fs)
            .unwrap_or_else(|| "unknown".into());
    };
    // "... <mount point> <opts> [optional fields] - <fstype> <source> <superopts>"
    let mut found = "unknown".to_string();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        if left.split_whitespace().nth(4) == Some("/")
            && let Some(fs) = right.split_whitespace().next()
        {
            found = fs.to_string(); // last mount over / wins
        }
    }
    found
}

/// The BSDs (as opposed to Linux): no /proc, /sys or /run to read.
fn is_bsd() -> bool {
    matches!(
        std::env::consts::OS,
        "freebsd" | "dragonfly" | "netbsd" | "openbsd"
    )
}

/// `uefi` / `bios` from `sysctl machdep.bootmethod` text (FreeBSD says
/// `UEFI` or `BIOS`). Pure.
pub fn parse_bootmethod(text: &str) -> &'static str {
    match text.trim().to_ascii_lowercase().as_str() {
        "uefi" => "uefi",
        "bios" => "bios",
        _ => "unknown",
    }
}

fn detect_init() -> String {
    let run = |p: &str| Path::new(p).exists();
    if is_bsd() {
        // pid 1 is /sbin/init running /etc/rc; no systemd, no /proc to ask
        return if run("/etc/rc.subr") {
            "rc.d (BSD init)".into()
        } else {
            "init".into()
        };
    }
    if run("/run/systemd/system") {
        return "systemd".into();
    }
    if run("/run/openrc") {
        return "openrc".into();
    }
    if run("/run/runit") || run("/etc/runit/runsvdir") {
        return "runit".into();
    }
    if run("/run/s6") || run("/run/s6-rc") {
        return "s6".into();
    }
    if run("/run/dinit") || run("/run/dinitctl") {
        return "dinit".into();
    }
    match std::fs::read_to_string("/proc/1/comm") {
        Ok(c) => match c.trim() {
            "systemd" => "systemd".into(),
            "runit" | "runsvdir" => "runit".into(),
            "s6-svscan" => "s6".into(),
            "dinit" => "dinit".into(),
            "openrc-init" => "openrc".into(),
            "init" => "sysvinit-compatible".into(),
            other => format!("none (pid 1 is {other})"),
        },
        Err(_) => "unknown".into(),
    }
}

fn detect_boot() -> BootInfo {
    let firmware = if is_bsd() {
        std::process::Command::new("sysctl")
            .args(["-n", "machdep.bootmethod"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_bootmethod(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or("unknown")
    } else if Path::new("/sys/firmware/efi").exists() {
        "uefi"
    } else if Path::new("/sys/firmware").exists() {
        "bios"
    } else {
        "unknown"
    };
    let mut managers: Vec<String> = Vec::new();
    let mut loaders: Vec<String> = Vec::new();
    let mut incomplete = false;
    let esp_roots = ["/boot", "/efi", "/boot/efi"];
    let mut probe = |rels: &[&str], name: &str, into: &mut Vec<String>| {
        for root in esp_roots {
            for rel in rels {
                match std::fs::symlink_metadata(Path::new(root).join(rel)) {
                    Ok(_) => {
                        if !into.iter().any(|x| x == name) {
                            into.push(name.to_string());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        incomplete = true;
                    }
                    Err(_) => {}
                }
            }
        }
    };
    probe(
        &["loader/loader.conf", "EFI/systemd/systemd-bootx64.efi"],
        "systemd-boot",
        &mut managers,
    );
    probe(&["EFI/refind/refind.conf"], "refind", &mut managers);
    probe(&["grub/grub.cfg", "grub2/grub.cfg"], "grub", &mut loaders);
    probe(
        &["limine.conf", "limine/limine.conf", "EFI/limine"],
        "limine",
        &mut loaders,
    );
    probe(
        &["syslinux/syslinux.cfg", "extlinux/extlinux.conf"],
        "syslinux",
        &mut loaders,
    );
    probe(&["EFI/Linux"], "uki (efi stub)", &mut loaders);
    // the BSD boot loader, read from the root file system by the boot blocks
    if is_bsd() && Path::new("/boot/loader").exists() {
        loaders.push("bsd loader (/boot/loader)".into());
    }
    if (Path::new("/etc/default/grub").exists() || crate::util::have("grub-mkconfig"))
        && !loaders.iter().any(|l| l == "grub")
    {
        loaders.push("grub".into());
    }
    if firmware == "uefi" {
        managers.insert(0, "uefi firmware".into());
    }
    BootInfo {
        firmware: firmware.into(),
        managers,
        loaders,
        incomplete,
    }
}

/// Whether opening `path` for reading is refused for lack of permission:
/// the file is there and closed to this user, or it lies in a directory
/// this user may not search (then `Path::exists` says it is not there).
fn closed_to_this_user(path: &Path) -> bool {
    matches!(
        std::fs::File::open(path),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// The value of the `package logs` line. A log this user may not read is
/// said to exist and to be unreadable, whether or not `package_logs` has
/// it. Pure.
pub fn render_package_logs(logs: &[String], unreadable: &[String]) -> String {
    const CLOSED: &str = " (exists; not readable by this user)";
    let mut parts: Vec<String> = logs
        .iter()
        .map(|l| {
            if unreadable.contains(l) {
                format!("{l}{CLOSED}")
            } else {
                l.clone()
            }
        })
        .collect();
    parts.extend(
        unreadable
            .iter()
            .filter(|u| !logs.contains(u))
            .map(|u| format!("{u}{CLOSED}")),
    );
    if parts.is_empty() {
        "none detected".to_string()
    } else {
        parts.join(", ")
    }
}

fn detect_logs() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut systems = Vec::new();
    if Path::new("/run/systemd/journal").exists() {
        systems.push("journald".to_string());
    }
    for (path, name) in [
        ("/var/log/syslog", "syslog (/var/log/syslog)"),
        ("/var/log/messages", "syslog (/var/log/messages)"),
        ("/var/log/socklog", "socklog"),
    ] {
        if Path::new(path).exists() {
            systems.push(name.to_string());
        }
    }
    const CANDIDATES: [&str; 10] = [
        "/var/log/pacman.log",
        "/var/log/dpkg.log",
        "/var/log/apt/history.log",
        "/var/log/dnf.rpm.log",
        "/var/log/dnf5.log",
        "/var/lib/dnf/history.sqlite",
        "/usr/lib/sysimage/libdnf5/transaction_history.sqlite",
        "/var/log/zypp/history",
        "/var/log/emerge.log",
        "/var/log/apk.log",
    ];
    let mut package_logs: Vec<String> = CANDIDATES
        .iter()
        .filter(|p| Path::new(p).exists())
        .map(|p| p.to_string())
        .collect();
    let unreadable: Vec<String> = CANDIDATES
        .iter()
        .filter(|p| closed_to_this_user(Path::new(p)))
        .map(|p| p.to_string())
        .collect();
    // FreeBSD pkg has no log file of its own: it logs through syslog
    if Path::new("/var/db/pkg/local.sqlite").exists() && Path::new("/var/log/messages").exists() {
        package_logs.push("/var/log/messages (pkg lines)".into());
    }
    (systems, package_logs, unreadable)
}

fn detect_libc() -> String {
    if is_bsd() {
        return format!("{} libc", std::env::consts::OS);
    }
    let has_musl = ["/lib", "/usr/lib"].iter().any(|d| {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("ld-musl-"))
            })
            .unwrap_or(false)
    });
    let has_glibc = [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/lib/ld-linux-armhf.so.3",
        "/usr/lib/libc.so.6",
        "/lib/x86_64-linux-gnu/libc.so.6",
        "/lib/aarch64-linux-gnu/libc.so.6",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    match (has_glibc, has_musl) {
        (true, true) => "glibc (musl also present)".into(),
        (true, false) => "glibc".into(),
        (false, true) => "musl".into(),
        (false, false) => "unknown".into(),
    }
}

/// Which recognised package managers are present, and which one owns the
/// base system. This is the only probe `why` needs, so it is separate from
/// the full [`detect`].
pub fn native_manager() -> (Vec<PkgManager>, Option<PkgManager>) {
    let present: Vec<PkgManager> = PkgManager::ALL
        .into_iter()
        .filter(|m| m.present())
        .collect();
    // one manager present: it is the native one, no need to ask who owns what
    let native = if present.len() == 1 {
        Some(present[0])
    } else {
        let owner = present.iter().copied().find(|m| {
            m.owns("/usr/lib/os-release") || m.owns("/etc/os-release") || m.owns("/bin/sh")
        });
        match owner {
            // dnf and zypper read the same rpm database: both "own" the
            // file, and the first one asked is not the answer
            Some(PkgManager::Dnf) if present.contains(&PkgManager::Zypper) => Some(rpm_front_end(
                has_repo_file(Path::new("/etc/zypp/repos.d")),
                has_repo_file(Path::new("/etc/yum.repos.d")),
                family(&read_os_release()),
            )),
            other => other,
        }
    };
    (present, native)
}

/// Does `dir` hold a repository definition (`*.repo`)?
fn has_repo_file(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "repo"))
        })
        .unwrap_or(false)
}

/// Which of dnf and zypper is the system's own when both are installed
/// (openSUSE packages dnf, Fedora packages zypper). The rpm database they
/// share cannot tell them apart, so: the one that has repositories
/// configured (`/etc/zypp/repos.d`, `/etc/yum.repos.d`); when both or
/// neither have, the distribution family (zypper on SUSE, dnf elsewhere).
/// This is the one place where the distribution name decides. Pure.
pub fn rpm_front_end(zypp_repos: bool, yum_repos: bool, family: &str) -> PkgManager {
    match (zypp_repos, yum_repos) {
        (true, false) => PkgManager::Zypper,
        (false, true) => PkgManager::Dnf,
        _ if family == "suse" => PkgManager::Zypper,
        _ => PkgManager::Dnf,
    }
}

/// A manager whose package database is on this machine but whose tool is not
/// on PATH (`/var/db/pkg/local.sqlite` without `pkg`: a stripped PATH, a
/// jail, a broken base). Only meaningful when no manager is present: the
/// caller then names this one instead of guessing another.
pub fn missing_native_tool() -> Option<(PkgManager, &'static str)> {
    missing_tool_in(
        &PkgManager::ALL,
        PkgManager::has_database,
        crate::util::have,
    )
}

/// The words for "no native manager": the generic sentence, or, when a
/// package database is here but its tool is not on PATH, that fact.
pub fn no_native_manager_message() -> String {
    match missing_native_tool() {
        Some((m, tool)) => missing_tool_message(m, tool),
        None => "no native package manager could be determined on this system".into(),
    }
}

/// `this system uses pkg but `pkg` is not on PATH ...`.
pub fn missing_tool_message(m: PkgManager, tool: &str) -> String {
    format!(
        "this system uses {} but `{tool}` is not on PATH (PATH={}); install it or fix PATH",
        m.name(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Pure core of [`missing_native_tool`]: the first of `managers` with a
/// database (`has_db`) whose tool (`have`) is missing.
pub fn missing_tool_in(
    managers: &[PkgManager],
    has_db: impl Fn(PkgManager) -> bool,
    have: impl Fn(&str) -> bool,
) -> Option<(PkgManager, &'static str)> {
    managers.iter().copied().find_map(|m| {
        let tool = m.evidence().1;
        (has_db(m) && !have(tool)).then_some((m, tool))
    })
}

/// Probe every axis. Read-only; spawns only the native managers' own
/// "who owns this file" query.
pub fn detect() -> Platform {
    let os = read_os_release();
    let fam = family(&os).to_string();
    let (present, native) = native_manager();
    let mut pkg_managers = present.clone();
    if let Some(n) = native {
        pkg_managers.retain(|m| *m != n);
        pkg_managers.insert(0, n);
    }
    let extra_sources = [
        "flatpak",
        "snap",
        "brew",
        "distrobox",
        "toolbox",
        "podman",
        "docker",
    ]
    .iter()
    .filter(|t| crate::util::have(t))
    .map(|t| t.to_string())
    .collect();
    let fstype = root_fstype();
    let mut snapshot_providers = Vec::new();
    for tool in ["snapper", "timeshift"] {
        if crate::util::have(tool) {
            snapshot_providers.push(tool.to_string());
        }
    }
    match fstype.as_str() {
        "btrfs" if crate::util::have("btrfs") => snapshot_providers.push("btrfs".into()),
        "zfs" if crate::util::have("zfs") => snapshot_providers.push("zfs".into()),
        _ => {}
    }
    let (log_systems, package_logs, package_logs_unreadable) = detect_logs();
    let container = Path::new("/run/.containerenv").exists() || Path::new("/.dockerenv").exists();
    let image_based = Path::new("/run/ostree-booted").exists();
    let support_tier = native.map(PkgManager::tier).unwrap_or(0);
    Platform {
        os,
        family: fam,
        arch: std::env::consts::ARCH.to_string(),
        libc: detect_libc(),
        native_pkg_manager: native,
        pkg_managers,
        extra_sources,
        init: detect_init(),
        boot: detect_boot(),
        log_systems,
        package_logs,
        package_logs_unreadable,
        snapshot_providers,
        root_fstype: fstype,
        container,
        image_based,
        support_tier,
    }
}

impl Platform {
    pub fn render(&self) -> String {
        let list = |v: &[String]| {
            if v.is_empty() {
                "none detected".to_string()
            } else {
                v.join(", ")
            }
        };
        let tier = match self.support_tier {
            1 => "1 — main target",
            2 => "2 — supported; some features are missing (`yapacc doctor` says which)",
            3 => "3 — detected only",
            _ => "unknown platform (read-only commands still work where data exists)",
        };
        let mut s = String::new();
        s.push_str(&format!(
            "distribution      {} [{}] (family: {})\n",
            if self.os.pretty_name.is_empty() {
                &self.os.id
            } else {
                &self.os.pretty_name
            },
            self.os.id,
            self.family
        ));
        s.push_str(&format!(
            "architecture      {} / {}\n",
            self.arch, self.libc
        ));
        s.push_str(&format!(
            "native packages   {}\n",
            self.native_pkg_manager
                .map(|m| m.name().to_string())
                .unwrap_or_else(|| "not determined".into())
        ));
        let others: Vec<String> = self
            .pkg_managers
            .iter()
            .skip(usize::from(self.native_pkg_manager.is_some()))
            .map(|m| m.name().to_string())
            .collect();
        if !others.is_empty() {
            s.push_str(&format!("other managers    {}\n", others.join(", ")));
        }
        s.push_str(&format!(
            "extra sources     {}\n",
            list(&self.extra_sources)
        ));
        s.push_str(&format!("init              {}\n", self.init));
        s.push_str(&format!(
            "firmware          {}\nboot managers     {}\nbootloaders       {}{}\n",
            self.boot.firmware,
            list(&self.boot.managers),
            list(&self.boot.loaders),
            if self.boot.incomplete {
                "  (incomplete: boot partition not readable without root)"
            } else {
                ""
            }
        ));
        s.push_str(&format!("log systems       {}\n", list(&self.log_systems)));
        s.push_str(&format!(
            "package logs      {}\n",
            render_package_logs(&self.package_logs, &self.package_logs_unreadable)
        ));
        s.push_str(&format!(
            "root filesystem   {}\nsnapshots         {}\n",
            self.root_fstype,
            list(&self.snapshot_providers)
        ));
        if self.container {
            s.push_str("environment       container\n");
        }
        if self.image_based {
            s.push_str("environment       image-based (ostree)\n");
        }
        s.push_str(&format!("support tier      {tier}\n"));
        s
    }
}

#[cfg(test)]
mod tests {
    /// VM round of 2026-11-19, Tumbleweed as an ordinary user: `platform`
    /// said `package logs      none detected` while /var/log/zypp/history
    /// was there, in a directory only root may enter.
    #[test]
    fn a_log_this_user_may_not_read_is_said_to_exist() {
        use super::{closed_to_this_user, render_package_logs};
        let zypp = vec!["/var/log/zypp/history".to_string()];
        assert_eq!(
            render_package_logs(&[], &zypp),
            "/var/log/zypp/history (exists; not readable by this user)"
        );
        // visible and closed: said once
        assert_eq!(
            render_package_logs(&zypp, &zypp),
            "/var/log/zypp/history (exists; not readable by this user)"
        );
        // what is readable is printed as before
        let apt = vec![
            "/var/log/dpkg.log".to_string(),
            "/var/log/apt/history.log".to_string(),
        ];
        assert_eq!(
            render_package_logs(&apt, &[]),
            "/var/log/dpkg.log, /var/log/apt/history.log"
        );
        assert_eq!(render_package_logs(&[], &[]), "none detected");
        // a file behind a directory this user may not enter
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("yapacc-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("history");
        std::fs::write(&file, b"x").unwrap();
        assert!(!closed_to_this_user(&file));
        assert!(!closed_to_this_user(&dir.join("absent")));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root reads through any mode: nothing is closed to it
        let closed = closed_to_this_user(&file);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(closed, !crate::util::is_root());
    }

    /// openSUSE Tumbleweed with `zypper install dnf` (2026-11-19): no
    /// /etc/yum.repos.d, six files in /etc/zypp/repos.d, and `yapacc
    /// platform` said `native packages   dnf`.
    #[test]
    fn dnf_beside_zypper_does_not_make_a_dnf_system() {
        use super::PkgManager::{Dnf, Zypper};
        use super::rpm_front_end;
        // openSUSE with the dnf package: only zypper has repositories
        assert_eq!(rpm_front_end(true, false, "suse"), Zypper);
        // Fedora with the zypper package
        assert_eq!(rpm_front_end(false, true, "redhat"), Dnf);
        // both configured, or neither: the family decides
        assert_eq!(rpm_front_end(true, true, "suse"), Zypper);
        assert_eq!(rpm_front_end(true, true, "redhat"), Dnf);
        assert_eq!(rpm_front_end(false, false, "suse"), Zypper);
        assert_eq!(rpm_front_end(false, false, "other"), Dnf);
        // repositories win over the name: a zypper-only setup elsewhere
        assert_eq!(rpm_front_end(true, false, "redhat"), Zypper);
        let dir = std::env::temp_dir().join(format!("yapacc-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!super::has_repo_file(&dir));
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        assert!(!super::has_repo_file(&dir));
        std::fs::write(dir.join("repo-oss.repo"), "[repo-oss]\n").unwrap();
        assert!(super::has_repo_file(&dir));
        assert!(!super::has_repo_file(&dir.join("absent")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_without_its_tool_names_that_manager() {
        use super::PkgManager::{Pacman, Pkg};
        let all = super::PkgManager::ALL;
        // FreeBSD database, no `pkg` on PATH
        assert_eq!(
            super::missing_tool_in(&all, |m| m == Pkg, |_| false),
            Some((Pkg, "pkg"))
        );
        // tool present: nothing is missing
        assert_eq!(
            super::missing_tool_in(&all, |m| m == Pkg, |t| t == "pkg"),
            None
        );
        // no database anywhere: nothing to name (do not blame pacman)
        assert_eq!(super::missing_tool_in(&all, |_| false, |_| false), None);
        // the first manager in ALL order wins
        assert_eq!(
            super::missing_tool_in(&all, |m| m == Pkg || m == Pacman, |_| false),
            Some((Pacman, "pacman"))
        );
    }

    #[test]
    fn bsd_mount_listing_and_bootmethod() {
        let ufs = "/dev/gpt/rootfs\t/\tufs\trw\t1\t1\ndevfs\t/dev\tdevfs\trw\t0\t0\n/dev/gpt/efiesp\t/boot/efi\tmsdosfs\trw\t0\t0\n";
        assert_eq!(
            super::parse_mount_p_root(ufs),
            Some(("ufs".into(), "/dev/gpt/rootfs".into()))
        );
        let zfs =
            "zroot/ROOT/default\t/\tzfs\trw,noatime\t0\t0\nzroot/home\t/home\tzfs\trw\t0\t0\n";
        assert_eq!(
            super::parse_mount_p_root(zfs),
            Some(("zfs".into(), "zroot/ROOT/default".into()))
        );
        assert_eq!(super::parse_mount_p_root(""), None);
        assert_eq!(super::parse_mount_p_root("devfs /dev devfs rw 0 0\n"), None);
        assert_eq!(super::parse_bootmethod("UEFI\n"), "uefi");
        assert_eq!(super::parse_bootmethod("BIOS\n"), "bios");
        assert_eq!(super::parse_bootmethod(""), "unknown");
    }

    use super::*;

    #[test]
    fn os_release_parsing_and_families() {
        let cases = [
            ("ID=arch\nPRETTY_NAME=\"Arch Linux\"\n", "arch"),
            ("ID=cachyos\nID_LIKE=arch\n", "arch"),
            (
                "ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n",
                "debian",
            ),
            ("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", "debian"),
            ("ID=debian\n", "debian"),
            ("ID=fedora\n", "redhat"),
            ("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n", "redhat"),
            (
                "ID=opensuse-tumbleweed\nID_LIKE=\"opensuse suse\"\n",
                "suse",
            ),
            ("ID=void\n", "void"),
            ("ID=gentoo\n", "gentoo"),
            ("ID=alpine\n", "alpine"),
            ("", "other"),
        ];
        for (text, fam) in cases {
            assert_eq!(family(&parse_os_release(text)), fam, "for {text:?}");
        }
        let os = parse_os_release("ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n");
        assert_eq!(os.version_id, "24.04");
        assert_eq!(os.id_like, vec!["debian"]);
    }

    #[test]
    fn tiers_match_the_documented_scope() {
        for m in [PkgManager::Pacman, PkgManager::Apt, PkgManager::Dnf] {
            assert_eq!(m.tier(), 1);
        }
        for m in [PkgManager::Xbps, PkgManager::Zypper, PkgManager::Pkg] {
            assert_eq!(m.tier(), 2);
        }
        for m in [PkgManager::Portage, PkgManager::Nix] {
            assert_eq!(m.tier(), 3);
        }
    }

    #[test]
    fn detect_runs_everywhere_and_is_self_consistent() {
        let p = detect();
        assert!(!p.arch.is_empty());
        assert!(!p.init.is_empty());
        if let Some(n) = p.native_pkg_manager {
            assert_eq!(
                p.pkg_managers.first(),
                Some(&n),
                "native manager listed first"
            );
            assert_eq!(p.support_tier, n.tier());
        }
        assert!(p.render().contains("support tier"));
    }
}
