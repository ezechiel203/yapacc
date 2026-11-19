// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Smart installer flows: `get`, recipe lane, brew cascade, flatpak.

use super::*;

// ---------------------------------------------------------------------------
// Smart installer (yapacc get) — resolution engine + recipe fallback
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)] // dispatch plumbing
pub(super) fn get_flow(
    names: &[String],
    from: Option<&str>,
    explain: bool,
    source_url: Option<&str>,
    no_review: bool,
    build_user: Option<&str>,
    expose: Option<&str>,
    rflags: RepairFlags,
    factory: Option<&str>,
    retention: (bool, bool),
    non_free: bool,
    gate: &Gate,
    noninteractive: bool,
    script_mode: bool,
    json: bool,
) -> Result<(), String> {
    let _ = non_free; // used by foreign/brew lanes
    let (keep_factory, discard_factory) = retention;
    if names.is_empty() {
        return Err("get requires at least one package name".into());
    }
    let cfg_order = config::get().install.order.clone();
    if let Some(lane) = from
        && (!resolve::LANES.contains(&lane) || lane == "tap")
    {
        return Err(format!(
            "{USAGE}invalid --from lane '{lane}' (expected: repo | aur | flatpak | brew | recipe)"
        ));
    }
    let execute = gate_execute(gate);
    // --explain: names for which no source was chosen (exit 4 at the end)
    let mut unresolved: Vec<String> = Vec::new();

    for name in names {
        let resolution = match from {
            Some(lane) => {
                // forced lane: probe only that lane
                let mut candidates = vec![resolve::probe_lane(lane, name)];
                let chosen = resolve::Resolution::choose(&[lane.to_string()], &mut candidates);
                let mut r = resolve::Resolution {
                    name: name.clone(),
                    candidates,
                    chosen,
                    runtime: config::get().install.runtime_default.clone(),
                };
                if r.chosen.is_none() {
                    return Err(format!("{NOT_FOUND}{name}: not found in lane '{lane}'"));
                }
                if lane == "recipe" {
                    r.chosen = Some("recipe".into());
                }
                r
            }
            None => resolve::resolve(name, &cfg_order),
        };
        if json {
            print_json(&resolution);
            if explain {
                if resolution.chosen.is_none() {
                    unresolved.push(name.clone());
                }
                continue;
            }
            // unreachable from the command line: `run` refuses it first
            return Err(format!(
                "{USAGE}no JSON output for this command: `get` prints its decision as JSON only with --explain"
            ));
        }
        print!("{}", resolution.render());
        if explain {
            if resolution.chosen.is_none() {
                unresolved.push(name.clone());
            }
            continue;
        }
        let Some(chosen) = resolution.chosen.clone() else {
            return Err(format!("{NOT_FOUND}{name}: no lane resolved a candidate"));
        };
        match chosen.as_str() {
            "repo" => {
                println!("lane: repo → install through pacman, behind the gate");
                action_flow(
                    "install",
                    vec![name.clone()],
                    false,
                    execute,
                    gate.yes,
                    noninteractive,
                    script_mode,
                )?;
            }
            "aur" => {
                println!(
                    "lane: aur → closure build behind the PKGBUILD review gate (as paru/yay would)"
                );
                aur_install_flow(
                    name,
                    None,
                    None,
                    build_user,
                    no_review,
                    true,
                    rflags,
                    execute,
                    gate.yes,
                    noninteractive,
                    script_mode,
                )?;
            }
            "flatpak" => {
                println!("lane: flatpak → sandboxed app install (system)");
                install_flatpak_flow(
                    name,
                    false,
                    &Gate {
                        dryrun: gate.dryrun,
                        yes: gate.yes,
                    },
                    noninteractive,
                    script_mode,
                )?;
            }
            "recipe" => {
                println!(
                    "lane: recipe → REVIEW-GATED build into the yapacc prefix (non-root build)"
                );
                recipe_flow(
                    name,
                    source_url,
                    factory,
                    (keep_factory, discard_factory),
                    no_review,
                    build_user,
                    expose,
                    rflags,
                    gate,
                    noninteractive,
                    script_mode,
                )?;
            }
            "brew" => {
                println!("lane: brew → homebrew formula cascade (linux source → repair → Darling)");
                brew_cascade_flow(
                    name,
                    noninteractive,
                    script_mode,
                    factory,
                    retention,
                    no_review,
                    build_user,
                    expose,
                    rflags,
                    gate,
                )?;
            }
            other => {
                return Err(format!(
                    "lane '{other}' is listed for information only: yapacc installs nothing through it. Pick another source with --from."
                ));
            }
        }
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "{NOT_FOUND}{}",
            portable_write::explain_not_found(&unresolved)
        ));
    }
    Ok(())
}

