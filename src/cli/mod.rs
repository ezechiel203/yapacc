// yapacc — Yet Another PACkage Companion
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
// CLI: verbs and subverbs. Mutating commands apply with --yes (or an
// interactive localized [y/N] prompt); --dryrun plans only. Never dry-run
// by default.

mod args;
mod change;
mod flows;
mod get;
mod install;
mod pkg;
mod planning;
mod portable;
mod portable_write;
mod read;

pub use args::*;
use change::*;
use flows::*;
use get::*;
use install::*;
use pkg::*;
use planning::*;
pub use portable::orphans_unknown;
use read::*;

use crate::{
    appimage, aur_rpc, backend, brew, config, cross_format, foreign_extract, ledger, plan, plugin,
    prompt, recipe, repair, resolve, safety, source_repos, update_service,
};
use clap::{CommandFactory, FromArgMatches};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Entry + shared helpers
// ---------------------------------------------------------------------------

pub fn run() -> crate::error::Result<()> {
    let matches = Cli::command().get_matches();
    let path = command_path(&matches);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let scripted = cli.script;
    let noconfirm = cli.noconfirm;
    let json = cli.json;
    config::init(cli.config.as_deref()).map_err(crate::Error::usage)?;
    let Some(verb) = cli.command else {
        print_help();
        return Ok(());
    };
    // Before anything is read, contacted or changed.
    if json {
        json_gate(&path, &verb)?;
    }
    match verb {
        // optional components: separate binaries, started on demand
        Verb::I => run_component("yapacc-tui", cli.config.as_deref()),
        Verb::Mcp => run_component("yapacc-mcp", cli.config.as_deref()),
        // verbs with a typed exit-code contract
        Verb::Why { name } => crate::why::run(&name, json),
        Verb::Platform => {
            let p = crate::platform::detect();
            if json {
                print_json(&p);
            } else {
                print!("{}", p.render());
            }
            Ok(())
        }
        Verb::Doctor => crate::doctor::run(json, cli.config.as_deref()),
        Verb::Timeline {
            since,
            limit,
            since_boot,
            html,
        } => crate::timeline::run(
            &crate::timeline::Options {
                since,
                limit,
                since_boot,
                html,
            },
            json,
        ),
        Verb::Restart => crate::restart::run(json),
        Verb::Configs => crate::configs::run(json),
        Verb::Names { name } => crate::names::run(&name, json),
        Verb::Snapshot { cmd } => match cmd {
            SnapshotVerb::List => crate::snapshot::list(json),
            SnapshotVerb::Create { description, gate } => crate::snapshot::create(
                description.as_deref(),
                crate::ops::GateFlags {
                    dryrun: gate.dryrun,
                    yes: gate.yes,
                    noninteractive: scripted || noconfirm,
                    script: scripted,
                },
            ),
        },
        Verb::Ledger { cmd } => dispatch_ledger(cmd, json),
        Verb::Briefing { offline } => crate::briefing::run(json, offline),
        Verb::Clean => crate::clean::run(json),
        Verb::Undo { gate } => crate::undo::run(crate::ops::GateFlags {
            dryrun: gate.dryrun,
            yes: gate.yes,
            noninteractive: scripted || noconfirm,
            script: scripted,
        }),
        other => {
            // Off pacman: changes and reads go through the native backend;
            // what only exists for pacman says so instead of failing oddly.
            if let Some(answered) =
                portable_write::route(&other, scripted || noconfirm, scripted, json)
            {
                return answered;
            }
            if let Some(answered) = portable::route(&other, json) {
                return answered;
            }
            portable_write::pacman_only(&other)?;
            // the pacman lane: the other managers check names in their backend
            check_names(&other)?;
            dispatch(other, scripted || noconfirm, scripted, json).map_err(typed_error)
        }
    }
}

/// Replace this process with an optional first-party component.
fn run_component(name: &str, config_path: Option<&str>) -> crate::error::Result<()> {
    use std::os::unix::process::CommandExt;
    let Some(exe) = crate::util::component_exe(name) else {
        return Err(crate::Error::unavailable(format!(
            "this command needs the optional `{name}` component, which is not installed next to yapacc.\nInstall the `{name}` package, or build it from the source tree: cargo build --release --bin {name}"
        )));
    };
    let mut c = Command::new(&exe);
    if let Some(p) = config_path {
        c.args(["--config", p]);
    }
    c.env("YAPACC_BIN", crate::util::core_exe());
    // this process has already printed any configuration warning
    c.env(crate::config::WARNED_ENV, "1");
    let err = c.exec();
    Err(crate::Error::from(format!(
        "failed to start {}: {err}",
        exe.display()
    )))
}

/// The words of the command that was parsed: `inspect owns`, `ledger list`.
fn command_path(matches: &clap::ArgMatches) -> String {
    let mut words = Vec::new();
    let mut at = matches;
    while let Some((name, sub)) = at.subcommand() {
        words.push(name);
        at = sub;
    }
    words.join(" ")
}

/// Every command that has a JSON form, by its words, with the condition
/// when there is one. `--json` on any other command is a usage error (exit
/// 2): a script must never get prose and a success status. The `json`
/// documentation topic and the manual page print this table; a test walks
/// every command against it.
pub const JSON_COMMANDS: &[(&str, &str)] = &[
    ("why", ""),
    ("timeline", "not with --html"),
    ("briefing", ""),
    ("clean", ""),
    ("restart", ""),
    ("configs", ""),
    ("doctor", ""),
    ("platform", ""),
    ("list", ""),
    ("info", ""),
    ("search", ""),
    ("check", ""),
    ("inspect owns", ""),
    ("inspect files", ""),
    ("inspect orphans", ""),
    ("names", ""),
    ("get", "only with --explain"),
    ("snapshot list", ""),
    ("ledger list", ""),
    ("ledger head", ""),
    ("ledger verify", ""),
    ("docs", "the list of topics only"),
    ("config", ""),
    ("capabilities", ""),
    ("external list", ""),
    ("foreign list", ""),
    ("sources apt", ""),
    ("sources rpm", ""),
    ("repair", ""),
    ("news", "Arch only"),
    ("inspect leaves", "Arch only"),
    ("inspect pacdiff", "Arch only"),
    ("aur deps", "Arch only"),
    ("index search", "Arch only"),
    ("monitor check", "Arch only"),
    ("plan install", "Arch only"),
    ("plan remove", "Arch only"),
    ("plan upgrade", "Arch only"),
    ("plan container", "Arch only"),
];

