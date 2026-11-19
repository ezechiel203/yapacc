// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Read-only verbs: list / info / search / inspect / ledger / sources / aur reads.

use super::*;

// ----- read dispatchers ----------------------------------------------------

pub(super) fn list_cmd(
    foreign: bool,
    explicit: bool,
    limit: Option<usize>,
    include_external: bool,
    json: bool,
) -> Result<(), String> {
    let pkgs = backend::list_installed(foreign, explicit)?;
    let shown: Vec<backend::PackageSummary> =
        pkgs.into_iter().take(limit.unwrap_or(usize::MAX)).collect();
    if json {
        print_json(&shown);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                p.version.clone(),
                p.reason.clone(),
                p.installed_size.clone(),
                p.description.clone(),
            ]
        })
        .collect();
    print!("{}", table(&package_columns(true), &rows, terminal_width()));
    if include_external {
        println!("\n-- External installs (ledger-tracked, user+system scopes) --");
        let mut rows: Vec<Vec<String>> = Vec::new();
        for scope in [&ledger::Scope::User, &ledger::Scope::System] {
            let lroot = ledger::default_root(scope);
            match ledger::list_records(&lroot) {
                Ok(recs) => {
                    for r in recs.into_iter().filter(|r| r.status != "removed") {
                        rows.push(vec![
                            format!("{} [{}]", r.name, r.format),
                            r.version,
                            r.scope,
                            r.status,
                            r.install_root,
                        ]);
                    }
                }
                Err(e) => {
                    if matches!(scope, ledger::Scope::User) {
                        println!("(user ledger unavailable: {e})");
                    }
                }
            }
        }
        let shown_n = rows.len();
        if shown_n > 0 {
            let columns = [
                Column {
                    head: "NAME [FORMAT]",
                    right: false,
                    cap: 40,
                },
                Column {
                    head: "VERSION",
                    right: false,
                    cap: 24,
                },
                Column {
                    head: "SCOPE",
                    right: false,
                    cap: 16,
                },
                Column {
                    head: "STATUS",
                    right: false,
                    cap: 16,
                },
                Column {
                    head: "INSTALL ROOT",
                    right: false,
                    cap: 0,
                },
            ];
            // a path is never cut
            print!("{}", table(&columns, &rows, None));
        }
        if shown_n == 0 {
            println!("(none found in accessible ledgers)");
        }
    }
    Ok(())
}

/// The columns of a list of ledger records. Nothing in it is ever cut: the
/// ID is what `remove --id` takes, and the last column is a path.
fn record_columns(scope: bool, last: &'static str) -> Vec<Column> {
    let column = |head: &'static str| Column {
        head,
        right: false,
        cap: 48,
    };
    let mut columns = vec![column("ID"), column("NAME"), column("FORMAT")];
    if scope {
        columns.push(column("SCOPE"));
    }
    columns.push(column("STATUS"));
    columns.push(Column {
        head: last,
        right: false,
        cap: 0,
    });
    columns
}

/// "not found" from pacman or pactree is exit 4, like everywhere else.
fn missing_is_not_found(e: String) -> String {
    if e.contains("not found") {
        format!("{NOT_FOUND}{e}")
    } else {
        e
    }
}

/// The columns of a list of packages; `reason` adds the install reason.
pub(super) fn package_columns(reason: bool) -> Vec<Column> {
    let mut columns = vec![
        Column {
            head: "NAME",
            right: false,
            cap: 40,
        },
        Column {
            head: "VERSION",
            right: false,
            cap: 24,
        },
    ];
    if reason {
        columns.push(Column {
            head: "REASON",
            right: false,
            cap: 24,
        });
    }
    columns.push(Column {
        head: "SIZE",
        right: true,
        cap: 16,
    });
    columns.push(Column {
        head: "DESCRIPTION",
        right: false,
        cap: 0,
    });
    columns
}

/// Name, version, size and description of each package, as a table.
fn print_packages(pkgs: &[backend::PackageSummary]) {
    let rows: Vec<Vec<String>> = pkgs
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                p.version.clone(),
                p.installed_size.clone(),
                p.description.clone(),
            ]
        })
        .collect();
    print!(
        "{}",
        table(&package_columns(false), &rows, terminal_width())
    );
}