/// Homebrew formula cascade:
/// linux-compatible formula → recipe lane build (factories + autorepair);
/// mac-only (marker or MacOsOnly diagnosis) → ask; Darling → fetch mac
/// bottle → extract keg → validate one binary under `darling exec` →
/// install a `fgn.darling.<pkg>--<bin>` launcher alias.
#[allow(clippy::too_many_arguments)] // dispatch plumbing
pub(super) fn brew_cascade_flow(
    name: &str,
    noninteractive: bool,
    script_mode: bool,
    factory: Option<&str>,
    retention: (bool, bool),
    no_review: bool,
    build_user: Option<&str>,
    expose: Option<&str>,
    rflags: RepairFlags,
    gate: &Gate,
) -> Result<(), String> {
    let formula = brew::fetch_formula(name).map_err(formula_error)?;
    println!(
        "== homebrew formula: {} {} ({}) ==",
        formula.name,
        formula.version,
        formula.classification()
    );
    println!("  desc: {}", formula.description);
    println!("  url:  {}", formula.url);
    if !formula.depends_on.is_empty() {
        println!("  deps: {}", formula.depends_on.join(", "));
    }
    match formula.classification() {
        "linux-bottled" | "inconclusive" => {
            if formula.classification() == "inconclusive" {
                println!(
                    "  note: metadata inconclusive — trying the Linux build; MacOsOnly failures fall through to Darling."
                );
            }
            // build from the formula's source URL via the recipe lane
            // (review gate + factories + autorepair + keg install)
            recipe_flow(
                name,
                Some(&formula.url),
                factory,
                retention,
                no_review,
                build_user,
                expose,
                rflags,
                gate,
                noninteractive,
                script_mode,
            )
        }
        "mac-only" => darling_fallback(name, &formula, noninteractive, script_mode),
        other => Err(format!("unhandled classification '{other}'")),
    }
}

/// Mac-only path: ask; if Darling is installed and accepted, fetch the macOS
/// bottle, extract into a keg, validate one binary, install a launcher alias.
pub(super) fn darling_fallback(
    name: &str,
    formula: &brew::Formula,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    println!("⚠  {name} appears to be macOS-only ({}).", formula.homepage);
    let darling_installed = brew::darling_available();
    // Consent rule (user-validated): Darling already installed (native or a
    // yapacc keg) or a non-stopping run (--yes/--noconfirm) = prior consent;
    // the question is ONLY asked when Darling is absent and we are interactive.
    if darling_installed {
        println!("Darling detected — proceeding without asking (installed = prior consent).");
    } else if noninteractive || script_mode {
        println!("--yes/--noconfirm run — Darling path accepted without prompting.");
    } else {
        darling_consent(
            name,
            prompt::confirm_or_proceed(
                &format!(
                    "{name} is macOS-only; proceed on the Darling path (Darling is NOT installed yet)"
                ),
                false,
                false,
                false,
            ),
        )?;
    }
    if !darling_installed {
        return Err(format!(
            "{name} is macOS-only and the Darling path was accepted, but Darling is not installed yet. Install Darling (https://docs.darlinghq.org) or `yapacc install container --image archlinux` and build it there, then run this again."
        ));
    }
    // macOS bottle URL (github packages): tag → bottle blob; brew layout:
    // https://ghcr.io/v2/homebrew/core/<name>/blobs/sha256:<hash> requires the
    // bottle sha; without evaluating Ruby we only have stable sha256 — fetch
    // the bottle tar from the formula's stable URL is NOT the bottle. Be
    // honest: bottle blob extraction needs the bottle sha from `brew`
    // itself; use the source tarball under Darling instead (darling can
    // build/run source), or instruct installing the bottle via brew-on-darling.
    Err(format!(
        "Darling path for {name}: accepted, but macOS bottle extraction needs the bottle hash (brew metadata we do not evaluate). Current recommendation: install Darling's brew inside darling (https://docs.darlinghq.org) and `brew install {name}` there; yapacc will automate this once the tap registry lands (DESIGN_RESOLUTION.md §9)."
    ))
}

