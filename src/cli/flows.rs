// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Shared mutation flows: gated actions, plan apply, AUR install, safety apply.
//!
//! Order in every flow: validate, show the plan (the native manager's own
//! simulation and the exact command), stop here under `--dryrun`, ask, and
//! only then take a snapshot and run. Nothing before the gate changes
//! anything.

use super::*;
use std::collections::HashSet;
use std::io::IsTerminal;

pub(super) fn print_help() {
    let _ = Cli::command().print_help();
    println!();
}

/// `[safety] snapshot` for the pacman flows (the portable managers do this in
/// `ops::apply`): parsed first so a bad value fails closed, previewed in the
/// plan, taken after consent and before the command.
pub(super) fn snapshot_mode() -> Result<crate::snapshot::Mode, String> {
    crate::snapshot::Mode::parse(&crate::config::get().safety.snapshot)
        .map_err(|e| format!("[safety] snapshot: {e}"))
}

fn snapshot_gate_flags(noninteractive: bool, script_mode: bool) -> crate::ops::GateFlags {
    crate::ops::GateFlags {
        dryrun: false,
        yes: true,
        noninteractive,
        script: script_mode,
    }
}

/// Say, in the plan, what happens about the snapshot before the change
/// (`snapshot::announce`): `Err` is the refusal when the configured tool is
/// not usable. Runs nothing.
pub(super) fn snapshot_preview(
    mode: crate::snapshot::Mode,
    what: &str,
    dryrun: bool,
    noninteractive: bool,
    script: bool,
) -> Result<(), String> {
    let root = crate::ops::elevation_for(snapshot_gate_flags(noninteractive, script))
        .unwrap_or_else(|_| vec!["<root>".into()]);
    crate::snapshot::announce(mode, true, &root, what, dryrun)
        .map(|_| ())
        .map_err(marked)
}

/// Take the `[safety] snapshot`. Call after consent, before the command.
pub(super) fn snapshot_first(
    mode: crate::snapshot::Mode,
    what: &str,
    noninteractive: bool,
    script: bool,
) -> Result<(), String> {
    let root = crate::ops::elevation_for(snapshot_gate_flags(noninteractive, script))
        .map_err(|e| e.to_string())?;
    crate::snapshot::before_change(mode, false, true, &root, what).map_err(marked)
}

/// How far a gated action got. A rollback may only take back what the native
/// command did, so a failure before it ran must roll nothing back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reached {
    BeforeNative,
    NativeRan,
}

/// A pacman-lane action that was validated and shown in full, ready to run
/// after consent.
#[derive(Clone, Debug)]
pub(super) struct PreparedAction {
    op: String,
    /// privilege prefix (`sudo`, `doas -n`, empty for root and for yay)
    prefix: Vec<String>,
    /// the native command, as shown
    native: Vec<String>,
    snap_mode: crate::snapshot::Mode,
    snap_what: String,
}

/// Why yay cannot be run here, if it cannot: its questions are the review
/// of the AUR build code, and `--yes` does not answer them. Pure.
pub(super) fn yay_needs_a_terminal(
    op: &str,
    use_yay: bool,
    noninteractive: bool,
    script_mode: bool,
    stdin_is_terminal: bool,
) -> Option<String> {
    if !backend::goes_through_yay(op, use_yay) {
        return None;
    }
    if !(noninteractive || script_mode) && stdin_is_terminal {
        return None;
    }
    let instead = if op == "upgrade" {
        "Run `yapacc upgrade --aur` from a terminal, or upgrade the repository packages alone with `yapacc upgrade`."
    } else {
        "Run it from a terminal, or read the PKGBUILD (`yapacc aur review PKG`) and use `yapacc install aur PKG --no-review --yes`."
    };
    Some(format!(
        "--aur hands the build to yay, and the questions yay asks before it builds (which diffs to show, whether to proceed) are where that code is reviewed. They need your answers at a terminal: --yes, --noconfirm, --script and a pipe do not answer them, and yapacc does not pass --noconfirm to yay. Nothing was changed. {instead}"
    ))
}

/// What a failed simulation means. An upgrade is simulated without its
/// refresh, so a failure there (no databases yet) does not predict the real
/// run; everywhere else the real command fails the same way. Pure.
pub(super) fn simulation_blocks(op: &str, code: i32) -> bool {
    code != 0 && op != "upgrade"
}

/// The heading of the upgrade simulation: it runs against the databases of
/// the last refresh, and says how old they are, and why there is no current
/// list (`checkupdates` is not installed, or it failed and with what). Pure.
pub(super) fn stale_list_note(
    refresh_age_secs: Option<u64>,
    checkupdates: &backend::CheckUpdates,
) -> String {
    let age = match refresh_age_secs {
        Some(s) => format!("{} ago", backend::age_text(s)),
        None => "never, or at a time that cannot be read".to_string(),
    };
    let current = match checkupdates {
        backend::CheckUpdates::Failed(why) => {
            format!("`checkupdates` is installed but failed ({why}), so there is no current list.")
        }
        _ => "Install pacman-contrib for a current list (checkupdates).".to_string(),
    };
    format!(
        "This list is as of the last repository refresh ({age}). The upgrade refreshes first and may include more packages than are listed here. {current}"
    )
}