pub(super) fn info_cmd(pkg: &str, json: bool) -> Result<(), String> {
    match backend::package_info(pkg) {
        Ok(info) => {
            if json {
                print_json(&info);
                return Ok(());
            }
            println!(
                "Name: {}  Version: {}\nDescription: {}\nURL: {}\nLicenses: {}\nArch: {}\nDepends: {}\nOptional: {}\nRequired By: {}\nInstalled Size: {}\nReason: {}\nForeign: {}  Explicit: {}  Leaf: {}",
                info.name,
                info.version,
                info.description,
                info.url,
                info.licenses,
                info.architecture,
                info.depends,
                info.optional_deps,
                info.required_by,
                info.installed_size,
                info.install_reason,
                info.is_foreign,
                info.is_explicit,
                info.is_leaf
            );
            Ok(())
        }
        Err(e) => {
            let mut found = Vec::new();
            for scope in [&ledger::Scope::User, &ledger::Scope::System] {
                let lroot = ledger::default_root(scope);
                if let Ok(recs) = ledger::list_records(&lroot) {
                    for r in recs
                        .into_iter()
                        .filter(|r| r.status != "removed" && (r.name == pkg || r.id == pkg))
                    {
                        found.push(r);
                    }
                }
            }
            if found.is_empty() {
                // "not found" is exit 4; pacman missing or failing stays 1
                return Err(if e.contains("not found") {
                    format!("{NOT_FOUND}package '{pkg}' is not installed")
                } else {
                    e
                });
            }
            if json {
                print_json(&found);
                return Ok(());
            }
            for r in found {
                println!(
                    "Name: {}  Version: {}\nSource: external (ledger-tracked)\nFormat: {}\nScope: {}\nStatus: {}\nInstall Root: {}\nOrigin/Source: {}\nTracked Paths: {}\nMetadata: {}",
                    r.name,
                    r.version,
                    r.format,
                    r.scope,
                    r.status,
                    r.install_root,
                    r.source,
                    r.paths.len(),
                    if r.metadata.is_empty() {
                        "(none)".into()
                    } else {
                        r.metadata
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                );
                if !r.paths.is_empty() {
                    println!(
                        "Paths (sample): {}",
                        r.paths
                            .iter()
                            .take(10)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                println!();
            }
            Ok(())
        }
    }
}

/// What `search` says on Arch when no sync database is on disk. `why` is
/// `native::missing_metadata`'s sentence. A repository search has nothing
/// else to say; with `--all` the AUR rows are still an answer, and the
/// note says which part is missing. Pure.
pub(super) fn search_without_sync_note(mode: &str, why: &str) -> String {
    if mode == "all" {
        format!("note: the repository part of this search is missing: {why}")
    } else {
        format!("note: {why}")
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn search_cmd(
    query: &str,
    aur: bool,
    all: bool,
    limit: usize,
    category: Option<String>,
    kind: Option<String>,
    ptype: Option<String>,
    _limit2: usize,
    json: bool,
) -> Result<(), String> {
    let mode = if aur {
        "aur"
    } else if all {
        "all"
    } else {
        "repo"
    };
    let mut results = Vec::new();
    let mut expansion_note = String::new();
    // no sync database on disk (a fresh system before its first `pacman
    // -Sy`): the repositories cannot be searched, which is not "0 found"
    let no_sync = if mode == "aur" {
        None
    } else {
        crate::native::missing_metadata(crate::platform::PkgManager::Pacman)
    };
    if (mode == "repo" || mode == "all") && no_sync.is_none() {
        // deterministic meaning expansion when the direct query has no hits
        let (r, note) = backend::search_smart(query, limit * 3)?;
        results.extend(r);
        expansion_note = note;
    }
    if mode == "aur" || mode == "all" {
        match backend::search_aur(query, limit * 3) {
            Ok(found) => results.extend(found),
            // "0 found" is not said of a question the AUR refused or that
            // never reached it
            Err(e) if mode == "aur" => return Err(format!("{UNAVAILABLE}{e}")),
            // the repository rows are still an answer: say what is missing
            Err(e) => eprintln!("note: the AUR part of this search is missing: {e}"),
        }
    }
    if let Some(c) = &category {
        let c = c.to_lowercase();
        results.retain(|r| {
            r.categories.iter().any(|x| x.to_lowercase().contains(&c))
                || r.description.to_lowercase().contains(&c)
        });
    }
    if let Some(k) = &kind {
        let k = k.to_lowercase();
        results.retain(|r| r.kind.to_lowercase().contains(&k));
    }
    if let Some(t) = &ptype {
        let t = t.to_lowercase();
        results.retain(|r| {
            r.pkg_type.to_lowercase().contains(&t) || r.size_class.to_lowercase().contains(&t)
        });
    }
    let q = query.to_lowercase();
    let rank = |r: &backend::SearchResult| -> u8 {
        let n = r.name.to_lowercase();
        let d = r.description.to_lowercase();
        if n == q {
            0
        } else if n.contains(&q) {
            1
        } else if d.contains(&q) {
            2
        } else {
            3
        }
    };
    results.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.source.cmp(&b.source))
    });
    results.truncate(limit);
    if json {
        // the same array as ever; the reason goes to standard error
        print_json(&results);
        if let Some(why) = &no_sync {
            eprintln!("{}", search_without_sync_note(mode, why));
        }
        return Ok(());
    }
    if let Some(why) = &no_sync {
        if mode == "repo" {
            // in the place of the empty answer, as on the other managers
            println!("{}", search_without_sync_note(mode, why));
            return Ok(());
        }
        eprintln!("{}", search_without_sync_note(mode, why));
    }
    if !expansion_note.is_empty() {
        println!("{expansion_note}");
    }
    println!(
        "SEARCH '{}' (mode={mode}, limit={limit}): {} found",
        query,
        results.len()
    );
    println!(
        "{:<5} {:<24} {:<14} {:<4} {:<12} {:<10} {:<12} {:<22} DESC",
        "SRC", "NAME", "VER", "INST", "KIND", "TYPE", "SIZE", "CATS"
    );
    println!("{}", "-".repeat(150));
    for r in &results {
        let inst = if r.installed { "yes" } else { "no" };
        let cats = r
            .categories
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{:<5} {:<24} {:<14} {:<4} {:<12} {:<10} {:<12} {:<22} {}",
            tr(&r.source, 5),
            tr(&r.name, 24),
            tr(&r.version, 14),
            inst,
            tr(&r.kind, 12),
            tr(&r.pkg_type, 10),
            tr(&r.size_class, 12),
            tr(&cats, 22),
            tr(&r.description, 28)
        );
        println!(
            "      purpose: {} | dl/inst: {}/{}",
            tr(&r.purpose, 80),
            r.download_size.clone().unwrap_or("-".into()),
            r.installed_size.clone().unwrap_or("-".into())
        );
    }
    Ok(())
}

pub(super) fn dispatch_inspect(iv: InspectVerb, json: bool) -> Result<(), String> {
    match iv {
        InspectVerb::Leaves => {
            let pkgs = backend::list_leaves()?;
            if json {
                print_json(&pkgs);
                return Ok(());
            }
            print_packages(&pkgs);
            Ok(())
        }
        InspectVerb::Orphans => {
            let pkgs = backend::list_orphans()?;
            if json {
                print_json(&pkgs);
                return Ok(());
            }
            if pkgs.is_empty() {
                // the same line as on every other manager, not a lone header
                println!("{}", portable::NO_ORPHANS);
                return Ok(());
            }
            println!("Orphan dependencies: {} found", pkgs.len());
            print_packages(&pkgs);
            Ok(())
        }
        InspectVerb::Facets { pkg } => {
            println!(
                "{}",
                backend::classify_installed(&pkg).map_err(missing_is_not_found)?
            );
            Ok(())
        }
        // The same answer, JSON form and exit codes as on every other
        // manager (not found is 4, a relative path is 2).
        InspectVerb::Files { pkg } => {
            let b = crate::native::backend(crate::platform::PkgManager::Pacman).map_err(marked)?;
            portable::lines(b.files(&pkg).map_err(marked)?, portable::NO_FILES, json)
                .map_err(marked)
        }
        InspectVerb::Owns { path } => {
            let b = crate::native::backend(crate::platform::PkgManager::Pacman).map_err(marked)?;
            portable::owns(b.as_ref(), &path, json).map_err(marked)
        }
        InspectVerb::Deps { pkg, depth } => {
            println!(
                "{}",
                backend::dep_tree(&pkg, depth).map_err(missing_is_not_found)?
            );
            Ok(())
        }
        InspectVerb::Rdepends { pkg, depth } => {
            println!(
                "{}",
                backend::reverse_dep_tree(&pkg, depth).map_err(missing_is_not_found)?
            );
            Ok(())
        }
        InspectVerb::FileSearch { path, refresh } => {
            println!("{}", backend::file_search(&path, refresh)?);
            Ok(())
        }
        InspectVerb::Groups { group } => {
            println!("{}", backend::groups(group.as_deref())?);
            Ok(())
        }
        InspectVerb::Pacdiff { root } => {
            let files = backend::pacdiff_candidates(&root)?;
            if json {
                print_json(&files);
                return Ok(());
            }
            println!("== Config merge candidates under {root} ==");
            if files.is_empty() {
                println!("(none: no .pacnew/.pacsave/.orig files found)");
                return Ok(());
            }
            for f in &files {
                println!("  {f}");
            }
            println!(
                "\nMerge with `pacdiff` (pacman-contrib) or your merge tool of choice;\n.pacnew = new default config shipped by a package, .pacsave = your file kept by pacman."
            );
            Ok(())
        }
        InspectVerb::Alpm { cmd: av } => match av {
            AlpmVerb::List { limit, leaves } => {
                if let Some(why) = alpm_not_built(
                    "inspect alpm list",
                    if leaves { "inspect leaves" } else { "list" },
                    false,
                ) {
                    return Err(why);
                }
                let pkgs = if leaves {
                    crate::alpm_backend::list_leaves()
                } else {
                    crate::alpm_backend::list_local()
                }
                .map_err(|e| e.to_string())?;
                println!(
                    "ALPM-backed local package list ({} shown of {}):",
                    limit.min(pkgs.len()),
                    pkgs.len()
                );
                println!(
                    "{:<32} {:<16} {:<10} {:>12}  DESC",
                    "NAME", "VERSION", "REASON", "SIZE"
                );
                println!("{}", "-".repeat(110));
                for p in pkgs.into_iter().take(limit) {
                    println!(
                        "{:<32} {:<16} {:<10} {:>12}  {}",
                        tr(&p.name, 32),
                        tr(&p.version, 16),
                        p.reason,
                        human_size(p.installed_size),
                        tr(&p.desc, 40)
                    );
                }
                Ok(())
            }
            AlpmVerb::Info { pkg } => {
                if let Some(why) = alpm_not_built("inspect alpm info", "info PKG", false) {
                    return Err(why);
                }
                let p = crate::alpm_backend::info(&pkg)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("{NOT_FOUND}package '{pkg}' is not installed"))?;
                println!(
                    "(ALPM) Name: {}\nVersion: {}\nDescription: {}\nInstalled Size: {} ({} bytes)\nInstall Reason: {}\nRequired By: {}",
                    p.name,
                    p.version,
                    p.desc,
                    human_size(p.installed_size),
                    p.installed_size,
                    p.reason,
                    if p.required_by.is_empty() {
                        "None".into()
                    } else {
                        p.required_by.join(" ")
                    }
                );
                Ok(())
            }
            AlpmVerb::Leaves => {
                if let Some(why) = alpm_not_built("inspect alpm leaves", "inspect leaves", false) {
                    return Err(why);
                }
                let pkgs = crate::alpm_backend::list_leaves().map_err(|e| e.to_string())?;
                println!("ALPM-backed leaves: {} packages", pkgs.len());
                println!("{:<32} {:<16} {:>12}  DESC", "NAME", "VERSION", "SIZE");
                println!("{}", "-".repeat(100));
                for p in pkgs.into_iter().take(30) {
                    println!(
                        "{:<32} {:<16} {:>12}  {}",
                        tr(&p.name, 32),
                        tr(&p.version, 16),
                        human_size(p.installed_size),
                        tr(&p.desc, 40)
                    );
                }
                Ok(())
            }
            AlpmVerb::Search { query, limit } => {
                if let Some(why) = alpm_not_built("inspect alpm search", "search QUERY", false) {
                    return Err(why);
                }
                let hits = crate::alpm_flows::search(&query).map_err(|e| e.to_string())?;
                println!(
                    "ALPM sync search '{query}' : {} hits (showing {})",
                    hits.len(),
                    hits.len().min(limit)
                );
                println!(
                    "{:<8} {:<30} {:<16} {:<5} DESC",
                    "REPO", "NAME", "VERSION", "INST"
                );
                println!("{}", "-".repeat(110));
                for h in hits.into_iter().take(limit) {
                    println!(
                        "{:<8} {:<30} {:<16} {:<5} {}",
                        tr(&h.repo, 8),
                        tr(&h.name, 30),
                        tr(&h.version, 16),
                        if h.installed { "yes" } else { "no" },
                        tr(&h.desc, 45)
                    );
                }
                Ok(())
            }
            AlpmVerb::SyncQuery { pkg } => {
                match crate::alpm_flows::query_sync(&pkg).map_err(|e| e.to_string())? {
                    Some((repo, name, ver, desc)) => {
                        println!(
                            "(ALPM sync) Repo: {repo}\nName: {name}\nVersion: {ver}\nDescription: {desc}"
                        );
                        Ok(())
                    }
                    None => Err(format!(
                        "{NOT_FOUND}package '{pkg}' is in no repository database on disk"
                    )),
                }
            }
        },
    }
}