/// The answer to the Darling question as a result: a no, and a run that
/// cannot ask, are refusals (exit 3), like every other declined gate.
fn darling_consent(name: &str, answer: Result<bool, String>) -> Result<(), String> {
    match answer {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "{REFUSED}Darling path declined for {name}: nothing was downloaded or installed."
        )),
        Err(e) => Err(format!(
            "{REFUSED}{e}\nNothing was downloaded or installed for {name}."
        )),
    }
}

/// The outcome of the recipe review gate as a result: a no, and a run that
/// cannot ask, are refusals (exit 3). `yapacc get x && x` must not run `x`
/// after the user declined the build.
fn recipe_gate_outcome(
    answer: Result<bool, String>,
    recipe: &recipe::GeneratedRecipe,
) -> Result<(), String> {
    match answer {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "{REFUSED}Recipe review declined: nothing was built or installed. The recipe is kept at {}.",
            recipe.pkgbuild_path.display()
        )),
        Err(e) => Err(format!("{REFUSED}{e}")),
    }
}

/// The name and URL that would be written into a PKGBUILD (which makepkg
/// sources as bash) are checked before anything is downloaded or written.
/// A value that is not accepted is a refusal (exit 3).
fn recipe_inputs_accepted(name: &str, source_url: Option<&str>) -> Result<(), String> {
    recipe::valid_recipe_name(name).map_err(|e| format!("{REFUSED}{e}"))?;
    if let Some(url) = source_url {
        recipe::valid_source_url(url).map_err(|e| format!("{REFUSED}{e}"))?;
    }
    Ok(())
}

/// A formula yapacc declines to use (hostile name or source URL) is a
/// refusal (exit 3); one that could not be fetched or parsed is a failure.
fn formula_error(e: String) -> String {
    if brew::declined(&e) {
        format!("{REFUSED}{e}")
    } else {
        e
    }
}

/// What a dry run of the recipe lane says. It has written nothing.
fn recipe_dry_run_report(recipe: &recipe::GeneratedRecipe) -> String {
    format!(
        "{}\n{}\nNothing was written or built.\n{}",
        recipe::describe(recipe),
        recipe.pkgbuild,
        prompt::DRY_RUN_FOOTER
    )
}

