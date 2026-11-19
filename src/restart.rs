// yapacc — `restart`: what must be restarted after an upgrade?
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! A report, not an action: after an upgrade, programs that were already
//! running keep the old, now deleted, libraries mapped. This module finds
//! them in `/proc/<pid>/maps`, names the systemd unit they belong to when
//! there is one, and says whether the kernel calls for a reboot. It needs
//! no tool, restarts nothing and writes nothing.
//!
//! The parsers and the decisions are pure functions (text in, struct out);
//! only [`collect`] reads the machine.

use crate::error::{Error, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// What this method can and cannot see; printed with every report.
pub const LIMITS: &str = "Detects programs still mapping replaced files. Does not detect: interpreters holding replaced scripts, statically linked programs built against a replaced library, kernel live patches.";

/// Shown whenever some processes could not be read.
pub const ROOT_HINT: &str = "run as root to see every process";

/// Deleted files are only meaningful under the directories packages
/// install to; `/memfd:`, `/dev/`, `/tmp`, `/run`, `/var/`, `/home`, SysV
/// segments and `/[aio]` are all outside this list.
const WATCHED: [&str; 6] = ["/usr/", "/lib/", "/lib64/", "/bin/", "/sbin/", "/opt/"];

/// How many deleted files are shown per process or unit.
const EXAMPLES: usize = 3;

// ------------------------------------------------------------- parsers

/// The file of one `/proc/<pid>/maps` line, when that file was deleted and
/// lived where packages install. `None` for every other line.
pub fn deleted_mapping(line: &str) -> Option<&str> {
    // address perms offset dev inode pathname — the path may hold spaces
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        rest = &rest[end..];
    }
    let path = rest.trim().strip_suffix(" (deleted)")?;
    let watched = WATCHED.iter().any(|prefix| path.starts_with(prefix));
    (watched && !path.contains("/.cache/")).then_some(path)
}

/// Every distinct deleted file of one `maps` text, in order of appearance.
pub fn deleted_files(maps: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for path in maps.lines().filter_map(deleted_mapping) {
        if !found.iter().any(|f| f == path) {
            found.push(path.to_string());
        }
    }
    found
}

/// A systemd unit a process belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Unit {
    pub name: String,
    /// "system" or "user"
    pub scope: &'static str,
    /// for a user unit: whose service manager runs it
    pub user_uid: Option<u32>,
}

/// `user@1000.service` → 1000.
fn user_manager_uid(component: &str) -> Option<u32> {
    component
        .strip_prefix("user@")?
        .strip_suffix(".service")?
        .parse()
        .ok()
}

/// The innermost `*.service` of a `/proc/<pid>/cgroup` text, and whether it
/// is a system or a user unit. `None` when the process is in no service:
/// no systemd, a login session, or a program started from a desktop (those
/// sit in a `.scope` under `user@UID.service`, and restarting that unit
/// would end the whole session — the program is listed by name instead).
pub fn unit_of_cgroup(text: &str) -> Option<Unit> {
    // cgroup v2 is the line `0::PATH`; v1 keeps systemd's tree in the
    // `name=systemd` hierarchy.
    let path = text.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        let _id = fields.next()?;
        let controllers = fields.next()?;
        let path = fields.next()?;
        (controllers.is_empty() || controllers == "name=systemd").then_some(path)
    })?;
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let innermost = components.iter().rposition(|c| c.ends_with(".service"))?;
    let name = components[innermost];
    if user_manager_uid(name).is_some() {
        return None;
    }
    let user_uid = components[..innermost]
        .iter()
        .find_map(|c| user_manager_uid(c));
    Some(Unit {
        name: name.to_string(),
        scope: if user_uid.is_some() { "user" } else { "system" },
        user_uid,
    })
}

// -------------------------------------------------------------- kernel

/// The running kernel compared with the kernels installed on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// its modules are on disk and no other kernel is more recent
    Current,
    /// its module directory no longer exists
    ModulesGone,
    /// its modules are on disk, and this other kernel was written later
    OtherNewer(String),
    /// no kernel was found on disk to compare with
    Unknown,
}

/// Decide from the running release and the installed kernels, each given
/// as (directory name under the modules root, modification time).
pub fn kernel_state(running: &str, installed: &[(String, u64)]) -> Kernel {
    if running.is_empty() || installed.is_empty() {
        return Kernel::Unknown;
    }
    let Some(mine) = installed.iter().find(|k| k.0 == running) else {
        return Kernel::ModulesGone;
    };
    let newer = installed
        .iter()
        .filter(|k| k.0 != running && k.1 > mine.1)
        .max_by_key(|k| k.1);
    match newer {
        Some(other) => Kernel::OtherNewer(other.0.clone()),
        None => Kernel::Current,
    }
}

