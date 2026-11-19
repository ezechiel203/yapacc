// yapacc — doctor: what works on this machine?
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! The checks of `yapacc doctor` as a
//! pure function: [`evaluate`] turns facts that were already gathered
//! ([`Inputs`]) into one [`Check`] per line of the report, always in the
//! same order. Nothing in this part touches the machine.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Warn,
    Fail,
    #[serde(rename = "n/a")]
    NotApplicable,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Warn => "warn",
            State::Fail => "fail",
            State::NotApplicable => "n/a",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub state: State,
    pub detail: String,
}

/// What the native backend said when asked for the installed list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// it answered, with this many packages
    Answers(usize),
    Errors(String),
    /// there is no backend for this manager, or no manager
    Missing,
    /// a package database is here but its tool is not on PATH (the message)
    ToolMissing(String),
}

/// What verifying one ledger found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ledger {
    NoJournal,
    /// the journal exists but could not be read (for example: no permission)
    Unreadable(String),
    /// the chain verifies; `witnessed`: an independent anchor confirms it
    Verified {
        events: u64,
        records: usize,
        witnessed: bool,
    },
    Broken(String),
}

/// Facts about this machine, gathered before [`evaluate`] is called.
#[derive(Clone, Debug)]
pub struct Inputs {
    /// support tier of the native package manager; 0 when there is none
    pub tier: u8,
    /// name of the native package manager
    pub manager: Option<String>,
    pub backend: Backend,
    /// where the configuration comes from, in words
    pub config: String,
    /// paths of the optional components, when they are installed
    pub tui: Option<String>,
    pub mcp: Option<String>,
    /// result of `sandbox::probe(None)`, and whether `[build] sandbox` is `require`
    pub sandbox: Result<(), String>,
    pub sandbox_required: bool,
    pub ledger_user: Ledger,
    pub ledger_system: Ledger,
    /// tools found on PATH
    pub flatpak: bool,
    pub gpgv: bool,
    pub bwrap: bool,
    /// the native manager has repository indexes on disk
    pub repo_index: bool,
    pub curl: bool,
    /// `ID` of os-release and the distribution family: `undo` chooses its
    /// public archive from them
    pub os_id: String,
    pub family: String,
    /// where `timeline` reads this manager's history, or why it has none
    pub timeline: Result<String, String>,
    /// how a change gets root: "root", "sudo", "doas", or none
    pub elevation: Option<String>,
    pub arch_audit: bool,
}

fn ledger_state(ledger: &Ledger) -> (State, String) {
    match ledger {
        Ledger::NoJournal => (
            State::NotApplicable,
            "no journal (nothing recorded yet)".into(),
        ),
        Ledger::Unreadable(why) => (State::NotApplicable, format!("not readable: {why}")),
        Ledger::Broken(why) => (State::Fail, why.clone()),
        Ledger::Verified {
            events,
            witnessed: true,
            ..
        } => (
            State::Ok,
            format!("chain verifies: {events} events, confirmed by a witness"),
        ),
        Ledger::Verified {
            events, records: 0, ..
        } => (
            State::Ok,
            format!("chain verifies: {events} events, no install record yet"),
        ),
        Ledger::Verified {
            events, records, ..
        } => (
            State::Warn,
            format!("chain verifies: {events} events, {records} records, no witness confirms it"),
        ),
    }
}

/// Where `yapacc undo` would take old versions from on this machine, in the
/// order it tries them; `Err` with the reason when it cannot undo here.
fn undo_sources(i: &Inputs) -> Result<String, String> {
    let archive = |name: &str| match i.curl {
        true => format!(", then {name}"),
        false => format!(" ({name} needs curl, which is not on PATH)"),
    };
    match i.manager.as_deref() {
        Some("pacman") if i.family == "arch" => Ok(format!(
            "old versions from the pacman cache{}",
            archive("the Arch Linux Archive")
        )),
        Some("pacman") => Ok(
            "old versions from the pacman cache only (no public archive is known for this distribution)"
                .into(),
        ),
        Some("apt") => {
            let base = "old versions from the configured repositories, then the apt cache";
            Ok(match i.os_id.as_str() {
                "debian" => format!("{base}{}", archive("snapshot.debian.org")),
                "ubuntu" => format!("{base}{}", archive("Launchpad")),
                _ => format!("{base} (no public archive is known for this distribution)"),
            })
        }
        Some("dnf") => Ok("through `dnf history undo`, from the configured repositories".into()),
        Some("zypper") => Ok(
            "old versions from the configured repositories, as listed in the metadata on disk"
                .into(),
        ),
        Some(other) => Err(format!(
            "not available with {other}: yapacc does not read its transaction log"
        )),
        None => Err("no native package manager could be determined".into()),
    }
}