/// Recipe fallback: generate → review gate → build non-root → install keg.
///
/// A PKGBUILD the user edited is never overwritten: it is what the gate
/// shows and what is built. `--dryrun` writes nothing.
#[allow(clippy::too_many_arguments)] // dispatch plumbing
pub(super) fn recipe_flow(
    name: &str,
    source_url: Option<&str>,
    factory: Option<&str>,
    retention: (bool, bool), // (keep_factory, discard_factory)
    no_review: bool,
    build_user: Option<&str>,
    expose: Option<&str>,
    rflags: RepairFlags,
    gate: &Gate,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    recipe_inputs_accepted(name, source_url)?;
    let kind = factory.map(recipe::parse_factory_kind).transpose()?;
    let execute = gate_execute(gate);
    let generated = recipe::prepare(name, source_url, kind, execute)?;
    if !execute {
        println!("{}", recipe_dry_run_report(&generated));
        return Ok(());
    }
    recipe_gate_outcome(
        recipe::review_gate(&generated, no_review, noninteractive, script_mode),
        &generated,
    )?;
    let policy = rflags.policy("recipe", name, !noninteractive && !script_mode);
    let artifacts = repair::build_with_repair(&generated.dir, build_user, &policy)?;
    println!(
        "Built: {}",
        artifacts
            .iter()
            .map(|a| a.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let expose_bins = resolve_expose(expose)?;
    let lroot = ledger::default_root(&ledger::Scope::User);
    let mut records = Vec::new();
    for artifact in &artifacts {
        let prefix = foreign_extract::default_prefix(name, "pkg");
        let rec = foreign_extract::install(
            name,
            "pkg",
            &artifact.display().to_string(),
            &prefix,
            &lroot,
            expose_bins,
        )?;
        records.push(rec);
    }
    println!("Recipe keg installed (lane 2, attributed bins):");
    for r in &records {
        println!("  - {} ({}) -> {}", r.id, r.version, r.install_root);
    }
    println!(
        "note: this is an extract install; to RUN it in a real environment use `yapacc install container --image archlinux`."
    );
    // build-factory retention: keep for reuse/audit, or discard — user's call
    if retention.1 {
        let _ = std::fs::remove_dir_all(&generated.dir);
        println!("build factory discarded (--discard-factory).");
    } else if retention.0 {
        println!(
            "build factory kept for reuse/audit: {}",
            generated.dir.display()
        );
    } else if !noninteractive && !script_mode {
        if prompt::confirm_or_proceed(
            "Keep the build factory (recipes dir) for reuse/audit",
            false,
            false,
            false,
        )? {
            println!("build factory kept: {}", generated.dir.display());
        } else {
            let _ = std::fs::remove_dir_all(&generated.dir);
            println!("build factory discarded.");
        }
    } else {
        println!(
            "build factory kept (non-interactive default): {}",
            generated.dir.display()
        );
    }
    Ok(())
}

/// Shared flatpak install flow (used by `install flatpak` and `get`).
pub(super) fn install_flatpak_flow(
    app: &str,
    user: bool,
    gate: &Gate,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    let execute = gate_execute(gate);
    println!("{}", backend::flatpak_install_plan(app, user)?);
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
    println!("{}", backend::flatpak_install(app, user)?);
    // ledger is created/updated automatically on install
    let scope = if user { "user" } else { "system" };
    let lroot = ledger_root(scope, None)?;
    let rec = ledger::InstallRecord {
        id: String::new(),
        name: app.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Kind;
    use std::path::PathBuf;

    fn sample(state: recipe::RecipeState, on_disk: bool) -> recipe::GeneratedRecipe {
        recipe::GeneratedRecipe {
            dir: PathBuf::from("/nonexistent/recipes/hello"),
            pkgbuild_path: PathBuf::from("/nonexistent/recipes/hello/PKGBUILD"),
            pkgbuild: "pkgname=hello\n".into(),
            checksum_filled: true,
            state,
            on_disk,
        }
    }

    /// Finding 21 of the 2026-11 review: `yapacc get x && x` must not go on
    /// after a no at the recipe gate.
    #[test]
    fn a_declined_recipe_gate_exits_3_and_says_what_was_not_done() {
        let r = sample(recipe::RecipeState::New, true);
        let e = typed_error(recipe_gate_outcome(Ok(false), &r).unwrap_err());
        assert_eq!(e.kind, Kind::Refused);
        assert_eq!(e.kind.exit_code(), 3);
        assert!(e.message.contains("nothing was built or installed"), "{e}");
        assert!(e.message.contains("/nonexistent/recipes/hello/PKGBUILD"));
        assert!(!e.message.contains("REFUSED"), "the marker is not shown");
    }

    #[test]
    fn a_recipe_gate_that_cannot_ask_exits_3() {
        let r = sample(recipe::RecipeState::New, true);
        // what review_gate returns in --script/--noconfirm without --no-review
        let refusal = recipe::review_gate(&r, false, true, true).unwrap_err();
        assert!(
            refusal.contains("Nothing was built or installed"),
            "{refusal}"
        );
        let e = typed_error(recipe_gate_outcome(Err(refusal), &r).unwrap_err());
        assert_eq!(e.kind.exit_code(), 3);
    }

    #[test]
    fn an_accepted_recipe_gate_goes_on() {
        let r = sample(recipe::RecipeState::Kept, true);
        assert!(recipe_gate_outcome(Ok(true), &r).is_ok());
        // --no-review accepts without asking, also in script mode
        assert_eq!(recipe::review_gate(&r, true, true, true), Ok(true));
    }

    #[test]
    fn a_declined_darling_path_exits_3_and_says_what_was_not_done() {
        let e = typed_error(darling_consent("somemacapp", Ok(false)).unwrap_err());
        assert_eq!(e.kind.exit_code(), 3);
        assert!(
            e.message.contains("nothing was downloaded or installed"),
            "{e}"
        );
        // a run that cannot ask is a refusal as well
        let e = typed_error(
            darling_consent("somemacapp", Err("stdin is not a terminal".into())).unwrap_err(),
        );
        assert_eq!(e.kind.exit_code(), 3);
        assert!(e.message.contains("stdin is not a terminal"));
        assert!(darling_consent("somemacapp", Ok(true)).is_ok());
    }

    #[test]
    fn hostile_recipe_values_are_refused_with_exit_3_before_anything_else() {
        for (name, url) in [
            ("$(id)", None),
            ("a b", None),
            ("-rf", None),
            ("hello", Some("https://example.org/$(id).tar.gz")),
            ("hello", Some("https://example.org/a'b.tar.gz")),
            ("hello", Some("https://example.org/`id`.tar.gz")),
            ("hello", Some("https://example.org/a\nb.tar.gz")),
            ("hello", Some("file:///etc/passwd")),
        ] {
            let e = typed_error(recipe_inputs_accepted(name, url).unwrap_err());
            assert_eq!(e.kind.exit_code(), 3, "{name:?} {url:?}");
            assert!(e.message.contains("nothing was generated"), "{e}");
        }
        assert!(
            recipe_inputs_accepted("hello", Some("https://example.org/hello-1.0.tar.gz")).is_ok()
        );
        assert!(recipe_inputs_accepted("hello", None).is_ok());
    }

    #[test]
    fn a_declined_formula_exits_3_and_a_missing_one_does_not() {
        let hostile = brew::parse_formula(
            "x",
            "class X < Formula\n  url \"https://example.com/x-$(id).tar.gz\"\nend\n",
        )
        .unwrap_err();
        assert_eq!(typed_error(formula_error(hostile)).kind.exit_code(), 3);
        let bad_name = brew::fetch_formula("../x").unwrap_err();
        assert_eq!(typed_error(formula_error(bad_name)).kind.exit_code(), 3);
        let missing = "formula 'x' not found in homebrew-core: curl failed".to_string();
        assert_eq!(typed_error(formula_error(missing)).kind.exit_code(), 1);
    }

    #[test]
    fn the_dry_run_report_says_nothing_was_written() {
        let would = recipe_dry_run_report(&sample(recipe::RecipeState::New, false));
        assert!(would.contains("generated for display only"), "{would}");
        assert!(would.contains("It would be written to"), "{would}");
        assert!(would.contains("pkgname=hello"), "the recipe is shown");
        assert!(would.contains("Nothing was written or built."), "{would}");
        assert!(would.ends_with(prompt::DRY_RUN_FOOTER), "{would}");
        let kept = recipe_dry_run_report(&sample(recipe::RecipeState::Kept, true));
        assert!(kept.contains("Existing recipe"), "{kept}");
        assert!(kept.contains("NOT regenerated"), "{kept}");
    }
}