/// Read the answer of `dnf needs-restarting -r`: `Some(true)` reboot
/// needed, `Some(false)` not needed, `None` it did not answer the question
/// (dnf 4 without the plugin also exits 1, so the status alone is not
/// enough: the sentence must be there too).
pub fn dnf_says(code: i32, output: &str) -> Option<bool> {
    match code {
        0 => Some(false),
        1 if output.to_lowercase().contains("reboot is required") => Some(true),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reboot {
    /// "yes", "possibly", "no" or "unknown"
    pub needed: &'static str,
    pub reasons: Vec<String>,
    pub running_kernel: String,
    /// packages that asked for the reboot (`/run/reboot-required.pkgs`)
    pub packages: Vec<String>,
    /// what could not be determined
    pub unknown: Vec<String>,
}

/// Facts about the reboot question, gathered before [`reboot_verdict`].
#[derive(Clone, Debug, Default)]
pub struct RebootFacts {
    pub running_kernel: String,
    /// (name, mtime) of each kernel under the modules root
    pub installed_kernels: Vec<(String, u64)>,
    /// `/run/reboot-required` (Debian) or `/run/reboot-needed` (openSUSE) exists
    pub flag_file: bool,
    pub flag_packages: Vec<String>,
    /// (exit status, output) of `dnf needs-restarting -r`, when dnf exists
    pub dnf: Option<(i32, String)>,
}

pub fn reboot_verdict(f: &RebootFacts) -> Reboot {
    let mut reasons: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut certain = false;
    let mut possible = false;
    let mut dnf_unanswered = false;
    let kernel = kernel_state(&f.running_kernel, &f.installed_kernels);
    match &kernel {
        Kernel::Current => {}
        Kernel::ModulesGone => {
            certain = true;
            reasons.push(format!(
                "running kernel's modules are gone: reboot needed (running {}; new modules cannot be loaded until then)",
                f.running_kernel
            ));
        }
        Kernel::OtherNewer(other) => {
            possible = true;
            reasons.push(format!(
                "a newer kernel is installed: reboot to use it ({other} was written after the running {}; judged by date only, so a second kernel kept side by side also shows up here)",
                f.running_kernel
            ));
        }
        Kernel::Unknown => unknown.push(
            "kernel: no installed kernel found under /usr/lib/modules or /lib/modules to compare with (container, or a kernel installed elsewhere)"
                .into(),
        ),
    }
    if f.flag_file {
        certain = true;
        reasons.push("the distribution asks for a reboot (/run/reboot-required or /run/reboot-needed exists)".into());
    }
    if let Some((code, output)) = &f.dnf {
        match dnf_says(*code, output) {
            Some(true) => {
                certain = true;
                reasons.push("dnf needs-restarting -r says a reboot is required".into());
            }
            Some(false) => {}
            None => {
                dnf_unanswered = true;
                // dnf 5 as a normal user with no metadata cache says
                // `Cache-only enabled but no cache for repository ...`
                let said = output.lines().rev().map(str::trim).find(|l| !l.is_empty());
                unknown.push(match said {
                    Some(l) => format!(
                        "dnf needs-restarting -r did not answer (exit status {code}: {l}); the kernel comparison is all that was checked"
                    ),
                    None => format!("dnf needs-restarting -r did not answer (exit status {code})"),
                });
            }
        }
    }
    let needed = if certain {
        "yes"
    } else if possible {
        "possibly"
    } else if kernel == Kernel::Unknown || dnf_unanswered {
        // dnf was asked and gave no answer: do not claim "no"
        "unknown"
    } else {
        "no"
    };
    Reboot {
        needed,
        reasons,
        running_kernel: f.running_kernel.clone(),
        packages: f.flag_packages.clone(),
        unknown,
    }
}

// ------------------------------------------------------------ grouping

/// One process that still maps deleted files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// owner of `/proc/<pid>`; `None` when it could not be read
    pub uid: Option<u32>,
    pub unit: Option<Unit>,
    /// up to three examples
    pub deleted: Vec<String>,
    /// how many distinct deleted files it maps
    pub deleted_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Service {
    pub unit: String,
    /// "system" or "user"
    pub scope: &'static str,
    pub user_uid: Option<u32>,
    /// the exact command that restarts it
    pub command: String,
    pub pids: Vec<u32>,
    /// up to three deleted files its processes map
    pub deleted: Vec<String>,
    /// set when restarting this unit has a known side effect
    pub caution: Option<&'static str>,
}

/// Quote `s` for a POSIX shell when it holds anything a shell would touch
/// (unit names can contain `\x2d` escapes and `@`).
fn sh_quote(s: &str) -> String {
    // a control character (a cgroup may be named anything but `/` and a
    // line break) is written `$'..\x1b..'`, so the line stays printable
    if s.chars().any(char::is_control) {
        let esc = crate::configs::visible(&s.replace('\\', "\\\\").replace('\'', "\\'"));
        return format!("$'{esc}'");
    }
    let plain = !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@' | b':' | b'/' | b'+')
        });
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The exact command that restarts `unit`.
pub fn restart_command(unit: &Unit) -> String {
    match unit.scope {
        "user" => format!("systemctl --user restart {}", sh_quote(&unit.name)),
        _ => format!("sudo systemctl restart {}", sh_quote(&unit.name)),
    }
}

/// Units whose restart takes more than the unit down with it.
fn caution_for(unit: &str) -> Option<&'static str> {
    const SESSION: [&str; 6] = [
        "display-manager.service",
        "gdm.service",
        "sddm.service",
        "lightdm.service",
        "greetd.service",
        "systemd-logind.service",
    ];
    const BUS: [&str; 2] = ["dbus.service", "dbus-broker.service"];
    if SESSION.contains(&unit) {
        Some(
            "restarting it can end the graphical sessions that are open: save your work first, or reboot instead",
        )
    } else if BUS.contains(&unit) {
        Some(
            "restarting the message bus breaks the programs connected to it: reboot (or log out and in) instead",
        )
    } else {
        None
    }
}