pub(super) fn dispatch_fixes(
    fv: FixesVerb,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    match fv {
        FixesVerb::List => {
            let fixes = repair::list_fixes()?;
            println!(
                "LOCAL FIX RECORDS ({}): {}",
                fixes.len(),
                repair::fixes_root().display()
            );
            if fixes.is_empty() {
                println!("(none — fixes are recorded automatically after a successful autorepair)");
            }
            for (pkg, path) in fixes {
                println!("  {pkg}  ({})", path.display());
            }
            Ok(())
        }
        FixesVerb::Push { gate } => {
            if !gate_execute(&gate) {
                println!(
                    "Would push the recorded fixes to the git remote in [fixes] remote.\n{}",
                    prompt::DRY_RUN_FOOTER
                );
                return Ok(());
            }
            if !confirm_gate(gate.yes, noninteractive, script_mode)? {
                println!("Aborted.");
                return Ok(());
            }
            println!("{}", repair::fixes_push()?);
            Ok(())
        }
        FixesVerb::Pull => {
            println!("{}", repair::fixes_pull()?);
            Ok(())
        }
    }
}

/// Browse the taxonomy tree: tops → subcategories → packages.
pub(super) fn tree_cmd(path: Option<&str>, installed_only: bool) -> Result<(), String> {
    let Some(p) = path else {
        println!(
            "TAXONOMY ({} tops, {} leaves — offline, deterministic)",
            crate::taxonomy::tops().len(),
            crate::taxonomy::effective_leaves().len()
        );
        for t in crate::taxonomy::tops() {
            let subs = crate::taxonomy::subs(&t).unwrap_or_default();
            println!("  {t} ({} subcategories)", subs.len());
        }
        println!(
            "\ndrill down: `yapacc tree <category>` · `yapacc tree <category>.<subcategory>` · TUI: `yapacc i` → Tree tab"
        );
        return Ok(());
    };
    let (top, sub) = match p.split_once('.') {
        Some((t, s)) => (t, Some(s)),
        None => (p, None),
    };
    let Some(subs) = crate::taxonomy::subs(top) else {
        return Err(format!(
            "{NOT_FOUND}unknown category '{top}' (`yapacc tree` lists them)"
        ));
    };
    let Some(sub) = sub else {
        println!("{top}:");
        for s in &subs {
            println!("  {top}.{s}");
        }
        return Ok(());
    };
    if !subs.iter().any(|s| s == sub) {
        return Err(format!(
            "{NOT_FOUND}unknown subcategory '{top}.{sub}' (`yapacc tree {top}` lists them)"
        ));
    }
    // classify packages: installed (with descriptions) + repo names
    let mut packages: Vec<(String, String, bool)> = Vec::new(); // (name, desc, installed)
    for p in backend::list_installed(false, false)? {
        packages.push((p.name, p.description, true));
    }
    if !installed_only && let Ok(rows) = backend::build_repo_index(0) {
        let installed_names: std::collections::HashSet<String> =
            packages.iter().map(|(n, _, _)| n.clone()).collect();
        for r in rows {
            if !installed_names.contains(&r.1) {
                packages.push((r.1, String::new(), false));
            }
        }
    }
    let full = format!("{top}.{sub}");
    let mut hits: Vec<(String, String, bool)> = packages
        .into_iter()
        .filter(|(n, d, _)| crate::taxonomy::classify_pkg(n, d) == full)
        .collect();
    hits.sort();
    println!(
        "{full}: {} package(s){}",
        hits.len(),
        if installed_only {
            " (installed only)"
        } else {
            ""
        }
    );
    println!("{:<34} {:<6} DESCRIPTION", "NAME", "INST");
    println!("{}", "-".repeat(90));
    for (n, d, inst) in hits.iter().take(200) {
        println!(
            "{:<34} {:<6} {}",
            tr(n, 34),
            if *inst { "yes" } else { "-" },
            tr(d, 44)
        );
    }
    if hits.len() > 200 {
        println!("... and {} more", hits.len() - 200);
    }
    Ok(())
}

