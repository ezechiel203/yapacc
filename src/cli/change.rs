// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Remove / update / mark verbs.

use super::*;

pub(super) fn dispatch_remove(
    rv: RemoveVerb,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    match rv {
        RemoveVerb::Repo { pkgs, gate } => {
            let execute = gate_execute(&gate);
            action_flow(
                "remove",
                pkgs,
                false,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        RemoveVerb::Orphans { gate } => {
            let execute = gate_execute(&gate);
            let pkgs = backend::list_orphans()?;
            if pkgs.is_empty() {
                println!("No orphan dependencies found. Nothing to remove.");
                return Ok(());
            }
            let names: Vec<String> = pkgs.iter().map(|p| p.name.clone()).collect();
            println!(
                "Remove plan (orphan dependencies): {} package(s)",
                names.len()
            );
            for p in &pkgs {
                println!(
                    "  - {} {} ({})  {}",
                    p.name,
                    p.version,
                    p.installed_size,
                    tr(&p.description, 60)
                );
            }
            action_flow(
                "remove",
                names,
                false,
                execute,
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        RemoveVerb::Alpm { pkgs, gate } => {
            let execute = gate_execute(&gate);
            // a build without libalpm: said first, with the command that works
            if let Some(why) = alpm_not_built("remove alpm", "remove repo", true) {
                return Err(why);
            }
            if pkgs.is_empty() {
                return Err(format!("{USAGE}remove alpm requires package name(s)"));
            }
            for name in &pkgs {
                if !crate::util::valid_pkg_name(name) {
                    return Err(format!("{USAGE}'{name}' is not a valid package name"));
                }
            }
            // Refused up front, plan or commit alike: libalpm needs the
            // database lock for the prepare step already. Asking for consent
            // (and a sudo password) first and failing on the lock afterwards
            // is what this replaces.
            if let Some(why) =
                crate::alpm_flows::needs_root(is_root(), "remove alpm", "remove repo")
            {
                return Err(format!("{REFUSED}{why}"));
            }
            // the plan, before the gate
            let plan = crate::alpm_flows::plan_remove(&pkgs).map_err(|e| e.to_string())?;
            println!(
                "ALPM REMOVE PLAN (prepared through libalpm; nothing committed)\nTo remove ({}):",
                plan.to_remove.len()
            );
            for x in &plan.to_remove {
                println!("  - {x}");
            }
            println!("To install ({}):", plan.to_install.len());
            for x in &plan.to_install {
                println!("  + {x}");
            }
            for n in &plan.notes {
                println!("Note: {n}");
            }
            println!(
                "\nWill run:\n  {}",
                crate::ops::shown(&[], &crate::alpm_flows::remove_commit_argv(&pkgs))
            );
            println!(
                "note: this is `pacman -R`: the named packages only. Their dependencies stay, and configuration files you modified are kept as .pacsave files."
            );
            let snap_mode = super::flows::snapshot_mode()?;
            let snap_what = format!("remove {}", pkgs.join(" "));
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
            // [safety] snapshot: after consent, before the commit
            super::flows::snapshot_first(snap_mode, &snap_what, noninteractive, script_mode)?;
            match crate::alpm_flows::commit_remove(&pkgs) {
                Ok(p) => {
                    println!("ALPM REMOVE COMMITTED\nRemoved ({}):", p.to_remove.len());
                    for x in &p.to_remove {
                        println!("  - {x}");
                    }
                    for n in &p.notes {
                        println!("Note: {n}");
                    }
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            }
        }
        RemoveVerb::Flatpak { app, user, gate } => {
            let execute = gate_execute(&gate);
            println!("{}", backend::flatpak_remove_plan(&app, user)?);
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
                    "flatpak-remove-user"
                } else {
                    "flatpak-remove-system"
                },
                execute,
                user,
                noninteractive,
                script_mode,
            )?;
            println!("Executing flatpak uninstall...");
            println!("{}", backend::flatpak_remove(&app, user)?);
            let scope = if user { "user" } else { "system" };
            if let Ok(lroot) = ledger_root(scope, None)
                && let Ok(recs) = ledger::list_records(&lroot)
            {
                for r in recs.into_iter().filter(|r| {
                    r.format == "flatpak"
                        && r.name == app
                        && r.scope == scope
                        && r.status != "removed"
                }) {
                    let mut rec = r;
                    rec.status = "removed".into();
                    let _ = ledger::upsert_record(&lroot, rec);
                }
            }
            println!("Ledger: marked flatpak remove ({scope}) for {app} (if tracked).");
            Ok(())
        }
        RemoveVerb::Appimage {
            id,
            ledger_path,
            gate,
        } => {
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let recs = ledger::list_records(&lroot)?;
            let rec = recs
                .into_iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("ledger record not found: {id}"))?;
            println!("{}", appimage::remove_plan(&rec));
            if !gate_execute(&gate) {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            let rec = appimage::remove(&id, &lroot)?;
            println!(
                "AppImage removed and ledger updated: {} ({}) status={}",
                rec.id, rec.name, rec.status
            );
            Ok(())
        }
        RemoveVerb::Foreign {
            id,
            ledger_path,
            gate,
        } => {
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            if !gate_execute(&gate) {
                println!(
                    "Plan: remove foreign extract record {id} and delete its prefix.\n{}",
                    prompt::DRY_RUN_FOOTER
                );
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            let rec = foreign_extract::remove(&id, &lroot)?;
            println!(
                "Foreign extract removed and ledger updated: {} ({}) status={}",
                rec.id, rec.name, rec.status
            );
            Ok(())
        }
        RemoveVerb::External { id, gate } => {
            let mut found: Option<(std::path::PathBuf, ledger::InstallRecord)> = None;
            for scope in [&ledger::Scope::User, &ledger::Scope::System] {
                let lroot = ledger::default_root(scope);
                if let Ok(recs) = ledger::list_records(&lroot)
                    && let Some(r) = recs.into_iter().find(|r| r.id == id)
                {
                    found = Some((lroot, r));
                    break;
                }
            }
            let (lroot, rec) = found.ok_or_else(|| format!("ledger record not found: {id}"))?;
            match rec.format.as_str() {
                "appimage" => {
                    println!("{}", appimage::remove_plan(&rec));
                    if !gate_execute(&gate) {
                        println!("{}", prompt::DRY_RUN_FOOTER);
                        return Ok(());
                    }
                    if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                        println!("Aborted.");
                        return Ok(());
                    }
                    let rec = appimage::remove(&id, &lroot)?;
                    println!(
                        "External remove complete (appimage): {} status={}",
                        rec.id, rec.status
                    );
                }
                "deb-extract" | "rpm-extract" | "snap-extract" | "pkg-extract" => {
                    println!(
                        "Plan: remove foreign extract record {} and delete prefix {}.",
                        rec.id, rec.install_root
                    );
                    if !gate_execute(&gate) {
                        println!("{}", prompt::DRY_RUN_FOOTER);
                        return Ok(());
                    }
                    if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                        println!("Aborted.");
                        return Ok(());
                    }
                    let rec = foreign_extract::remove(&id, &lroot)?;
                    println!(
                        "External remove complete ({}): {} status={}",
                        rec.format, rec.id, rec.status
                    );
                }
                "flatpak" => {
                    let user = rec.scope == "user" || rec.install_root.contains("user");
                    println!(
                        "Plan: flatpak uninstall {} (scope={}) for record {}.",
                        rec.name,
                        if user { "user" } else { "system" },
                        rec.id
                    );
                    if !gate_execute(&gate) {
                        println!("{}", prompt::DRY_RUN_FOOTER);
                        return Ok(());
                    }
                    if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                        println!("Aborted.");
                        return Ok(());
                    }
                    ensure_privileges(
                        if user {
                            "flatpak-remove-user"
                        } else {
                            "flatpak-remove-system"
                        },
                        true,
                        user,
                        noninteractive,
                        script_mode,
                    )?;
                    println!("{}", backend::flatpak_remove(&rec.name, user)?);
                    let mut r = rec.clone();
                    r.status = "removed".into();
                    let _ = ledger::upsert_record(&lroot, r);
                    println!("External remove complete (flatpak): {id} marked removed.");
                }
                other => {
                    return Err(format!(
                        "remove external not supported for format '{other}'"
                    ));
                }
            }
            Ok(())
        }
    }
}

pub(super) fn dispatch_update(
    uv: UpdateVerb,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    match uv {
        UpdateVerb::Flatpak { app, user, gate } => {
            print!("{}", backend::flatpak_update_plan(app.as_deref(), user));
            if !gate_execute(&gate) {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            ensure_privileges(
                if user {
                    "flatpak-update-user"
                } else {
                    "flatpak-update-system"
                },
                true,
                user,
                noninteractive,
                script_mode,
            )?;
            println!("Executing flatpak update...");
            println!("{}", backend::flatpak_update(app.as_deref(), user)?);
            Ok(())
        }
        UpdateVerb::Appimage {
            id,
            source,
            ledger_path,
            gate,
        } => {
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let recs = ledger::list_records(&lroot)?;
            let rec = recs
                .into_iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("ledger record not found: {id}"))?;
            println!("{}", appimage::update_plan(&rec, &source));
            if !gate_execute(&gate) {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            let (_old, newr) = appimage::update(&id, &source, &lroot)?;
            println!(
                "AppImage updated and ledger recorded: new={} ({}) path={} source={}",
                newr.id, newr.name, newr.install_root, newr.source
            );
            Ok(())
        }
        UpdateVerb::Foreign {
            id,
            source,
            ledger_path,
            expose,
            gate,
        } => {
            let expose_bins = resolve_expose(expose.as_deref())?;
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let recs = ledger::list_records(&lroot)?;
            let rec = recs
                .into_iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("ledger record not found: {id}"))?;
            println!("{}", foreign_extract::update_plan(&rec, &source));
            if !gate_execute(&gate) {
                println!("{}", prompt::DRY_RUN_FOOTER);
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            let (_old, newr) = foreign_extract::update(&id, &source, &lroot, expose_bins)?;
            println!(
                "Foreign extract updated and ledger recorded: new={} ({}, {}) prefix={} source={}",
                newr.id, newr.name, newr.format, newr.install_root, newr.source
            );
            Ok(())
        }
        UpdateVerb::External { id, source, gate } => {
            let mut found: Option<(std::path::PathBuf, ledger::InstallRecord)> = None;
            for scope in [&ledger::Scope::User, &ledger::Scope::System] {
                let lroot = ledger::default_root(scope);
                if let Ok(recs) = ledger::list_records(&lroot)
                    && let Some(r) = recs.into_iter().find(|r| r.id == id)
                {
                    found = Some((lroot, r));
                    break;
                }
            }
            let (lroot, rec) = found.ok_or_else(|| format!("ledger record not found: {id}"))?;
            match rec.format.as_str() {
                "appimage" => {
                    println!("{}", appimage::update_plan(&rec, &source));
                    if !gate_execute(&gate) {
                        println!("{}", prompt::DRY_RUN_FOOTER);
                        return Ok(());
                    }
                    if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                        println!("Aborted.");
                        return Ok(());
                    }
                    let (_old, newr) = appimage::update(&id, &source, &lroot)?;
                    println!(
                        "External update complete (appimage): {} -> {}",
                        newr.id, newr.source
                    );
                }
                "deb-extract" | "rpm-extract" | "snap-extract" | "pkg-extract" => {
                    println!("{}", foreign_extract::update_plan(&rec, &source));
                    if !gate_execute(&gate) {
                        println!("{}", prompt::DRY_RUN_FOOTER);
                        return Ok(());
                    }
                    if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                        println!("Aborted.");
                        return Ok(());
                    }
                    let expose = rec
                        .metadata
                        .get("expose_bins")
                        .map(|v| v != "false")
                        .unwrap_or(true);
                    let (_old, newr) = foreign_extract::update(&id, &source, &lroot, expose)?;
                    println!(
                        "External update complete ({}): {} -> {} (expose_bins={})",
                        newr.format, newr.id, newr.source, expose
                    );
                }
                other => {
                    return Err(format!(
                        "update external not supported for format '{other}' (supported: appimage, deb-extract, rpm-extract)"
                    ));
                }
            }
            Ok(())
        }
    }
}