/// Split the processes into services (one line per unit) and programs
/// (processes that belong to no unit). Both come back sorted.
pub fn group(procs: &[Proc]) -> (Vec<Service>, Vec<Proc>) {
    let mut units: BTreeMap<Unit, Service> = BTreeMap::new();
    let mut programs: Vec<Proc> = Vec::new();
    for p in procs {
        let Some(unit) = &p.unit else {
            programs.push(p.clone());
            continue;
        };
        let service = units.entry(unit.clone()).or_insert_with(|| Service {
            unit: unit.name.clone(),
            scope: unit.scope,
            user_uid: unit.user_uid,
            command: restart_command(unit),
            pids: Vec::new(),
            deleted: Vec::new(),
            caution: caution_for(&unit.name),
        });
        service.pids.push(p.pid);
        for file in &p.deleted {
            if service.deleted.len() < EXAMPLES && !service.deleted.contains(file) {
                service.deleted.push(file.clone());
            }
        }
    }
    let mut services: Vec<Service> = units.into_values().collect();
    for s in &mut services {
        s.pids.sort_unstable();
    }
    programs.sort_by(|a, b| a.name.cmp(&b.name).then(a.pid.cmp(&b.pid)));
    (services, programs)
}

// -------------------------------------------------------------- report

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub reboot: Reboot,
    pub services: Vec<Service>,
    pub programs: Vec<Proc>,
    /// processes whose maps were read
    pub examined_processes: usize,
    /// processes that could not be read (other users', when not root)
    pub unreadable_processes: usize,
    /// what to do about the unreadable ones; empty when there are none
    pub hint: String,
    /// set when processes could not be checked at all (FreeBSD without
    /// checkrestart): the service and program lists are then not printed,
    /// since "0" would read as "nothing to restart"
    pub process_note: String,
    pub limits: &'static str,
}

pub fn report(reboot: Reboot, procs: &[Proc], examined: usize, unreadable: usize) -> Report {
    let (services, programs) = group(procs);
    Report {
        reboot,
        services,
        programs,
        examined_processes: examined,
        unreadable_processes: unreadable,
        hint: if unreadable > 0 {
            ROOT_HINT.to_string()
        } else {
            String::new()
        },
        process_note: String::new(),
        limits: LIMITS,
    }
}

/// Services are matched to systemd units only. Without systemd a daemon that
/// needs a restart shows up under "Programs", and a "0" above would read as
/// "nothing to do", so say what the number means.
fn no_systemd_note(systemd: bool, services: usize, programs: usize) -> Option<&'static str> {
    (!systemd && services == 0 && programs > 0).then_some(
        "  (this machine does not run systemd, so processes are not matched to services: restart the daemons listed below with its init system, e.g. `rc-service NAME restart` on OpenRC or `sv restart NAME` on runit)\n",
    )
}

fn examples(files: &[String], total: usize) -> String {
    let more = total.saturating_sub(files.len());
    let shown: Vec<String> = files.iter().map(|f| crate::configs::visible(f)).collect();
    let mut s = shown.join(", ");
    if more > 0 {
        s.push_str(&format!(", … and {more} more"));
    }
    s
}