/// Whether the command at `path` has a JSON form.
pub fn has_json(path: &str) -> bool {
    JSON_COMMANDS.iter().any(|(p, _)| *p == path)
}

/// Refuse `--json` where it would be ignored.
fn json_gate(path: &str, verb: &Verb) -> crate::error::Result<()> {
    let refuse = |why: &str| {
        Err(crate::Error::usage(format!(
            "no JSON output for this command: {why}. `yapacc docs json` lists the commands that have one."
        )))
    };
    if !has_json(path) {
        return refuse(&format!("`yapacc {path}` prints text only"));
    }
    match verb {
        Verb::Docs { topic, man } if topic.is_some() || *man => {
            refuse("only the list of topics (`yapacc --json docs`) has a JSON form")
        }
        Verb::Get { explain: false, .. } => refuse(
            "`get` prints its decision as JSON only with --explain; an install has no JSON form",
        ),
        _ => Ok(()),
    }
}

/// On the pacman lane a package name the user typed is checked here, before
/// any command sees it, so that a name that is not one is a usage error
/// (exit 2) as it is in the backends of the other package managers.
fn check_names(verb: &Verb) -> crate::error::Result<()> {
    let plain = |names: &[String]| -> crate::error::Result<()> {
        match names.iter().find(|n| !crate::util::valid_pkg_name(n)) {
            Some(bad) => Err(crate::Error::usage(format!(
                "'{bad}' is not a valid package name"
            ))),
            None => Ok(()),
        }
    };
    // `repo/name` is a target pacman accepts
    let target = |names: &[String]| -> crate::error::Result<()> {
        match names.iter().find(|n| !backend::valid_pacman_target(n)) {
            Some(bad) => Err(crate::Error::usage(format!(
                "'{bad}' is not a valid package name"
            ))),
            None => Ok(()),
        }
    };
    let one = std::slice::from_ref::<String>;
    match verb {
        Verb::Info { pkg } | Verb::Query { pkg, .. } => plain(one(pkg)),
        Verb::Get { names, .. } => plain(names),
        Verb::Download { pkgs, .. } => target(pkgs),
        Verb::Install {
            cmd: InstallVerb::Repo { pkgs, .. } | InstallVerb::Alpm { pkgs, .. },
        } => target(pkgs),
        Verb::Install {
            cmd: InstallVerb::Aur { pkg, .. },
        } => plain(one(pkg)),
        Verb::Remove {
            cmd: RemoveVerb::Repo { pkgs, .. } | RemoveVerb::Alpm { pkgs, .. },
        } => plain(pkgs),
        Verb::Mark {
            cmd: MarkVerb::Explicit { pkgs, .. } | MarkVerb::Dependency { pkgs, .. },
        } => plain(pkgs),
        Verb::Plan {
            cmd: PlanVerb::Install { pkgs, .. },
        } => target(pkgs),
        Verb::Plan {
            cmd: PlanVerb::Remove { pkgs, .. },
        } => plain(pkgs),
        Verb::Aur { cmd } => match cmd {
            AurVerb::Deps { pkgs } => plain(pkgs),
            AurVerb::Review { pkg } | AurVerb::Pkgbuild { pkg } => plain(one(pkg)),
        },
        Verb::Inspect {
            cmd:
                InspectVerb::Files { pkg }
                | InspectVerb::Facets { pkg }
                | InspectVerb::Deps { pkg, .. }
                | InspectVerb::Rdepends { pkg, .. },
        } => plain(one(pkg)),
        _ => Ok(()),
    }
}

/// Width of the terminal on standard output, in columns. `COLUMNS` wins
/// when it is set; None when standard output is not a terminal (a pipe gets
/// whole lines) or the width cannot be read.
pub(crate) fn terminal_width() -> Option<usize> {
    use std::io::IsTerminal;
    if let Some(n) = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 20)
    {
        return Some(n);
    }
    if !io::stdout().is_terminal() {
        return None;
    }
    window_columns()
}

/// `TIOCGWINSZ` on standard output, where its number is known.
#[cfg(any(
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "loongarch64",
            target_arch = "s390x"
        )
    ),
    target_os = "freebsd",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn window_columns() -> Option<usize> {
    #[repr(C)]
    struct Winsize {
        row: u16,
        col: u16,
        xpixel: u16,
        ypixel: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    }
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;
    #[cfg(not(target_os = "linux"))]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x4008_7468;
    let mut size = Winsize {
        row: 0,
        col: 0,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: ioctl(2) with TIOCGWINSZ writes one `struct winsize` to the
    // pointer it is given; `size` is one, alive for the whole call.
    let rc = unsafe { ioctl(1, TIOCGWINSZ, &mut size as *mut Winsize) };
    (rc == 0 && size.col >= 20).then_some(size.col as usize)
}

#[cfg(not(any(
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "loongarch64",
            target_arch = "s390x"
        )
    ),
    target_os = "freebsd",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn window_columns() -> Option<usize> {
    None
}

fn print_json<T: Serialize>(v: &T) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("error: json serialization failed: {e}"),
    }
}

/// Marks a gate refusal while it travels through the older
/// `Result<_, String>` call chains; `run()` turns it into exit code 3.
const REFUSED: &str = "REFUSED: ";
/// The same for "does not exist" (exit code 4) and for a usage error (2).
const NOT_FOUND: &str = "NOT-FOUND: ";
const USAGE: &str = "USAGE: ";
/// The same for "this build or this machine lacks what the command needs"
/// (exit code 6).
const UNAVAILABLE: &str = "UNAVAILABLE: ";

