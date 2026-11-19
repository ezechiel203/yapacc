// yapacc — gated changes through the native package manager
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! The write side for every native manager: plan → show → gate → apply.
//!
//! yapacc never changes the system itself here. A change is the native
//! manager's own command, shown in full before it runs, preceded by the
//! native manager's own simulation where it has one. Nothing is recorded
//! by yapacc: the native log is the record (STRATEGY C1), and `timeline`
//! and `undo` read it from there.

use crate::error::{Error, Result};
use crate::platform::PkgManager;
use std::process::{Command, Stdio};

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Install(Vec<String>),
    Remove(Vec<String>),
    Upgrade,
}

impl Change {
    pub fn describe(&self) -> String {
        match self {
            Change::Install(p) => format!("install {}", p.join(" ")),
            Change::Remove(p) => format!("remove {}", p.join(" ")),
            Change::Upgrade => "upgrade the system".into(),
        }
    }
}

/// The native commands for a change. Commands are argument vectors; no
/// shell is involved and nothing is interpolated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePlan {
    pub manager: PkgManager,
    /// commands that make the change, in order (without `sudo`/`doas`)
    pub steps: Vec<Vec<String>>,
    /// the manager's own simulation of the change, if it has one
    pub preview: Option<Vec<String>>,
    /// the simulation itself needs root (dnf, zypper)
    pub preview_needs_root: bool,
    pub notes: Vec<String>,
}

/// A Debian architecture name as dpkg writes it (`amd64`, `i386`,
/// `armhf`, `any`): lower-case letters, digits and `-`. Pure.
pub fn valid_deb_arch(arch: &str) -> bool {
    !arch.is_empty()
        && arch.len() <= 32
        && !arch.starts_with('-')
        && arch
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A package as apt names it on a multiarch system: `name`, or `name:arch`
/// for one architecture of it (`libpng16-16t64:i386`). apt's history, its
/// simulation and `apt-get -s autoremove` write the second form, and
/// leaving the architecture out makes apt act on the native package
/// instead. Pure.
pub fn valid_apt_target(target: &str) -> bool {
    match target.split_once(':') {
        Some((name, arch)) => crate::util::valid_pkg_name(name) && valid_deb_arch(arch),
        None => crate::util::valid_pkg_name(target),
    }
}

/// Whether `name` may be passed to manager `m` as a package argument: the
/// `name:arch` form exists only on apt. Pure.
pub fn valid_target(m: PkgManager, name: &str) -> bool {
    match m {
        PkgManager::Apt => valid_apt_target(name),
        _ => crate::util::valid_pkg_name(name),
    }
}

fn argv(parts: &[&str], pkgs: &[String]) -> Vec<String> {
    parts
        .iter()
        .map(|s| s.to_string())
        .chain(pkgs.iter().cloned())
        .collect()
}

/// Build the plan. `os_id` is the `ID` of os-release (zypper needs it to
/// choose between `update` and `dup`). Pure.
pub fn plan(m: PkgManager, change: &Change, os_id: &str) -> Result<NativePlan> {
    let pkgs: &[String] = match change {
        Change::Install(p) | Change::Remove(p) => {
            if p.is_empty() {
                return Err(Error::usage("no package named"));
            }
            for name in p {
                if !valid_target(m, name) {
                    return Err(Error::usage(format!(
                        "'{name}' is not a valid package name"
                    )));
                }
            }
            p
        }
        Change::Upgrade => &[],
    };
    let mut notes = Vec::new();
    let mut preview_needs_root = false;
    let (steps, preview): (Vec<Vec<String>>, Option<Vec<String>>) = match (m, change) {
        (PkgManager::Pacman, Change::Install(_)) => (
            vec![argv(
                &["pacman", "-S", "--needed", "--noconfirm", "--"],
                pkgs,
            )],
            Some(argv(
                &[
                    "pacman",
                    "-S",
                    "--needed",
                    "--print",
                    "--print-format",
                    "%r/%n %v",
                    "--",
                ],
                pkgs,
            )),
        ),
        (PkgManager::Pacman, Change::Remove(_)) => (
            vec![argv(&["pacman", "-Rs", "--noconfirm", "--"], pkgs)],
            Some(argv(
                &["pacman", "-Rs", "--print", "--print-format", "%n %v", "--"],
                pkgs,
            )),
        ),
        (PkgManager::Pacman, Change::Upgrade) => {
            notes.push("the preview lists what was pending at the last refresh; the upgrade refreshes first".into());
            (
                vec![argv(&["pacman", "-Syu", "--noconfirm"], &[])],
                Some(argv(&["pacman", "-Qu"], &[])),
            )
        }
        // apt's simulations keep `-y`: apt then fails in the simulation
        // where the real run would ("Essential packages were removed and -y
        // was used", "Held packages were changed and -y was used"; observed
        // with apt 2.6 and 3.0). Without it those passed and the real run
        // failed after the gate.
        (PkgManager::Apt, Change::Install(_)) => (
            vec![argv(&["apt-get", "install", "-y", "--"], pkgs)],
            Some(argv(&["apt-get", "-s", "-y", "install", "--"], pkgs)),
        ),
        (PkgManager::Apt, Change::Remove(_)) => (
            vec![argv(&["apt-get", "remove", "-y", "--"], pkgs)],
            Some(argv(&["apt-get", "-s", "-y", "remove", "--"], pkgs)),
        ),
        (PkgManager::Apt, Change::Upgrade) => {
            notes.push("the preview uses the index from the last `apt-get update`; the upgrade refreshes first".into());
            (
                vec![
                    argv(&["apt-get", "update"], &[]),
                    argv(&["apt-get", "upgrade", "--with-new-pkgs", "-y"], &[]),
                ],
                Some(argv(
                    &["apt-get", "-s", "-y", "upgrade", "--with-new-pkgs"],
                    &[],
                )),
            )
        }
        (PkgManager::Dnf, _) => {
            preview_needs_root = true;
            notes.push("dnf can only simulate as root: the preview is `--assumeno`, which resolves, prints the transaction and stops".into());
            match change {
                Change::Install(_) => (
                    vec![argv(&["dnf", "install", "-y", "--"], pkgs)],
                    Some(argv(&["dnf", "install", "--assumeno", "--"], pkgs)),
                ),
                Change::Remove(_) => (
                    vec![argv(&["dnf", "remove", "-y", "--"], pkgs)],
                    Some(argv(&["dnf", "remove", "--assumeno", "--"], pkgs)),
                ),
                Change::Upgrade => (
                    vec![argv(&["dnf", "upgrade", "--refresh", "-y"], &[])],
                    Some(argv(&["dnf", "upgrade", "--assumeno"], &[])),
                ),
            }
        }
        (PkgManager::Zypper, _) => {
            preview_needs_root = true;
            let z = ["zypper", "--non-interactive"];
            match change {
                Change::Install(_) => (
                    vec![argv(&[z[0], z[1], "install", "--"], pkgs)],
                    Some(argv(&[z[0], z[1], "install", "--dry-run", "--"], pkgs)),
                ),
                Change::Remove(_) => (
                    vec![argv(&[z[0], z[1], "remove", "--"], pkgs)],
                    Some(argv(&[z[0], z[1], "remove", "--dry-run", "--"], pkgs)),
                ),
                Change::Upgrade => {
                    // rolling openSUSE must use a distribution upgrade
                    let rolling = os_id.contains("tumbleweed")
                        || os_id.contains("microos")
                        || os_id.contains("slowroll");
                    let verb = if rolling { "dist-upgrade" } else { "update" };
                    if rolling {
                        notes.push("rolling openSUSE: using `zypper dist-upgrade`, as the distribution requires".into());
                    }
                    (
                        vec![argv(&[z[0], z[1], verb], &[])],
                        Some(argv(&[z[0], z[1], verb, "--dry-run"], &[])),
                    )
                }
            }
        }
        (PkgManager::Apk, Change::Install(_)) => (
            vec![argv(&["apk", "add", "--"], pkgs)],
            Some(argv(
                &["apk", "add", "--simulate", "--no-progress", "--"],
                pkgs,
            )),
        ),
        (PkgManager::Apk, Change::Remove(_)) => (
            vec![argv(&["apk", "del", "--"], pkgs)],
            Some(argv(
                &["apk", "del", "--simulate", "--no-progress", "--"],
                pkgs,
            )),
        ),
        (PkgManager::Apk, Change::Upgrade) => (
            vec![
                argv(&["apk", "update"], &[]),
                argv(&["apk", "upgrade"], &[]),
            ],
            Some(argv(
                &["apk", "upgrade", "--simulate", "--no-progress"],
                &[],
            )),
        ),
        (PkgManager::Xbps, Change::Install(_)) => (
            vec![argv(&["xbps-install", "-y", "--"], pkgs)],
            Some(argv(&["xbps-install", "-n", "--"], pkgs)),
        ),
        (PkgManager::Xbps, Change::Remove(_)) => (
            vec![argv(&["xbps-remove", "-y", "--"], pkgs)],
            Some(argv(&["xbps-remove", "-n", "--"], pkgs)),
        ),
        (PkgManager::Xbps, Change::Upgrade) => {
            notes.push(
                "if xbps asks to update itself first, run `xbps-install -u xbps` and repeat".into(),
            );
            (
                vec![argv(&["xbps-install", "-Syu"], &[])],
                Some(argv(&["xbps-install", "-un"], &[])),
            )
        }
        (PkgManager::Pkg, Change::Install(_)) => (
            vec![argv(&["pkg", "install", "-y", "--"], pkgs)],
            Some(argv(&["pkg", "install", "-n", "--"], pkgs)),
        ),
        (PkgManager::Pkg, Change::Remove(_)) => (
            vec![argv(&["pkg", "delete", "-y", "--"], pkgs)],
            Some(argv(&["pkg", "delete", "-n", "--"], pkgs)),
        ),
        (PkgManager::Pkg, Change::Upgrade) => (
            vec![argv(&["pkg", "upgrade", "-y"], &[])],
            Some(argv(&["pkg", "upgrade", "-n"], &[])),
        ),
        (PkgManager::Portage | PkgManager::Nix, _) => {
            return Err(Error::unavailable(format!(
                "changes through yapacc are not offered for {}: its transactions depend on configuration (USE flags, profiles, generations) that yapacc does not model. Use {} directly; yapacc's read commands still work beside it.",
                m.name(),
                m.name()
            )));
        }
    };
    Ok(NativePlan {
        manager: m,
        steps,
        preview,
        preview_needs_root,
        notes,
    })
}

/// The native "refresh the repository indexes" command.
pub fn refresh_argv(m: PkgManager) -> Result<Vec<String>> {
    let parts: &[&str] = match m {
        PkgManager::Pacman => &["pacman", "-Sy"],
        PkgManager::Apt => &["apt-get", "update"],
        PkgManager::Dnf => &["dnf", "makecache", "--refresh"],
        PkgManager::Zypper => &["zypper", "--non-interactive", "refresh"],
        PkgManager::Apk => &["apk", "update"],
        PkgManager::Xbps => &["xbps-install", "-S"],
        PkgManager::Pkg => &["pkg", "update"],
        PkgManager::Portage | PkgManager::Nix => {
            return Err(Error::unavailable(format!(
                "refresh through yapacc is not offered for {}; use it directly",
                m.name()
            )));
        }
    };
    Ok(argv(parts, &[]))
}

/// Refresh the repository indexes after the gate. Installs nothing.
pub fn refresh(m: PkgManager, g: GateFlags) -> Result<()> {
    let cmd = refresh_argv(m)?;
    let root = elevation_for(g);
    let shown_prefix = root
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| vec!["<root>".into()]);
    println!("Will run:\n  {}", shown(&shown_prefix, &cmd));
    if m == PkgManager::Pacman {
        println!(
            "note: on Arch, refreshing without upgrading and then installing a package is a partial upgrade; follow with `yapacc upgrade`."
        );
    }
    if g.dryrun {
        println!("\n{}", crate::prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    gate("Proceed", g)?;
    match run_attached(&root?, &cmd, true)? {
        0 => Ok(()),
        code => Err(Error::from(format!("{} exited with status {code}", cmd[0]))),
    }
}

/// Whether this process has a controlling terminal a password could be typed
/// on (`/dev/tty` cannot be opened without one).
fn has_terminal() -> bool {
    std::fs::File::open("/dev/tty").is_ok()
}

/// `elevation`, but when the run must never wait for a password (`--script`,
/// or no terminal to type one on) the tool is told so (`sudo -n`, `doas -n`):
/// otherwise some doas builds (FreeBSD, no tty) wait forever.
pub fn elevation_for(g: GateFlags) -> Result<Vec<String>> {
    Ok(never_ask(elevation()?, g.script || !has_terminal()))
}

/// `sudo`/`doas` plus `-n` when the run must never prompt. Pure.
pub fn never_ask(mut prefix: Vec<String>, no_prompt: bool) -> Vec<String> {
    if no_prompt && !prefix.is_empty() {
        prefix.push("-n".into());
    }
    prefix
}

/// What to add when `pkg` fails with status 3: observed on a FreeBSD 15.1 VM
/// with the network down ("Error updating repositories!"), before anything
/// was installed. Pure.
pub fn pkg_failure_hint(m: PkgManager, code: i32) -> &'static str {
    if m == PkgManager::Pkg && code == 3 {
        "\nhint: pkg status 3 is what it returned when it could not update its repository catalogue (is the network up?); nothing was changed. `yapacc search`, `list` and `check` keep working from the catalogue on disk."
    } else {
        ""
    }
}

/// What to add to a failed step's message when it went through `sudo -n` /
/// `doas -n`: both exit 1 for "password needed" and "not permitted", which
/// looks like any package-manager failure. Pure.
pub fn elevation_failure_hint(prefix: &[String], code: i32) -> String {
    let [tool, flag, ..] = prefix else {
        return String::new();
    };
    if code != 1 || flag != "-n" {
        return String::new();
    }
    format!(
        "\nhint: `{tool} -n` never asks for a password (--script, or no terminal to type one on). If {tool} reported that authentication is required, nothing was changed: allow this user without a password (doas: `permit nopass`, sudo: NOPASSWD), run from a terminal without --script, or run yapacc as root."
    )
}

/// Why `tool -n true` was refused, from what the tool printed. Pure.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// a password would be needed (`doas: Authentication required`,
    /// `sudo: a password is required`)
    Password,
    /// the rules do not allow this command for this user (`doas: Operation
    /// not permitted`, `sudo: Sorry, user x is not allowed to execute`)
    NotPermitted,
    Unknown,
}