pub fn render(r: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("Reboot: {}\n", r.reboot.needed));
    if r.reboot.reasons.is_empty() && r.reboot.needed == "no" {
        s.push_str(&format!(
            "  running kernel {} still has its modules, no other kernel is more recent, no distribution flag is set\n",
            r.reboot.running_kernel
        ));
    }
    for reason in &r.reboot.reasons {
        s.push_str(&format!("  {reason}\n"));
    }
    if !r.reboot.packages.is_empty() {
        s.push_str(&format!(
            "  asked for by: {}\n",
            r.reboot.packages.join(", ")
        ));
    }
    for unknown in &r.reboot.unknown {
        s.push_str(&format!("  unknown: {unknown}\n"));
    }

    if !r.process_note.is_empty() {
        s.push_str(&format!("\nProcesses: {}\n", r.process_note));
        s.push_str(&format!("Limits: {}\n", r.limits));
        s.push_str("Nothing was restarted: `restart` only reports.\n");
        return s;
    }

    s.push_str(&format!("\nServices to restart: {}\n", r.services.len()));
    if let Some(note) = no_systemd_note(
        std::path::Path::new("/run/systemd/system").exists(),
        r.services.len(),
        r.programs.len(),
    ) {
        s.push_str(note);
    }
    for service in &r.services {
        let pids: Vec<String> = service.pids.iter().map(u32::to_string).collect();
        s.push_str(&format!(
            "  {} ({} unit, pid {})\n",
            crate::configs::visible(&service.unit),
            service.scope,
            pids.join(", ")
        ));
        s.push_str(&format!(
            "      still maps: {}\n",
            examples(&service.deleted, service.deleted.len())
        ));
        match (service.scope, service.user_uid) {
            ("user", Some(uid)) => s.push_str(&format!(
                "      {}   (as the user with uid {uid})\n",
                service.command
            )),
            _ => s.push_str(&format!("      {}\n", service.command)),
        }
        if let Some(caution) = service.caution {
            s.push_str(&format!("      caution: {caution}\n"));
        }
    }

    s.push_str(&format!("\nPrograms to restart: {}\n", r.programs.len()));
    for p in &r.programs {
        let owner = match p.uid {
            Some(uid) => format!("uid {uid}"),
            None => "uid unknown".to_string(),
        };
        s.push_str(&format!(
            "  {} (pid {}, {owner})\n",
            crate::configs::visible(&p.name),
            p.pid
        ));
        s.push_str(&format!(
            "      still maps: {}\n",
            examples(&p.deleted, p.deleted_count)
        ));
        if p.pid == 1 {
            s.push_str(
                "      pid 1 cannot be restarted: `sudo systemctl daemon-reexec` on systemd, otherwise reboot\n",
            );
        }
    }

    s.push_str(&format!("\nExamined {} process(es).", r.examined_processes));
    if r.unreadable_processes > 0 {
        s.push_str(&format!(
            " Not examined: {} process(es) that this user may not read — {}.",
            r.unreadable_processes, r.hint
        ));
    }
    s.push('\n');
    s.push_str(&format!("Limits: {}\n", r.limits));
    s.push_str("Nothing was restarted: `restart` only reports.\n");
    s
}

// ------------------------------------------------------------ FreeBSD

/// What this check can and cannot see on FreeBSD.
pub const BSD_LIMITS: &str = "Compares the running kernel with the installed one (freebsd-version -r and -k). Processes still using replaced libraries are only listed when sysutils/checkrestart is installed; its answer is shown as it printed it.";

/// The reboot question on FreeBSD, from `freebsd-version -r` (running
/// kernel), `-k` (installed kernel) and `-u` (userland). A kernel that was
/// updated on disk but is not the one running calls for a reboot; a userland
/// that differs from the installed kernel is a half-finished update. Pure.
pub fn bsd_reboot_verdict(running: &str, installed: &str, userland: &str) -> Reboot {
    let (running, installed, userland) = (running.trim(), installed.trim(), userland.trim());
    let mut reasons = Vec::new();
    let mut unknown = Vec::new();
    let needed = if running.is_empty() || installed.is_empty() {
        unknown.push(
            "freebsd-version did not report the running and the installed kernel".to_string(),
        );
        "unknown"
    } else if running != installed {
        reasons.push(format!(
            "the installed kernel is {installed} but {running} is running: reboot to use it"
        ));
        "yes"
    } else {
        reasons.push(format!(
            "the running kernel {running} is the installed one (freebsd-version -r and -k agree)"
        ));
        "no"
    };
    if !userland.is_empty() && !installed.is_empty() && userland != installed {
        reasons.push(format!(
            "the userland is {userland} but the installed kernel is {installed}: an update may be half done (`freebsd-update install` finishes it after the reboot)"
        ));
    }
    Reboot {
        needed,
        reasons,
        running_kernel: running.to_string(),
        packages: Vec::new(),
        unknown,
    }
}

fn run_text(cmd: &str, args: &[&str]) -> Option<(i32, String)> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.code()?, text))
}

/// What to say about `checkrestart`'s output: its own lines when it listed
/// processes, an explicit "none" when it printed nothing (an empty answer must
/// not read as an error), and a warning that a non-root run only sees the
/// caller's own processes. Pure.
pub fn checkrestart_note(output: &str, root: bool) -> String {
    let lines: Vec<String> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("    {}", crate::configs::visible(l)))
        .collect();
    let scope = if root {
        ""
    } else {
        " Run as root to see other users' processes."
    };
    if lines.is_empty() {
        format!("checkrestart lists no process that still uses replaced libraries.{scope}")
    } else {
        format!(
            "checkrestart lists these processes (restart them):\n{}\n{}",
            lines.join("\n"),
            scope.trim_start()
        )
        .trim_end()
        .to_string()
    }
}

/// The FreeBSD report: kernel verdict, plus checkrestart's own words about
/// processes when that tool is installed.
fn collect_bsd() -> Report {
    let ver = |flag: &str| {
        run_text("freebsd-version", &[flag])
            .filter(|(c, _)| *c == 0)
            .map(|(_, t)| t.trim().to_string())
            .unwrap_or_default()
    };
    let reboot = bsd_reboot_verdict(&ver("-r"), &ver("-k"), &ver("-u"));
    let mut r = report(reboot, &[], 0, 0);
    r.limits = BSD_LIMITS;
    r.process_note = if crate::util::have("checkrestart") {
        match run_text("checkrestart", &[]) {
            Some((_, text)) => checkrestart_note(&text, crate::util::is_root()),
            None => "not checked. checkrestart could not be run".to_string(),
        }
    } else {
        "not checked. Install sysutils/checkrestart (`pkg install checkrestart`) to list the processes that still use replaced libraries; FreeBSD has no /proc to read them from.".to_string()
    };
    r
}