/// The report: one check per line of the table in YAPACC-005 §2, in order.
pub fn evaluate(i: &Inputs) -> Vec<Check> {
    use State::{Fail, NotApplicable, Warn};
    let m = i.manager.as_deref().unwrap_or("unknown");
    // An optional tool or component that is absent is `n/a`, with what it
    // would be for: a healthy machine without them has nothing to fix.
    // `warn` is kept for what a feature of THIS system depends on.
    let optional_tool = |on_path: bool, used_for: &str| match on_path {
        true => (State::Ok, "on PATH".to_string()),
        false => (NotApplicable, format!("not on PATH ({used_for})")),
    };
    let component = |path: &Option<String>, what: &str| match path {
        Some(path) => (State::Ok, format!("found at {path}")),
        None => (NotApplicable, format!("not installed (optional: {what})")),
    };
    let checks: [(&str, (State, String)); 18] = [
        (
            "platform",
            match (i.manager.is_some(), i.tier) {
                (true, 1) => (State::Ok, format!("{m}: tier 1, a main target")),
                (true, tier @ 2..) => (
                    Warn,
                    format!("{m}: tier {tier}, supported with some features missing (see the lines below)"),
                ),
                _ => (
                    NotApplicable,
                    match &i.backend {
                        Backend::ToolMissing(why) => why.clone(),
                        _ => "no native package manager could be determined".into(),
                    },
                ),
            },
        ),
        (
            "native-manager",
            match &i.backend {
                Backend::Answers(n) => (State::Ok, format!("{m} lists {n} installed packages")),
                Backend::Errors(why) => (Fail, format!("{m} does not answer: {why}")),
                Backend::ToolMissing(why) => (Fail, why.clone()),
                Backend::Missing => (NotApplicable, "no backend for this package manager".into()),
            },
        ),
        ("config", (State::Ok, i.config.clone())),
        (
            "component:yapacc-tui",
            component(&i.tui, "the interactive interface"),
        ),
        (
            "component:yapacc-mcp",
            component(&i.mcp, "the MCP server for AI assistants"),
        ),
        (
            "sandbox",
            match &i.sandbox {
                _ if i.manager.as_deref() != Some("pacman") => (
                    NotApplicable,
                    "only used for source builds on pacman systems".into(),
                ),
                Ok(()) => (State::Ok, "bubblewrap can create a sandbox".into()),
                Err(why) if i.sandbox_required => (
                    Warn,
                    format!("{why}; source builds are refused ([build] sandbox = \"require\")"),
                ),
                Err(why) => (Warn, format!("{why}; source builds run unconfined")),
            },
        ),
        ("ledger:user", ledger_state(&i.ledger_user)),
        ("ledger:system", ledger_state(&i.ledger_system)),
        (
            "tool:flatpak",
            optional_tool(i.flatpak, "only needed for Flatpak applications"),
        ),
        (
            "tool:gpgv",
            optional_tool(
                i.gpgv,
                "only needed to check a repository signature: install tx-apt, tx-rpm with --keyring",
            ),
        ),
        (
            "tool:bwrap",
            match (i.bwrap, i.manager.as_deref()) {
                (true, _) => (State::Ok, "on PATH".to_string()),
                // source builds exist on pacman systems, and they are
                // confined with bubblewrap: there its absence costs something
                (false, Some("pacman")) => (
                    Warn,
                    "not on PATH; source builds on this system have no sandbox without it"
                        .to_string(),
                ),
                (false, _) => (
                    NotApplicable,
                    "not on PATH (only used for source builds on pacman systems)".to_string(),
                ),
            },
        ),
        (
            "repo-index",
            match i.repo_index {
                true => (State::Ok, "repository indexes are on disk".into()),
                false => (
                    Warn,
                    "no repository index on disk; search and check will be empty until the repositories are refreshed".into(),
                ),
            },
        ),
        (
            "tool:curl",
            match i.curl {
                true => (State::Ok, "on PATH".into()),
                false => (
                    Warn,
                    "not on PATH; undo's archive fallback, names and the news feed need it".into(),
                ),
            },
        ),
        (
            "undo",
            match undo_sources(i) {
                Ok(sources) => (State::Ok, sources),
                Err(why) => (NotApplicable, why),
            },
        ),
        (
            "backend-maturity",
            match (&i.backend, i.manager.as_deref()) {
                (Backend::Missing | Backend::ToolMissing(_), _) | (_, None) => {
                    (NotApplicable, "no backend for this package manager".into())
                }
                (_, Some(name @ ("pacman" | "apt" | "dnf" | "zypper" | "apk" | "xbps" | "pkg"))) => {
                    let systems = match name {
                        "pacman" => "Arch Linux",
                        "apt" => "Debian 13 and Ubuntu 24.04",
                        "dnf" => "Fedora 45 (dnf5) and Rocky Linux 9.9 (dnf 4)",
                        "zypper" => "openSUSE Leap 16.1 and Tumbleweed",
                        "apk" => "Alpine 3.24",
                        "xbps" => "Void Linux",
                        _ => "FreeBSD 14.5 and 15.1",
                    };
                    (
                        State::Ok,
                        format!(
                            "{name}: exercised on virtual machines of {systems}; not on real hardware"
                        ),
                    )
                }
                (_, Some(other)) => (
                    NotApplicable,
                    format!("{other}: no statement is made about this backend"),
                ),
            },
        ),
        (
            "timeline",
            match &i.timeline {
                Ok(source) => (State::Ok, source.clone()),
                Err(why) => (NotApplicable, why.clone()),
            },
        ),
        (
            "privilege",
            match i.elevation.as_deref() {
                Some("root") => (State::Ok, "running as root".into()),
                Some(tool) => (State::Ok, format!("changes run through {tool}")),
                None => (
                    Warn,
                    "not root, and neither sudo nor doas is on PATH: install, remove and upgrade cannot run"
                        .into(),
                ),
            },
        ),
        (
            "briefing",
            match i.manager.as_deref() {
                Some("pacman") if i.arch_audit => (
                    State::Ok,
                    "Arch news feed (needs curl), security fixes from arch-audit, reboot hints"
                        .into(),
                ),
                Some("pacman") => (
                    Warn,
                    "Arch news feed (needs curl) and reboot hints; no security labels: install arch-audit"
                        .into(),
                ),
                Some("apt" | "dnf" | "zypper") => (
                    State::Ok,
                    "security-update labels and reboot hints; no distribution news feed".into(),
                ),
                Some(other @ ("apk" | "xbps" | "pkg")) => (
                    NotApplicable,
                    format!(
                        "{other}: pending list, reboot hints and unmerged configuration only; no security labels and no news feed"
                    ),
                ),
                _ => (NotApplicable, "no native package manager".into()),
            },
        ),
    ];
    checks
        .into_iter()
        .map(|(name, (state, detail))| Check {
            name: name.to_string(),
            state,
            detail,
        })
        .collect()
}