pub fn classify_refusal(stderr: &str) -> Refusal {
    let s = stderr.to_ascii_lowercase();
    if s.contains("authentication required") || s.contains("password is required") {
        Refusal::Password
    } else if s.contains("not permitted") || s.contains("not allowed") {
        Refusal::NotPermitted
    } else {
        Refusal::Unknown
    }
}

/// The hint for a refusal by a rule that lists commands: the probe
/// `tool -n true` is itself outside such a rule, so it proves nothing about
/// the failed step; the tool's own message above tells. Pure.
pub fn restricted_rule_hint(tool: &str) -> String {
    format!(
        "\nhint: `{tool} -n true` is refused too, as \"not permitted\": the rules for this user allow only certain commands (doas: `permit nopass user cmd apt-get`). If {tool} itself printed \"not permitted\" above, it refused the command shown; allow that exact command in the rules. If it did not, the command ran and failed on its own."
    )
}

/// The hint when the probe `tool -n true` failed for a reason that is
/// neither "password needed" nor "not permitted" (a syntax error in the
/// rules file, a missing tool): say what the tool printed, claim nothing. Pure.
pub fn probe_failure_hint(tool: &str, stderr: &str) -> String {
    match stderr.lines().find(|l| !l.trim().is_empty()) {
        Some(l) => format!(
            "\nhint: `{tool} -n true` is refused too, and printed: {}. Fix that before retrying.",
            l.trim()
        ),
        None => String::new(),
    }
}