// ------------------------------------------- gathering (reads only)

fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Kernels under `root`: directories that hold a module tree. A directory
/// left behind with only stray files (dkms leftovers) is not a kernel.
fn kernels_under(root: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        if !path.join("kernel").is_dir() && !path.join("modules.dep").is_file() {
            continue;
        }
        found.push((
            entry.file_name().to_string_lossy().to_string(),
            mtime_secs(&meta),
        ));
    }
    found.sort();
    found
}

fn gather_reboot() -> RebootFacts {
    let running_kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    // `/lib/modules` is the same directory on merged-/usr systems
    let installed_kernels = ["/usr/lib/modules", "/lib/modules"]
        .iter()
        .map(|root| kernels_under(Path::new(root)))
        .find(|k| !k.is_empty())
        .unwrap_or_default();
    let flag_packages = std::fs::read_to_string("/run/reboot-required.pkgs")
        .map(|text| {
            let mut names: Vec<String> = text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_default();
    // optional: `-C` keeps dnf on its cache, so nothing is downloaded
    let dnf = if crate::util::have("dnf") {
        std::process::Command::new("dnf")
            // `--disablerepo=*`: the question is answered from the rpm
            // database; loading repositories only adds ways to fail (dnf 5
            // as root also dies with "Cache-only enabled but no cache for
            // repository" on a machine that never refreshed metadata)
            .args(["-C", "--disablerepo=*", "needs-restarting", "-r"])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|out| {
                let code = out.status.code()?;
                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                Some((code, text))
            })
    } else {
        None
    };
    RebootFacts {
        running_kernel,
        installed_kernels,
        // Debian/Ubuntu write /run/reboot-required, openSUSE (zypper's needreboot plugin) /run/reboot-needed
        flag_file: ["/run/reboot-required", "/run/reboot-needed"]
            .iter()
            .any(|p| Path::new(p).exists()),
        flag_packages,
        dnf,
    }
}

/// Every process that maps deleted files, how many processes were read,
/// and how many could not be.
fn gather_processes() -> Result<(Vec<Proc>, usize, usize)> {
    use std::os::unix::fs::MetadataExt;
    let entries = std::fs::read_dir("/proc")
        .map_err(|e| Error::unavailable(format!("cannot read /proc: {e}")))?;
    let mut procs: Vec<Proc> = Vec::new();
    let mut examined = 0usize;
    let mut unreadable = 0usize;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let base = entry.path();
        let maps = match std::fs::read(base.join("maps")) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            // the process ended while we were looking: nothing to report
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(3) => {
                continue;
            }
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        examined += 1;
        let files = deleted_files(&maps);
        if files.is_empty() {
            continue;
        }
        let name = std::fs::read_to_string(base.join("comm"))
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "?".to_string());
        procs.push(Proc {
            pid,
            name,
            uid: std::fs::metadata(&base).ok().map(|m| m.uid()),
            unit: std::fs::read_to_string(base.join("cgroup"))
                .ok()
                .and_then(|text| unit_of_cgroup(&text)),
            deleted_count: files.len(),
            deleted: files.into_iter().take(EXAMPLES).collect(),
        });
    }
    procs.sort_by_key(|p| p.pid);
    Ok((procs, examined, unreadable))
}

/// Gather the report from this machine. Reads only; creates nothing.
pub fn collect() -> Result<Report> {
    if !Path::new("/proc/self/maps").exists() && crate::util::have("freebsd-version") {
        return Ok(collect_bsd());
    }
    if !Path::new("/proc/self/maps").exists() {
        return Err(Error::unavailable(
            "yapacc restart needs /proc (the Linux process filesystem), which is not available on this system",
        ));
    }
    let (procs, examined, unreadable) = gather_processes()?;
    Ok(report(
        reboot_verdict(&gather_reboot()),
        &procs,
        examined,
        unreadable,
    ))
}

