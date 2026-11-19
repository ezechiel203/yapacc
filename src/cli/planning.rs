// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Plan-as-data and safety verbs.

use super::*;

/// Trust handed to the fetch layer. An install gets exactly what the flags
/// and the configuration say. A plan or a dry run installs nothing, so it
/// may read the metadata of a repository that has no keyring.
pub(super) fn fetch_trust(
    keyring: Option<&str>,
    allow_unsigned_flag: bool,
    installing: bool,
) -> foreign_extract::RepoTrust<'_> {
    let mut trust = repo_trust(keyring, allow_unsigned_flag);
    if !installing && keyring.is_none() {
        trust.allow_unsigned = true;
    }
    trust
}

// ----- plan dispatcher -------------------------------------------------------

pub(super) fn dispatch_plan(
    pv: PlanVerb,
    noninteractive: bool,
    script_mode: bool,
    json: bool,
) -> Result<(), String> {
    let _ = (noninteractive, script_mode);
    match pv {
        PlanVerb::Install { pkgs, aur, out } => {
            let plan = plan::build_install_plan(&pkgs, aur)?;
            if let Some(path) = &out {
                plan.save(Path::new(path))?;
                println!("Plan saved: {path} (apply later: yapacc plan apply {path} --yes)");
            }
            if json {
                print_json(&plan);
                return Ok(());
            }
            print!("{}", plan.render());
            Ok(())
        }
        PlanVerb::Remove { pkgs, out } => {
            let plan = plan::build_remove_plan(&pkgs)?;
            if let Some(path) = &out {
                plan.save(Path::new(path))?;
                println!("Plan saved: {path} (apply later: yapacc plan apply {path} --yes)");
            }
            if json {
                print_json(&plan);
                return Ok(());
            }
            print!("{}", plan.render());
            Ok(())
        }
        PlanVerb::Upgrade { aur, out } => {
            let plan = plan::build_upgrade_plan(aur)?;
            if let Some(path) = &out {
                plan.save(Path::new(path))?;
                println!("Plan saved: {path} (apply later: yapacc plan apply {path} --yes)");
            }
            if json {
                print_json(&plan);
                return Ok(());
            }
            print!("{}", plan.render());
            Ok(())
        }
        PlanVerb::Apply { file, force, gate } => {
            if !Path::new(&file).exists() {
                return Err(format!("{NOT_FOUND}there is no plan file at {file}"));
            }
            apply_plan_flow(
                &file,
                force,
                gate_execute(&gate),
                gate.yes,
                noninteractive,
                script_mode,
            )
        }
        PlanVerb::Flatpak { app, user } => {
            println!("{}", backend::flatpak_install_plan(&app, user)?);
            println!("{}", prompt::PLAN_ONLY_FOOTER);
            Ok(())
        }
        PlanVerb::FlatpakRemove { app, user } => {
            println!("{}", backend::flatpak_remove_plan(&app, user)?);
            println!("{}", prompt::PLAN_ONLY_FOOTER);
            Ok(())
        }
        PlanVerb::FlatpakUpdate { app, user } => {
            print!("{}", backend::flatpak_update_plan(app.as_deref(), user));
            println!("{}", prompt::PLAN_ONLY_FOOTER);
            Ok(())
        }
        PlanVerb::Appimage { name, source, dest } => {
            let dest_dir = dest
                .map(std::path::PathBuf::from)
                .unwrap_or_else(appimage::default_dest_dir);
            println!("{}", appimage::install_plan(&name, &source, &dest_dir));
            Ok(())
        }
        PlanVerb::Foreign {
            name,
            format,
            source,
            prefix,
        } => {
            let prefix = prefix
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| foreign_extract::default_prefix(&name, &format));
            println!(
                "{}",
                foreign_extract::plan(&name, &format, &source, &prefix)
            );
            Ok(())
        }
        PlanVerb::Tx {
            format,
            dir,
            name,
            sources,
        } => {
            let srcs = if let Some(d) = dir {
                if name.is_empty() {
                    return Err("plan tx with --dir requires at least one --name <package>".into());
                }
                foreign_extract::resolve_sources_from_dir(&format, Path::new(&d), &name)?
            } else {
                if sources.is_empty() {
                    return Err("provide artifact sources or --dir with --name".into());
                }
                sources
            };
            println!("{}", foreign_extract::tx_plan(&format, &srcs)?);
            Ok(())
        }
        PlanVerb::TxAptRepo {
            base_url,
            packages,
            name,
        } => {
            if name.is_empty() {
                return Err("plan tx-apt-repo requires at least one --name <package>".into());
            }
            let srcs =
                foreign_extract::resolve_sources_from_apt_packages(&base_url, &packages, &name)?;
            println!(
                "Resolved {} artifact URL(s) from APT Packages index.",
                srcs.len()
            );
            println!("{}", foreign_extract::tx_plan("deb", &srcs)?);
            Ok(())
        }
        PlanVerb::TxApt {
            base_url,
            suite,
            component,
            arch,
            name,
            keyring,
            allow_unsigned,
        } => {
            if name.is_empty() {
                return Err("plan tx-apt requires at least one --name <package>".into());
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, false);
            let (srcs, used) = foreign_extract::resolve_sources_from_apt_repo(
                &base_url, &suite, &component, &arch, &name, &trust,
            )?;
            println!(
                "Resolved {} artifact URL(s) from APT repo (Packages index: {used}; GPG verification: {}).",
                srcs.len(),
                if keyring.is_some() {
                    "keyring"
                } else {
                    "not requested (plan); installs require --keyring or --allow-unsigned"
                }
            );
            println!("{}", foreign_extract::tx_plan("deb", &srcs)?);
            Ok(())
        }
        PlanVerb::TxAptSources {
            root,
            arch,
            name,
            keyring,
            allow_unsigned,
        } => {
            if name.is_empty() {
                return Err("plan tx-apt-sources requires at least one --name <package>".into());
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, false);
            let (srcs, used, srcinfo) = foreign_extract::resolve_sources_from_apt_sources_root(
                Path::new(&root),
                &name,
                &arch,
                &trust,
            )?;
            println!(
                "Resolved {} artifact URL(s) from parsed APT sources (index: {used}, source: {srcinfo}; GPG verification: {}).",
                srcs.len(),
                if keyring.is_some() {
                    "keyring"
                } else {
                    "not requested (plan); installs require --keyring or --allow-unsigned"
                }
            );
            println!("{}", foreign_extract::tx_plan("deb", &srcs)?);
            Ok(())
        }
        PlanVerb::TxRpmRepo {
            base_url,
            primary,
            name,
        } => {
            if name.is_empty() {
                return Err("plan tx-rpm-repo requires at least one --name <package>".into());
            }
            let arts =
                foreign_extract::resolve_artifacts_from_rpm_primary(&base_url, &primary, &name)?;
            println!("Resolved {} artifact(s) from RPM primary.xml.", arts.len());
            print!("{}", foreign_extract::tx_plan_resolved(&arts, "rpm"));
            Ok(())
        }
        PlanVerb::TxRpm {
            base_url,
            name,
            keyring,
            allow_unsigned,
        } => {
            if name.is_empty() {
                return Err("plan tx-rpm requires at least one --name <package>".into());
            }
            let trust = fetch_trust(keyring.as_deref(), allow_unsigned, false);
            let (arts, primary, repomd) =
                foreign_extract::resolve_artifacts_from_rpm_repo(&base_url, &name, &trust)?;
            println!(
                "Resolved {} artifact(s) from RPM repo (repomd: {repomd}, primary: {primary}; GPG verification: {}).",
                arts.len(),
                if keyring.is_some() {
                    "keyring"
                } else {
                    "not requested (plan); installs require --keyring or --allow-unsigned"
                }
            );
            print!("{}", foreign_extract::tx_plan_resolved(&arts, "rpm"));
            Ok(())
        }
        PlanVerb::Container { image, name } => {
            let cname = container_name(&image, &name);
            let m = cross_format::container_mediation_plan(&image, &cname);
            if json {
                print_json(&m);
                return Ok(());
            }
            println!("== Container mediation plan ==");
            println!("Image: {image}   Name: {cname}");
            println!("Engine: {} (available={})", m.engine, m.available);
            println!("Commands:");
            for c in &m.commands {
                println!("  {c}");
            }
            for n in &m.notes {
                println!("note: {n}");
            }
            Ok(())
        }
        PlanVerb::Cross { name, format } => {
            let fmt = match format.as_str() {
                "deb" => cross_format::PkgFormat::Deb,
                "rpm" => cross_format::PkgFormat::Rpm,
                "flatpak" => cross_format::PkgFormat::Flatpak,
                "snap" => cross_format::PkgFormat::Snap,
                "appimage" => cross_format::PkgFormat::AppImage,
                "arch" => cross_format::PkgFormat::Arch,
                other => cross_format::PkgFormat::Unknown(other.into()),
            };
            println!(
                "{}",
                cross_format::plan_cross_format_install(&cross_format::PackageRef {
                    name,
                    format: fmt,
                    source: "user-request".into()
                })
            );
            Ok(())
        }
        PlanVerb::Safety { targets } => {
            println!("{}", safety::plan_update_with_safety(&targets));
            Ok(())
        }
    }
}

pub(super) fn dispatch_safety(
    sv: SafetyVerb,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    match sv {
        SafetyVerb::Apply {
            op,
            targets,
            aur,
            snapshot_tag,
            skip_pretest,
            health_check,
            auto_rollback,
            gate,
        } => safe_apply_flow(
            &op,
            &targets,
            aur,
            snapshot_tag.as_deref(),
            skip_pretest,
            health_check.as_deref(),
            auto_rollback,
            gate_execute(&gate),
            gate.yes,
            noninteractive,
            script_mode,
        ),
    }
}