/// A plain-string error is `Failed`, unless it carries one of the marks
/// above. A message that pacman already started with `error: ` loses that
/// prefix: `report` prints its own.
fn typed_error(message: String) -> crate::Error {
    let clean = |rest: &str| rest.trim().trim_start_matches("error: ").to_string();
    if let Some(rest) = message.strip_prefix(REFUSED) {
        crate::Error::refused(clean(rest))
    } else if let Some(rest) = message.strip_prefix(NOT_FOUND) {
        crate::Error::not_found(clean(rest))
    } else if let Some(rest) = message.strip_prefix(USAGE) {
        crate::Error::usage(clean(rest))
    } else if let Some(rest) = message.strip_prefix(UNAVAILABLE) {
        crate::Error::unavailable(clean(rest))
    } else {
        crate::Error::from(clean(&message))
    }
}

/// Carry a typed error through a `Result<_, String>` chain: the kind
/// becomes one of the marks `typed_error` reads back.
fn marked(e: crate::Error) -> String {
    let mark = match e.kind {
        crate::Kind::Refused => REFUSED,
        crate::Kind::NotFound => NOT_FOUND,
        crate::Kind::Usage => USAGE,
        crate::Kind::Unavailable => UNAVAILABLE,
        _ => "",
    };
    format!("{mark}{}", e.message)
}

/// The marked error for an AUR name that was not found (exit 4) or could
/// not be looked up (exit 6). `doing` is what was being attempted, as a
/// prefix (`"cannot install X from the AUR: "`), and `""` for a read. A
/// name that was found has no error: the empty string. Pure.
fn aur_lookup_error(name: &str, lookup: &backend::AurLookup, doing: &str) -> String {
    let end = if doing.is_empty() {
        ""
    } else {
        ". Nothing was changed."
    };
    match lookup {
        backend::AurLookup::Found(_) => String::new(),
        backend::AurLookup::NotInAur => format!(
            "{NOT_FOUND}{doing}{}{end}",
            backend::not_in_aur_message(name)
        ),
        backend::AurLookup::Unreachable(why) => format!(
            "{UNAVAILABLE}{doing}{}{end}",
            backend::aur_unreachable_message(name, why)
        ),
    }
}

/// What a libalpm verb answers in a build without the `alpm` feature: that
/// this build cannot do it, and the command that does the same thing through
/// the pacman command line. `change` adds that nothing was changed. `None`
/// in a build that links libalpm. The answer is "unavailable" (exit 6).
fn alpm_not_built(verb: &str, instead: &str, change: bool) -> Option<String> {
    if crate::alpm_flows::COMPILED_IN {
        return None;
    }
    Some(alpm_not_built_message(verb, instead, change))
}

/// The words of [`alpm_not_built`]. Pure.
fn alpm_not_built_message(verb: &str, instead: &str, change: bool) -> String {
    format!(
        "{UNAVAILABLE}`yapacc {verb}` needs native libalpm support, and this build of yapacc has none (it was built without the `alpm` feature).{} `yapacc {instead}` {} through the pacman command.",
        if change { " Nothing was changed." } else { "" },
        if change {
            "does the same change"
        } else {
            "gives the same answer"
        }
    )
}

/// Interactive confirmation gate (localized Y/N; --yes/--noconfirm proceed;
/// --script and non-TTY stdin fail fast; --dryrun is handled by callers).
/// Never returns `Ok(false)`: an answer of no, and a run that cannot ask,
/// are both errors marked `REFUSED`.
fn confirm_gate(yes: bool, noninteractive: bool, script_mode: bool) -> Result<bool, String> {
    match prompt::confirm_or_proceed("Proceed", yes, noninteractive && !script_mode, script_mode) {
        Ok(true) => Ok(true),
        Ok(false) => Err(format!("{REFUSED}Aborted: nothing was changed.")),
        Err(e) => Err(format!("{REFUSED}{e}")),
    }
}

impl RepairFlags {
    pub fn policy(self, lane: &str, pkg: &str, interactive: bool) -> crate::repair::RepairPolicy {
        use crate::repair::DepConsent;
        let cfg = config::get();
        crate::repair::RepairPolicy {
            enabled: cfg.repair.enabled && !self.no_autorepair,
            max_attempts: cfg.repair.max_attempts,
            allow_checksum_refresh: self.allow_checksum_refresh,
            interactive,
            record_fix: !self.no_fix_record,
            lane: lane.to_string(),
            pkg: pkg.to_string(),
            sandbox: if self.no_sandbox {
                crate::sandbox::Policy::Off
            } else {
                crate::sandbox::Policy::from_config()
            },
            build_net: self.build_net || cfg.build.network == "always",
            dep_consent: if self.repair_deps {
                DepConsent::Granted
            } else if interactive {
                DepConsent::Ask
            } else {
                DepConsent::Deny
            },
        }
    }
}

/// Convert a Gate into `execute` (never dry-run by default).
fn gate_execute(gate: &Gate) -> bool {
    !gate.dryrun
}

fn tr(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// `text` in at most `max` columns, cut at a word and ended with `…` when
/// it is longer. Pure.
pub(crate) fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(max - 1).collect();
    // back to the last space, unless that would throw most of the room away
    let kept = match kept.rfind(' ') {
        Some(at) if at >= kept.len() / 2 => &kept[..at],
        _ => kept.as_str(),
    };
    format!("{}…", kept.trim_end())
}

/// One column of [`table`]: its heading, whether it is aligned to the
/// right, and the widest it is padded to.
pub(crate) struct Column {
    pub head: &'static str,
    pub right: bool,
    pub cap: usize,
}