/// `yapacc restart [--json]`: print the report. It never restarts anything.
pub fn run(json: bool) -> Result<()> {
    let report = collect()?;
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => return Err(Error::from(format!("json serialization failed: {e}"))),
        }
    } else {
        print!("{}", render(&report));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freebsd_kernel_verdict_follows_freebsd_version() {
        let same = bsd_reboot_verdict("15.1-RELEASE-p6\n", "15.1-RELEASE-p6", "15.1-RELEASE-p6");
        assert_eq!(same.needed, "no");
        assert_eq!(same.running_kernel, "15.1-RELEASE-p6");
        let behind = bsd_reboot_verdict("15.1-RELEASE-p6", "15.1-RELEASE-p7", "15.1-RELEASE-p6");
        assert_eq!(behind.needed, "yes");
        assert!(behind.reasons[0].contains("p7") && behind.reasons[0].contains("p6 is running"));
        // new kernel booted, userland not yet installed: no reboot, but say so
        let half = bsd_reboot_verdict("15.1-RELEASE-p7", "15.1-RELEASE-p7", "15.1-RELEASE-p6");
        assert_eq!(half.needed, "no");
        assert!(
            half.reasons
                .iter()
                .any(|r| r.contains("freebsd-update install"))
        );
        let none = bsd_reboot_verdict("", "", "");
        assert_eq!(none.needed, "unknown");
        assert!(!none.unknown.is_empty());
    }

    #[test]
    fn checkrestart_output_is_explained() {
        let none = checkrestart_note("", true);
        assert!(none.contains("no process"));
        assert!(!none.contains("Run as root"));
        assert!(checkrestart_note("\n", false).contains("Run as root"));
        let some = checkrestart_note(
            "  PID JID USER COMMAND WHY\n 2278 0 tester curl .so\n",
            false,
        );
        assert!(some.contains("    2278 0 tester curl .so"));
        assert!(some.contains("restart them"));
        assert!(some.ends_with("Run as root to see other users' processes."));
    }

    #[test]
    fn a_report_that_could_not_check_processes_does_not_print_zero_services() {
        let mut r = report(bsd_reboot_verdict("1", "1", "1"), &[], 0, 0);
        r.process_note = "not checked. Install checkrestart.".into();
        let text = render(&r);
        assert!(text.contains("Processes: not checked. Install checkrestart."));
        assert!(!text.contains("Services to restart: 0"));
    }

    #[test]
    fn without_systemd_a_zero_service_count_is_explained() {
        assert!(no_systemd_note(false, 0, 3).unwrap().contains("rc-service"));
        assert_eq!(no_systemd_note(true, 0, 3), None);
        assert_eq!(no_systemd_note(false, 0, 0), None);
        assert_eq!(no_systemd_note(false, 2, 3), None);
    }

    const MAPS: &str = "\
55d0c8a00000-55d0c8a2c000 r--p 00000000 fe:01 1311442                    /usr/bin/sshd
7f1c2a000000-7f1c2a1b0000 r-xp 00000000 fe:01 1234567                    /usr/lib/libcrypto.so.3 (deleted)
7f1c2a1b0000-7f1c2a1c0000 r--p 001b0000 fe:01 1234567                    /usr/lib/libcrypto.so.3 (deleted)
7f1c2b000000-7f1c2b010000 r-xp 00000000 fe:01 1234568                    /lib64/ld-linux-x86-64.so.2 (deleted)
7f1c2c000000-7f1c2c010000 r-xp 00000000 fe:01 1234569                    /opt/My App/lib/libfoo.so (deleted)
7f1c2d000000-7f1c2d010000 rw-s 00000000 00:01 2053                       /memfd:pulseaudio (deleted)
7f1c2e000000-7f1c2e010000 rw-s 00000000 00:01 2054                       /SYSV00000000 (deleted)
7f1c2f000000-7f1c2f010000 rw-s 00000000 00:0d 2055                       /[aio] (deleted)
7f1c30000000-7f1c30010000 rw-s 00000000 00:05 2056                       /dev/shm/x (deleted)
7f1c31000000-7f1c31010000 r--s 00000000 fe:02 2057                       /home/u/.cache/mesa/index (deleted)
7f1c32000000-7f1c32010000 r--s 00000000 fe:02 2058                       /opt/app/.cache/blob.so (deleted)
7f1c33000000-7f1c33010000 r--s 00000000 fe:02 2059                       /tmp/x.so (deleted)
7f1c34000000-7f1c34010000 r--s 00000000 fe:02 2060                       /var/lib/x.so (deleted)
7f1c35000000-7f1c35010000 r--s 00000000 fe:02 2061                       /run/x.so (deleted)
7ffd4a5e0000-7ffd4a601000 rw-p 00000000 00:00 0                          [stack]
7f1c36000000-7f1c36010000 rw-p 00000000 00:00 0
";

    #[test]
    fn only_deleted_files_from_package_directories_count() {
        assert_eq!(
            deleted_files(MAPS),
            [
                "/usr/lib/libcrypto.so.3",
                "/lib64/ld-linux-x86-64.so.2",
                "/opt/My App/lib/libfoo.so"
            ]
        );
        assert_eq!(deleted_mapping(""), None);
        assert_eq!(deleted_mapping("garbage"), None);
        assert_eq!(
            deleted_mapping("1-2 r-xp 0 fe:01 7 /usr/lib/libz.so.1 (deleted)"),
            Some("/usr/lib/libz.so.1")
        );
        // present on disk: not a candidate
        assert_eq!(
            deleted_mapping("1-2 r-xp 0 fe:01 7 /usr/lib/libz.so.1"),
            None
        );
    }

    #[test]
    fn the_innermost_service_is_the_unit() {
        assert_eq!(
            unit_of_cgroup("0::/system.slice/sshd.service\n"),
            Some(Unit {
                name: "sshd.service".into(),
                scope: "system",
                user_uid: None
            })
        );
        assert_eq!(
            unit_of_cgroup(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/foo.service\n"
            ),
            Some(Unit {
                name: "foo.service".into(),
                scope: "user",
                user_uid: Some(1000)
            })
        );
        // cgroup v1: systemd's own hierarchy is the one that counts
        assert_eq!(
            unit_of_cgroup(
                "12:pids:/system.slice/cron.service\n1:name=systemd:/system.slice/cron.service\n"
            )
            .map(|u| u.name),
            Some("cron.service".to_string())
        );
    }

    #[test]
    fn sessions_scopes_and_non_systemd_machines_have_no_unit() {
        for text in [
            "0::/user.slice/user-1000.slice/session-2.scope\n",
            // a desktop program: restarting user@1000.service would end the session
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox-1234.scope\n",
            "0::/user.slice/user-1000.slice/user@1000.service/init.scope\n",
            "0::/init.scope\n",
            "0::/\n",
            "0::/openrc.sshd\n",
            "3:cpu:/docker/abc\n",
            "",
        ] {
            assert_eq!(unit_of_cgroup(text), None, "{text:?}");
        }
    }

    fn kernels(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(n, t)| (n.to_string(), *t)).collect()
    }

    #[test]
    fn the_kernel_decision_follows_the_module_directories() {
        let run = "6.12.1-arch1-1";
        assert_eq!(kernel_state(run, &kernels(&[(run, 100)])), Kernel::Current);
        assert_eq!(
            kernel_state(run, &kernels(&[(run, 100), ("6.6.60-1-lts", 50)])),
            Kernel::Current
        );
        assert_eq!(
            kernel_state(run, &kernels(&[("6.12.4-arch1-1", 200)])),
            Kernel::ModulesGone
        );
        assert_eq!(
            kernel_state(
                run,
                &kernels(&[(run, 100), ("6.12.4-arch1-1", 200), ("6.6.60-1-lts", 150)])
            ),
            Kernel::OtherNewer("6.12.4-arch1-1".into())
        );
        // nothing to compare with: unknown, never "no reboot"
        assert_eq!(kernel_state(run, &[]), Kernel::Unknown);
        assert_eq!(kernel_state("", &kernels(&[(run, 100)])), Kernel::Unknown);
    }

    #[test]
    fn the_reboot_verdict_names_its_reasons_and_its_unknowns() {
        let run = "6.12.1-arch1-1";
        let base = RebootFacts {
            running_kernel: run.into(),
            installed_kernels: kernels(&[(run, 100)]),
            ..RebootFacts::default()
        };
        let v = reboot_verdict(&base);
        assert_eq!((v.needed, v.reasons.len(), v.unknown.len()), ("no", 0, 0));

        let gone = reboot_verdict(&RebootFacts {
            installed_kernels: kernels(&[("6.12.4-arch1-1", 200)]),
            ..base.clone()
        });
        assert_eq!(gone.needed, "yes");
        assert!(
            gone.reasons[0].starts_with("running kernel's modules are gone: reboot needed"),
            "{:?}",
            gone.reasons
        );

        let newer = reboot_verdict(&RebootFacts {
            installed_kernels: kernels(&[(run, 100), ("6.12.4-arch1-1", 200)]),
            ..base.clone()
        });
        assert_eq!(newer.needed, "possibly");
        assert!(
            newer.reasons[0].starts_with("a newer kernel is installed: reboot to use it"),
            "{:?}",
            newer.reasons
        );

        let flagged = reboot_verdict(&RebootFacts {
            flag_file: true,
            flag_packages: vec!["linux-image-6.8.0-50-generic".into()],
            ..base.clone()
        });
        assert_eq!(flagged.needed, "yes");
        assert_eq!(flagged.packages, ["linux-image-6.8.0-50-generic"]);

        let unknown = reboot_verdict(&RebootFacts {
            installed_kernels: vec![],
            ..base.clone()
        });
        assert_eq!(unknown.needed, "unknown");
        assert_eq!(unknown.unknown.len(), 1);

        let dnf_yes = reboot_verdict(&RebootFacts {
            dnf: Some((
                1,
                "Reboot is required to fully utilize these updates.\n".into(),
            )),
            ..base.clone()
        });
        assert_eq!(dnf_yes.needed, "yes");
        // dnf 4 without the plugin exits 1 too: that is not an answer
        let dnf_silent = reboot_verdict(&RebootFacts {
            dnf: Some((1, "No such command: needs-restarting.\n".into())),
            ..base
        });
        assert_eq!(dnf_silent.needed, "unknown");
        assert_eq!(dnf_silent.unknown.len(), 1);
        assert_eq!(dnf_says(0, "Reboot should not be necessary."), Some(false));
    }

    fn process(pid: u32, name: &str, unit: Option<Unit>) -> Proc {
        Proc {
            pid,
            name: name.into(),
            uid: Some(1000),
            unit,
            deleted: vec!["/usr/lib/libssl.so.3".into()],
            deleted_count: 5,
        }
    }

    /// A cgroup may be named anything but `/` and a line break, and a
    /// container's root names its own: such a name went to the terminal as
    /// it was, in the unit line and in the command.
    #[test]
    fn a_unit_name_with_an_escape_sequence_is_printed_visibly() {
        let unit = unit_of_cgroup("0::/system.slice/evil\x1b[2J\x1b[31m.service\n").unwrap();
        assert!(unit.name.contains('\x1b'));
        let reboot = reboot_verdict(&RebootFacts {
            running_kernel: "6.12.1".into(),
            installed_kernels: kernels(&[("6.12.1", 100)]),
            ..RebootFacts::default()
        });
        let r = report(reboot, &[process(70, "evil", Some(unit))], 1, 0);
        let text = render(&r);
        assert!(!text.chars().any(|c| c == '\x1b'), "{text:?}");
        assert!(text.contains("  evil\\x1b[2J\\x1b[31m.service (system unit, pid 70)"));
        assert!(text.contains("sudo systemctl restart $'evil\\x1b[2J\\x1b[31m.service'"));
        // an ordinary name is quoted as before
        assert_eq!(sh_quote("sshd.service"), "sshd.service");
        assert_eq!(
            sh_quote("dev-disk-by\\x2did.service"),
            "'dev-disk-by\\x2did.service'"
        );
    }

    fn sample() -> Report {
        let sshd = Unit {
            name: "sshd.service".into(),
            scope: "system",
            user_uid: None,
        };
        let pipewire = Unit {
            name: "pipewire.service".into(),
            scope: "user",
            user_uid: Some(1000),
        };
        let odd = Unit {
            name: "dev-disk-by\\x2did.service".into(),
            scope: "system",
            user_uid: None,
        };
        let procs = [
            process(900, "sshd", Some(sshd.clone())),
            process(4321, "firefox", None),
            process(800, "sshd", Some(sshd)),
            process(1500, "pipewire", Some(pipewire)),
            process(70, "odd", Some(odd)),
            process(
                60,
                "dbus-broker",
                Some(Unit {
                    name: "dbus-broker.service".into(),
                    scope: "system",
                    user_uid: None,
                }),
            ),
        ];
        let reboot = reboot_verdict(&RebootFacts {
            running_kernel: "6.12.1".into(),
            installed_kernels: kernels(&[("6.12.1", 100)]),
            ..RebootFacts::default()
        });
        report(reboot, &procs, 240, 12)
    }

    #[test]
    fn processes_are_grouped_by_unit_with_the_exact_command() {
        let r = sample();
        assert_eq!(r.programs.len(), 1);
        assert_eq!(r.programs[0].name, "firefox");
        let sshd = r
            .services
            .iter()
            .find(|s| s.unit == "sshd.service")
            .unwrap();
        assert_eq!(sshd.pids, [800, 900]);
        assert_eq!(sshd.command, "sudo systemctl restart sshd.service");
        assert_eq!(sshd.deleted, ["/usr/lib/libssl.so.3"]);
        let pw = r
            .services
            .iter()
            .find(|s| s.unit == "pipewire.service")
            .unwrap();
        assert_eq!(pw.command, "systemctl --user restart pipewire.service");
        let odd = r
            .services
            .iter()
            .find(|s| s.unit.starts_with("dev-"))
            .unwrap();
        assert_eq!(
            odd.command,
            "sudo systemctl restart 'dev-disk-by\\x2did.service'"
        );
        let bus = r
            .services
            .iter()
            .find(|s| s.unit == "dbus-broker.service")
            .unwrap();
        assert!(bus.caution.is_some());
        assert!(sshd.caution.is_none());
    }

    #[test]
    fn the_report_states_what_it_did_not_see() {
        let out = render(&sample());
        assert!(out.starts_with("Reboot: no\n"), "{out}");
        assert!(out.contains("Services to restart: 4"), "{out}");
        assert!(
            out.contains("      sudo systemctl restart sshd.service\n"),
            "{out}"
        );
        assert!(
            out.contains("systemctl --user restart pipewire.service   (as the user with uid 1000)"),
            "{out}"
        );
        assert!(out.contains("Programs to restart: 1"), "{out}");
        assert!(out.contains("firefox (pid 4321, uid 1000)"), "{out}");
        assert!(out.contains("/usr/lib/libssl.so.3, … and 4 more"), "{out}");
        assert!(out.contains("Not examined: 12 process(es)"), "{out}");
        assert!(out.contains("run as root to see every process"), "{out}");
        assert!(out.contains(LIMITS), "{out}");
        assert!(out.contains("Nothing was restarted"), "{out}");

        let quiet = render(&report(sample().reboot, &[], 10, 0));
        assert!(!quiet.contains("Not examined"), "{quiet}");
        assert!(quiet.contains("Services to restart: 0"), "{quiet}");
    }

    #[test]
    fn json_is_one_object() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["reboot"]["needed"], "no");
        assert_eq!(v["unreadable_processes"], 12);
        assert_eq!(v["hint"], ROOT_HINT);
        assert_eq!(v["programs"][0]["unit"], serde_json::Value::Null);
        assert_eq!(v["services"].as_array().unwrap().len(), 4);
        assert_eq!(v["limits"], LIMITS);
    }
}