pub(super) fn dispatch_flatpak_read(fv: FlatpakVerb) -> Result<(), String> {
    match fv {
        FlatpakVerb::List => {
            println!("{}", backend::list_flatpak()?);
            Ok(())
        }
        FlatpakVerb::Search { query } => {
            println!("{}", backend::search_flatpak(&query)?);
            Ok(())
        }
        FlatpakVerb::Info { app } => {
            println!("{}", backend::flatpak_info(&app)?);
            Ok(())
        }
    }
}

pub(super) fn dispatch_foreign_read(fv: ForeignReadVerb, json: bool) -> Result<(), String> {
    match fv {
        ForeignReadVerb::Info { format, source } => {
            println!(
                "{}",
                foreign_extract::inspect_foreign(&format, Path::new(&source))?
            );
            Ok(())
        }
        ForeignReadVerb::List { ledger_path } => {
            let lroot = ledger_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ledger::default_root(&ledger::Scope::User));
            let recs = ledger::list_records(&lroot)?;
            if json {
                let f: Vec<_> = recs
                    .into_iter()
                    .filter(|r| r.format.ends_with("-extract"))
                    .collect();
                print_json(&f);
                return Ok(());
            }
            println!("Isolated installs recorded in the ledger:");
            let rows: Vec<Vec<String>> = recs
                .into_iter()
                .filter(|r| r.format.ends_with("-extract"))
                .map(|r| vec![r.id, r.name, r.format, r.status, r.install_root])
                .collect();
            if rows.is_empty() {
                println!("(none)");
            } else {
                print!("{}", table(&record_columns(false, "PREFIX"), &rows, None));
            }
            Ok(())
        }
    }
}