/// Print pacman's own simulation of the action. `Err` when the change
/// cannot be made as asked.
fn show_simulation(
    op: &str,
    pkgs: &[String],
    use_yay: bool,
    checkupdates: &backend::CheckUpdates,
) -> Result<(), String> {
    let Some(sim) = backend::preview_argv(op, pkgs, use_yay) else {
        if backend::goes_through_yay(op, use_yay) {
            println!(
                "\n(no simulation: pacman cannot simulate what yay builds from the AUR; yay lists what it will build and asks before it does)"
            );
        }
        return Ok(());
    };
    println!(
        "\npacman's own simulation ({}):",
        crate::ops::shown(&[], &sim)
    );
    let (code, lines, stderr) = backend::run_simulation(&sim)?;
    for l in &lines {
        println!("  {l}");
    }
    if !stderr.is_empty() {
        for l in stderr.lines() {
            println!("  {l}");
        }
    }
    if code == 0 && lines.is_empty() {
        println!("  (nothing to do)");
    }
    if op == "upgrade" {
        let age = backend::last_refresh().and_then(|t| t.elapsed().ok().map(|d| d.as_secs()));
        println!("note: {}", stale_list_note(age, checkupdates));
    }
    if simulation_blocks(op, code) {
        return Err(format!(
            "{REFUSED}{} failed (status {code}): the change cannot be made as asked; nothing was changed",
            crate::ops::shown(&[], &sim)
        ));
    }
    if code != 0 {
        println!(
            "note: the simulation failed (status {code}); it runs without the refresh, so this does not say the upgrade will fail."
        );
    }
    Ok(())
}

/// What is pending before an upgrade. A list is either current
/// (`checkupdates`) or says that it is not (the simulation's note).
fn show_pending_upgrade(use_yay: bool) -> Result<(), String> {
    match backend::current_pending_updates() {
        backend::CheckUpdates::Current(list) => {
            println!(
                "== Pending upgrades: {} package(s), current (checkupdates, on a private copy of the databases) ==",
                list.len()
            );
            for l in &list {
                println!("  {l}");
            }
            if list.is_empty() {
                println!("  (none)");
            }
            println!(
                "note: this list names the installed packages that get a newer version. Packages pacman adds or replaces as dependencies are resolved when it runs, after its own refresh, and are not in this list."
            );
        }
        other => show_simulation("upgrade", &[], false, &other)?,
    }
    if use_yay {
        println!("== AUR updates (RPC vercmp) ==");
        match aur_rpc::aur_update_check() {
            Ok(v) if v.is_empty() => println!("  (none)"),
            Ok(v) => {
                for (n, cur, avail) in v {
                    println!("  {n} {cur} -> {avail}");
                }
            }
            Err(e) => println!("  (AUR update check unavailable: {e})"),
        }
    }
    Ok(())
}

/// Validate a pacman-lane action and show all of it: impact, pacman's own
/// simulation, the exact command, the snapshot that precedes it. Changes
/// nothing; everything here happens before the gate.
pub(super) fn plan_action(
    op: &str,
    pkgs: &[String],
    use_yay: bool,
    execute: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<PreparedAction, String> {
    if (op == "install" || op == "remove" || op == "install-local") && pkgs.is_empty() {
        return Err(format!("{USAGE}{op} requires target(s)"));
    }
    if matches!(op, "install" | "remove") {
        for name in pkgs {
            if !backend::valid_pacman_target(name) {
                return Err(format!("{USAGE}'{name}' is not a valid package name"));
            }
        }
    }
    let never_ask = noninteractive || script_mode;
    // Consent is given at yapacc's gate, with the plan and pacman's own
    // simulation in view: pacman is run with --noconfirm so that it does not
    // ask a second time, as apt, dnf and the others are run with -y. yay is
    // never given it (`native_argv`).
    let native = backend::native_argv(op, pkgs, use_yay);
    if native.is_empty() {
        return Err(format!("unsupported operation: {op}"));
    }
    let prefix = if backend::goes_through_yay(op, use_yay) {
        Vec::new()
    } else {
        backend::pacman_elevation(never_ask)
    };
    match op {
        "remove" => {
            let plan = backend::remove_plan(pkgs)?;
            println!("== Pre-action impact (remove) ==");
            for (p, d) in &plan.direct_dependents {
                println!("  impact: {p} required by {}: {}", d.len(), d.join(", "));
            }
            for w in &plan.warnings {
                println!("  warning: {w}");
            }
            // a name that is neither an installed package nor a group with
            // installed members: nothing to remove (exit 4), said before
            // any simulation, as on every other package manager
            let missing: Vec<String> = plan
                .not_found
                .iter()
                .filter(|n| !backend::installed_group(n))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Err(marked(crate::ops::not_installed_refusal(&missing)));
            }
            show_simulation(op, pkgs, use_yay, &backend::CheckUpdates::Missing)?;
        }
        "install" => {
            // no sync database on disk: nothing can be planned, and every
            // target would be called "not in sync repos"
            if crate::native::missing_metadata(crate::platform::PkgManager::Pacman).is_some() {
                return Err(format!(
                    "{UNAVAILABLE}{}",
                    backend::install_without_sync_message(pkgs)
                ));
            }
            print!("{}", backend::install_plan_body(pkgs, use_yay)?);
            show_simulation(op, pkgs, use_yay, &backend::CheckUpdates::Missing)?;
        }
        "install-local" => show_simulation(op, pkgs, use_yay, &backend::CheckUpdates::Missing)?,
        "upgrade" => show_pending_upgrade(use_yay)?,
        "update" => {
            println!(
                "== Pending updates as of the last repository refresh (the refresh installs nothing; this list changes with it) =="
            );
            match backend::check_updates() {
                Ok(s) => print!("{s}"),
                Err(e) => println!("(update check unavailable: {e})"),
            }
        }
        _ => {}
    }
    println!("\nWill run:\n  {}", crate::ops::shown(&prefix, &native));
    if op == "remove" {
        println!(
            "note: this is `pacman -Rs`: the packages above and the dependencies nothing else needs. Configuration files you modified are kept as .pacsave files (yapacc does not pass -n, which would delete them)."
        );
    }
    if backend::goes_through_yay(op, use_yay) {
        println!(
            "note: yay is run without --noconfirm: it asks its own questions before it builds (which diffs to show, whether to proceed). --yes does not answer them."
        );
    }
    let snap_mode = snapshot_mode()?;
    let snap_what = format!("{op} {}", pkgs.join(" ")).trim().to_string();
    snapshot_preview(snap_mode, &snap_what, !execute, noninteractive, script_mode)?;
    if op == "update" {
        println!("{}", backend::partial_upgrade_warning());
    }
    if execute
        && let Some(why) = yay_needs_a_terminal(
            op,
            use_yay,
            noninteractive,
            script_mode,
            io::stdin().is_terminal(),
        )
    {
        return Err(format!("{REFUSED}{why}"));
    }
    Ok(PreparedAction {
        op: op.to_string(),
        prefix,
        native,
        snap_mode,
        snap_what,
    })
}