/// `elevation_failure_hint`, but only when the elevation tool is plausibly
/// the cause: a harmless `tool -n true` is refused too (under `permit
/// nopass` it succeeds, so the failure was the command's own). When the
/// refusal says "not permitted" rather than "password needed", a restricted
/// rule is the cause and the hint says that instead.
pub fn elevation_hint(prefix: &[String], code: i32) -> String {
    let [tool, flag, ..] = prefix else {
        return String::new();
    };
    if flag != "-n" {
        return String::new();
    }
    let Ok(out) = Command::new(tool)
        .args(["-n", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
    else {
        return String::new();
    };
    if out.status.success() {
        return String::new();
    }
    let err = String::from_utf8_lossy(&out.stderr);
    match classify_refusal(&err) {
        Refusal::NotPermitted => restricted_rule_hint(tool),
        Refusal::Password => elevation_failure_hint(prefix, code),
        Refusal::Unknown if err.trim().is_empty() => elevation_failure_hint(prefix, code),
        Refusal::Unknown => probe_failure_hint(tool, &err),
    }
}

/// How to become root: nothing when already root, else `sudo` or `doas`.
pub fn elevation() -> Result<Vec<String>> {
    if crate::util::is_root() {
        return Ok(vec![]);
    }
    for tool in ["sudo", "doas"] {
        if crate::util::have(tool) {
            return Ok(vec![tool.to_string()]);
        }
    }
    Err(Error::unavailable(
        "this change needs root, and neither sudo nor doas is installed: run yapacc as root",
    ))
}

pub fn shown(prefix: &[String], argv: &[String]) -> String {
    prefix
        .iter()
        .chain(argv.iter())
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run one command with the terminal attached. Returns its exit status.
pub fn run_attached(prefix: &[String], argv: &[String], interactive: bool) -> Result<i32> {
    let full: Vec<&String> = prefix.iter().chain(argv.iter()).collect();
    let Some((prog, rest)) = full.split_first() else {
        return Err(Error::from("empty command".to_string()));
    };
    let mut cmd = Command::new(prog);
    cmd.args(rest);
    if !interactive {
        cmd.stdin(Stdio::null());
    }
    let status = cmd
        .status()
        .map_err(|e| Error::unavailable(format!("cannot run {prog}: {e}")))?;
    Ok(status.code().unwrap_or(1))
}

/// What a command run by [`run_relayed`] wrote, and its exit status.
#[derive(Debug, Default)]
pub struct Relayed {
    pub code: i32,
    /// empty when standard output was left attached
    pub stdout: String,
    pub stderr: String,
}

/// `chunk` with a carriage return in front of every line feed that has
/// none. `last` is the last byte of the chunk before (0 at the start) and
/// is updated. While `sudo` runs a command in a terminal of its own
/// (`use_pty`, the default since sudo 1.9.14) it keeps the user's terminal
/// in raw mode, where a bare line feed does not return to the first column:
/// what is passed on to a terminal from a pipe needs both. Pure.
pub fn with_carriage_returns(chunk: &[u8], last: &mut u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len() + 8);
    for &b in chunk {
        if b == b'\n' && *last != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        *last = b;
    }
    out
}

/// Pass everything `from` gives on to `to` as it arrives, and keep a copy.
/// `to_terminal`: `to` is a terminal ([`with_carriage_returns`]).
fn relay(mut from: impl std::io::Read, mut to: impl std::io::Write, to_terminal: bool) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 4096];
    let mut last = 0u8;
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = if to_terminal {
                    to.write_all(&with_carriage_returns(&buf[..n], &mut last))
                } else {
                    to.write_all(&buf[..n])
                };
                let _ = to.flush();
                kept.extend_from_slice(&buf[..n]);
            }
        }
    }
    kept
}