/// Where `index` keeps its file when `--path` is not given: under the user's
/// cache directory. Never under `/tmp`, where any local user could prepare
/// the file or a link in its place. Pure.
pub fn default_index_path(
    xdg_cache_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    let absolute =
        |v: Option<std::ffi::OsString>| v.map(std::path::PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg_cache_home).or_else(|| absolute(home).map(|h| h.join(".cache")))?;
    Some(base.join("yapacc").join("repo-index.tsv"))
}

fn index_path(given: Option<String>) -> Result<String, String> {
    if let Some(p) = given {
        return Ok(p);
    }
    default_index_path(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
        .map(|p| p.display().to_string())
        .ok_or_else(|| {
            format!(
                "{USAGE}neither XDG_CACHE_HOME nor HOME names a directory for the index file: pass --path FILE"
            )
        })
}

pub(super) fn dispatch_index(iv: IndexVerb, json: bool) -> Result<(), String> {
    match iv {
        IndexVerb::Build { path, limit } => {
            let path = index_path(path)?;
            println!("{}", backend::build_enriched_index_file(&path, limit)?);
            Ok(())
        }
        IndexVerb::Search {
            query,
            path,
            category,
            limit,
        } => {
            let path = index_path(path)?;
            let rows = backend::search_index_file(&path, &query, category.as_deref(), limit)?;
            if json {
                print_json(&rows);
                return Ok(());
            }
            println!("INDEX SEARCH '{query}' in {path}: {} rows", rows.len());
            println!("{}", "-".repeat(120));
            for r in rows {
                println!(
                    "{:<8} {:<30} {:<18} {:<9} {:<14} {}",
                    tr(&r.0, 8),
                    tr(&r.1, 30),
                    tr(&r.2, 18),
                    r.3,
                    tr(&r.4, 14),
                    tr(&r.5, 35)
                );
            }
            Ok(())
        }
    }
}

pub(super) fn dispatch_ledger(lv: LedgerVerb, json: bool) -> crate::error::Result<()> {
    match lv {
        LedgerVerb::Status { scope, path } => {
            let root = ledger_root(&scope, path)?;
            println!("{}", ledger::status(&root)?);
            Ok(())
        }
        LedgerVerb::List { scope, path } => {
            let root = ledger_root(&scope, path)?;
            let recs = ledger::list_records(&root)?;
            if json {
                print_json(&recs);
                return Ok(());
            }
            println!("Ledger records in {}: {}", root.display(), recs.len());
            if !recs.is_empty() {
                let rows: Vec<Vec<String>> = recs
                    .into_iter()
                    .map(|r| {
                        vec![
                            r.id,
                            r.name,
                            r.format,
                            r.scope,
                            r.status,
                            format!("{} | {}", r.version, r.install_root),
                        ]
                    })
                    .collect();
                print!(
                    "{}",
                    table(&record_columns(true, "VERSION | ROOT"), &rows, None)
                );
            }
            Ok(())
        }
        LedgerVerb::Head { scope, path } => {
            let root = ledger_root(&scope, path)?;
            let (seq, hash) = ledger::head(&root)?;
            if json {
                print_json(
                    &serde_json::json!({"seq": seq, "hash": hash, "anchor": format!("{seq}:{hash}")}),
                );
            } else {
                println!("{seq}:{hash}");
            }
            Ok(())
        }
        LedgerVerb::Verify {
            scope,
            path,
            anchor,
            accept_legacy,
        } => {
            let root = ledger_root(&scope, path)?;
            let anchors = anchor
                .iter()
                .map(|a| ledger::parse_anchor(&root, a))
                .collect::<Result<Vec<_>, String>>()
                .map_err(crate::Error::usage)?;
            let report = ledger::verify(
                &root,
                &ledger::VerifyOptions {
                    accept_legacy,
                    anchors,
                },
            )?;
            if json {
                print_json(&report);
            } else {
                println!("{}", report.render());
            }
            Ok(())
        }
        LedgerVerb::Audit { scope, path } => {
            let root = ledger_root(&scope, path)?;
            let findings = ledger::audit(&root)?;
            println!(
                "LEDGER AUDIT ({}): {} finding(s)",
                root.display(),
                findings.len()
            );
            if findings.is_empty() {
                println!("(no conflicts/hints detected; analysis is best-effort)");
            }
            for f in findings {
                println!("- [{}] {} :: {}", f.kind, f.subject, f.detail);
            }
            Ok(())
        }
        LedgerVerb::Conflicts { scope, path, paths } => {
            if paths.is_empty() {
                return Err(crate::Error::usage(
                    "ledger conflicts requires at least one path",
                ));
            }
            let root = ledger_root(&scope, path)?;
            let conflicts = ledger::check_path_conflicts(&root, &paths)?;
            if conflicts.is_empty() {
                println!("No conflicts found for {} path(s).", paths.len());
            } else {
                println!("CONFLICTS ({}):", conflicts.len());
                for c in conflicts {
                    println!("- [{}] {} :: {}", c.kind, c.path, c.detail);
                }
            }
            Ok(())
        }
    }
}

pub(super) fn dispatch_external(ev: ExternalVerb, json: bool) -> Result<(), String> {
    match ev {
        ExternalVerb::List => {
            let mut all: Vec<ledger::InstallRecord> = Vec::new();
            for scope in [&ledger::Scope::User, &ledger::Scope::System] {
                let lroot = ledger::default_root(scope);
                if let Ok(recs) = ledger::list_records(&lroot) {
                    all.extend(recs.into_iter().filter(|r| r.status != "removed"));
                }
            }
            if json {
                print_json(&all);
                return Ok(());
            }
            println!(
                "Installs recorded in the user and system ledgers: {}",
                all.len()
            );
            if all.is_empty() {
                println!("(none found in the ledgers this user can read)");
            } else {
                let rows: Vec<Vec<String>> = all
                    .iter()
                    .map(|r| {
                        vec![
                            r.id.clone(),
                            r.name.clone(),
                            r.format.clone(),
                            r.scope.clone(),
                            r.status.clone(),
                            r.install_root.clone(),
                        ]
                    })
                    .collect();
                print!("{}", table(&record_columns(true, "ROOT"), &rows, None));
            }
            Ok(())
        }
        ExternalVerb::Audit => {
            println!(
                "EXTERNAL AUDIT (user+system ledgers, best-effort ownership/provides/dependency hints)"
            );
            let mut total = 0;
            for scope in [&ledger::Scope::User, &ledger::Scope::System] {
                let lroot = ledger::default_root(scope);
                match ledger::audit(&lroot) {
                    Ok(findings) => {
                        println!(
                            "-- scope={scope:?} root={} findings={} --",
                            lroot.display(),
                            findings.len()
                        );
                        if findings.is_empty() {
                            println!("  (no conflicts/hints detected)");
                        }
                        for f in &findings {
                            println!("  - [{}] {} :: {}", f.kind, f.subject, f.detail);
                        }
                        total += findings.len();
                    }
                    Err(e) => println!("-- scope={scope:?} unavailable: {e} --"),
                }
            }
            println!("Total findings: {total}");
            Ok(())
        }
    }
}

pub(super) fn dispatch_sources(sv: SourcesVerb, json: bool) -> Result<(), String> {
    let (entries, header) = match &sv {
        SourcesVerb::Apt { root } => (
            source_repos::parse_apt_sources(Path::new(root))?,
            format!("APT sources (root: {root})"),
        ),
        SourcesVerb::Rpm { dir } => (
            source_repos::parse_rpm_repos_dir(Path::new(dir))?,
            format!("RPM repos (dir: {dir})"),
        ),
    };
    if json {
        #[derive(serde::Serialize)]
        struct Src<'a> {
            kind: &'a str,
            id: &'a str,
            uri: &'a str,
            suite: &'a str,
            components: &'a str,
            enabled: bool,
            origin: &'a str,
        }
        let out: Vec<Src> = entries
            .iter()
            .map(|e| Src {
                kind: &e.kind,
                id: &e.id,
                uri: &e.uri,
                suite: &e.suite,
                components: &e.components,
                enabled: e.enabled,
                origin: &e.origin,
            })
            .collect();
        print_json(&out);
        return Ok(());
    }
    println!("== {header} ==");
    print!("{}", source_repos::format_sources(&entries));
    println!(
        "\nNote: read-only parsing of source definitions. Foreign installs must go to an isolated managed root (not /bin,/usr,standard home dirs) with foreign-attributed names to avoid conflicts."
    );
    Ok(())
}

