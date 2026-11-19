// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Install verbs (every lane).

use super::*;

// ----- action dispatchers ---------------------------------------------------

pub(super) fn dispatch_install(
    iv: InstallVerb,
    noninteractive: bool,
    script_mode: bool,
    json: bool,
) -> Result<(), String> {
    let _ = json;
    match iv {
        InstallVerb::Repo { pkgs, aur, gate } => {
            let execute = gate_execute(&gate);
            action_flow(
                "install",
                pkgs,
                aur,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::Aur {
            pkg,
            url,
            dir,
            build_user,
            no_review,
            deps,
            build: rflags,
            gate,
        } => aur_install_flow(
            &pkg,
            url.as_deref(),
            dir.as_deref(),
            build_user.as_deref(),
            no_review,
            deps,
            rflags,
            gate_execute(&gate),
            gate.yes,
            noninteractive,
            script_mode,
        ),
        InstallVerb::Local { files, gate } => {
            let execute = gate_execute(&gate);
            if files.is_empty() {
                return Err("install local requires at least one package file path".into());
            }
            println!(
                "== Local package install plan ==\nTargets: {}",
                files.join(", ")
            );
            action_flow(
                "install-local",
                files,
                false,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::Alpm {
            pkgs,
            providers,
            gate,
        } => {
            let execute = gate_execute(&gate);
            // a build without libalpm: said first, with the command that works
            if let Some(why) = alpm_not_built("install alpm", "install repo", true) {
                return Err(why);
            }
            if pkgs.is_empty() {
                return Err(format!("{USAGE}install alpm requires package name(s)"));
            }
            let provider_map: Vec<(String, String)> = providers
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .map(|(v, pkg)| (v.trim().to_string(), pkg.trim().to_string()))
                        .ok_or_else(|| {
                            format!("invalid --provider '{p}' (expected VIRTUAL=PACKAGE)")
                        })
                })
                .collect::<Result<_, _>>()?;
            // The commit runs in this process (libalpm prepare, then pacman)
            // and needs the pacman database lock, which only root can take.
            // Said before anything is planned, asked or elevated.
            if execute && alpm_needs_root(cfg!(feature = "alpm"), is_root()) {
                return Err(format!(
                    "{REFUSED}install alpm commits in this process and needs the pacman database lock, which only root can take: run it as root (for example `sudo yapacc install alpm {}`), or use `yapacc install repo`, which elevates the pacman command itself. Nothing was changed.",
                    pkgs.join(" ")
                ));
            }
            // `[safety] snapshot`, as the other pacman flows do it: the mode
            // is parsed first so that a bad value fails closed, the snapshot
            // is announced with the plan, and it is taken after consent and
            // before the commit
            let snap_mode = super::flows::snapshot_mode()?;
            let snap_what = format!("install alpm {}", pkgs.join(" "));
            // the plan is shown before the gate, also for a real run
            let p = crate::alpm_flows::plan_install_with_providers(&pkgs, &provider_map)
                .map_err(|e| {
                    // a build without libalpm says so itself; root has the lock
                    if is_root() || !cfg!(feature = "alpm") {
                        e.to_string()
                    } else {
                        format!(
                            "{e}\nThe libalpm plan takes the pacman database lock, which only root can take: run this as root, or use `yapacc install repo --dryrun`. Nothing was changed."
                        )
                    }
                })?;
            println!(
                "ALPM INSTALL PLAN (prepared, nothing committed)\nTo install ({}):",
                p.to_install.len()
            );
            for x in &p.to_install {
                println!("  + {x}");
            }
            println!("To remove ({}):", p.to_remove.len());
            for x in &p.to_remove {
                println!("  - {x}");
            }
            for n in &p.notes {
                println!("Note: {n}");
            }
            super::flows::snapshot_preview(
                snap_mode,
                &snap_what,
                !execute,
                noninteractive,
                script_mode,
            )?;
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            super::flows::snapshot_first(snap_mode, &snap_what, noninteractive, script_mode)?;
            let p = crate::alpm_flows::commit_install_with_providers(&pkgs, &provider_map)
                .map_err(|e| e.to_string())?;
            println!(
                "ALPM INSTALL COMMITTED\nInstalled ({}):",
                p.to_install.len()
            );
            for x in &p.to_install {
                println!("  + {x}");
            }
            println!("Removed ({}):", p.to_remove.len());
            for x in &p.to_remove {
                println!("  - {x}");
            }
            for n in &p.notes {
                println!("Note: {n}");
            }
            Ok(())
        }
        InstallVerb::Flatpak { app, user, gate } => {
            let execute = gate_execute(&gate);
            println!("{}", backend::flatpak_install_plan(&app, user)?);
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            ensure_privileges(
                if user {
                    "flatpak-install-user"
                } else {
                    "flatpak-install-system"
                },
                execute,
                user,
                noninteractive,
                script_mode,
            )?;
            println!("Executing flatpak install...");
            println!("{}", backend::flatpak_install(&app, user)?);
            // ledger is created/updated automatically on install
            let scope = if user { "user" } else { "system" };
            let lroot = ledger_root(scope, None)?;
            let rec = ledger::InstallRecord {
                id: String::new(),
                name: app.clone(),
                format: "flatpak".into(),
                source: "flatpak".into(),
                version: "unknown".into(),
                scope: scope.into(),
                install_root: if user {
                    "flatpak:user".into()
                } else {
                    "flatpak:system".into()
                },
                paths: vec![],
                status: "installed".into(),
                installed_at: 0,
                updated_at: 0,
                metadata: std::collections::BTreeMap::new(),
            };
            match ledger::upsert_record(&lroot, rec) {
                Ok(r) => println!(
                    "Ledger: recorded flatpak install ({scope}) for {app} ({}).",
                    r.id
                ),
                Err(e) => {
                    return Err(format!(
                        "flatpak install succeeded but ledger recording failed: {e}"
                    ));
                }
            }
            Ok(())
        }
        InstallVerb::Appimage {
            name,
            source,
            dest,
            ledger_path,
            sha256,
            allow_http,
            gate,
        } => {
            let execute = gate_execute(&gate);
            let opts = appimage::FetchOptions {
                sha256: sha256.as_deref(),
                allow_http,
            };
            let dest_dir = dest
                .map(std::path::PathBuf::from)
                .unwrap_or_else(appimage::default_dest_dir);
            println!(
                "{}",
                appimage::install_plan_with(&name, &source, &dest_dir, &opts)
            );
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            // refused before the gate and before anything is fetched: a plain
            // http:// download without --allow-http, a malformed --sha256
            carry(appimage::check_source(&source, &opts))?;
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let rec = carry(appimage::install(&name, &source, &dest_dir, &lroot, &opts))?;
            println!(
                "AppImage installed and recorded in ledger: {} ({}) -> {}",
                rec.id, rec.name, rec.install_root
            );
            println!("{}", appimage::verification_note(&rec));
            Ok(())
        }
        InstallVerb::Foreign {
            name,
            format,
            source,
            prefix,
            ledger_path,
            replace,
            expose,
            non_free,
            gate,
        } => {
            let execute = gate_execute(&gate);
            let expose_bins = resolve_expose(expose.as_deref())?;
            let prefix = prefix
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| foreign_extract::default_prefix(&name, &format));
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            println!(
                "{}",
                foreign_extract::plan_body(&name, &format, &source, &prefix)
            );
            println!(
                "NOT verified: `install foreign` checks no signature and no hash; the file is installed as it is."
            );
            if let Ok(recs) = ledger::list_records(&lroot)
                && let Some(existing) = recs.into_iter().find(|r| {
                    r.status != "removed"
                        && r.name == name
                        && r.format == format!("{}-extract", format)
                })
            {
                if replace {
                    println!(
                        "Conflict resolution: existing active record {} ({}) will be replaced.",
                        existing.id, existing.install_root
                    );
                } else {
                    return Err(format!(
                        "ledger conflict: active install already exists for {} ({}), id={}. Re-run with --replace, or use update foreign.",
                        name, existing.format, existing.id
                    ));
                }
            }
            if !execute {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            // non-free licence gate (deb Section / rpm License): after the
            // plan, never in a dry run, and --yes does not answer it
            if Path::new(&source).exists() {
                let analysis = foreign_extract::analyze_package(&format, Path::new(&source));
                if let Some(why) = foreign_extract::nonfree_descriptor(&analysis) {
                    licence_gate(
                        &[(name.clone(), why)],
                        non_free,
                        noninteractive,
                        script_mode,
                    )?;
                }
            }
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            if replace
                && let Ok(recs) = ledger::list_records(&lroot)
                && let Some(existing) = recs.into_iter().find(|r| {
                    r.status != "removed"
                        && r.name == name
                        && r.format == format!("{}-extract", format)
                })
            {
                let _ = foreign_extract::remove(&existing.id, &lroot)?;
            }
            let rec =
                foreign_extract::install(&name, &format, &source, &prefix, &lroot, expose_bins)?;
            println!(
                "Exposure mode: {}",
                if expose_bins {
                    "attributed (foreign-attributed bin names in prefix/bin)"
                } else {
                    "none (not added to PATH/exposed bins)"
                }
            );
            println!(
                "Foreign package extracted and recorded: {} ({}, {}) -> {} ({} files)",
                rec.id,
                rec.name,
                rec.format,
                rec.install_root,
                rec.paths.len()
            );
            Ok(())
        }
        InstallVerb::Tx {
            format,
            dir,
            name,
            sources,
            ledger_path,
            expose,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            let srcs = if let Some(d) = dir {
                if name.is_empty() {
                    return Err(
                        "install tx with --dir requires at least one --name <package>".into(),
                    );
                }
                foreign_extract::resolve_sources_from_dir(&format, Path::new(&d), &name)?
            } else {
                if sources.is_empty() {
                    return Err("provide artifact sources or --dir with --name".into());
                }
                sources
            };
            // URL sources are downloaded once, here: the plan below and the
            // install after the gate use these same files
            let staged = carry(foreign_extract::stage(&format, &srcs))?;
            print!("{}", staged.plan());
            println!(
                "NOT verified: `install tx` checks no signature and no hash; each file is installed as it was given or downloaded. For packages checked against a signed repository use `install tx-apt` or `install tx-rpm` with --keyring."
            );
            if !gate_execute(&gate) {
                // deletes what was downloaded for the plan
                drop(staged);
                println!(
                    "Files downloaded for this plan were deleted.\n{}",
                    prompt::DRY_RUN_FOOTER
                );
                return Ok(());
            }
            licence_gate(
                &foreign_extract::nonfree_artifacts(staged.artifacts()),
                non_free,
                noninteractive,
                script_mode,
            )?;
            confirm_gate(gate.yes, noninteractive, script_mode)?;
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let recs = staged.install(&lroot, expose_bins)?;
            println!(
                "Foreign transaction completed: {} package(s) installed; a failed step would have removed the installs of this run.",
                recs.len()
            );
            for r in &recs {
                println!(
                    "  - {} ({}, {}) -> {}",
                    r.id, r.name, r.version, r.install_root
                );
            }
            println!(
                "Ledger verify: {}",
                ledger::verify(&lroot, &ledger::VerifyOptions::default())
                    .map(|r| r.render())
                    .unwrap_or_else(|e| e.to_string())
            );
            Ok(())
        }
        InstallVerb::TxAptRepo {
            base_url,
            packages,
            name,
            ledger_path,
            expose,
            allow_unsigned,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            if name.is_empty() {
                return Err("install tx-apt-repo requires at least one --name <package>".into());
            }
            // no signature exists for a bare index: an install from a remote
            // one is refused in the fetch layer without --allow-unsigned
            let trust = fetch_trust(None, allow_unsigned, gate_execute(&gate));
            let resolved = carry(foreign_extract::resolve_apt_index(
                &base_url, &packages, &name, &trust,
            ))?;
            println!(
                "Resolved {} package(s) from the APT Packages index {}.",
                resolved.artifacts.len(),
                resolved.index
            );
            install_resolved(
                "deb",
                &resolved,
                ledger_path,
                expose_bins,
                non_free,
                &gate,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::TxApt {
            base_url,
            suite,
            component,
            arch,
            name,
            ledger_path,
            expose,
            keyring,
            allow_unsigned,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            if name.is_empty() {
                return Err("install tx-apt requires at least one --name <package>".into());
            }
            if gate_execute(&gate) {
                unsigned_gate(keyring.as_deref(), allow_unsigned)?;
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, gate_execute(&gate));
            let resolved = carry(foreign_extract::resolve_apt_repo(
                &base_url, &suite, &component, &arch, &name, &trust,
            ))?;
            println!(
                "Resolved {} package(s) from the APT repository (Packages index: {}).",
                resolved.artifacts.len(),
                resolved.index
            );
            install_resolved(
                "deb",
                &resolved,
                ledger_path,
                expose_bins,
                non_free,
                &gate,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::TxAptSources {
            root,
            arch,
            name,
            ledger_path,
            expose,
            keyring,
            allow_unsigned,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            if name.is_empty() {
                return Err("install tx-apt-sources requires at least one --name <package>".into());
            }
            if gate_execute(&gate) {
                unsigned_gate(keyring.as_deref(), allow_unsigned)?;
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, gate_execute(&gate));
            let (resolved, entry) = carry(foreign_extract::resolve_apt_sources_root(
                Path::new(&root),
                &name,
                &arch,
                &trust,
            ))?;
            println!(
                "Resolved {} package(s) from the parsed APT sources (index: {}, source: {entry}).",
                resolved.artifacts.len(),
                resolved.index
            );
            install_resolved(
                "deb",
                &resolved,
                ledger_path,
                expose_bins,
                non_free,
                &gate,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::TxRpmRepo {
            base_url,
            primary,
            name,
            ledger_path,
            expose,
            allow_unsigned,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            if name.is_empty() {
                return Err("install tx-rpm-repo requires at least one --name <package>".into());
            }
            // no signature exists for a bare index: an install from a remote
            // one is refused in the fetch layer without --allow-unsigned
            let trust = fetch_trust(None, allow_unsigned, gate_execute(&gate));
            let resolved = carry(foreign_extract::resolve_rpm_primary(
                &base_url, &primary, &name, &trust,
            ))?;
            println!(
                "Resolved {} package(s) from the RPM primary.xml {}.",
                resolved.artifacts.len(),
                resolved.index
            );
            install_resolved(
                "rpm",
                &resolved,
                ledger_path,
                expose_bins,
                non_free,
                &gate,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::TxRpm {
            base_url,
            name,
            ledger_path,
            expose,
            keyring,
            allow_unsigned,
            non_free,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            if name.is_empty() {
                return Err("install tx-rpm requires at least one --name <package>".into());
            }
            if gate_execute(&gate) {
                unsigned_gate(keyring.as_deref(), allow_unsigned)?;
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, gate_execute(&gate));
            let resolved = carry(foreign_extract::resolve_rpm_repo(&base_url, &name, &trust))?;
            println!(
                "Resolved {} package(s) from the RPM repository (repomd: {}, primary: {}).",
                resolved.artifacts.len(),
                resolved.release,
                resolved.index
            );
            install_resolved(
                "rpm",
                &resolved,
                ledger_path,
                expose_bins,
                non_free,
                &gate,
                noninteractive,
                script_mode,
            )
        }
        InstallVerb::Container { image, name, gate } => {
            let cname = container_name(&image, &name);
            let m = cross_format::container_mediation_plan(&image, &cname);
            println!("== Container create (mediation) ==");
            for c in &m.commands {
                println!("  planned: {c}");
            }
            if !m.available {
                return Err(format!("no supported engine available: {}", m.engine));
            }
            if !gate_execute(&gate) {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            println!("{}", cross_format::container_create(&m)?);
            Ok(())
        }
    }
}

/// How a typed error travels through the older call chain of `dispatch`,
/// which carries plain strings.
#[derive(Debug)]
enum Carried {
    /// as a string: a plain failure (exit 1), or a refusal with the
    /// `REFUSED` marker (exit 3)
    Message(String),
    /// the chain has no marker for this kind (integrity 5, unavailable 6,
    /// usage 2, not found 4): it is reported with its own exit code
    Exit(crate::Error),
}

fn classify(e: crate::Error) -> Carried {
    match e.kind {
        crate::Kind::Failed => Carried::Message(e.message),
        crate::Kind::Refused => Carried::Message(format!("{REFUSED}{}", e.message)),
        _ => Carried::Exit(e),
    }
}

/// Bring a typed result into the string chain of `dispatch` without losing
/// its exit code. An integrity failure must exit 5, and the chain would turn
/// it into 1, so it is reported from here. Nothing is left behind by
/// leaving at this point: the functions that return such an error have
/// already deleted what they fetched.
fn carry<T>(result: crate::error::Result<T>) -> Result<T, String> {
    result.map_err(|e| match classify(e) {
        Carried::Message(m) => m,
        Carried::Exit(e) => std::process::exit(crate::error::report(Err(e))),
    })
}

/// The install-time gate for an unverified remote repository, as a refusal
/// (exit 3) that says nothing was done.
fn unsigned_gate(keyring: Option<&str>, allow_unsigned: bool) -> Result<(), String> {
    enforce_unsigned_gate(keyring, allow_unsigned)
        .map_err(|e| format!("{REFUSED}{e}. Nothing was downloaded or installed."))
}

/// The non-free gate, per package: accepted with `--non-free` or by a yes at
/// the terminal. A no, and a run that cannot ask, are refusals (exit 3).
/// `--yes` consents to the install, not to a licence.
fn licence_gate(
    found: &[(String, String)],
    non_free: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    for (name, why) in found {
        match foreign_extract::nonfree_gate(
            &format!("{name}: {why}"),
            non_free,
            noninteractive,
            script_mode,
        ) {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!(
                    "{REFUSED}Licence declined for {name}: nothing was installed."
                ));
            }
            Err(e) => return Err(format!("{REFUSED}{e}")),
        }
    }
    Ok(())
}

/// Plan, gates and install for packages resolved from a repository index:
/// what was and was not verified, the plan from the index (nothing is
/// downloaded for it), the licence gate, consent, then one download per
/// package, each compared with its index hash before anything is extracted.
#[allow(clippy::too_many_arguments)]
fn install_resolved(
    format: &str,
    resolved: &foreign_extract::Resolved,
    ledger_path: Option<String>,
    expose_bins: bool,
    non_free: bool,
    gate: &Gate,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    println!("{}", resolved.trust_statement());
    print!(
        "{}",
        foreign_extract::resolved_plan(&resolved.artifacts, format)
    );
    if !gate_execute(gate) {
        println!("Nothing was downloaded.\n{}", prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    licence_gate(
        &foreign_extract::nonfree_artifacts(&resolved.artifacts),
        non_free,
        noninteractive,
        script_mode,
    )?;
    confirm_gate(gate.yes, noninteractive, script_mode)?;
    let staged = carry(foreign_extract::stage(format, &resolved.artifacts))?;
    let lroot = ledger_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
    let recs = staged.install(&lroot, expose_bins)?;
    println!(
        "Foreign transaction completed: {} package(s) installed (expose_bins={expose_bins}).",
        recs.len()
    );
    for r in &recs {
        println!(
            "  - {} ({}, {}) -> {}",
            r.id, r.name, r.version, r.install_root
        );
    }
    Ok(())
}

/// Whether `install alpm` must refuse because it is not run as root: the
/// in-process commit cannot take the pacman database lock otherwise. A
/// build without libalpm has nothing to commit and reports that instead.
fn alpm_needs_root(built_with_alpm: bool, is_root: bool) -> bool {
    built_with_alpm && !is_root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_from_the_fetch_layer_keeps_exit_code_3() {
        let refused: crate::error::Result<()> = Err(crate::Error::refused("no keyring"));
        let carried = carry(refused).unwrap_err();
        assert_eq!(carried, "REFUSED: no keyring");
        assert_eq!(typed_error(carried).kind, crate::Kind::Refused);
        // any other failure stays a plain failure
        let failed: crate::error::Result<()> = Err(crate::Error::from("curl failed"));
        assert_eq!(
            typed_error(carry(failed).unwrap_err()).kind,
            crate::Kind::Failed
        );
        assert_eq!(carry(Ok(7)), Ok(7));
    }

    #[test]
    fn an_integrity_failure_keeps_exit_code_5() {
        // the string chain would report these as exit 1: they leave with
        // their own code instead
        for (e, code) in [
            (crate::Error::integrity("checksum mismatch"), 5),
            (crate::Error::unavailable("gpgv is not installed"), 6),
            (crate::Error::usage("--sha256 must be 64 hex digits"), 2),
        ] {
            match classify(e) {
                Carried::Exit(e) => assert_eq!(e.kind.exit_code(), code),
                other => panic!("expected an exit with code {code}, got {other:?}"),
            }
        }
        assert!(matches!(
            classify(crate::Error::refused("no")),
            Carried::Message(m) if m == "REFUSED: no"
        ));
        assert!(matches!(
            classify(crate::Error::from("failed")),
            Carried::Message(m) if m == "failed"
        ));
    }

    #[test]
    fn a_declined_or_unanswerable_licence_is_a_refusal() {
        let found = [(
            "nvidia-driver".to_string(),
            "Section: non-free/libs".to_string(),
        )];
        // --non-free accepts, also without a terminal
        assert!(licence_gate(&found, true, true, true).is_ok());
        // a non-interactive run without --non-free cannot answer: exit 3
        for (noninteractive, script) in [(true, false), (false, true), (true, true)] {
            let e = licence_gate(&found, false, noninteractive, script).unwrap_err();
            assert!(e.starts_with(REFUSED), "{e}");
            assert!(e.contains("nvidia-driver: Section: non-free/libs"), "{e}");
            assert!(e.ends_with("Nothing was installed."), "{e}");
            assert_eq!(typed_error(e).kind, crate::Kind::Refused);
        }
        // the test process has no terminal on stdin: --yes does not answer
        // the licence question, so this is a refusal too
        let e = licence_gate(&found, false, false, false).unwrap_err();
        assert!(e.starts_with(REFUSED), "{e}");
        // nothing to accept: nothing asked
        assert!(licence_gate(&[], false, true, true).is_ok());
    }

    #[test]
    fn an_unverified_remote_repo_is_a_refusal_that_says_nothing_was_done() {
        if config::get().foreign.allow_unsigned_repos {
            return; // the host configuration accepts unsigned repositories
        }
        let e = unsigned_gate(None, false).unwrap_err();
        assert!(e.starts_with(REFUSED), "{e}");
        assert!(e.ends_with("Nothing was downloaded or installed."), "{e}");
        assert!(unsigned_gate(Some("/k.gpg"), false).is_ok());
        assert!(unsigned_gate(None, true).is_ok());
    }

    #[test]
    fn install_alpm_refuses_up_front_when_it_cannot_take_the_lock() {
        // built with libalpm, not root: refuse before any plan or prompt
        assert!(alpm_needs_root(true, false));
        assert!(!alpm_needs_root(true, true));
        // built without libalpm: the stub's own message is the right one
        assert!(!alpm_needs_root(false, false));
    }
}