pub(super) fn dispatch_mark(
    mv: MarkVerb,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    let (pkgs, as_explicit, gate) = match mv {
        MarkVerb::Explicit { pkgs, gate } => (pkgs, true, gate),
        MarkVerb::Dependency { pkgs, gate } => (pkgs, false, gate),
    };
    let execute = gate_execute(&gate);
    let what = if as_explicit {
        "explicit"
    } else {
        "dependency"
    };
    if pkgs.is_empty() {
        return Err(format!("mark {what} requires at least one package"));
    }
    for name in &pkgs {
        if !crate::util::valid_pkg_name(name) {
            return Err(format!("'{name}' is not a valid package name"));
        }
    }
    // `pacman -D` writes the local database: it runs behind the same
    // privilege prefix as every other pacman change, and is shown with it
    let prefix = backend::pacman_elevation(noninteractive || script_mode);
    print!("{}", backend::mark_plan(&pkgs, as_explicit, &prefix));
    if !execute {
        println!("{}", prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    confirm_gate(gate.yes, noninteractive, script_mode)?;
    ensure_privileges("mark", true, false, noninteractive, script_mode)?;
    let out = backend::mark_reason(&pkgs, as_explicit, &prefix)?;
    if !out.trim().is_empty() {
        print!("{out}");
    }
    println!("Marked as {what}: {}", pkgs.join(", "));
    Ok(())
}