pub(super) fn dispatch_monitor(
    mv: MonitorVerb,
    noninteractive: bool,
    script_mode: bool,
    json: bool,
) -> Result<(), String> {
    match mv {
        MonitorVerb::Check => {
            let r = update_service::run_check()?;
            if json {
                print_json(&r);
                return Ok(());
            }
            println!(
                "== Monitor check ==\nRepo updates:\n{}\nAUR updates:\n{}\nFlatpak updates:\n{}\nSafety/pipeline: {}",
                r.repo_updates, r.aur_updates, r.flatpak_updates, r.safety_note
            );
            Ok(())
        }
        MonitorVerb::DaemonInstall { user, gate } => daemon_install_flow(
            user,
            gate_execute(&gate),
            gate.yes,
            noninteractive,
            script_mode,
        ),
    }
}

pub(super) fn dispatch_aur(av: AurVerb, json: bool) -> Result<(), String> {
    match av {
        AurVerb::Deps { pkgs } => {
            if pkgs.is_empty() {
                return Err("aur deps requires at least one package".into());
            }
            let report = aur_rpc::aur_closure(&pkgs)?;
            if json {
                print_json(&report);
                return Ok(());
            }
            println!("== AUR build closure ==");
            println!("Requested: {}", report.requested.join(", "));
            if !report.build_order.is_empty() {
                println!("\nBuild order (dependencies first):");
                for (i, n) in report.build_order.iter().enumerate() {
                    println!("  {}. {n}", i + 1);
                }
            }
            if !report.from_repo.is_empty() {
                println!(
                    "\nSatisfiable from repos (install first if missing): {}",
                    report.from_repo.join(", ")
                );
            }
            if !report.satisfied_installed.is_empty() {
                println!(
                    "\nAlready installed (satisfied): {}",
                    report.satisfied_installed.join(", ")
                );
            }
            if !report.missing.is_empty() {
                println!("\nUnresolvable (not in AUR/repos/installed):");
                for m in &report.missing {
                    println!("  - {m}");
                }
            }
            for n in &report.notes {
                println!("note: {n}");
            }
            Ok(())
        }
        AurVerb::Review { pkg } => {
            println!("{}", backend::aur_review(&pkg)?);
            Ok(())
        }
        AurVerb::Pkgbuild { pkg } => {
            // needs curl only; a name the AUR does not have is exit 4, an
            // AUR that cannot be reached is exit 6
            let text = backend::fetch_pkgbuild(&pkg).map_err(|e| match e {
                backend::PkgbuildError::NotInAur(m) => format!("{NOT_FOUND}{m}"),
                backend::PkgbuildError::Unreachable(m) => format!("{UNAVAILABLE}{m}"),
            })?;
            println!("{text}");
            Ok(())
        }
    }
}

pub(super) fn dispatch_package(pv: PackageVerb) -> Result<(), String> {
    match pv {
        PackageVerb::Scaffold { name, dir } => cmd_scaffold(&name, dir.as_deref()),
        PackageVerb::Validate { target } => cmd_validate(&target),
        PackageVerb::Srcinfo { dir } => {
            println!("{}", backend::generate_srcinfo(&dir)?);
            Ok(())
        }
    }
}