/// [`run_attached`] with a copy kept of what the command writes: its
/// standard error always, its standard output when `keep_stdout` (otherwise
/// that stays attached to the terminal). What is kept is printed as it
/// arrives, so the user sees the same thing. Standard input stays attached:
/// `sudo` asks for its password on the terminal itself. The command runs
/// with `LC_ALL=C`, since the copy is read for the manager's own sentences.
pub fn run_relayed(prefix: &[String], argv: &[String], keep_stdout: bool) -> Result<Relayed> {
    let full: Vec<&String> = prefix.iter().chain(argv.iter()).collect();
    let Some((prog, rest)) = full.split_first() else {
        return Err(Error::from("empty command".to_string()));
    };
    let mut cmd = Command::new(prog);
    cmd.args(rest).env("LC_ALL", "C").stderr(Stdio::piped());
    if keep_stdout {
        cmd.stdout(Stdio::piped());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::unavailable(format!("cannot run {prog}: {e}")))?;
    let err_pipe = child.stderr.take();
    use std::io::IsTerminal;
    let err_thread = std::thread::spawn(move || {
        let stderr = std::io::stderr();
        let terminal = stderr.is_terminal();
        err_pipe.map(|p| relay(p, stderr, terminal))
    });
    let out = child
        .stdout
        .take()
        .map(|p| {
            let stdout = std::io::stdout();
            let terminal = stdout.is_terminal();
            relay(p, stdout, terminal)
        })
        .unwrap_or_default();
    let err = err_thread.join().ok().flatten().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| Error::unavailable(format!("cannot run {prog}: {e}")))?;
    Ok(Relayed {
        code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

/// Whether dnf's `--assumeno` simulation says the change cannot be made.
/// Its status cannot tell: it is 1 when dnf resolved the transaction and
/// stopped at the question, and 1 when it could not resolve it. What dnf
/// wrote to standard error tells (`LC_ALL=C`; dnf 4.14 on Rocky Linux 9 and
/// dnf5 5.4.8 on Fedora, 2026-11-19):
/// - resolved, then stopped: `Operation aborted.` (dnf 4),
///   `Operation aborted by the user.` (dnf5);
/// - not resolved: a line `Error: ...` (dnf 4: `Error: Unable to find a
///   match: NAME`, or `Error: ` followed by ` Problem: ...` lines for a
///   conflict or a protected package), `Failed to resolve the
///   transaction:` (dnf5, for both).
///
/// Status 0 is "Nothing to do". A failure that is not recognised (another
/// language because the elevation tool dropped `LC_ALL`, another error) is
/// not called one here: the real command then fails as it did before. Pure.
pub fn dnf_simulation_failed(code: i32, stderr: &str) -> bool {
    if code == 0 {
        return false;
    }
    let mut failed = false;
    for line in stderr.lines() {
        let line = line.trim_start();
        if line.starts_with("Operation aborted") {
            return false;
        }
        failed |=
            line.starts_with("Error:") || line.starts_with("Failed to resolve the transaction");
    }
    failed
}

/// Whether a dnf simulation that failed ([`dnf_simulation_failed`]) refuses
/// the change. Not an upgrade: the real command is `dnf upgrade --refresh`
/// and the simulation runs on the metadata on disk, so its failure does not
/// predict the real run (the same rule as on pacman). Pure.
pub fn dnf_failure_refuses(change: &Change) -> bool {
    !matches!(change, Change::Upgrade)
}

/// The names `pkg install -n` says no repository has, from what it wrote:
/// `pkg: No packages available to install matching 'NAME' have been found
/// in the repositories` (pkg 2.9.2 on FreeBSD 15.1, as root, right after it
/// fetched its catalogues). Only what is a package name is kept. Pure.
pub fn pkg_names_not_available(output: &str) -> Vec<String> {
    const BEFORE: &str = "No packages available to install matching '";
    let mut names: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some((_, rest)) = line.split_once(BEFORE) else {
            continue;
        };
        let Some((name, _)) = rest.split_once('\'') else {
            continue;
        };
        if crate::util::valid_pkg_name(name) && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The refusal of an install of names pkg's own simulation did not find
/// (exit 4). pkg run as root fetches its catalogues first, so this is not
/// said of "the catalogue on disk", and of packages only that none was
/// changed. Pure.
pub fn pkg_not_available_refusal(names: &[String]) -> Error {
    Error::not_found(format!(
        "cannot install {}: pkg found no package of that name in its repositories (`yapacc search` looks for similar names). No package was changed.",
        names.join(", ")
    ))
}

/// What is said under the heading of pkg's simulation when pkg printed
/// nothing at all: without a catalogue on disk `pkg install -n` run as an
/// ordinary user is silent (it cannot fetch one), for a package that exists
/// and for one that does not. `missing` is `native::missing_metadata`'s
/// sentence. `None` when pkg printed something or the catalogue is there.
/// Pure.
pub fn pkg_silent_simulation_note(
    stdout: &str,
    stderr: &str,
    missing: Option<&str>,
) -> Option<String> {
    let silent = stdout.trim().is_empty() && stderr.trim().is_empty();
    missing
        .filter(|_| silent)
        .map(|why| format!("pkg printed no simulation: {why}"))
}

/// zypper's exit codes: 1..=99 are errors (4 is a dependency problem the
/// non-interactive run cannot resolve), 104 is "package not found"; the
/// other 100s are informational (updates needed, reboot required). Pure.
fn zypper_simulation_failed(code: i32) -> bool {
    (1..100).contains(&code) || code == 104
}

/// Whether the exit status of the manager's own simulation says the change
/// cannot be made as asked, so that it must not be applied (not under
/// `--yes` either). A status that also means "nothing to do" or "there is
/// something to do" is not a failure. Pure.
///
/// Observed in containers, 2026-11-19 (the simulation commands of `plan`):
/// - apt (Debian 13, `apt-get -s -y`): 0 for an installed package, for
///   `remove` of a package that is not installed and for an upgrade with
///   nothing pending; 100 for an unknown package, and for a removal apt
///   refuses under `-y` (a package it counts as essential).
/// - pacman (Arch): `-S --needed --print` gives 0 for an installed package
///   and 1 for an unknown one; `-Rs --print` gives 1 for a package that is
///   not installed and for a removal that breaks a dependency. `pacman -Qu`
///   (the upgrade preview) gives 1 when nothing is pending, so the upgrade
///   preview never refuses.
/// - zypper (Tumbleweed): 0 for an installed package and for an upgrade
///   with nothing pending; 104 for an unknown package and for `remove` of a
///   package that is not installed (the real run gives 104 too).
///
/// Not observed, read in pkg's source (src/install.c, src/upgrade.c,
/// src/delete.c): `pkg install -n` exits 1 whenever there is something to
/// install (the dry run counts as a question answered no), the same status
/// as a failure, so it cannot refuse; `pkg upgrade -n` is left out too
/// because its status was not observed on the pkg versions in use. Only
/// `pkg delete -n` refuses by its status. dnf's `--assumeno` exits 1 by
/// design. Those two are judged by what they print instead:
/// [`dnf_simulation_failed`], [`pkg_names_not_available`].
pub fn simulation_refuses(m: PkgManager, change: &Change, code: i32) -> bool {
    if code == 0 {
        return false;
    }
    match m {
        PkgManager::Apt | PkgManager::Apk | PkgManager::Xbps => true,
        PkgManager::Pacman => !matches!(change, Change::Upgrade),
        PkgManager::Zypper => zypper_simulation_failed(code),
        PkgManager::Pkg => matches!(change, Change::Remove(_)),
        PkgManager::Dnf | PkgManager::Portage | PkgManager::Nix => false,
    }
}

/// Flags of the gate, as every mutating command receives them.
#[derive(Clone, Copy, Debug, Default)]
pub struct GateFlags {
    pub dryrun: bool,
    pub yes: bool,
    pub noninteractive: bool,
    pub script: bool,
}

/// Ask for consent. An answer of no, and a run that cannot ask, are both
/// refusals (exit 3) and change nothing.
pub fn gate(what: &str, g: GateFlags) -> Result<()> {
    match crate::prompt::confirm_or_proceed(what, g.yes, g.noninteractive && !g.script, g.script) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::refused("Aborted: nothing was changed.")),
        Err(e) => Err(Error::refused(e)),
    }
}

/// Whether `YAPACC_VIA` says the MCP server asked for this change.
fn via_is_mcp(v: Option<&str>) -> bool {
    v == Some("mcp")
}

/// How the manager's own simulation is run, or `None` when it cannot be.
/// A simulation that needs no root runs as this user; root runs it as it
/// is; a run that may ask for a password runs it behind the elevation tool.
/// A run that may not (`--script`, `--noconfirm`, no terminal) runs it
/// behind `tool -n`, when `nopass(tool)` says the tool lets this user
/// through without a password: if `sudo -n` can run the real command, it
/// can run the simulation. `elevation` is the tool alone (`["sudo"]`), or
/// `None` when there is none. Pure.
pub fn preview_prefix(
    needs_root: bool,
    is_root: bool,
    elevation: Option<&[String]>,
    never_ask: bool,
    nopass: impl FnOnce(&str) -> bool,
) -> Option<Vec<String>> {
    if !needs_root || is_root {
        return Some(Vec::new());
    }
    let tool = elevation?.first()?;
    if !never_ask {
        return Some(vec![tool.clone()]);
    }
    nopass(tool).then(|| vec![tool.clone(), "-n".to_string()])
}

/// Whether `tool -n true` succeeds: the tool lets this user through
/// without a password (NOPASSWD, `permit nopass`, a `sudo -v` still valid).
pub fn runs_without_password(tool: &str) -> bool {
    Command::new(tool)
        .args(["-n", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The packages of `names` that are not installed. `installed` holds the
/// installed names (and `name-version`, which pkg, dnf and zypper also take
/// as a target); on apt a target may carry its architecture (`name:arch`),
/// which the installed names do not. Pure.
pub fn not_installed(names: &[String], installed: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| {
            let bare = n.split_once(':').map_or(n.as_str(), |(name, _)| name);
            !installed.iter().any(|i| i == *n || i == bare)
        })
        .cloned()
        .collect()
}

/// Whether an installed package provides `name` (a virtual name, a
/// capability): dnf, zypper and apt then remove that package, so such a
/// name is not refused as "not installed". The name is a checked package
/// name. `false` where the manager has no such thing or cannot be asked.
fn provided_by_installed(m: PkgManager, name: &str) -> bool {
    let bare = name.split_once(':').map_or(name, |(n, _)| n);
    match m {
        PkgManager::Dnf | PkgManager::Zypper => Command::new("rpm")
            .args(["-q", "--whatprovides", "--", bare])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        PkgManager::Apt => Command::new("dpkg-query")
            .args(["-W", "-f=${db:Status-Abbrev}\t${Provides}\n"])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .output()
            .map(|o| dpkg_provides(&String::from_utf8_lossy(&o.stdout), bare))
            .unwrap_or(false),
        _ => false,
    }
}

/// Whether an installed package (`ii`) lists `name` in its `Provides`.
/// `text` is one line per package: `Status-Abbrev<TAB>Provides`. Pure.
pub fn dpkg_provides(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let Some((status, provides)) = line.split_once('\t') else {
            return false;
        };
        status.starts_with("ii")
            && provides
                .split(',')
                .any(|p| p.trim().split([' ', '(']).next() == Some(name))
    })
}

/// The refusal of a removal of packages that are not installed (exit 4).
/// Pure.
pub fn not_installed_refusal(missing: &[String]) -> Error {
    Error::not_found(format!(
        "cannot remove {}: not installed. Nothing was changed.",
        missing.join(", ")
    ))
}

/// Every installed package with its version, sorted: what a change is
/// compared against to say whether anything was changed. `None` when the
/// list cannot be read.
fn installed_state(m: PkgManager) -> Option<Vec<(String, String)>> {
    let mut v: Vec<(String, String)> = crate::native::backend(m)
        .ok()?
        .installed()
        .ok()?
        .into_iter()
        .map(|p| (p.name, p.version))
        .collect();
    v.sort();
    Some(v)
}

/// Whether the installed packages differ between two readings; `None` when
/// one of them could not be read. Pure.
pub fn state_changed<T: PartialEq>(before: Option<&T>, after: Option<&T>) -> Option<bool> {
    Some(before? != after?)
}

/// What is said when a step failed, after the command that failed:
/// whether anything was changed, as far as the installed packages tell.
/// Pure.
pub fn failure_outcome(changed: Option<bool>) -> &'static str {
    match changed {
        Some(false) => "Nothing was changed: the installed packages are the same as before.",
        Some(true) => {
            "Packages were changed before it failed: `yapacc timeline` shows what was changed."
        }
        None => "`yapacc timeline` shows what was changed, if anything.",
    }
}