/// A table for a terminal. Every column but the last is as wide as its
/// longest cell, up to its cap; a cell is never cut, a longer one only
/// moves the rest of its own row. The last column takes what is left of
/// `width` and is cut at a word; when the terminal is narrow the first
/// column gives up padding (not text) to leave it room, and a row with no
/// room left has none. With no width (a pipe) every line is whole. Pure.
pub(crate) fn table(columns: &[Column], rows: &[Vec<String>], width: Option<usize>) -> String {
    /// Columns the last one should get before the first gives up padding.
    const LAST_WANTS: usize = 24;
    /// Below this the last column says nothing useful and is left out.
    const LAST_NEEDS: usize = 12;
    /// The first column is not padded to less than this.
    const FIRST_KEEPS: usize = 16;
    let fixed = columns.len().saturating_sub(1);
    let len = |s: &str| s.chars().count();
    let mut widths: Vec<usize> = (0..fixed)
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, |c| len(c)))
                .max()
                .unwrap_or(0)
                .clamp(
                    len(columns[i].head),
                    columns[i].cap.max(len(columns[i].head)),
                )
        })
        .collect();
    if let (Some(w), Some(first)) = (width, widths.first().copied()) {
        let used: usize = widths.iter().sum::<usize>() + 2 * fixed;
        let short = (used + LAST_WANTS).saturating_sub(w);
        widths[0] = first.saturating_sub(short).max(FIRST_KEEPS.min(first));
    }
    let line = |cells: &[&str], head: bool| -> String {
        let mut out = String::new();
        for (i, w) in widths.iter().enumerate() {
            let cell = cells.get(i).copied().unwrap_or("");
            if columns[i].right {
                out.push_str(&format!("{cell:>w$}"));
            } else {
                out.push_str(&format!("{cell:<w$}"));
            }
            out.push_str("  ");
        }
        let last = cells.get(fixed).copied().unwrap_or("");
        match width.map(|w| w.saturating_sub(len(&out))) {
            None => out.push_str(last),
            Some(room) if head || len(last) <= room => out.push_str(last),
            Some(room) if room >= LAST_NEEDS => out.push_str(&cut_at_word(last, room)),
            Some(_) => {}
        }
        format!("{}\n", out.trim_end())
    };
    let heads: Vec<&str> = columns.iter().map(|c| c.head).collect();
    let mut out = line(&heads, true);
    for row in rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        out.push_str(&line(&cells, false));
    }
    out
}

fn human_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{b:.0} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.2} KiB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} MiB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", b / (1024.0 * 1024.0 * 1024.0))
    }
}

fn is_root() -> bool {
    crate::util::is_root()
}

fn ledger_root(scope: &str, path: Option<String>) -> Result<std::path::PathBuf, String> {
    if let Some(p) = path {
        return Ok(std::path::PathBuf::from(p));
    }
    match scope.to_lowercase().as_str() {
        "user" => Ok(ledger::default_root(&ledger::Scope::User)),
        "system" => Ok(ledger::default_root(&ledger::Scope::System)),
        _ => Err(format!(
            "unknown ledger scope '{scope}' (use user or system)"
        )),
    }
}

fn requires_privileged_mutation(op: &str, user_scope: bool) -> bool {
    if user_scope {
        return false;
    }
    matches!(
        op,
        "install"
            | "remove"
            | "update"
            | "upgrade"
            | "mark"
            | "cache clean"
            | "arch-install"
            | "arch-remove"
            | "arch-update"
            | "arch-upgrade"
            | "flatpak-install-system"
            | "flatpak-remove-system"
            | "flatpak-update-system"
    )
}

/// What a --noconfirm/--script run says when `tool -n true` is refused: the
/// exact probe that failed and the three ways through. Pure.
fn no_password_message(op: &str, tool: &str) -> String {
    format!(
        "operation '{op}' needs root, and `{tool} -n true` was refused: --noconfirm and --script never ask for a password. Nothing was changed. Run yapacc as root, let this user use {tool} without a password (sudo: NOPASSWD, or `sudo -v` shortly before; doas: `permit nopass`), or run from a terminal without --noconfirm/--script."
    )
}

fn ensure_privileges(
    op: &str,
    execute: bool,
    user_scope: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    if !execute {
        return Ok(());
    }
    if !requires_privileged_mutation(op, user_scope) {
        return Ok(());
    }
    if is_root() {
        return Ok(());
    }
    if noninteractive || script_mode {
        // No password can be asked for here. The change goes ahead exactly
        // when the tool lets this user through without one (NOPASSWD,
        // `permit nopass`, or a `sudo -v` still in its grace period); the
        // command itself then runs behind `sudo -n` / `doas -n`.
        let tool = crate::ops::elevation().map_err(|e| e.to_string())?;
        let Some(tool) = tool.first() else {
            return Ok(());
        };
        let passes = Command::new(tool)
            .args(["-n", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return if passes {
            Ok(())
        } else {
            Err(no_password_message(op, tool))
        };
    }
    // doas has no `-v` (no credential refresh to probe): the step itself asks
    // for the password, or is refused by the rules, and reports that.
    if crate::ops::elevation().ok().as_deref() == Some(&["doas".to_string()][..]) {
        return Ok(());
    }
    if Command::new("sudo")
        .arg("-v")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    Err(format!(
        "operation '{op}' requires elevated privileges (root, sudo or doas)."
    ))
}

/// Resolve bin exposure: explicit flag wins, else config [foreign] expose.
fn resolve_expose(expose: Option<&str>) -> Result<bool, String> {
    let s = expose.unwrap_or(&config::get().foreign.expose);
    match s {
        "attributed" => Ok(true),
        "none" => Ok(false),
        _ => Err(format!(
            "invalid --expose value '{s}' (expected: attributed|none). Use 'none' to not add exposed bin names to PATH."
        )),
    }
}

/// Build a RepoTrust from flags, with config default for allow_unsigned.
fn repo_trust<'a>(
    keyring: Option<&'a str>,
    allow_unsigned_flag: bool,
) -> foreign_extract::RepoTrust<'a> {
    foreign_extract::RepoTrust {
        keyring,
        allow_unsigned: allow_unsigned_flag || config::get().foreign.allow_unsigned_repos,
    }
}

/// Install-time gate: refuse installing from unsigned remote repos unless explicitly allowed.
fn enforce_unsigned_gate(keyring: Option<&str>, allow_unsigned_flag: bool) -> Result<(), String> {
    if keyring.is_none() && !allow_unsigned_flag && !config::get().foreign.allow_unsigned_repos {
        return Err("refusing to install from an unsigned remote repo: pass --keyring <keyring> to verify GPG signatures, or --allow-unsigned (or set foreign.allow_unsigned_repos=true in config) to explicitly accept the risk".into());
    }
    Ok(())
}