/// Text report: `STATE  name  — detail`, one line per check.
pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| format!("{:<4}  {:<20}  — {}\n", c.state.label(), c.name, c.detail))
        .collect()
}

/// `yapacc doctor` exits 1 when this is true.
pub fn any_failed(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.state == State::Fail)
}

// ---------------------------------------------- gathering (reads only)

fn ledger_input(scope: crate::ledger::Scope) -> Ledger {
    use crate::ledger;
    let root = ledger::default_root(&scope);
    match ledger::verify(&root, &ledger::VerifyOptions::default()) {
        Ok(report) if report.events == 0 => Ledger::NoJournal,
        Ok(report) => Ledger::Verified {
            events: report.events,
            records: ledger::list_records(&root).map(|r| r.len()).unwrap_or(0),
            witnessed: report.anchored_through.is_some(),
        },
        Err(e) if e.kind == crate::error::Kind::Integrity => Ledger::Broken(e.message),
        Err(e) => Ledger::Unreadable(e.message),
    }
}

/// Gather the facts from this machine. Reads only; creates nothing.
pub fn gather(config_path: Option<&str>) -> Inputs {
    use crate::sandbox::{Policy, probe};
    use crate::util::{component_exe, have};
    let platform = crate::platform::detect();
    let backend = match crate::native::native_backend() {
        Err(_) => match crate::platform::missing_native_tool() {
            Some((m, tool)) => Backend::ToolMissing(crate::platform::missing_tool_message(m, tool)),
            None => Backend::Missing,
        },
        Ok(b) => match b.installed() {
            Ok(list) => Backend::Answers(list.len()),
            Err(e) => Backend::Errors(e.message),
        },
    };
    let component = |name: &str| component_exe(name).map(|p| p.display().to_string());
    Inputs {
        tier: platform.support_tier,
        manager: platform.native_pkg_manager.map(|m| m.name().to_string()),
        backend,
        config: crate::config::loaded_from(config_path),
        tui: component("yapacc-tui"),
        mcp: component("yapacc-mcp"),
        sandbox: probe(None),
        sandbox_required: Policy::from_config() == Policy::Require,
        ledger_user: ledger_input(crate::ledger::Scope::User),
        ledger_system: ledger_input(crate::ledger::Scope::System),
        flatpak: have("flatpak"),
        gpgv: have("gpgv"),
        bwrap: have("bwrap"),
        // real repository metadata, per manager: not any entry of the cache
        // directory (`@System` on zypper; `expired_repos.json`,
        // `packages.db` and `tempfiles.json` on dnf 4)
        repo_index: platform
            .native_pkg_manager
            .is_some_and(|m| crate::native::missing_metadata(m).is_none()),
        curl: have("curl"),
        os_id: platform.os.id.clone(),
        family: platform.family.clone(),
        timeline: timeline_source(platform.native_pkg_manager),
        elevation: if crate::util::is_root() {
            Some("root".into())
        } else {
            ["sudo", "doas"]
                .into_iter()
                .find(|t| have(t))
                .map(String::from)
        },
        arch_audit: have("arch-audit"),
    }
}