/// Show the plan and the native simulation; apply after consent.
pub fn apply(m: PkgManager, change: &Change, g: GateFlags) -> Result<()> {
    // fail closed: an unreadable [safety] snapshot value must not mean "off"
    let mode = crate::snapshot::Mode::parse(&crate::config::get().safety.snapshot)
        .map_err(|e| Error::usage(format!("[safety] snapshot: {e}")))?;
    let eligible = crate::snapshot::manager_is_snapshotted(m);
    let os_id = crate::platform::detect().os.id;
    let p = plan(m, change, &os_id)?;
    println!("Plan: {} (through {})", change.describe(), m.name());
    // What can be known before any simulation is refused here, with the
    // exit code of "not found": a package to remove that is not installed
    // (every manager), a package to install that the catalogue on disk does
    // not have (pkg, whose simulation's status cannot refuse:
    // `simulation_refuses`).
    match change {
        Change::Remove(names) => {
            let installed: Option<Vec<String>> = crate::native::backend(m)
                .ok()
                .and_then(|b| b.installed().ok())
                .map(|v| {
                    v.into_iter()
                        .flat_map(|p| [format!("{}-{}", p.name, p.version), p.name])
                        .collect()
                });
            if let Some(installed) = installed {
                let missing: Vec<String> = not_installed(names, &installed)
                    .into_iter()
                    .filter(|n| !provided_by_installed(m, n))
                    .collect();
                if !missing.is_empty() {
                    return Err(not_installed_refusal(&missing));
                }
            }
        }
        Change::Install(names)
            if m == PkgManager::Pkg && crate::native::missing_metadata(m).is_none() =>
        {
            let unknown: Vec<String> = names
                .iter()
                .filter(|n| crate::native::pkgng::in_catalogue(n) == Some(false))
                .cloned()
                .collect();
            if !unknown.is_empty() {
                return Err(Error::not_found(format!(
                    "cannot install {}: not in the pkg catalogue on disk (`pkg update` refreshes it; `yapacc search` looks for similar names). Nothing was changed.",
                    unknown.join(", ")
                )));
            }
        }
        _ => {}
    }
    let root = elevation_for(g);
    if let Some(preview) = &p.preview {
        let never_ask = g.script || g.noninteractive || !has_terminal();
        let tool = elevation().ok();
        let prefix = preview_prefix(
            p.preview_needs_root,
            crate::util::is_root(),
            tool.as_deref(),
            never_ask,
            runs_without_password,
        );
        if let Some(prefix) = prefix {
            println!(
                "\n{}'s own simulation ({}):",
                m.name(),
                shown(&prefix, preview)
            );
            // simulations exit non-zero by design on some managers (dnf
            // --assumeno, pkg install -n, pacman -Qu): see simulation_refuses.
            // dnf and `pkg install` are read for what they say instead.
            let (code, failed) = match (m, change) {
                (PkgManager::Dnf, _) => {
                    let r = run_relayed(&prefix, preview, false)?;
                    let failed = dnf_simulation_failed(r.code, &r.stderr);
                    if failed && !dnf_failure_refuses(change) {
                        println!(
                            "note: the simulation failed (status {}); it runs without the refresh, so this does not say the upgrade will fail.",
                            r.code
                        );
                    }
                    (r.code, failed && dnf_failure_refuses(change))
                }
                (PkgManager::Pkg, Change::Install(_)) => {
                    let r = run_relayed(&prefix, preview, true)?;
                    let unknown = pkg_names_not_available(&format!("{}\n{}", r.stderr, r.stdout));
                    if !unknown.is_empty() {
                        return Err(pkg_not_available_refusal(&unknown));
                    }
                    let missing = crate::native::missing_metadata(m);
                    if let Some(note) =
                        pkg_silent_simulation_note(&r.stdout, &r.stderr, missing.as_deref())
                    {
                        println!("note: {note}");
                    }
                    (r.code, false)
                }
                _ => (run_attached(&prefix, preview, true)?, false),
            };
            // a change is applied only after a simulation that worked: a
            // failed one (unknown package, conflict, broken package state)
            // refuses here, before the gate, and --yes does not override it
            if failed || simulation_refuses(m, change, code) {
                return Err(Error::refused(format!(
                    "{} failed (status {code}): the change cannot be made as asked; nothing was changed",
                    shown(&prefix, preview)
                )));
            }
        } else {
            println!(
                "\n(no simulation: {} can only simulate as root, and this run may not ask for a password)",
                m.name()
            );
        }
    }
    println!("\nWill run:");
    let prefix = root
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| vec!["<root>".into()]);
    for step in &p.steps {
        println!("  {}", shown(&prefix, step));
    }
    // what happens about the snapshot is part of the plan: the consent
    // covers it, and a configured tool that is not usable refuses here
    let snap = crate::snapshot::announce(mode, eligible, &prefix, &change.describe(), g.dryrun)?;
    for n in &p.notes {
        println!("note: {n}");
    }
    if g.dryrun {
        println!("\n{}", crate::prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    let proceed = match snap {
        Some(tool) => format!("Proceed (a {tool} snapshot is taken first)"),
        None => "Proceed".to_string(),
    };
    gate(&proceed, g)?;
    let prefix = root?;
    // [safety] snapshot: after consent, before the native command
    crate::snapshot::before_change(mode, g.dryrun, eligible, &prefix, &change.describe())?;
    if via_is_mcp(std::env::var("YAPACC_VIA").ok().as_deref()) {
        println!("requested via mcp");
    }
    // the installed packages before and after: what was changed is said
    // from them, not assumed from the exit status
    let before = installed_state(m);
    for step in &p.steps {
        let code = run_attached(&prefix, step, true)?;
        if code != 0 {
            let changed = state_changed(before.as_ref(), installed_state(m).as_ref());
            return Err(Error::from(format!(
                "{} exited with status {code}; later steps were not run. {}{}{}",
                shown(&prefix, step),
                failure_outcome(changed),
                elevation_hint(&prefix, code),
                pkg_failure_hint(m, code)
            )));
        }
    }
    let changed = state_changed(before.as_ref(), installed_state(m).as_ref());
    println!(
        "\n{}",
        closing_message(m, changed, matches!(change, Change::Upgrade))
    );
    Ok(())
}

/// The closing line of a change that ran without an error: that nothing
/// was changed when the installed packages are the same as before (the
/// manager had nothing to do), else [`done_message`]. `refreshed` is
/// whether the steps refreshed the repository indexes first (an upgrade):
/// then it is no package that was changed, not nothing. Pure.
pub fn closing_message(m: PkgManager, changed: Option<bool>, refreshed: bool) -> String {
    match (changed, refreshed) {
        (Some(false), false) => format!(
            "Nothing was changed: {} had nothing to do (the installed packages are the same as before).",
            m.name()
        ),
        (Some(false), true) => format!(
            "No package was changed: {} refreshed its repository indexes and had nothing to upgrade (the installed packages are the same as before).",
            m.name()
        ),
        _ => done_message(m).to_string(),
    }
}

/// The closing line of a change: `undo` is offered only for the managers
/// whose transaction history it reads. Pure.
pub fn done_message(m: PkgManager) -> &'static str {
    if supports_undo(m) {
        "Done. `yapacc timeline` shows the change; `yapacc undo` can take it back."
    } else if m == PkgManager::Xbps {
        // xbps keeps no log of transactions: there is nothing for timeline to read
        "Done. xbps keeps no log, so `yapacc timeline` and `yapacc undo` are not available with this package manager."
    } else {
        "Done. `yapacc timeline` shows the change (`yapacc undo` is not available with this package manager)."
    }
}