fn container_name(image: &str, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| {
        let base: String = image
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("pc-box")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect();
        base + "-box"
    })
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn dispatch(verb: Verb, noninteractive: bool, script_mode: bool, json: bool) -> Result<(), String> {
    match verb {
        Verb::I | Verb::Mcp | Verb::Platform | Verb::Why { .. } | Verb::Ledger { .. } => {
            unreachable!("handled in run()")
        }
        Verb::Doctor => unreachable!("handled in run()"),
        Verb::Briefing { .. }
        | Verb::Clean
        | Verb::Undo { .. }
        | Verb::Restart
        | Verb::Configs
        | Verb::Names { .. }
        | Verb::Snapshot { .. } => {
            unreachable!("handled in run()")
        }
        Verb::Timeline { .. } => unreachable!("handled in run()"),
        Verb::Docs { topic, man } => {
            if man {
                print!("{}", crate::docs::MAN_PAGE);
                return Ok(());
            }
            match topic.as_deref() {
                None => {
                    if json {
                        #[derive(serde::Serialize)]
                        struct Topic<'a> {
                            slug: &'a str,
                            title: &'a str,
                            about: &'a str,
                        }
                        let list: Vec<Topic> = crate::docs::topics()
                            .iter()
                            .map(|(s, t, a)| Topic {
                                slug: s,
                                title: t,
                                about: a,
                            })
                            .collect();
                        print_json(&list);
                        return Ok(());
                    }
                    print!("{}", crate::docs::index());
                    Ok(())
                }
                Some(t) => match crate::docs::render(t) {
                    Some(doc) => {
                        print!("{doc}");
                        Ok(())
                    }
                    None => Err(format!(
                        "{NOT_FOUND}unknown docs topic '{t}'. Run `yapacc docs` to list the topics."
                    )),
                },
            }
        }
        Verb::Config => {
            if json {
                print_json(config::get());
                return Ok(());
            }
            println!("== yapacc effective configuration ==");
            println!(
                "Loaded from: {}",
                config::loaded_from(config::explicit_path())
            );
            println!(
                "{}",
                toml::to_string_pretty(config::get())
                    .unwrap_or_else(|e| format!("(serialize error: {e})"))
            );
            Ok(())
        }
        Verb::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "yapacc", &mut io::stdout());
            Ok(())
        }
        Verb::Capabilities => {
            let caps = cross_format::detect_host_capabilities();
            let safety_status = safety::detect_safety_capabilities();
            if json {
                #[derive(serde::Serialize)]
                struct CapabilitiesJson<'a> {
                    capabilities: &'a [cross_format::CapabilityReport],
                    safety: &'a safety::SafetyStatus,
                }
                print_json(&CapabilitiesJson {
                    capabilities: &caps,
                    safety: &safety_status,
                });
                return Ok(());
            }
            println!("== Host/backend capabilities ==");
            for c in &caps {
                println!(
                    "- {}: host={} install={} | {}",
                    c.format, c.host_supported, c.install_supported, c.notes
                );
            }
            println!("\n== Safety ==");
            println!(
                "snapshots_available={} backend={}\n  hint: {}\n  notes: {}",
                safety_status.snapshots_available,
                safety_status.snapshot_backend,
                safety_status.boot_rollback_hint,
                safety_status.notes
            );
            Ok(())
        }

        Verb::List {
            foreign,
            explicit,
            limit,
            include_external,
        } => list_cmd(foreign, explicit, limit, include_external, json),
        Verb::Info { pkg } => info_cmd(&pkg, json),
        Verb::Search {
            query,
            aur,
            all,
            limit,
            category,
            kind,
            ptype,
        } => search_cmd(&query, aur, all, limit, category, kind, ptype, limit, json),
        Verb::Query { pkg, repo, aur } => {
            let mode = if aur {
                "aur"
            } else if repo {
                "repo"
            } else {
                "any"
            };
            if mode == "repo" || mode == "any" {
                match backend::si_repo(&pkg) {
                    Ok(s) => {
                        println!("== Sync repo info ==\n{s}");
                        if mode == "any" {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        if mode == "repo" {
                            return Err(e);
                        }
                    }
                }
            }
            if mode == "aur" {
                // a name the AUR does not have is "not found" (exit 4); an
                // AUR that cannot be asked is "unavailable" (exit 6)
                match backend::aur_lookup(&pkg) {
                    backend::AurLookup::Found(i) => {
                        println!("== AUR info ==\n{}", backend::render_aur_info(&i))
                    }
                    other => return Err(aur_lookup_error(&pkg, &other, "")),
                }
            } else if mode == "any" {
                match backend::aur_info(&pkg) {
                    Ok(s) => println!("== AUR info ==\n{s}"),
                    Err(e) => println!("AUR: {e}"),
                }
            }
            Ok(())
        }
        Verb::Get {
            names,
            from,
            explain,
            source_url,
            no_review,
            build_user,
            expose,
            build,
            factory,
            keep_factory,
            discard_factory,
            non_free,
            gate,
        } => get_flow(
            &names,
            from.as_deref(),
            explain,
            source_url.as_deref(),
            no_review,
            build_user.as_deref(),
            expose.as_deref(),
            build,
            factory.as_deref(),
            (keep_factory, discard_factory),
            non_free,
            &gate,
            noninteractive,
            script_mode,
            json,
        ),
        Verb::Check => {
            if json {
                // the same array as on every other package manager
                let (rows, note) = backend::check_updates_rows(&backend::check_updates_data())?;
                print_json(&rows);
                if let Some(note) = note {
                    eprintln!("note: {note}");
                }
                return Ok(());
            }
            println!("{}", backend::check_updates()?);
            Ok(())
        }
        Verb::News { limit } => {
            let items = backend::arch_news(limit)?;
            if json {
                #[derive(serde::Serialize)]
                struct NewsItem {
                    title: String,
                    date: String,
                }
                let list: Vec<NewsItem> = items
                    .iter()
                    .map(|(t, d)| NewsItem {
                        title: t.clone(),
                        date: d.clone(),
                    })
                    .collect();
                print_json(&list);
                return Ok(());
            }
            println!("== Arch Linux news (most recent {}) ==", items.len());
            if items.is_empty() {
                println!("(no items parsed; feed unavailable?)");
            }
            for (i, (title, date)) in items.iter().enumerate() {
                println!("{}. {title}\n   {date}", i + 1);
            }
            println!("\nCheck news before upgrading (manual interventions are announced here).");
            Ok(())
        }
        Verb::Inspect { cmd: iv } => dispatch_inspect(iv, json),
        Verb::Flatpak { cmd: fv } => dispatch_flatpak_read(fv),
        Verb::Tree { path, installed } => tree_cmd(path.as_deref(), installed),
        Verb::Repair { log } => {
            let text = if log == "-" {
                let mut t = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)
                    .map_err(|e| e.to_string())?;
                t
            } else {
                fs::read_to_string(&log).map_err(|e| format!("cannot read {log}: {e}"))?
            };
            let diagnoses = repair::parse_failures(&text);
            if json {
                print_json(&diagnoses);
                return Ok(());
            }
            println!("== Log diagnosis: {} finding(s) ==", diagnoses.len());
            if diagnoses.is_empty() {
                println!("(nothing in this log matches a failure yapacc knows: read it yourself)");
            }
            for d in &diagnoses {
                // "yapacc can try": during a build it attempts this repair,
                // asking first where the repair installs packages
                println!(
                    "  [{}] {}",
                    if d.auto {
                        "yapacc can try"
                    } else {
                        "needs you"
                    },
                    d.evidence
                );
                println!("        class: {:?}", d.class);
                println!("        repair: {}", d.repair);
            }
            Ok(())
        }
        Verb::Fixes { cmd } => dispatch_fixes(cmd, noninteractive, script_mode),
        Verb::Plugins => {
            let adapters = plugin::discover();
            let mut places = crate::config::get().plugins.dirs.clone();
            places.push("~/.local/share/yapacc/plugins".into());
            places.push("PATH".into());
            println!(
                "Plugin adapters found: {}. Searched, in this order: {}",
                adapters.len(),
                places.join(", ")
            );
            if adapters.is_empty() {
                println!(
                    "(none: an adapter is a separate program named yapacc-source-NAME or yapacc-runtime-NAME; see `yapacc docs plugins`)"
                );
            }
            for a in &adapters {
                let proto = if plugin::protocol_ok(a) {
                    "ok"
                } else {
                    "PROTOCOL MISMATCH"
                };
                let extra = match a.kind {
                    plugin::PluginKind::Runtime => {
                        if plugin::runtime_available(a) {
                            " available"
                        } else {
                            " unavailable"
                        }
                    }
                    plugin::PluginKind::Source => "",
                };
                println!(
                    "  {:<8} {:<16}{} {} [v{} {proto}]",
                    match a.kind {
                        plugin::PluginKind::Source => "source",
                        plugin::PluginKind::Runtime => "runtime",
                    },
                    a.name,
                    extra,
                    a.path.display(),
                    plugin::PROTOCOL_VERSION
                );
            }
            Ok(())
        }
        Verb::Foreign { cmd: fv } => dispatch_foreign_read(fv, json),
        Verb::Index { cmd: iv } => dispatch_index(iv, json),
        Verb::External { cmd: ev } => dispatch_external(ev, json),
        Verb::Sources { cmd: sv } => dispatch_sources(sv, json),
        Verb::Monitor { cmd: mv } => dispatch_monitor(mv, noninteractive, script_mode, json),
        Verb::Aur { cmd: av } => dispatch_aur(av, json),
        Verb::Package { cmd: pv } => dispatch_package(pv),

        Verb::Install { cmd: iv } => dispatch_install(iv, noninteractive, script_mode, json),
        Verb::Remove { cmd: rv } => dispatch_remove(rv, noninteractive, script_mode),
        Verb::Update { cmd: uv } => dispatch_update(uv, noninteractive, script_mode),
        Verb::Refresh { gate } => {
            let execute = gate_execute(&gate);
            action_flow(
                "update",
                vec![],
                false,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        Verb::Upgrade { aur, gate } => {
            let execute = gate_execute(&gate);
            action_flow(
                "upgrade",
                vec![],
                aur,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        Verb::Mark { cmd: mv } => dispatch_mark(mv, noninteractive, script_mode),
        Verb::Download { pkgs, gate } => {
            let execute = gate_execute(&gate);
            // `pacman -Sw` writes the package cache: root, like an install
            let prefix = backend::pacman_elevation(noninteractive || script_mode);
            print!("{}", backend::download_plan(&pkgs, &prefix));
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            ensure_privileges("install", true, false, noninteractive, script_mode)?;
            let out = backend::download(&pkgs, &prefix)?;
            if !out.trim().is_empty() {
                print!("{out}");
            }
            println!("Download complete (cached): {}", pkgs.join(", "));
            Ok(())
        }
        Verb::Cache {
            cmd: CacheVerb::Clean { all, gate },
        } => {
            let execute = gate_execute(&gate);
            let prefix = backend::pacman_elevation(noninteractive || script_mode);
            print!("{}", backend::cache_clean_plan(all, &prefix));
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            ensure_privileges("cache clean", true, false, noninteractive, script_mode)?;
            println!("{}", backend::cache_clean(all, &prefix)?);
            Ok(())
        }
        Verb::Plan { cmd: pv } => dispatch_plan(pv, noninteractive, script_mode, json),
        Verb::Safety { cmd: sv } => dispatch_safety(sv, noninteractive, script_mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn a_run_that_cannot_ask_for_a_password_says_what_was_tried() {
        let m = no_password_message("mark", "sudo");
        assert!(m.contains("`sudo -n true` was refused"), "{m}");
        assert!(m.contains("Nothing was changed"), "{m}");
        // the old text promised that pre-authenticated sudo works while
        // refusing every non-root run
        assert!(!m.contains("pre-authenticated"), "{m}");
        assert!(no_password_message("install", "doas").contains("`doas -n true`"));
        // privileged operations, and the ones that are not
        assert!(requires_privileged_mutation("mark", false));
        assert!(requires_privileged_mutation("cache clean", false));
        assert!(!requires_privileged_mutation(
            "flatpak-install-system",
            true
        ));
        // nothing to check on a dry run
        assert_eq!(ensure_privileges("mark", false, false, true, true), Ok(()));
    }

    #[test]
    fn typed_errors_survive_the_string_chains_and_lose_a_doubled_prefix() {
        for (kind, code) in [
            (crate::Kind::Refused, 3),
            (crate::Kind::NotFound, 4),
            (crate::Kind::Usage, 2),
            (crate::Kind::Unavailable, 6),
            (crate::Kind::Failed, 1),
        ] {
            let back = typed_error(marked(crate::Error::new(kind, "the message")));
            assert_eq!(back.kind, kind);
            assert_eq!(back.kind.exit_code(), code);
            assert_eq!(back.to_string(), "the message");
        }
        // pacman's own "error: " is not printed after ours
        let e = typed_error("error: No package owns /nonexistent\n".into());
        assert_eq!(e.to_string(), "No package owns /nonexistent");
        let e = typed_error(format!("{NOT_FOUND}error: package 'x' was not found"));
        assert_eq!(e.kind, crate::Kind::NotFound);
        assert_eq!(e.to_string(), "package 'x' was not found");
    }

    /// VM round of 2026-11-19 (Arch): `query NAME --aur` exited 1 for a
    /// name the AUR does not have, and `install aur no-such-package-xyz
    /// --dryrun` planned a clone of a repository that does not exist and
    /// exited 0.
    #[test]
    fn an_aur_name_that_does_not_exist_is_exit_4_and_no_network_is_exit_6() {
        let name = "no-such-package-xyz";
        let doing = format!("cannot install {name} from the AUR: ");
        // a read
        let e = typed_error(aur_lookup_error(name, &backend::AurLookup::NotInAur, ""));
        assert_eq!(e.kind.exit_code(), 4);
        assert_eq!(
            e.to_string(),
            "no-such-package-xyz is not in the AUR (`yapacc search no-such-package-xyz --aur` looks for similar names there)"
        );
        // a change: says what was not done
        let e = typed_error(aur_lookup_error(
            name,
            &backend::AurLookup::NotInAur,
            &doing,
        ));
        assert_eq!(e.kind.exit_code(), 4);
        assert_eq!(
            e.to_string(),
            "cannot install no-such-package-xyz from the AUR: no-such-package-xyz is not in the AUR (`yapacc search no-such-package-xyz --aur` looks for similar names there). Nothing was changed."
        );
        let offline = backend::AurLookup::Unreachable(
            "AUR RPC request failed (curl exit exit status: 6): curl: (6) Could not resolve host: aur.archlinux.org".into(),
        );
        let e = typed_error(aur_lookup_error(name, &offline, &doing));
        assert_eq!(e.kind.exit_code(), 6);
        assert!(
            e.to_string().starts_with(
                "cannot install no-such-package-xyz from the AUR: the AUR (aur.archlinux.org) could not be reached"
            ) && e.to_string().ends_with("Nothing was changed."),
            "{e}"
        );
        assert_eq!(
            typed_error(aur_lookup_error(name, &offline, ""))
                .kind
                .exit_code(),
            6
        );
        // the PKGBUILD reader's two failures carry the same two codes
        assert_eq!(
            backend::PkgbuildError::NotInAur(backend::not_in_aur_message(name)).to_string(),
            backend::not_in_aur_message(name)
        );
    }

    /// VM round of 2026-11-19 (Arch, no sync database): `search ripgrep`
    /// printed "0 found" under an empty table, and `install repo figlet
    /// --dryrun` sent the user to the AUR (exit 3).
    #[test]
    fn without_a_sync_database_search_and_install_name_the_refresh() {
        let why = crate::native::no_metadata_message(crate::platform::PkgManager::Pacman, &[]);
        assert_eq!(
            read::search_without_sync_note("repo", &why),
            "note: no repository metadata on disk: run `pacman -Syu` (yapacc does not refresh on a read command)"
        );
        assert_eq!(
            read::search_without_sync_note("all", &why),
            format!("note: the repository part of this search is missing: {why}")
        );
        let e = typed_error(format!(
            "{UNAVAILABLE}{}",
            backend::install_without_sync_message(&["figlet".to_string()])
        ));
        assert_eq!(e.kind.exit_code(), 6);
        assert!(
            e.to_string().contains("run `pacman -Syu` as root first"),
            "{e}"
        );
    }

    #[test]
    fn a_build_without_libalpm_names_the_command_that_works_and_exits_6() {
        // A3: `install alpm` and `inspect alpm ...` printed a bare "no
        // native libalpm support" and exited 1
        for (verb, instead, change) in [
            ("install alpm", "install repo", true),
            ("remove alpm", "remove repo", true),
            ("inspect alpm list", "list", false),
            ("inspect alpm info", "info PKG", false),
            ("inspect alpm leaves", "inspect leaves", false),
            ("inspect alpm search", "search QUERY", false),
        ] {
            let e = typed_error(alpm_not_built_message(verb, instead, change));
            assert_eq!(e.kind.exit_code(), 6, "{verb}");
            let m = e.to_string();
            assert!(m.starts_with(&format!("`yapacc {verb}` needs")), "{m}");
            assert!(m.contains(&format!("`yapacc {instead}`")), "{m}");
            assert!(m.contains("built without the `alpm` feature"), "{m}");
            assert_eq!(m.contains("Nothing was changed."), change, "{m}");
        }
        assert_eq!(
            alpm_not_built("install alpm", "install repo", true).is_none(),
            crate::alpm_flows::COMPILED_IN
        );
    }

    #[test]
    fn the_json_table_names_commands_once_and_the_gate_reads_it() {
        let mut seen = std::collections::HashSet::new();
        for (path, _) in JSON_COMMANDS {
            assert!(seen.insert(*path), "{path} is listed twice");
            assert!(has_json(path));
        }
        assert!(!has_json("plugins"));
        assert!(!has_json("inspect"), "a group of commands is not a command");
        let parse = |args: &[&str]| {
            let m = Cli::command().try_get_matches_from(args).unwrap();
            let path = command_path(&m);
            (path, Cli::from_arg_matches(&m).unwrap().command.unwrap())
        };
        let (path, verb) = parse(&["yapacc", "--json", "inspect", "owns", "/usr/bin/env"]);
        assert_eq!(path, "inspect owns");
        assert!(json_gate(&path, &verb).is_ok());
        let (path, verb) = parse(&["yapacc", "ledger", "status", "--json"]);
        assert_eq!(path, "ledger status");
        let e = json_gate(&path, &verb).unwrap_err();
        assert_eq!(e.kind.exit_code(), 2);
        assert!(e.to_string().starts_with("no JSON output for this command"));
        // the two conditions of the table
        let (path, verb) = parse(&["yapacc", "docs", "daily"]);
        assert_eq!(json_gate(&path, &verb).unwrap_err().kind.exit_code(), 2);
        let (path, verb) = parse(&["yapacc", "docs"]);
        assert!(json_gate(&path, &verb).is_ok());
        let (path, verb) = parse(&["yapacc", "get", "tree"]);
        assert_eq!(json_gate(&path, &verb).unwrap_err().kind.exit_code(), 2);
        let (path, verb) = parse(&["yapacc", "get", "tree", "--explain"]);
        assert!(json_gate(&path, &verb).is_ok());
    }

    #[test]
    fn a_name_that_is_not_a_package_name_is_a_usage_error_on_the_pacman_lane() {
        let verb = |args: &[&str]| Cli::try_parse_from(args).unwrap().command.unwrap();
        for args in [
            &["yapacc", "info", "a;b"][..],
            &["yapacc", "inspect", "files", "--", "-Syu"],
            &["yapacc", "inspect", "deps", "a b"],
            &["yapacc", "install", "repo", "ok", "--", "--config=/tmp/x"],
            &["yapacc", "remove", "repo", "extra/tree"],
            &["yapacc", "mark", "explicit", "a$b"],
            &["yapacc", "download", "../x"],
            &["yapacc", "plan", "install", "a;b"],
            &["yapacc", "get", "a;b", "--explain"],
            &["yapacc", "aur", "deps", "a;b"],
            &["yapacc", "query", "a;b"],
        ] {
            let e = check_names(&verb(args)).unwrap_err();
            assert_eq!(e.kind.exit_code(), 2, "{args:?}");
            assert!(
                e.to_string().ends_with("is not a valid package name"),
                "{e}"
            );
        }
        for args in [
            &["yapacc", "info", "gcc-libs"][..],
            &["yapacc", "install", "repo", "extra/tree", "libstdc++"],
            &["yapacc", "remove", "repo", "tree"],
            &["yapacc", "inspect", "owns", "/any/path is not a name"],
            &["yapacc", "search", "a;b"],
        ] {
            assert!(check_names(&verb(args)).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn a_table_never_cuts_anything_but_its_last_column() {
        let columns = [
            Column {
                head: "NAME",
                right: false,
                cap: 40,
            },
            Column {
                head: "SIZE",
                right: true,
                cap: 16,
            },
            Column {
                head: "DESCRIPTION",
                right: false,
                cap: 0,
            },
        ];
        let rows = vec![
            vec![
                "bash".to_string(),
                "1572.61 KiB".to_string(),
                "The GNU Bourne Again shell".to_string(),
            ],
            vec![
                "a-package-with-a-very-long-name".to_string(),
                "7 B".to_string(),
                "Something with a description that goes on for a while".to_string(),
            ],
        ];
        // into a pipe: every cell whole, columns as wide as their longest cell
        assert_eq!(
            table(&columns, &rows, None),
            "NAME                                    SIZE  DESCRIPTION\n\
             bash                             1572.61 KiB  The GNU Bourne Again shell\n\
             a-package-with-a-very-long-name          7 B  Something with a description that goes on for a while\n"
        );
        // on a terminal: sizes and names stay whole, descriptions give way
        // at a word, and no rule line is printed
        let narrow = table(&columns, &rows, Some(60));
        assert!(narrow.contains("1572.61 KiB"), "{narrow}");
        assert!(
            narrow.contains("a-package-with-a-very-long-name"),
            "{narrow}"
        );
        assert!(narrow.contains("The GNU Bourne Again…"), "{narrow}");
        assert!(
            narrow.contains("Something…") || narrow.contains("Something with…"),
            "{narrow}"
        );
        assert!(!narrow.contains("---"), "{narrow}");
        for line in narrow.lines() {
            assert!(line.chars().count() <= 60, "{line}");
        }
        // cut_at_word
        assert_eq!(cut_at_word("explicit,foreign", 20), "explicit,foreign");
        assert_eq!(cut_at_word("a tool for many things", 12), "a tool for…");
        assert_eq!(cut_at_word("supercalifragilistic", 8), "superca…");
        assert_eq!(cut_at_word("x", 0), "");
    }

    #[test]
    fn the_index_file_goes_under_the_cache_directory_never_under_tmp() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            read::default_index_path(os("/home/u/.cache"), os("/home/u")),
            Some("/home/u/.cache/yapacc/repo-index.tsv".into())
        );
        assert_eq!(
            read::default_index_path(None, os("/home/u")),
            Some("/home/u/.cache/yapacc/repo-index.tsv".into())
        );
        // a relative value is not a place; there is no fallback under /tmp
        assert_eq!(read::default_index_path(os("cache"), None), None);
        assert_eq!(read::default_index_path(None, None), None);
    }

    #[test]
    fn gate_refusals_become_exit_code_3() {
        let e = typed_error(format!("{REFUSED}Aborted: nothing was changed."));
        assert_eq!(e.kind, crate::Kind::Refused);
        assert_eq!(e.kind.exit_code(), 3);
        assert_eq!(e.to_string(), "Aborted: nothing was changed.");
        let e = typed_error("download failed".into());
        assert_eq!(e.kind, crate::Kind::Failed);
        assert_eq!(e.to_string(), "download failed");
        // --script without --yes is refused before any prompt
        let e = typed_error(confirm_gate(false, true, true).unwrap_err());
        assert_eq!(e.kind, crate::Kind::Refused);
        assert!(e.to_string().starts_with("Proceed: --script never prompts"));
        // --yes proceeds without a prompt
        assert_eq!(confirm_gate(true, false, false), Ok(true));
    }
}