/// Where `timeline` finds this manager's history, or why it cannot.
fn timeline_source(m: Option<crate::platform::PkgManager>) -> Result<String, String> {
    use crate::platform::PkgManager;
    let log = |path: &str, note: &str| match std::fs::File::open(path) {
        Ok(_) => Ok(format!("reads {path}{note}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("{path} does not exist: no history to read"))
        }
        Err(_) => Err(format!("{path} exists but this user cannot read it")),
    };
    match m {
        Some(PkgManager::Pacman) => log("/var/log/pacman.log", ""),
        Some(PkgManager::Apt) => log("/var/log/dpkg.log", " and its rotated predecessor"),
        Some(PkgManager::Zypper) => log("/var/log/zypp/history", ""),
        Some(PkgManager::Dnf) => Ok("reads `dnf history`".into()),
        Some(PkgManager::Apk) => log(
            "/var/log/apk.log",
            " (written by apk run as root; an entry has the time of its run, in UTC)",
        )
        .map_err(|e| format!("{e}; apk keeps no other history")),
        Some(PkgManager::Xbps) => Err("xbps keeps no transaction log: nothing to read".into()),
        Some(PkgManager::Pkg) => log(
            "/var/log/messages",
            " (written by syslog; the year is inferred and times are local time)",
        ),
        _ => Err(match crate::platform::missing_native_tool() {
            Some((m, tool)) => crate::platform::missing_tool_message(m, tool),
            None => "no native package manager could be determined".into(),
        }),
    }
}

/// `yapacc doctor [--json]`: print the report. An error (exit status 1)
/// when at least one check is `fail`.
pub fn run(json: bool, config_path: Option<&str>) -> crate::error::Result<()> {
    let checks = evaluate(&gather(config_path));
    if json {
        let text = serde_json::to_string_pretty(&checks)
            .map_err(|e| format!("json serialization failed: {e}"))?;
        println!("{text}");
    } else {
        print!("{}", render(&checks));
    }
    match checks.iter().filter(|c| c.state == State::Fail).count() {
        0 => Ok(()),
        n => Err(format!("doctor: {n} check(s) failed").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tier-1 pacman machine on which every check is `ok`.
    fn healthy() -> Inputs {
        let verified = Ledger::Verified {
            events: 3,
            records: 2,
            witnessed: true,
        };
        Inputs {
            tier: 1,
            manager: Some("pacman".into()),
            backend: Backend::Answers(1200),
            config: "/etc/yapacc.toml".into(),
            tui: Some("/usr/lib/yapacc/yapacc-tui".into()),
            mcp: Some("/usr/lib/yapacc/yapacc-mcp".into()),
            sandbox: Ok(()),
            sandbox_required: true,
            ledger_user: verified.clone(),
            ledger_system: verified,
            flatpak: true,
            gpgv: true,
            bwrap: true,
            repo_index: true,
            curl: true,
            os_id: "arch".into(),
            family: "arch".into(),
            timeline: Ok("reads /var/log/pacman.log".into()),
            elevation: Some("sudo".into()),
            arch_audit: true,
        }
    }

    fn report(i: &Inputs) -> String {
        render(&evaluate(i))
    }

    #[test]
    fn a_healthy_machine_is_all_ok_in_the_order_of_the_spec() {
        let expected = "\
ok    platform              — pacman: tier 1, a main target
ok    native-manager        — pacman lists 1200 installed packages
ok    config                — /etc/yapacc.toml
ok    component:yapacc-tui  — found at /usr/lib/yapacc/yapacc-tui
ok    component:yapacc-mcp  — found at /usr/lib/yapacc/yapacc-mcp
ok    sandbox               — bubblewrap can create a sandbox
ok    ledger:user           — chain verifies: 3 events, confirmed by a witness
ok    ledger:system         — chain verifies: 3 events, confirmed by a witness
ok    tool:flatpak          — on PATH
ok    tool:gpgv             — on PATH
ok    tool:bwrap            — on PATH
ok    repo-index            — repository indexes are on disk
ok    tool:curl             — on PATH
ok    undo                  — old versions from the pacman cache, then the Arch Linux Archive
ok    backend-maturity      — pacman: exercised on virtual machines of Arch Linux; not on real hardware
ok    timeline              — reads /var/log/pacman.log
ok    privilege             — changes run through sudo
ok    briefing              — Arch news feed (needs curl), security fixes from arch-audit, reboot hints
";
        assert_eq!(report(&healthy()), expected);
        assert!(!any_failed(&evaluate(&healthy())));
    }

    #[test]
    fn warnings_and_not_applicable_do_not_fail() {
        let i = Inputs {
            tier: 2,
            manager: Some("zypper".into()),
            backend: Backend::Missing,
            config: "~/.config/yapacc/config.toml (not present, defaults active)".into(),
            tui: None,
            mcp: None,
            sandbox: Err("bubblewrap (bwrap) is not installed".into()),
            sandbox_required: true,
            ledger_user: Ledger::NoJournal,
            ledger_system: Ledger::Unreadable("Permission denied".into()),
            flatpak: false,
            gpgv: false,
            bwrap: false,
            repo_index: false,
            curl: false,
            os_id: "opensuse-tumbleweed".into(),
            family: "suse".into(),
            timeline: Err("/var/log/zypp/history does not exist: no history to read".into()),
            elevation: None,
            arch_audit: false,
        };
        let expected = "\
warn  platform              — zypper: tier 2, supported with some features missing (see the lines below)
n/a   native-manager        — no backend for this package manager
ok    config                — ~/.config/yapacc/config.toml (not present, defaults active)
n/a   component:yapacc-tui  — not installed (optional: the interactive interface)
n/a   component:yapacc-mcp  — not installed (optional: the MCP server for AI assistants)
n/a   sandbox               — only used for source builds on pacman systems
n/a   ledger:user           — no journal (nothing recorded yet)
n/a   ledger:system         — not readable: Permission denied
n/a   tool:flatpak          — not on PATH (only needed for Flatpak applications)
n/a   tool:gpgv             — not on PATH (only needed to check a repository signature: install tx-apt, tx-rpm with --keyring)
n/a   tool:bwrap            — not on PATH (only used for source builds on pacman systems)
warn  repo-index            — no repository index on disk; search and check will be empty until the repositories are refreshed
warn  tool:curl             — not on PATH; undo's archive fallback, names and the news feed need it
ok    undo                  — old versions from the configured repositories, as listed in the metadata on disk
n/a   backend-maturity      — no backend for this package manager
n/a   timeline              — /var/log/zypp/history does not exist: no history to read
warn  privilege             — not root, and neither sudo nor doas is on PATH: install, remove and upgrade cannot run
ok    briefing              — security-update labels and reboot hints; no distribution news feed
";
        assert_eq!(report(&i), expected);
        assert!(!any_failed(&evaluate(&i)));
        let unknown = Inputs {
            tier: 0,
            manager: None,
            ..i
        };
        assert_eq!(
            report(&unknown).lines().next().unwrap(),
            "n/a   platform              — no native package manager could be determined"
        );
    }

    /// A fresh Debian 13 without the optional components, flatpak, gpgv
    /// and bwrap printed five `warn` lines: a healthy machine looked
    /// half-broken. Only what a feature of this system depends on warns.
    #[test]
    fn an_absent_optional_part_is_not_a_warning() {
        let debian = Inputs {
            tui: None,
            mcp: None,
            flatpak: false,
            gpgv: false,
            bwrap: false,
            manager: Some("apt".into()),
            os_id: "debian".into(),
            family: "debian".into(),
            sandbox: Err("bubblewrap (bwrap) is not installed".into()),
            ..healthy()
        };
        let checks = evaluate(&debian);
        let warned: Vec<&str> = checks
            .iter()
            .filter(|c| c.state == State::Warn)
            .map(|c| c.name.as_str())
            .collect();
        assert!(warned.is_empty(), "{warned:?}");
        // on a pacman system the sandbox of source builds needs bwrap
        let arch = Inputs {
            bwrap: false,
            manager: Some("pacman".into()),
            ..healthy()
        };
        let line = report(&arch)
            .lines()
            .find(|l| l.contains("tool:bwrap"))
            .unwrap()
            .to_string();
        assert_eq!(
            line,
            "warn  tool:bwrap            — not on PATH; source builds on this system have no sandbox without it"
        );
    }

    #[test]
    fn a_database_without_its_tool_is_named_not_called_unknown() {
        let msg = "this system uses pkg but `pkg` is not on PATH (PATH=/x); install it or fix PATH";
        let i = Inputs {
            tier: 0,
            manager: None,
            backend: Backend::ToolMissing(msg.into()),
            ..healthy()
        };
        let checks = evaluate(&i);
        let get = |n: &str| checks.iter().find(|c| c.name == n).unwrap();
        assert_eq!(get("platform").detail, msg);
        assert_eq!(get("native-manager").detail, msg);
        assert_eq!(get("native-manager").state, State::Fail);
    }

    #[test]
    fn a_broken_backend_or_ledger_fails_and_the_other_states_are_reported() {
        let mut i = healthy();
        i.backend = Backend::Errors("pacman exited with status 1".into());
        i.sandbox = Err("bwrap failed to start: denied".into());
        i.ledger_user = Ledger::Verified {
            events: 5,
            records: 2,
            witnessed: false,
        };
        i.ledger_system = Ledger::Broken("verify failed: hash mismatch at seq 2 (line 2)".into());
        let text = report(&i);
        let line = |n: usize| text.lines().nth(n).unwrap().to_string();
        assert_eq!(
            line(1),
            "fail  native-manager        — pacman does not answer: pacman exited with status 1"
        );
        assert_eq!(
            line(5),
            "warn  sandbox               — bwrap failed to start: denied; source builds are refused ([build] sandbox = \"require\")"
        );
        assert_eq!(
            line(6),
            "warn  ledger:user           — chain verifies: 5 events, 2 records, no witness confirms it"
        );
        assert_eq!(
            line(7),
            "fail  ledger:system         — verify failed: hash mismatch at seq 2 (line 2)"
        );
        assert!(any_failed(&evaluate(&i)));
        i.sandbox_required = false;
        i.ledger_user = Ledger::Verified {
            events: 1,
            records: 0,
            witnessed: false,
        };
        let text = report(&i);
        let line = |n: usize| text.lines().nth(n).unwrap().to_string();
        assert_eq!(
            line(5),
            "warn  sandbox               — bwrap failed to start: denied; source builds run unconfined"
        );
        assert_eq!(
            line(6),
            "ok    ledger:user           — chain verifies: 1 events, no install record yet"
        );
    }

    #[test]
    fn undo_and_backend_maturity_follow_the_manager() {
        let last_two = |i: &Inputs| -> Vec<String> {
            let text = report(i);
            text.lines().skip(13).take(2).map(str::to_string).collect()
        };
        let on = |manager: &str, os_id: &str, family: &str, curl: bool| Inputs {
            manager: Some(manager.into()),
            os_id: os_id.into(),
            family: family.into(),
            curl,
            ..healthy()
        };
        assert_eq!(
            last_two(&on("apt", "debian", "debian", true)),
            [
                "ok    undo                  — old versions from the configured repositories, then the apt cache, then snapshot.debian.org",
                "ok    backend-maturity      — apt: exercised on virtual machines of Debian 13 and Ubuntu 24.04; not on real hardware",
            ]
        );
        assert_eq!(
            last_two(&on("apt", "ubuntu", "debian", false))[0],
            "ok    undo                  — old versions from the configured repositories, then the apt cache (Launchpad needs curl, which is not on PATH)"
        );
        assert_eq!(
            last_two(&on("apt", "linuxmint", "debian", true))[0],
            "ok    undo                  — old versions from the configured repositories, then the apt cache (no public archive is known for this distribution)"
        );
        assert_eq!(
            last_two(&on("pacman", "arch", "arch", false))[0],
            "ok    undo                  — old versions from the pacman cache (the Arch Linux Archive needs curl, which is not on PATH)"
        );
        assert_eq!(
            last_two(&on("pacman", "msys2", "msys2", true))[0],
            "ok    undo                  — old versions from the pacman cache only (no public archive is known for this distribution)"
        );
        assert_eq!(
            last_two(&on("dnf", "fedora", "fedora", true)),
            [
                "ok    undo                  — through `dnf history undo`, from the configured repositories",
                "ok    backend-maturity      — dnf: exercised on virtual machines of Fedora 45 (dnf5) and Rocky Linux 9.9 (dnf 4); not on real hardware",
            ]
        );
        for (manager, systems) in [
            ("zypper", "openSUSE Leap 16.1 and Tumbleweed"),
            ("apk", "Alpine 3.24"),
            ("xbps", "Void Linux"),
        ] {
            assert_eq!(
                last_two(&on(manager, "x", "x", true))[1],
                format!(
                    "ok    backend-maturity      — {manager}: exercised on virtual machines of {systems}; not on real hardware"
                )
            );
        }
        assert_eq!(
            last_two(&on("apk", "alpine", "alpine", true))[0],
            "n/a   undo                  — not available with apk: yapacc does not read its transaction log"
        );
        assert_eq!(
            last_two(&on("pkg", "freebsd", "freebsd", true)),
            [
                "n/a   undo                  — not available with pkg: yapacc does not read its transaction log",
                "ok    backend-maturity      — pkg: exercised on virtual machines of FreeBSD 14.5 and 15.1; not on real hardware",
            ]
        );
        let none = Inputs {
            tier: 0,
            manager: None,
            backend: Backend::Missing,
            ..healthy()
        };
        assert_eq!(
            last_two(&none),
            [
                "n/a   undo                  — no native package manager could be determined",
                "n/a   backend-maturity      — no backend for this package manager",
            ]
        );
        assert!(!any_failed(&evaluate(&none)));
    }

    #[test]
    fn json_uses_the_state_names_of_the_spec() {
        let checks = evaluate(&Inputs {
            tier: 0,
            manager: None,
            backend: Backend::Missing,
            ..healthy()
        });
        assert_eq!(
            serde_json::to_string(&checks[..3]).unwrap(),
            r#"[{"name":"platform","state":"n/a","detail":"no native package manager could be determined"},{"name":"native-manager","state":"n/a","detail":"no backend for this package manager"},{"name":"config","state":"ok","detail":"/etc/yapacc.toml"}]"#
        );
        let states: Vec<String> = [State::Ok, State::Warn, State::Fail, State::NotApplicable]
            .iter()
            .map(|s| serde_json::to_string(s).unwrap())
            .collect();
        assert_eq!(states, [r#""ok""#, r#""warn""#, r#""fail""#, r#""n/a""#]);
    }
}