/// Whether `yapacc undo` reads this manager's transaction history. Pure.
pub fn supports_undo(m: PkgManager) -> bool {
    matches!(
        m,
        PkgManager::Pacman | PkgManager::Apt | PkgManager::Dnf | PkgManager::Zypper
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_status_3_gets_the_network_hint_and_nothing_else_does() {
        assert!(pkg_failure_hint(PkgManager::Pkg, 3).contains("network"));
        assert_eq!(pkg_failure_hint(PkgManager::Pkg, 1), "");
        assert_eq!(pkg_failure_hint(PkgManager::Apt, 3), "");
    }

    #[test]
    fn elevation_failure_hint_only_for_n_flag_status_1() {
        let doas = vec!["doas".to_string(), "-n".to_string()];
        assert!(elevation_failure_hint(&[], 1).is_empty());
        assert!(elevation_failure_hint(&["doas".to_string()], 1).is_empty());
        assert!(elevation_failure_hint(&doas, 2).is_empty());
        let h = elevation_failure_hint(&doas, 1);
        assert!(h.contains("never asks for a password") && h.contains("permit nopass"));
    }

    #[test]
    fn refusals_are_told_apart() {
        assert_eq!(
            classify_refusal("doas: Authentication required\n"),
            Refusal::Password
        );
        assert_eq!(
            classify_refusal("sudo: a password is required\n"),
            Refusal::Password
        );
        assert_eq!(
            classify_refusal("doas: Operation not permitted\n"),
            Refusal::NotPermitted
        );
        assert_eq!(
            classify_refusal(
                "Sorry, user tester is not allowed to execute '/usr/bin/true' as root"
            ),
            Refusal::NotPermitted
        );
        assert_eq!(classify_refusal(""), Refusal::Unknown);
        let h = restricted_rule_hint("doas");
        assert!(h.contains("allow only certain commands") && !h.contains("never asks"));
        let u = probe_failure_hint("doas", "doas: syntax error at line 1\n");
        assert!(u.contains("syntax error at line 1") && !u.contains("never asks"));
        assert_eq!(probe_failure_hint("doas", "\n"), "");
    }

    #[test]
    fn script_mode_never_asks_for_a_password() {
        let sudo = vec!["sudo".to_string()];
        assert_eq!(never_ask(sudo.clone(), true), ["sudo", "-n"]);
        assert_eq!(never_ask(sudo.clone(), false), ["sudo"]);
        assert!(never_ask(vec![], true).is_empty());
    }

    /// Z2: `--script install repo X --yes` as an ordinary user skipped the
    /// simulation on zypper and dnf, then ran the real command behind
    /// `sudo -n`.
    #[test]
    fn the_simulation_goes_through_the_elevation_the_command_will_use() {
        let sudo = vec!["sudo".to_string()];
        let yes = |_: &str| true;
        let no = |_: &str| false;
        // no root needed, or root already: as it is
        assert_eq!(preview_prefix(false, false, None, true, no), Some(vec![]));
        assert_eq!(
            preview_prefix(true, true, Some(&sudo), true, no),
            Some(vec![])
        );
        // at a terminal: behind sudo, which may ask
        assert_eq!(
            preview_prefix(true, false, Some(&sudo), false, no),
            Some(vec!["sudo".to_string()])
        );
        // --script or no terminal: behind `sudo -n` when that works
        assert_eq!(
            preview_prefix(true, false, Some(&sudo), true, yes),
            Some(vec!["sudo".to_string(), "-n".to_string()])
        );
        let doas = vec!["doas".to_string()];
        assert_eq!(
            preview_prefix(true, false, Some(&doas), true, yes),
            Some(vec!["doas".to_string(), "-n".to_string()])
        );
        // ... and no simulation when it is refused, or there is no tool
        assert_eq!(preview_prefix(true, false, Some(&sudo), true, no), None);
        assert_eq!(preview_prefix(true, false, None, false, yes), None);
        // the probe is not run when it is not needed
        let never = |_: &str| -> bool { panic!("probed") };
        assert!(preview_prefix(true, false, Some(&sudo), false, never).is_some());
    }

    /// D6: `remove repo nosuchpackage-xyz --yes` on dnf exited 0 with
    /// "Done. `yapacc timeline` shows the change".
    #[test]
    fn a_removal_of_what_is_not_installed_is_refused_as_not_found() {
        let installed = names(&["bash", "libpng16-16t64", "tree"]);
        assert!(not_installed(&names(&["tree", "bash"]), &installed).is_empty());
        assert_eq!(
            not_installed(&names(&["tree", "nosuchpackage-xyz"]), &installed),
            ["nosuchpackage-xyz"]
        );
        // apt: `name:arch` is the package `name`
        assert!(not_installed(&names(&["libpng16-16t64:i386"]), &installed).is_empty());
        assert_eq!(
            not_installed(&names(&["zlib1g:i386"]), &installed),
            ["zlib1g:i386"]
        );
        // `name-version`, which pkg, dnf and zypper take as a target
        let with_versions = names(&["tree-2.3.1", "tree"]);
        assert!(not_installed(&names(&["tree-2.3.1"]), &with_versions).is_empty());
        // a virtual name an installed package provides is the manager's to
        // resolve (apt: "Note, selecting 'exim4' instead of ...")
        let provides = "ii \tmail-transport-agent, default-mta (= 1)\nrc \teditor\nii \t\n";
        assert!(dpkg_provides(provides, "mail-transport-agent"));
        assert!(dpkg_provides(provides, "default-mta"));
        assert!(!dpkg_provides(provides, "editor"), "not installed: rc");
        assert!(!dpkg_provides(provides, "mail"));
        let e = not_installed_refusal(&names(&["nosuchpackage-xyz"]));
        assert_eq!(e.kind.exit_code(), 4);
        assert_eq!(
            e.to_string(),
            "cannot remove nosuchpackage-xyz: not installed. Nothing was changed."
        );
    }

    /// D6: what was changed is said from the installed packages.
    #[test]
    fn the_closing_line_says_when_nothing_was_changed() {
        let a = vec![("tree".to_string(), "2.3.1-2".to_string())];
        let b = vec![("tree".to_string(), "2.3.2-1".to_string())];
        assert_eq!(state_changed(Some(&a), Some(&a)), Some(false));
        assert_eq!(state_changed(Some(&a), Some(&b)), Some(true));
        assert_eq!(state_changed(None, Some(&a)), None);
        assert_eq!(state_changed(Some(&a), None), None);
        assert_eq!(
            closing_message(PkgManager::Dnf, Some(false), false),
            "Nothing was changed: dnf had nothing to do (the installed packages are the same as before)."
        );
        // an upgrade refreshes the indexes first: no package was changed
        assert_eq!(
            closing_message(PkgManager::Apt, Some(false), true),
            "No package was changed: apt refreshed its repository indexes and had nothing to upgrade (the installed packages are the same as before)."
        );
        assert_eq!(
            closing_message(PkgManager::Dnf, Some(true), false),
            done_message(PkgManager::Dnf)
        );
        assert_eq!(
            closing_message(PkgManager::Dnf, Some(true), true),
            done_message(PkgManager::Dnf)
        );
        // not known: the old line, which claims no change that did not happen
        assert_eq!(
            closing_message(PkgManager::Apt, None, false),
            done_message(PkgManager::Apt)
        );
        assert!(failure_outcome(Some(false)).starts_with("Nothing was changed"));
        assert!(failure_outcome(Some(true)).contains("`yapacc timeline` shows what was changed"));
        assert!(failure_outcome(None).ends_with("if anything."));
    }

    #[test]
    fn done_message_offers_undo_only_where_it_works() {
        assert!(done_message(PkgManager::Dnf).contains("can take it back"));
        assert!(done_message(PkgManager::Pkg).contains("not available"));
        assert!(!done_message(PkgManager::Apk).contains("can take it back"));
        // xbps keeps no log: timeline has nothing to show there
        assert!(!done_message(PkgManager::Xbps).contains("shows the change"));
        assert!(done_message(PkgManager::Xbps).contains("xbps keeps no log"));
    }
    use crate::error::Kind;

    #[test]
    fn zypper_simulation_exit_codes() {
        assert!(zypper_simulation_failed(4));
        assert!(zypper_simulation_failed(104));
        assert!(!zypper_simulation_failed(0));
        assert!(!zypper_simulation_failed(100));
        assert!(!zypper_simulation_failed(102));
    }

    #[test]
    fn a_failed_simulation_refuses_except_where_non_zero_is_normal() {
        let install = Change::Install(names(&["tree"]));
        let remove = Change::Remove(names(&["tree"]));
        // success never refuses
        for m in [
            PkgManager::Apt,
            PkgManager::Pacman,
            PkgManager::Zypper,
            PkgManager::Pkg,
            PkgManager::Apk,
            PkgManager::Xbps,
            PkgManager::Dnf,
        ] {
            for c in [&install, &remove, &Change::Upgrade] {
                assert!(!simulation_refuses(m, c, 0), "{m:?} {c:?}");
            }
        }
        // apt: 100 is "unable to locate package", broken state, held packages
        for c in [&install, &remove, &Change::Upgrade] {
            assert!(simulation_refuses(PkgManager::Apt, c, 100), "{c:?}");
        }
        // pacman: target not found; but `pacman -Qu` gives 1 for "no updates"
        assert!(simulation_refuses(PkgManager::Pacman, &install, 1));
        assert!(simulation_refuses(PkgManager::Pacman, &remove, 1));
        assert!(!simulation_refuses(PkgManager::Pacman, &Change::Upgrade, 1));
        // zypper: errors and "not found" refuse, informational codes do not
        assert!(simulation_refuses(PkgManager::Zypper, &install, 104));
        assert!(simulation_refuses(PkgManager::Zypper, &Change::Upgrade, 4));
        assert!(!simulation_refuses(
            PkgManager::Zypper,
            &Change::Upgrade,
            102
        ));
        assert!(!simulation_refuses(PkgManager::Zypper, &install, 106));
        // pkg: `install -n` gives 1 when there is something to install
        assert!(simulation_refuses(PkgManager::Pkg, &remove, 1));
        assert!(!simulation_refuses(PkgManager::Pkg, &install, 1));
        assert!(!simulation_refuses(PkgManager::Pkg, &Change::Upgrade, 1));
        // dnf: `--assumeno` gives 1 by design
        assert!(!simulation_refuses(PkgManager::Dnf, &install, 1));
        // apk and xbps stopped before and still do
        assert!(simulation_refuses(PkgManager::Apk, &install, 1));
        assert!(simulation_refuses(PkgManager::Xbps, &remove, 2));
        // apt simulates the command it will run, `-y` included, so that a
        // held or essential package fails the simulation, not the real run
        for c in [&install, &remove, &Change::Upgrade] {
            let preview = plan(PkgManager::Apt, c, "debian").unwrap().preview.unwrap();
            assert_eq!(preview[..3], ["apt-get", "-s", "-y"], "{c:?}");
        }
    }

    /// VM round of 2026-11-19: `install repo no-such-package-xyz --dryrun`
    /// on dnf ended "DRY-RUN: nothing was changed. Run again without
    /// --dryrun to apply.", exit 0, under dnf's "No match for argument".
    /// The lines are dnf's own (`LC_ALL=C`), from rockylinux:9 (dnf 4.14.0)
    /// and fedora:latest (dnf5 5.4.8.0).
    #[test]
    fn dnf_says_in_words_that_its_simulation_failed() {
        // dnf 4, standard error
        let dnf4_unknown = "Error: Unable to find a match: no-such-package-xyz\n";
        let dnf4_conflict = "Error: \n Problem: package coreutils-8.32-43.el9.x86_64 from baseos conflicts with coreutils-single provided by coreutils-single-8.32-43.el9.x86_64 from baseos\n  - cannot install the best candidate for the job\n  - conflicting requests\n";
        let dnf4_protected = "Error: \n Problem: The operation would result in removing the following protected packages: yum\n";
        let dnf4_ok = "Operation aborted.\n";
        // dnf5, standard error
        let dnf5_unknown = "Updating and loading repositories:\nRepositories loaded.\nFailed to resolve the transaction:\nNo match for argument: no-such-package-xyz\nYou can try to add to command line:\n  --skip-unavailable to skip unavailable packages\n";
        let dnf5_conflict = "Updating and loading repositories:\nRepositories loaded.\nFailed to resolve the transaction:\nPackage \"coreutils-9.11-6.fc45.x86_64\" is already installed.\nProblem: problem with installed package\n  - installed package coreutils-9.11-6.fc45.x86_64 conflicts with coreutils-single provided by coreutils-single-9.11-6.fc45.x86_64 from fedora\n  - conflicting requests\nYou can try to add to command line:\n  --allowerasing to allow removing of installed packages to resolve problems\n  --skip-broken to skip uninstallable packages\n";
        let dnf5_protected = "Failed to resolve the transaction:\nProblem: The operation would result in removing the following protected packages: sudo\n";
        let dnf5_ok = "Updating and loading repositories:\nRepositories loaded.\nTotal size of inbound packages is 66 KiB. Need to download 66 KiB.\nAfter this operation, 121 KiB extra will be used (install 121 KiB, remove 0 B).\nOperation aborted by the user.\n";
        for failed in [
            dnf4_unknown,
            dnf4_conflict,
            dnf4_protected,
            dnf5_unknown,
            dnf5_conflict,
            dnf5_protected,
        ] {
            assert!(dnf_simulation_failed(1, failed), "{failed}");
        }
        // a simulation that resolved and stopped at the question passes:
        // install, remove and upgrade all end this way
        for passed in [dnf4_ok, dnf5_ok] {
            assert!(!dnf_simulation_failed(1, passed), "{passed}");
        }
        // "Nothing to do." is status 0, whatever was written
        assert!(!dnf_simulation_failed(
            0,
            "Package \"bash-5.3.15-2.fc45.x86_64\" is already installed.\n"
        ));
        assert!(!dnf_simulation_failed(0, dnf4_unknown));
        // not recognised (sudo's own refusal, another language): as before
        assert!(!dnf_simulation_failed(1, "sudo: a password is required\n"));
        assert!(!dnf_simulation_failed(1, "Opération abandonnée.\n"));
        assert!(!dnf_simulation_failed(1, ""));
        // (constructed, not observed) an error line about one repository,
        // then a transaction that resolved all the same
        assert!(!dnf_simulation_failed(
            1,
            "Error: Failed to download metadata for repo 'extra'\nOperation aborted.\n"
        ));
        // the status alone still refuses nothing on dnf
        let install = Change::Install(names(&["tree"]));
        assert!(!simulation_refuses(PkgManager::Dnf, &install, 1));
        // an install and a removal that failed are refused; an upgrade is
        // simulated without its refresh and is not
        assert!(dnf_failure_refuses(&install));
        assert!(dnf_failure_refuses(&Change::Remove(names(&["sudo"]))));
        assert!(!dnf_failure_refuses(&Change::Upgrade));
    }

    /// VM round of 2026-11-19, FreeBSD 15.1 (pkg 2.9.2), no catalogue on
    /// disk. As root the dry run fetched the catalogues, pkg said the
    /// package does not exist, and yapacc ended "Run again without --dryrun
    /// to apply", exit 0. As an ordinary user the simulation block was
    /// empty, with no word of why.
    #[test]
    fn pkg_says_in_words_that_no_repository_has_the_package() {
        let as_root = "Updating FreeBSD-ports repository catalogue...\nFetching meta.conf: . done\nFetching data: .......... done\nProcessing entries: .......... done\nFreeBSD-ports repository update completed. 38342 packages processed.\nAll repositories are up to date.\npkg: No packages available to install matching 'no-such-package-xyz' have been found in the repositories\n";
        assert_eq!(pkg_names_not_available(as_root), ["no-such-package-xyz"]);
        let two = "pkg: No packages available to install matching 'aaa-xyz' have been found in the repositories\npkg: No packages available to install matching 'bbb-xyz' have been found in the repositories\npkg: No packages available to install matching 'aaa-xyz' have been found in the repositories\n";
        assert_eq!(pkg_names_not_available(two), ["aaa-xyz", "bbb-xyz"]);
        // a package that exists: pkg's plan, nothing to refuse
        let found = "The following 1 package(s) will be affected (of 0 checked):\n\nNew packages to be INSTALLED:\n\tfiglet: 2.2.5_1 [FreeBSD-ports]\n\nNumber of packages to be installed: 1\n\n127 KiB to be downloaded.\n";
        assert!(pkg_names_not_available(found).is_empty());
        assert!(pkg_names_not_available("").is_empty());
        // what is not a package name is not repeated
        assert!(
            pkg_names_not_available(
                "pkg: No packages available to install matching '\u{1b}[31m' have been found"
            )
            .is_empty()
        );
        let e = pkg_not_available_refusal(&names(&["no-such-package-xyz"]));
        assert_eq!(e.kind.exit_code(), 4);
        assert_eq!(
            e.to_string(),
            "cannot install no-such-package-xyz: pkg found no package of that name in its repositories (`yapacc search` looks for similar names). No package was changed."
        );
        // an ordinary user, no catalogue: pkg printed nothing at all
        let why = crate::native::no_metadata_message(PkgManager::Pkg, &[]);
        assert_eq!(
            pkg_silent_simulation_note("", "", Some(&why)).as_deref(),
            Some(
                "pkg printed no simulation: no repository metadata on disk: run `pkg update` (yapacc does not refresh on a read command)"
            )
        );
        assert_eq!(
            pkg_silent_simulation_note("\n", " \n", Some(&why)),
            Some(format!("pkg printed no simulation: {why}"))
        );
        // pkg printed its plan, or the catalogue is there: nothing to add
        assert_eq!(pkg_silent_simulation_note(found, "", Some(&why)), None);
        assert_eq!(pkg_silent_simulation_note("", as_root, Some(&why)), None);
        assert_eq!(pkg_silent_simulation_note("", "", None), None);
    }

    /// What the simulation writes is passed on and kept, and its status is
    /// the command's.
    #[test]
    fn a_relayed_command_keeps_what_it_wrote() {
        let sh = names(&["sh", "-c", "echo to-out; echo to-err >&2; exit 3"]);
        let r = run_relayed(&[], &sh, true).unwrap();
        assert_eq!((r.code, r.stdout.as_str()), (3, "to-out\n"));
        assert_eq!(r.stderr, "to-err\n");
        // standard output left attached: only standard error is kept
        let quiet = names(&["sh", "-c", "echo $LC_ALL >&2"]);
        let r = run_relayed(&[], &quiet, false).unwrap();
        assert_eq!(
            (r.code, r.stdout.as_str(), r.stderr.as_str()),
            (0, "", "C\n")
        );
        let e = run_relayed(&[], &names(&["/nonexistent/yapacc-test-tool"]), false).unwrap_err();
        assert_eq!(e.kind, Kind::Unavailable);
        // passed on to a terminal that sudo keeps in raw mode: every line
        // feed gets its carriage return, once, also across two chunks
        let mut last = 0u8;
        assert_eq!(
            with_carriage_returns(b"Repositories loaded.\nFailed\r\n\n", &mut last),
            b"Repositories loaded.\r\nFailed\r\n\r\n"
        );
        assert_eq!(with_carriage_returns(b"a\r", &mut last), b"a\r");
        assert_eq!(with_carriage_returns(b"\nb\n", &mut last), b"\nb\r\n");
    }

    #[test]
    fn apt_takes_name_arch_and_no_other_manager_does() {
        assert!(valid_apt_target("libpng16-16t64:i386"));
        assert!(valid_apt_target("zlib1g:amd64"));
        assert!(valid_apt_target("tree"));
        for bad in [
            "tree:",
            ":i386",
            "tree:i386:x",
            "tree:I386",
            "tree:-o",
            "tree:i386=1",
            "--root=/tmp:i386",
            "a;b:i386",
            "tree:i 386",
            "",
        ] {
            assert!(!valid_apt_target(bad), "{bad}");
        }
        let multi = names(&["libpng16-16t64:i386"]);
        for change in [
            Change::Install(multi.clone()),
            Change::Remove(multi.clone()),
        ] {
            let p = plan(PkgManager::Apt, &change, "debian").unwrap();
            for cmd in p.steps.iter().chain(p.preview.iter()) {
                let dash = cmd.iter().position(|a| a == "--").expect("a -- separator");
                let name = cmd
                    .iter()
                    .position(|a| a == "libpng16-16t64:i386")
                    .expect("the architecture is kept");
                assert!(dash < name, "{cmd:?}");
            }
            for m in [PkgManager::Dnf, PkgManager::Zypper, PkgManager::Pacman] {
                assert_eq!(plan(m, &change, "").unwrap_err().kind, Kind::Usage);
            }
        }
    }

    #[test]
    fn via_mcp_is_recognised() {
        assert!(via_is_mcp(Some("mcp")));
        assert!(!via_is_mcp(Some("rpc")));
        assert!(!via_is_mcp(None));
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_supported_manager_has_a_plan_and_a_simulation() {
        for m in [
            PkgManager::Pacman,
            PkgManager::Apt,
            PkgManager::Dnf,
            PkgManager::Zypper,
            PkgManager::Apk,
            PkgManager::Xbps,
            PkgManager::Pkg,
        ] {
            for change in [
                Change::Install(names(&["tree"])),
                Change::Remove(names(&["tree"])),
                Change::Upgrade,
            ] {
                let p = plan(m, &change, "").unwrap();
                assert!(!p.steps.is_empty(), "{m:?} {change:?}");
                assert!(p.preview.is_some(), "{m:?} {change:?}");
                if change != Change::Upgrade {
                    // the package name is always after `--`
                    for cmd in p.steps.iter().chain(p.preview.iter()) {
                        let dash = cmd.iter().position(|a| a == "--").expect("a -- separator");
                        let name = cmd.iter().position(|a| a == "tree").unwrap();
                        assert!(dash < name, "{cmd:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn hostile_names_and_empty_requests_are_usage_errors() {
        for bad in ["--root=/tmp", "a;b", ""] {
            let e = plan(PkgManager::Apt, &Change::Install(names(&[bad])), "").unwrap_err();
            assert_eq!(e.kind, Kind::Usage);
        }
        let e = plan(PkgManager::Apt, &Change::Remove(vec![]), "").unwrap_err();
        assert_eq!(e.kind, Kind::Usage);
    }

    #[test]
    fn managers_yapacc_does_not_drive_say_why() {
        let e = plan(PkgManager::Portage, &Change::Upgrade, "gentoo").unwrap_err();
        assert_eq!(e.kind, Kind::Unavailable);
        assert!(e.message.contains("USE flags"), "{}", e.message);
    }

    #[test]
    fn rolling_opensuse_uses_dist_upgrade() {
        let roll = plan(PkgManager::Zypper, &Change::Upgrade, "opensuse-tumbleweed").unwrap();
        assert!(roll.steps[0].iter().any(|a| a == "dist-upgrade"));
        let leap = plan(PkgManager::Zypper, &Change::Upgrade, "opensuse-leap").unwrap();
        assert!(leap.steps[0].iter().any(|a| a == "update"));
    }

    #[test]
    fn commands_are_shown_unambiguously() {
        assert_eq!(
            shown(&names(&["sudo"]), &names(&["apt-get", "install", "a b"])),
            "sudo apt-get install 'a b'"
        );
    }

    #[test]
    fn dry_run_and_refusal_change_nothing() {
        // --script without --yes can never apply
        let g = GateFlags {
            script: true,
            ..Default::default()
        };
        assert_eq!(gate("Proceed", g).unwrap_err().kind, Kind::Refused);
        let yes = GateFlags {
            yes: true,
            ..Default::default()
        };
        assert!(gate("Proceed", yes).is_ok());
    }
}