/// Run a prepared action: privileges, the `[safety] snapshot`, the command.
/// Call only after consent. `reached` says whether the native command ran.
pub(super) fn run_action(
    a: &PreparedAction,
    noninteractive: bool,
    script_mode: bool,
    reached: &mut Reached,
) -> Result<(), String> {
    ensure_privileges(&a.op, true, false, noninteractive, script_mode)?;
    snapshot_first(a.snap_mode, &a.snap_what, noninteractive, script_mode)?;
    let full: Vec<&String> = a.prefix.iter().chain(a.native.iter()).collect();
    io::stdout().flush().ok();
    // the installed packages before and after: what was changed is said
    // from them, as on every other package manager
    let before = backend::installed_version_map().ok();
    let status = Command::new(full[0])
        .args(&full[1..])
        .status()
        .map_err(|e| format!("failed to execute {}: {e}", full[0]))?;
    *reached = Reached::NativeRan;
    let after = backend::installed_version_map().ok();
    let changed = crate::ops::state_changed(before.as_ref(), after.as_ref());
    if !status.success() {
        return Err(format!(
            "{} exited with {status}. {}{}",
            crate::ops::shown(&a.prefix, &a.native),
            crate::ops::failure_outcome(changed),
            crate::ops::elevation_hint(&a.prefix, status.code().unwrap_or(1))
        ));
    }
    println!("\n{}", closing_line(&a.op, changed));
    Ok(())
}

/// The last line of a pacman-lane action that ran without an error.
/// `changed` is whether the installed packages (names and versions) differ
/// from before. `pacman -S` and `-U` reinstall a package that is already
/// there at that version, so an unchanged list after an install is said as
/// that, not as "nothing to do". Pure.
pub(super) fn closing_line(op: &str, changed: Option<bool>) -> String {
    match (op, changed) {
        ("update", _) => {
            "Done: the repository databases were refreshed; no package was changed.".to_string()
        }
        ("install" | "install-local", Some(false)) => {
            "No package version was changed: the installed packages are the same as before (pacman reinstalls a package that is already installed at that version).".to_string()
        }
        _ => crate::ops::closing_message(
            crate::platform::PkgManager::Pacman,
            changed,
            op == "upgrade",
        ),
    }
}

#[allow(clippy::too_many_arguments)] // internal dispatch plumbing
pub(super) fn action_flow(
    op: &str,
    pkgs: Vec<String>,
    use_yay: bool,
    execute: bool,
    yes: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    let prepared = plan_action(op, &pkgs, use_yay, execute, noninteractive, script_mode)?;
    if !execute {
        println!("{}", prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    confirm_gate(yes, noninteractive, script_mode)?;
    run_action(
        &prepared,
        noninteractive,
        script_mode,
        &mut Reached::BeforeNative,
    )
}

/// Apply a plan file written by `plan install|remove|upgrade --out`. The file
/// supplies only the operation, the targets and the source, all validated;
/// what is shown and run is rebuilt and simulated now, never read back from
/// the file.
#[allow(clippy::too_many_arguments)] // dispatch plumbing
pub(super) fn apply_plan_flow(
    file: &str,
    force: bool,
    execute: bool,
    yes: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    let plan = crate::plan::ActionPlan::load(Path::new(file))?;
    println!("== Apply plan: {file} ==");
    plan.validate(force)
        .map_err(|e| format!("{REFUSED}plan {file} is refused: {e}"))?;
    println!(
        "Operation: {}\nTargets: {}\nSource: {}\nCreated: {} ago",
        plan.op,
        if plan.targets.is_empty() {
            "<none>".to_string()
        } else {
            plan.targets.join(", ")
        },
        plan.source,
        backend::age_text(plan.age_secs())
    );
    println!(
        "(the command and the package lists stored in the file are not used: the plan is rebuilt and simulated now)"
    );
    if plan.op == "install-local" {
        for t in &plan.targets {
            let is_file = fs::symlink_metadata(t)
                .map(|m| m.is_file())
                .unwrap_or(false);
            if !is_file {
                return Err(format!(
                    "{REFUSED}plan {file} is refused: install-local target '{t}' is not an existing regular file"
                ));
            }
        }
    }
    if plan.op == "remove" {
        // re-validate: plan targets must still be installed
        let impact = backend::remove_plan(&plan.targets)?;
        if !impact.not_found.is_empty() {
            return Err(format!(
                "plan targets no longer installed: {}",
                impact.not_found.join(", ")
            ));
        }
    }
    action_flow(
        &plan.op,
        plan.targets.clone(),
        plan.source == "aur",
        execute,
        yes,
        noninteractive,
        script_mode,
    )
}

#[allow(clippy::too_many_arguments)] // dispatch plumbing
pub(super) fn aur_install_flow(
    pkg: &str,
    url: Option<&str>,
    dir: Option<&str>,
    build_user: Option<&str>,
    no_review: bool,
    deps: bool,
    rflags: RepairFlags,
    execute: bool,
    yes: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    // The AUR is asked first whether it has the package: a clone of a
    // repository that does not exist is not planned. With --url the source
    // is the caller's own repository, which the AUR need not know; a build
    // directory that is already there is not cloned again.
    let cloned_already = Path::new(dir.unwrap_or(&format!("./{pkg}-aur-build"))).exists();
    if url.is_none() && !cloned_already {
        let lookup = backend::aur_lookup(pkg);
        if !matches!(lookup, backend::AurLookup::Found(_)) {
            return Err(aur_lookup_error(
                pkg,
                &lookup,
                &format!("cannot install {pkg} from the AUR: "),
            ));
        }
    }
    // Resolve the build set: single package, or the AUR closure in dependency order.
    let (build_set, notes) = if deps {
        let report = crate::aur_rpc::aur_closure(&[pkg.to_string()])?;
        if !report.missing.is_empty() {
            return Err(format!(
                "unresolvable dependencies (not in AUR/repos/installed): {}",
                report.missing.join(", ")
            ));
        }
        if report.build_order.is_empty() {
            return Err(format!("{pkg} not found in AUR"));
        }
        let mut notes = Vec::new();
        if !report.from_repo.is_empty() {
            notes.push(format!(
                "repo deps (install first if missing): {}",
                report.from_repo.join(", ")
            ));
        }
        if !report.satisfied_installed.is_empty() {
            notes.push(format!(
                "already satisfied: {}",
                report.satisfied_installed.join(", ")
            ));
        }
        (report.build_order, notes)
    } else {
        (vec![pkg.to_string()], vec![])
    };
    let dirs: Vec<String> = build_set
        .iter()
        .map(|p| {
            if p == pkg {
                dir.map(|d| d.to_string())
                    .unwrap_or_else(|| format!("./{p}-aur-build"))
            } else {
                format!("./{p}-aur-build")
            }
        })
        .collect();
    let urls: Vec<String> = build_set
        .iter()
        .map(|p| {
            if p == pkg {
                url.map(|u| u.to_string())
                    .unwrap_or_else(|| format!("https://aur.archlinux.org/{p}.git"))
            } else {
                format!("https://aur.archlinux.org/{p}.git")
            }
        })
        .collect();

    println!("== AUR install plan ==");
    println!(
        "Package: {pkg}   closure: {}",
        if deps { "yes (--deps)" } else { "no" }
    );
    for (i, p) in build_set.iter().enumerate() {
        println!("  {}. {p}  <- {}  (build dir: {})", i + 1, urls[i], dirs[i]);
    }
    for n in &notes {
        println!("note: {n}");
    }
    println!(
        "Steps:\n  1) git clone each source\n  2) review ALL PKGBUILDs (interactive gate unless --no-review)\n  3) build+install each with makepkg -f --noconfirm + pacman -U (deps first)"
    );
    if !execute {
        println!("{}", prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    if !confirm_gate(yes, noninteractive, script_mode)? {
        println!("Aborted.");
        return Ok(());
    }

    // clone every package
    for (p, (u, d)) in build_set.iter().zip(urls.iter().zip(dirs.iter())) {
        if std::path::Path::new(d).exists() {
            println!("Note: build dir already exists: {d}");
        } else {
            println!("Cloning {u} ...");
            let st = Command::new("git")
                .args(["-c", "safe.directory=*", "clone", u, d])
                .status()
                .map_err(|e| format!("failed to run git: {e}"))?;
            if !st.success() {
                return Err(format!("git clone failed with {st}"));
            }
            if is_root()
                && let Some(bu) = build_user
            {
                let _ = Command::new("chown").args(["-R", bu, d]).status();
            }
        }
        let _ = p;
    }

    // review: show ALL PKGBUILDs, one gate for the whole set
    for (p, d) in build_set.iter().zip(dirs.iter()) {
        let pb_path = std::path::Path::new(d).join("PKGBUILD");
        let pb = fs::read_to_string(&pb_path).map_err(|_| {
            // AUR clones succeed as empty repos for nonexistent packages; don't litter CWD
            let _ = fs::remove_dir(d);
            format!("PKGBUILD not found in {d} (empty/failed clone? nonexistent AUR package {p}?)")
        })?;
        println!("== PKGBUILD (review): {p} ==\n{pb}");
    }
    if !no_review {
        let kept = review_refusal_tail(&dirs);
        if noninteractive || script_mode {
            return Err(format!(
                "{REFUSED}PKGBUILD review gate: an unreviewed PKGBUILD is not built in --script/--noconfirm mode. Read the PKGBUILDs above and re-run with --no-review once you accept them. {kept}"
            ));
        }
        // the user's own answer at a terminal: --yes and a pipe do not review
        let question = format!(
            "PKGBUILD review: build and install {} package(s) ({})",
            build_set.len(),
            build_set.join(", ")
        );
        match prompt::confirm_on_terminal(&question) {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!(
                    "{REFUSED}Aborted at the PKGBUILD review gate. {kept}"
                ));
            }
            Err(e) => return Err(format!("{REFUSED}{e}. {kept}")),
        }
    } else {
        println!("(PKGBUILD review gate skipped by --no-review)");
    }

    // build + install each package in dependency order (autorepair loop)
    for (p, d) in build_set.iter().zip(dirs.iter()) {
        println!("\n== Building {p} with makepkg (autorepair engine armed) ==");
        let policy = rflags.policy("aur", p, !noninteractive && !script_mode);
        let built = repair::build_with_repair(Path::new(d), build_user, &policy)?;
        let built: Vec<String> = built.into_iter().map(|b| b.display().to_string()).collect();
        println!("Built packages for {p}: {}", built.join(", "));
        ensure_privileges("install-local", true, false, noninteractive, script_mode)?;
        action_flow(
            "install-local",
            built,
            false,
            true,
            true,
            noninteractive,
            script_mode,
        )?;
    }
    println!(
        "AUR install workflow completed for {} ({} package(s)).",
        pkg,
        build_set.len()
    );
    Ok(())
}

/// What a refusal at the PKGBUILD review gate leaves behind: nothing was
/// built or installed, but the sources were cloned. Pure.
pub(super) fn review_refusal_tail(dirs: &[String]) -> String {
    format!(
        "Nothing was built or installed; the cloned sources stay in {}.",
        dirs.join(", ")
    )
}

/// What `--auto-rollback` may do after a failed `arch-install` or
/// `flatpak-install`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Rollback {
    /// the failure came before the native command ran: nothing was changed
    NothingRan,
    /// the installed set could not be read before or after: which packages
    /// are new is unknown, and removing on a guess is how packages get lost
    Unknown,
    Decided {
        /// targets that were absent before and are present now
        remove: Vec<String>,
        /// targets that were installed before (reinstalled or upgraded at most)
        kept: Vec<String>,
        /// packages new in this operation that are not targets
        /// (dependencies, members of a group)
        extra: Vec<String>,
    },
}

/// The bare package name of a pacman target (`extra/htop` -> `htop`). Pure.
pub(super) fn bare_name(target: &str) -> &str {
    target.rsplit_once('/').map_or(target, |(_, n)| n)
}

/// Decide the rollback from what was installed before and what is installed
/// now. Only a target that this operation newly installed is ever removed.
/// Pure.
pub(super) fn rollback_for(
    targets: &[String],
    before: Option<&HashSet<String>>,
    after: Option<&HashSet<String>>,
    reached: Reached,
) -> Rollback {
    if reached == Reached::BeforeNative {
        return Rollback::NothingRan;
    }
    let (Some(before), Some(after)) = (before, after) else {
        return Rollback::Unknown;
    };
    let mut remove: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for t in targets {
        let name = bare_name(t).to_string();
        if before.contains(&name) {
            if !kept.contains(&name) {
                kept.push(name);
            }
        } else if after.contains(&name) && !remove.contains(&name) {
            remove.push(name);
        }
    }
    let mut extra: Vec<String> = after
        .difference(before)
        .filter(|n| !remove.contains(n))
        .cloned()
        .collect();
    extra.sort();
    Rollback::Decided {
        remove,
        kept,
        extra,
    }
}

/// The lines every rollback decision prints, apart from running the removal.
/// Pure.
pub(super) fn rollback_report(rb: &Rollback, manager: &str) -> Vec<String> {
    match rb {
        Rollback::NothingRan => vec![format!(
            "Auto-rollback: nothing to roll back. The failure happened before {manager} ran, so no package was changed."
        )],
        Rollback::Unknown => vec![
            "Auto-rollback: not attempted. The list of installed packages could not be read before and after the operation, so yapacc cannot tell which packages are new and removes none.".to_string()
        ],
        Rollback::Decided {
            remove,
            kept,
            extra,
        } => {
            let mut v = Vec::new();
            if !kept.is_empty() {
                v.push(format!(
                    "Not removed, because installed before this operation: {}. If this run upgraded them, `yapacc undo --dryrun` shows whether the last transaction can be taken back.",
                    kept.join(", ")
                ));
            }
            if !extra.is_empty() {
                v.push(format!(
                    "Left installed (new in this operation, but not a target: dependencies or group members): {}. `yapacc remove orphans --dryrun` lists the ones nothing needs.",
                    extra.join(", ")
                ));
            }
            if remove.is_empty() {
                v.push(
                    "Auto-rollback: no target was newly installed by this operation; nothing is removed."
                        .to_string(),
                );
            }
            v
        }
    }
}

/// The installed set for a rollback decision; a failure to read it is said,
/// and means "unknown", never "empty".
fn installed_for_rollback(when: &str) -> Option<HashSet<String>> {
    match backend::installed_names() {
        Ok(s) => Some(s),
        Err(e) => {
            println!("note: the installed packages could not be listed {when} the operation: {e}");
            None
        }
    }
}

/// Where a helper comes from; an untrusted replacement is a refusal.
fn helper_from(h: safety::Helper) -> Result<safety::HelperSource, String> {
    safety::helper_source(h).map_err(|e| format!("{REFUSED}{e}"))
}

/// Run one of the helper scripts and print what it said.
fn run_helper_step(h: safety::Helper, args: &[String]) -> Result<bool, String> {
    let out = safety::run_helper(h, &helper_from(h)?, args)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stdout.trim().is_empty() {
        print!("{stdout}");
    }
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    Ok(out.status.success())
}

const SNAPSHOT_WHERE: &str = "Snapshots taken by the snapshot step are under /.snapshots/yapacc (`yapacc snapshot list` shows them); restoring a live root needs a rescue boot.";

#[allow(clippy::too_many_arguments)] // internal dispatch plumbing
pub(super) fn safe_apply_flow(
    op: &str,
    targets: &[String],
    aur: bool,
    snapshot_tag: Option<&str>,
    skip_pretest: bool,
    health_check: Option<&str>,
    auto_rollback: bool,
    execute: bool,
    yes: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    println!("== SAFE APPLY WORKFLOW ==");
    println!(
        "Operation: {op}  Targets: {}",
        if targets.is_empty() {
            "<none>".into()
        } else {
            targets.join(", ")
        }
    );
    println!("{}", safety::plan_update_with_safety(targets));

    // ---- plan: everything the apply step will run, shown before the gate
    let arch_op = match op {
        "arch-install" => Some("install"),
        "arch-remove" => Some("remove"),
        "arch-update" => Some("update"),
        "arch-upgrade" => Some("upgrade"),
        _ => None,
    };
    let mut prepared: Option<PreparedAction> = None;
    match op {
        "arch-install" | "arch-remove" | "arch-update" | "arch-upgrade" => {
            let inner = arch_op.unwrap_or_default();
            let needs_targets = matches!(inner, "install" | "remove");
            if needs_targets && targets.is_empty() {
                return Err(format!("{op} requires target package(s)"));
            }
            let pkgs: &[String] = if needs_targets { targets } else { &[] };
            // the gate below is this flow's own: the command is planned as consented
            prepared = Some(plan_action(
                inner,
                pkgs,
                aur,
                execute,
                noninteractive,
                script_mode,
            )?);
        }
        "flatpak-install" => {
            if targets.is_empty() {
                return Err("flatpak-install requires target app(s)".into());
            }
            for t in targets {
                println!("{}", backend::flatpak_install_plan(t, false)?);
            }
        }
        "flatpak-remove" => {
            if targets.is_empty() {
                return Err("flatpak-remove requires target app(s)".into());
            }
            for t in targets {
                println!("{}", backend::flatpak_remove_plan(t, false)?);
            }
        }
        "flatpak-update" => {
            if targets.is_empty() {
                print!("{}", backend::flatpak_update_plan(None, false));
            } else {
                for t in targets {
                    print!("{}", backend::flatpak_update_plan(Some(t), false));
                }
            }
        }
        _ => return Err(format!("unsupported safety apply op: {op}")),
    }

    // ---- auto-rollback: say before the gate what a failure would remove
    if auto_rollback {
        println!("\n== Auto-rollback (only if the apply step or the health check fails) ==");
        match op {
            "arch-install" => {
                let candidates: Vec<String> = match backend::installed_names() {
                    Ok(now) => targets
                        .iter()
                        .map(|t| bare_name(t).to_string())
                        .filter(|n| !now.contains(n))
                        .collect(),
                    Err(_) => Vec::new(),
                };
                println!(
                    "Removes only targets that are not installed now and are installed after pacman ran. Packages installed before this operation are never removed, and nothing is removed when the failure comes before pacman ran."
                );
                if candidates.is_empty() {
                    println!(
                        "Every target is installed already: a failure of this run removes nothing."
                    );
                } else {
                    println!(
                        "Rollback command (for the ones that got installed): {}",
                        crate::ops::shown(
                            &backend::pacman_elevation(noninteractive || script_mode),
                            &backend::rollback_remove_argv(&candidates)
                        )
                    );
                }
            }
            "flatpak-install" => println!(
                "Uninstalls only target apps that are not installed now and are installed after flatpak ran (flatpak uninstall --system -y -- APP)."
            ),
            _ => println!(
                "No automatic rollback exists for {op}: a failure leaves the system as the failed command left it."
            ),
        }
    }

    // ---- pretest: read-only checks, so they may run before the gate
    let pretest_skipped_by_config = !config::get().safety.pretest_enabled;
    if skip_pretest {
        println!("\n== Pretest step ==\n(skipped by --skip-pretest)");
    } else if pretest_skipped_by_config {
        println!("\n== Pretest step ==\n(skipped: config safety.pretest_enabled=false)");
    } else {
        println!("\n== Pretest step (read-only checks) ==");
        let mut args: Vec<String> = vec![op.to_string()];
        args.extend(targets.iter().cloned());
        if !run_helper_step(safety::Helper::Pretest, &args)? {
            return Err("pretest failed; nothing was changed".into());
        }
    }

    // ---- snapshot: planned here, taken after consent
    // (config [safety] require_snapshot_before_upgrade asks for one on upgrades)
    let snapshot_tag: Option<String> = match snapshot_tag {
        Some(t) => Some(t.to_string()),
        None if matches!(op, "arch-upgrade" | "arch-update")
            && config::get().safety.require_snapshot_before_upgrade =>
        {
            println!(
                "(config safety.require_snapshot_before_upgrade=true: a snapshot is required)"
            );
            Some(format!(
                "auto-{op}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ))
        }
        None => None,
    };
    let snapshot_args: Option<Vec<String>> =
        snapshot_tag.map(|tag| vec!["create".to_string(), tag]);
    println!("\n== Snapshot step ==");
    match &snapshot_args {
        Some(args) => println!(
            "After you consent and before the change, a read-only btrfs snapshot is taken:\n  {}{}",
            safety::helper_shown(
                safety::Helper::SnapshotHook,
                &helper_from(safety::Helper::SnapshotHook)?,
                args
            ),
            if execute { "" } else { " (not under --dryrun)" }
        ),
        None => println!(
            "(none; pass --snapshot-tag <tag> or set safety.require_snapshot_before_upgrade for upgrades)"
        ),
    }

    println!("\n== Commit/apply step ==");
    if !execute {
        println!(
            "No snapshot was taken and no package command was run.\n{}",
            prompt::DRY_RUN_FOOTER
        );
        return Ok(());
    }
    confirm_gate(yes, noninteractive, script_mode)?;

    // ---- after consent: snapshot, then the change
    if let Some(args) = &snapshot_args {
        println!("Taking the snapshot...");
        if !run_helper_step(safety::Helper::SnapshotHook, args)? {
            return Err("the snapshot could not be taken; nothing was changed".into());
        }
    }
    let mut reached = Reached::BeforeNative;
    let pacman_before = if op == "arch-install" {
        installed_for_rollback("before")
    } else {
        None
    };
    let flatpak_before: Option<HashSet<String>> = (op == "flatpak-install").then(|| {
        targets
            .iter()
            .filter(|t| backend::flatpak_installed(t, false))
            .cloned()
            .collect()
    });
    let exec_res: Result<(), String> = match (op, &prepared) {
        (_, Some(action)) => run_action(action, noninteractive, script_mode, &mut reached),
        ("flatpak-install", _) => {
            ensure_privileges(
                "flatpak-install-system",
                true,
                false,
                noninteractive,
                script_mode,
            )?;
            reached = Reached::NativeRan;
            targets.iter().try_for_each(|t| {
                println!("Executing flatpak install {t}...");
                println!("{}", backend::flatpak_install(t, false)?);
                Ok(())
            })
        }
        ("flatpak-remove", _) => {
            ensure_privileges(
                "flatpak-remove-system",
                true,
                false,
                noninteractive,
                script_mode,
            )?;
            reached = Reached::NativeRan;
            targets.iter().try_for_each(|t| {
                println!("Executing flatpak uninstall {t}...");
                println!("{}", backend::flatpak_remove(t, false)?);
                Ok(())
            })
        }
        ("flatpak-update", _) => {
            ensure_privileges(
                "flatpak-update-system",
                true,
                false,
                noninteractive,
                script_mode,
            )?;
            reached = Reached::NativeRan;
            if targets.is_empty() {
                println!("Executing flatpak update (all)...");
                backend::flatpak_update(None, false).map(|out| println!("{out}"))
            } else {
                targets.iter().try_for_each(|t| {
                    println!("Executing flatpak update {t}...");
                    println!("{}", backend::flatpak_update(Some(t), false)?);
                    Ok(())
                })
            }
        }
        _ => Err(format!("unsupported safety apply op: {op}")),
    };
    let mut final_res = exec_res;
    if final_res.is_ok() {
        if let Some(hc) = health_check {
            println!("\n== Post-apply health check ==");
            println!("Running: {hc}");
            let out = Command::new("bash")
                .args(["-lc", hc])
                .output()
                .map_err(|e| format!("failed to run health check: {e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stdout.trim().is_empty() {
                print!("{}", stdout);
                if !stdout.ends_with('\n') {
                    println!();
                }
            }
            if !stderr.trim().is_empty() {
                eprint!("{}", stderr);
            }
            if out.status.success() {
                println!("Health check: PASS");
            } else {
                println!("Health check: FAIL (status: {})", out.status);
                final_res = Err("post-apply health check failed".into());
            }
        } else {
            println!("Post-apply health check: skipped (no --health-check provided)");
        }
    }
    let e = match final_res {
        Ok(()) => {
            println!("Apply completed. {SNAPSHOT_WHERE}");
            return Ok(());
        }
        Err(e) => e,
    };
    println!("Apply/health step failed: {e}");
    if !auto_rollback {
        println!("Nothing is rolled back (--auto-rollback was not given). {SNAPSHOT_WHERE}");
        return Err(e);
    }
    println!("== Auto-rollback ==");
    match op {
        "arch-install" => {
            let after = if reached == Reached::NativeRan {
                installed_for_rollback("after")
            } else {
                None
            };
            let rb = rollback_for(targets, pacman_before.as_ref(), after.as_ref(), reached);
            for line in rollback_report(&rb, "pacman") {
                println!("{line}");
            }
            if let Rollback::Decided { remove, .. } = &rb
                && !remove.is_empty()
            {
                let prefix = backend::pacman_elevation(noninteractive || script_mode);
                let argv = backend::rollback_remove_argv(remove);
                println!("Rollback command: {}", crate::ops::shown(&prefix, &argv));
                io::stdout().flush().ok();
                let full: Vec<&String> = prefix.iter().chain(argv.iter()).collect();
                match Command::new(full[0]).args(&full[1..]).status() {
                    Ok(s) if s.success() => println!(
                        "Auto-rollback: removed {} (pacman -R: their dependencies and configuration files stay).",
                        remove.join(", ")
                    ),
                    Ok(s) => println!(
                        "Auto-rollback: the removal failed ({s}); {} may still be installed.",
                        remove.join(", ")
                    ),
                    Err(err) => println!("Auto-rollback: could not run {}: {err}", full[0]),
                }
            }
        }
        "flatpak-install" => {
            let after: Option<HashSet<String>> = (reached == Reached::NativeRan).then(|| {
                targets
                    .iter()
                    .filter(|t| backend::flatpak_installed(t, false))
                    .cloned()
                    .collect()
            });
            let rb = rollback_for(targets, flatpak_before.as_ref(), after.as_ref(), reached);
            for line in rollback_report(&rb, "flatpak") {
                println!("{line}");
            }
            if let Rollback::Decided { remove, .. } = &rb {
                for t in remove {
                    println!("Rollback command: flatpak uninstall --system -y -- {t}");
                    match backend::flatpak_remove(t, false) {
                        Ok(_) => println!("Auto-rollback: uninstalled {t}."),
                        Err(err) => println!("Auto-rollback: uninstalling {t} failed: {err}"),
                    }
                }
            }
        }
        _ => println!(
            "Auto-rollback: no automatic rollback exists for {op}; nothing was rolled back. {SNAPSHOT_WHERE}"
        ),
    }
    Err(e)
}

#[cfg(test)]
mod flow_tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_package_installed_before_is_never_rolled_back() {
        // the review's case: base-devel was installed, pacman -S failed
        let before = set(&["base-devel", "gcc", "make"]);
        let rb = rollback_for(
            &names(&["base-devel"]),
            Some(&before),
            Some(&before),
            Reached::NativeRan,
        );
        assert_eq!(
            rb,
            Rollback::Decided {
                remove: vec![],
                kept: names(&["base-devel"]),
                extra: vec![],
            }
        );
        let said = rollback_report(&rb, "pacman").join("\n");
        assert!(
            said.contains("Not removed, because installed before this operation: base-devel"),
            "{said}"
        );
        assert!(said.contains("nothing is removed"), "{said}");
    }

    #[test]
    fn only_targets_absent_before_and_present_after_are_removed() {
        let before = set(&["glibc", "htop"]);
        // tree was installed with a new dependency; htop was only upgraded;
        // nope never made it
        let after = set(&["glibc", "htop", "tree", "libfoo"]);
        let rb = rollback_for(
            &names(&["extra/tree", "htop", "nope", "tree"]),
            Some(&before),
            Some(&after),
            Reached::NativeRan,
        );
        assert_eq!(
            rb,
            Rollback::Decided {
                remove: names(&["tree"]),
                kept: names(&["htop"]),
                extra: names(&["libfoo"]),
            }
        );
        let said = rollback_report(&rb, "pacman").join("\n");
        // a failed health check on an upgraded package: guidance, no removal
        assert!(said.contains("htop") && said.contains("yapacc undo --dryrun"));
        assert!(said.contains("Left installed") && said.contains("libfoo"));
        assert!(!said.contains("nothing is removed"));
    }

    #[test]
    fn no_rollback_when_the_failure_came_before_pacman_ran() {
        // privileges, snapshot, validation: even a target that is absent
        // before is not touched, whatever the sets say
        let before = set(&[]);
        let after = set(&["tree"]);
        let rb = rollback_for(
            &names(&["tree"]),
            Some(&before),
            Some(&after),
            Reached::BeforeNative,
        );
        assert_eq!(rb, Rollback::NothingRan);
        let said = rollback_report(&rb, "pacman").join("\n");
        assert!(said.contains("before pacman ran"), "{said}");
    }

    #[test]
    fn an_unreadable_package_list_means_unknown_not_empty() {
        let after = set(&["tree"]);
        for (b, a) in [(None, Some(&after)), (Some(&after), None), (None, None)] {
            let rb = rollback_for(&names(&["tree"]), b, a, Reached::NativeRan);
            assert_eq!(rb, Rollback::Unknown);
            assert!(rollback_report(&rb, "pacman")[0].contains("removes none"));
        }
    }

    #[test]
    fn the_rollback_command_removes_the_named_packages_only() {
        let argv = backend::rollback_remove_argv(&names(&["tree"]));
        assert_eq!(argv, ["pacman", "-R", "--noconfirm", "--", "tree"]);
        // never the recursive or the config-deleting form
        assert!(!argv.iter().any(|a| a.contains('s') && a.starts_with("-R")));
        assert!(!argv.iter().any(|a| a.contains('n') && a.starts_with("-R")));
    }

    #[test]
    fn bare_names() {
        assert_eq!(bare_name("extra/htop"), "htop");
        assert_eq!(bare_name("htop"), "htop");
    }

    #[test]
    fn yay_is_refused_when_nobody_can_answer_its_review() {
        // --yes at a terminal: yay runs, and asks its own questions
        assert!(yay_needs_a_terminal("install", true, false, false, true).is_none());
        // --script, --noconfirm, or a pipe on stdin: refused
        for (nonint, script, tty) in [
            (true, false, true),
            (true, true, true),
            (false, false, false),
        ] {
            let why = yay_needs_a_terminal("install", true, nonint, script, tty).unwrap();
            assert!(why.contains("Nothing was changed"), "{why}");
            assert!(why.contains("install aur PKG --no-review"), "{why}");
        }
        let why = yay_needs_a_terminal("upgrade", true, true, true, false).unwrap();
        assert!(why.contains("yapacc upgrade --aur"), "{why}");
        // not a yay run: no opinion
        assert!(yay_needs_a_terminal("install", false, true, true, false).is_none());
        assert!(yay_needs_a_terminal("remove", true, true, true, false).is_none());
        // and the refusal exits 3
        let e = typed_error(format!("{REFUSED}{why}"));
        assert_eq!(e.kind.exit_code(), 3);
    }

    #[test]
    fn a_failed_simulation_stops_everything_but_an_upgrade() {
        for op in ["install", "remove", "install-local"] {
            assert!(simulation_blocks(op, 1), "{op}");
            assert!(!simulation_blocks(op, 0), "{op}");
        }
        // -Su --print fails on a machine never refreshed; -Syu would not
        assert!(!simulation_blocks("upgrade", 1));
    }

    #[test]
    fn the_closing_line_follows_what_the_installed_packages_say() {
        assert_eq!(
            closing_line("update", Some(false)),
            "Done: the repository databases were refreshed; no package was changed."
        );
        // an install that left names and versions as they were: a reinstall
        let same = closing_line("install", Some(false));
        assert!(same.starts_with("No package version was changed"), "{same}");
        assert!(!same.contains("nothing to do"), "{same}");
        assert!(closing_line("install", Some(true)).starts_with("Done."));
        assert!(closing_line("remove", Some(true)).contains("`yapacc undo` can take it back"));
        // an upgrade with nothing pending
        assert_eq!(
            closing_line("upgrade", Some(false)),
            "No package was changed: pacman refreshed its repository indexes and had nothing to upgrade (the installed packages are the same as before)."
        );
        assert_eq!(
            closing_line("remove", Some(false)),
            "Nothing was changed: pacman had nothing to do (the installed packages are the same as before)."
        );
        // not known: the plain line
        assert!(closing_line("upgrade", None).starts_with("Done."));
    }

    #[test]
    fn a_list_from_the_last_refresh_says_so() {
        let missing = backend::CheckUpdates::Missing;
        let n = stale_list_note(Some(3 * 86_400 + 4 * 3600), &missing);
        assert!(
            n.contains("as of the last repository refresh (3 d 4 h ago)"),
            "{n}"
        );
        assert!(n.contains("may include more packages"), "{n}");
        assert!(n.ends_with("Install pacman-contrib for a current list (checkupdates)."));
        assert!(stale_list_note(None, &missing).contains("never"));
        // A6: pacman-contrib is installed and checkupdates failed: say that,
        // and why, not "install pacman-contrib"
        let failed =
            backend::CheckUpdates::Failed("status 1: Cannot find the fakeroot binary".into());
        let n = stale_list_note(Some(60), &failed);
        assert!(
            n.ends_with(
                "`checkupdates` is installed but failed (status 1: Cannot find the fakeroot binary), so there is no current list."
            ),
            "{n}"
        );
        assert!(!n.contains("Install pacman-contrib"), "{n}");
        assert_eq!(backend::age_text(59), "0 min");
        assert_eq!(backend::age_text(2 * 3600 + 60), "2 h");
    }

    #[test]
    fn a_refused_pkgbuild_review_exits_3_and_says_what_is_left() {
        let tail = review_refusal_tail(&names(&["./foo-aur-build"]));
        assert!(tail.contains("Nothing was built or installed"));
        assert!(tail.contains("./foo-aur-build"));
        let e = typed_error(format!(
            "{REFUSED}Aborted at the PKGBUILD review gate. {tail}"
        ));
        assert_eq!(e.kind, crate::Kind::Refused);
        assert_eq!(e.kind.exit_code(), 3);
        assert!(
            e.to_string()
                .starts_with("Aborted at the PKGBUILD review gate.")
        );
    }

    #[test]
    fn hostile_targets_are_refused_before_anything_is_shown_or_run() {
        for bad in [
            "--config=/tmp/evil-pacman.conf",
            "--hookdir=/tmp/hooks",
            "a;id",
        ] {
            for op in ["install", "remove"] {
                let e =
                    plan_action(op, &names(&["htop", bad]), false, false, true, true).unwrap_err();
                assert!(e.contains("is not a valid package name"), "{e}");
                // A2: a name that is not one is a usage error (exit 2) on
                // every path into the pacman lane
                assert_eq!(typed_error(e).kind.exit_code(), 2);
            }
        }
        let e = plan_action("install", &[], false, false, true, true).unwrap_err();
        assert_eq!(typed_error(e).kind.exit_code(), 2);
    }
}
