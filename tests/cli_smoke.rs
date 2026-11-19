//! End-to-end smoke tests: run the built binary and check behavior of
//! read-only commands. A test of something that exists only with pacman
//! checks, on any other system, that the command refuses with exit code 6
//! (or whatever the portable behaviour is) instead of being skipped; a test
//! that needs an installed pacman package is skipped without pacman.
//!
//! Every process started here has a private home (`common::isolate`): no
//! test reads the developer's configuration or writes below the real home.

mod common;

use std::process::{Command, Output};

fn bin() -> Command {
    common::command(env!("CARGO_BIN_EXE_yapacc"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("failed to run yapacc")
}

fn have_pacman() -> bool {
    Command::new("pacman")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// What a command or a lane that exists only with pacman must do on any
/// other system: exit code 6 ("not available here") and an error message
/// that contains `names` — never a crash, never a silent success.
fn assert_unavailable_here(o: &Output, names: &str) {
    let err = stderr(o);
    assert_eq!(
        o.status.code(),
        Some(6),
        "stdout: {} stderr: {err}",
        stdout(o)
    );
    assert!(
        err.contains(names),
        "the message must name `{names}`: {err}"
    );
}

#[test]
fn help_and_version() {
    let o = run(&["--help"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("Usage:"), "help output: {}", stdout(&o));
    let v = run(&["--version"]);
    assert!(v.status.success());
}

#[test]
fn config_subcommand_prints_effective_config() {
    let o = run(&["config"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = stdout(&o);
    assert!(out.contains("effective configuration"));
    assert!(out.contains("[general]"));
}

#[test]
fn completions_generate() {
    for sh in ["bash", "zsh", "fish"] {
        let o = run(&["completions", sh]);
        assert!(o.status.success(), "shell {sh} failed");
        assert!(!stdout(&o).trim().is_empty(), "empty completions for {sh}");
    }
    // a shell there is no script for is a usage error
    let bad = run(&["completions", "ksh"]);
    assert_eq!(bad.status.code(), Some(2), "stderr: {}", stderr(&bad));
}

#[test]
fn json_capabilities_is_valid_json() {
    let o = run(&["--json", "capabilities"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert!(v.get("capabilities").is_some(), "json: {v}");
    assert!(v.get("safety").is_some());
}

#[test]
fn json_list_when_pacman_present() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    let o = run(&["--json", "list", "--limit", "3"]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON array");
    let arr = v.as_array().expect("array");
    assert!(arr.len() <= 3);
    if !arr.is_empty() {
        assert!(arr[0].get("name").is_some());
    }
}

#[test]
fn ledger_is_auto_created_and_updated_on_install() {
    // No manual create/update verbs: an install must create the ledger and
    // record itself (user request). We use a local AppImage file.
    let tag = format!(
        "{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let dir = std::env::temp_dir().join(format!("pc-tui-ledger-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    let appimage_file = dir.join("smoke.AppImage");
    std::fs::write(&appimage_file, "#!/bin/sh\necho smoke\n").unwrap();
    let lroot = dir.join("ledger");
    let o = run(&[
        "install",
        "appimage",
        "--name",
        "smoke-app",
        "--source",
        &appimage_file.display().to_string(),
        "--dest",
        &dir.display().to_string(),
        "--ledger-path",
        &lroot.display().to_string(),
        "--yes",
    ]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        lroot.join("state.json").exists(),
        "ledger must be auto-created by the install"
    );
    let list = run(&[
        "--json",
        "ledger",
        "list",
        "--path",
        &lroot.display().to_string(),
    ]);
    assert!(list.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&list)).expect("valid JSON");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], serde_json::json!("smoke-app"));
    assert_eq!(arr[0]["format"], serde_json::json!("appimage"));
    let lpath = lroot.display().to_string();
    let verify = run(&["ledger", "verify", "--path", &lpath]);
    assert!(verify.status.success());
    let said = stdout(&verify);
    assert!(said.starts_with("verify: chain OK"), "verify said: {said}");
    assert!(
        said.contains("tamper evidence: NONE"),
        "an unanchored chain must say it is not tamper-evident: {said}"
    );

    // anchor round trip: the head printed now must verify later ...
    let head = stdout(&run(&["ledger", "head", "--path", &lpath]))
        .trim()
        .to_string();
    assert!(head.starts_with("2:"), "head said: {head}");
    let anchored = run(&["ledger", "verify", "--path", &lpath, "--anchor", &head]);
    assert!(anchored.status.success());
    assert!(stdout(&anchored).contains("confirmed unmodified"));

    // ... and a history that no longer contains it must fail with exit 5
    let forged = format!("2:{}", "0".repeat(64));
    let bad = run(&["ledger", "verify", "--path", &lpath, "--anchor", &forged]);
    assert_eq!(bad.status.code(), Some(5), "integrity failures exit 5");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("REWRITTEN"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exit_codes_follow_the_contract() {
    // 4: not found
    let o = run(&["why", "definitely-not-installed-xyz"]);
    assert_eq!(
        o.status.code(),
        Some(4),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    // 2: a name that could be mistaken for an option never reaches a package manager
    let o = run(&["why", "--", "--root=/tmp"]);
    assert_eq!(o.status.code(), Some(2));
    // 2: a config file that does not parse is an error, not a silent fallback
    let dir = std::env::temp_dir().join(format!("yapacc-badcfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");
    std::fs::write(&cfg, "[build]\nsandbox = \"sometimes\"\n").unwrap();
    let o = run(&["--config", &cfg.display().to_string(), "config"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("sandbox"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn platform_reports_every_axis_as_json() {
    let o = run(&["--json", "platform"]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    for key in [
        "os",
        "family",
        "native_pkg_manager",
        "pkg_managers",
        "init",
        "boot",
        "log_systems",
        "package_logs",
        "snapshot_providers",
        "support_tier",
    ] {
        assert!(v.get(key).is_some(), "platform JSON lacks '{key}': {v}");
    }
}

#[test]
fn why_answers_for_packages_installed_outside_yapacc() {
    if !have_pacman() {
        return;
    }
    // pacman itself was certainly not installed through yapacc
    let o = run(&["--json", "why", "pacman"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["native"]["name"], serde_json::json!("pacman"));
    assert_eq!(v["native"]["manager"], serde_json::json!("pacman"));
    assert!(v["ledger"].as_array().unwrap().is_empty());
}

#[test]
fn missing_component_is_a_clear_error_not_a_crash() {
    // Run the core from a directory that has no sibling components. A hard
    // link (same filesystem as the build output) avoids both a copy and the
    // "text file busy" race of executing a file another thread just wrote.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("yapacc-alone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lonely = dir.join("yapacc");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_yapacc"), &lonely).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_yapacc"), &lonely).unwrap();
    }
    let mut attempt = 0;
    let o = loop {
        match common::command(&lonely)
            .arg("i")
            .env_remove("YAPACC_LIBEXEC")
            .output()
        {
            Ok(o) => break o,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("cannot run {}: {e}", lonely.display()),
        }
    };
    // exit 6 unless a system-wide component happens to be installed
    if !std::path::Path::new("/usr/lib/yapacc/yapacc-tui").exists() {
        assert_eq!(o.status.code(), Some(6));
        assert!(String::from_utf8_lossy(&o.stderr).contains("yapacc-tui"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sources_apt_parses_test_root() {
    let dir = std::env::temp_dir().join(format!("pc-cli-apt-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sources.list.d")).unwrap();
    std::fs::write(
        dir.join("sources.list"),
        "deb http://smoke.example/debian stable main\n",
    )
    .unwrap();
    let o = run(&["sources", "apt", "--root", &dir.display().to_string()]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("smoke.example"), "out: {}", stdout(&o));
    let j = run(&[
        "--json",
        "sources",
        "apt",
        "--root",
        &dir.display().to_string(),
    ]);
    assert!(j.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&j)).expect("valid JSON");
    assert_eq!(
        v[0]["uri"],
        serde_json::json!("http://smoke.example/debian")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_expose_value_is_rejected() {
    // plan-only invocation still parses flags; invalid value must error
    let o = run(&[
        "foreign-install",
        "--name",
        "x",
        "--format",
        "deb",
        "--source",
        "/nonexistent.deb",
        "--expose",
        "bogus",
    ]);
    assert!(!o.status.success(), "bogus --expose must fail");
}

#[test]
fn aur_search_json_or_graceful_failure() {
    // network-dependent: accept success with JSON, or a clean non-zero exit with an error message
    let o = run(&["--json", "search", "ripgrep", "--limit", "3"]);
    if o.status.success() {
        let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
        assert!(v.is_array());
    } else {
        assert!(
            !String::from_utf8_lossy(&o.stderr).trim().is_empty(),
            "expected an error message on failure"
        );
    }
}

#[test]
fn docs_command_serves_topics_and_man() {
    let idx = run(&["docs"]);
    assert!(idx.status.success());
    let out = stdout(&idx);
    assert!(out.contains("getting-started") && out.contains("repo-trust"));
    assert!(
        out.contains("GNU General Public License, version 2"),
        "{out}"
    );
    for topic in ["workflow", "foreign", "ledger", "tui"] {
        let t = run(&["docs", topic]);
        assert!(t.status.success(), "topic {topic} failed");
        assert!(stdout(&t).len() > 200);
    }
    // the MCP server points its users at `yapacc docs mcp`
    let alias = run(&["docs", "mcp"]);
    assert!(alias.status.success());
    assert_eq!(stdout(&alias), stdout(&run(&["docs", "ai"])));
    let bad = run(&["docs", "no-such-topic"]);
    assert_eq!(bad.status.code(), Some(4), "stderr: {}", stderr(&bad));
    let man = run(&["docs", "--man"]);
    assert!(man.status.success());
    let m = stdout(&man);
    assert!(m.starts_with(".TH YAPACC 1"));
    assert!(m.contains(".SH COPYRIGHT"));
    assert!(m.contains("Alexandre BETRY"));
    assert!(m.contains("GNU General Public License"));
    assert!(m.contains("version 2 of the License only"));
    assert!(!m.contains("any later version"));
    // JSON topic listing
    let j = run(&["--json", "docs"]);
    assert!(j.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&j)).expect("valid JSON");
    assert!(
        v.as_array()
            .unwrap()
            .iter()
            .any(|t| t["slug"] == "workflow")
    );
}

#[test]
fn plan_save_and_dry_apply_roundtrip() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pc-cli-plan-{}", std::process::id()));
    let plan_path = dir.join("plan.json");
    let plan = run(&[
        "plan",
        "install",
        "ripgrep",
        "--out",
        &plan_path.display().to_string(),
    ]);
    assert!(
        plan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let data = std::fs::read_to_string(&plan_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&data).expect("plan is valid JSON");
    assert_eq!(v["op"], "install");
    assert_eq!(v["schema"], 1);
    // dry apply re-validates and prints the plan
    let apply = run(&[
        "plan",
        "apply",
        &plan_path.display().to_string(),
        "--dryrun",
    ]);
    assert!(
        apply.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(stdout(&apply).contains("DRY-RUN"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn container_plan_reports_engine() {
    let o = run(&["plan", "container", "--image", "debian:bookworm"]);
    // gated create is a dry run with --dryrun
    let d = run(&[
        "install",
        "container",
        "--image",
        "debian:bookworm",
        "--dryrun",
    ]);
    if !have_pacman() {
        // `plan` is a pacman-only command; `install container` is not.
        assert_unavailable_here(&o, "plan");
        if d.status.success() {
            assert!(stdout(&d).contains("DRY-RUN"), "out: {}", stdout(&d));
        } else {
            let err = stderr(&d);
            assert!(err.contains("no supported engine"), "stderr: {err}");
        }
        return;
    }
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("Container mediation plan"));
    assert!(out.contains("Engine:"), "out: {out}");
    if out.contains("available=false") || out.contains("Engine: none") {
        // no podman/docker here (e.g. the CI Arch container): the gated
        // create must refuse clearly instead of pretending
        assert!(!d.status.success());
        assert!(
            stderr(&d).contains("no supported engine"),
            "stderr: {}",
            stderr(&d)
        );
    } else {
        assert!(d.status.success());
        assert!(stdout(&d).contains("DRY-RUN"));
    }
}

#[test]
fn daemon_install_dryrun_plans_only() {
    let o = run(&["monitor", "daemon-install", "--dryrun"]);
    // the check the timer would run reads pacman and the AUR: on a system
    // with another package manager the command says so
    if native_manager().is_some_and(|m| m != "pacman") {
        assert_unavailable_here(&o, "monitor");
        return;
    }
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = stdout(&o);
    // the unit must start a command that exists: `monitor check`
    assert!(out.contains(" monitor check"), "out: {out}");
    assert!(!out.contains("monitor-check"), "out: {out}");
    assert!(out.contains("DRY-RUN"));
}

#[test]
fn pacdiff_list_scans_root() {
    // scan a temp root with a planted candidate
    let dir = std::env::temp_dir().join(format!("pc-pacdiff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("test.conf.pacnew"), "new default\n").unwrap();
    std::fs::write(dir.join("test.conf"), "current\n").unwrap();
    let o = run(&["inspect", "pacdiff", "--root", &dir.display().to_string()]);
    if !have_pacman() {
        // .pacnew files are pacman's: `inspect` does not exist elsewhere
        assert_unavailable_here(&o, "inspect");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(o.status.success());
    assert!(
        stdout(&o).contains("test.conf.pacnew"),
        "out: {}",
        stdout(&o)
    );
    let j = run(&[
        "--json",
        "inspect",
        "pacdiff",
        "--root",
        &dir.display().to_string(),
    ]);
    assert!(j.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&j)).unwrap();
    assert!(
        v.as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("test.conf.pacnew"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn news_command_network_tolerant() {
    let o = run(&["news", "--limit", "3"]);
    if o.status.success() {
        assert!(stdout(&o).contains("Arch Linux news"));
    } else {
        assert!(!String::from_utf8_lossy(&o.stderr).trim().is_empty());
    }
}

#[test]
fn aur_install_deps_flag_parses() {
    // --dryrun must print the closure-aware plan and stop
    let o = run(&["install", "aur", "yay", "--deps", "--dryrun"]);
    if !have_pacman() {
        // the AUR is pacman's: the lane refuses before it plans anything
        assert_unavailable_here(&o, "install aur");
        assert!(!stdout(&o).contains("AUR install plan"));
        return;
    }
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = stdout(&o);
    assert!(out.contains("AUR install plan"));
    assert!(out.contains("closure: yes (--deps)"));
    assert!(out.contains("DRY-RUN"));
}

#[test]
fn dryrun_flag_prevents_application() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    // autoremove --dryrun: plan only, explicit DRY-RUN marker
    let o = run(&["remove", "orphans", "--dryrun"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        stdout(&o).to_lowercase().contains("dry-run") || stdout(&o).contains("Nothing to remove")
    );
    // without --dryrun and without --yes: refuses to execute (gated)
    let gated = run(&["remove", "orphans"]);
    if !stdout(&gated).contains("Nothing to remove") {
        assert!(
            !gated.status.success(),
            "remove orphans without --yes/--dryrun must refuse"
        );
        let err = String::from_utf8_lossy(&gated.stderr);
        assert!(
            err.contains("not a terminal") || err.contains("--script") || err.contains("refusing"),
            "expected a gating error, got: {err}"
        );
    }
}

#[test]
fn get_explain_resolves_repo_lane() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    let o = run(&["get", "ripgrep", "--explain"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = stdout(&o);
    assert!(out.contains("Resolution: ripgrep"), "out: {out}");
    assert!(out.contains("repo"), "out: {out}");
    assert!(out.contains("chosen: repo (native mode)"), "out: {out}");
    // --explain never installs
    assert!(!out.to_lowercase().contains("installing"));
}

#[test]
fn get_explain_falls_back_to_recipe() {
    let o = run(&["get", "definitely-not-a-real-pkg-xyz-42", "--explain"]);
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("Resolution: definitely-not-a-real-pkg-xyz-42"),
        "out: {out}"
    );
    if !have_pacman() {
        // no recipe lane without pacman: a name found nowhere resolves to
        // nothing, and --explain says so
        assert!(out.contains("chosen: none"), "out: {out}");
        assert!(!out.contains("recipe"), "out: {out}");
        return;
    }
    assert!(out.contains("chosen: recipe"), "out: {out}");
    assert!(out.contains("review-gated"), "out: {out}");
}

#[test]
fn get_json_resolution_is_valid_json() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    let o = run(&["--json", "get", "ripgrep", "--explain"]);
    assert!(
        o.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert!(
        v["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["lane"] == "repo")
    );
    assert_eq!(v["chosen"], serde_json::json!("repo"));
}

#[test]
fn get_from_invalid_lane_rejected() {
    // a word that is no lane anywhere is a usage error on every manager,
    // and the refusal names the lane that was asked for
    let o = run(&["get", "foo", "--from", "nonsense"]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "stderr: {err}");
    assert!(
        err.contains("invalid --from lane 'nonsense'"),
        "stderr: {err}"
    );
    // a lane that exists only on Arch is unavailable elsewhere
    if native_manager().is_some_and(|m| m != "pacman") {
        let o = run(&["get", "foo", "--from", "recipe", "--dryrun"]);
        assert_unavailable_here(&o, "lanes here:");
    }
}

#[test]
fn get_from_recipe_generates_scaffold_dryrun() {
    // dry-run: recipe generated, gate NOT reached, nothing built
    let o = run(&["get", "sometestpkg", "--from", "recipe", "--dryrun"]);
    // The scaffold goes below $XDG_DATA_HOME, which is the private home of
    // this test run: nothing is written to the developer's own.
    let recipe_dir = common::data_home().join("yapacc/recipes/sometestpkg");
    if !have_pacman() {
        // a recipe is a PKGBUILD: the lane does not exist without pacman,
        // and a refused lane writes nothing
        assert_unavailable_here(&o, "'recipe' lane");
        assert!(!recipe_dir.exists(), "a refused lane wrote a recipe");
        return;
    }
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("recipe generated"), "out: {out}");
    assert!(out.contains("DRY-RUN"), "out: {out}");
    // the recipe is shown, and a dry run writes nothing at all
    assert!(out.contains("pkgname=sometestpkg"), "out: {out}");
    assert!(out.contains("REVIEW BEFORE BUILDING"), "out: {out}");
    assert!(out.contains("nothing was written"), "out: {out}");
    assert!(!recipe_dir.exists(), "a dry run wrote a recipe");
}

#[test]
fn get_explain_probes_brew_lane() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    // network-dependent: brew probe either finds the formula or reports a clean miss
    // the brew lane is opt-in: it appears only when the configuration names it
    let cfg = std::env::temp_dir().join(format!("yapacc-brew-{}.toml", std::process::id()));
    std::fs::write(
        &cfg,
        "[install]\norder = [\"repo\", \"aur\", \"flatpak\", \"brew\", \"recipe\"]\n",
    )
    .unwrap();
    let o = run(&[
        "--config",
        cfg.to_str().unwrap(),
        "get",
        "ripgrep",
        "--explain",
    ]);
    let _ = std::fs::remove_file(&cfg);
    assert!(o.status.success());
    let out = stdout(&o);
    assert!(out.contains("brew"), "brew lane must appear: {out}");
    let default = stdout(&run(&["get", "ripgrep", "--explain"]));
    assert!(
        !default.contains("brew"),
        "brew is not probed by default: {default}"
    );
    if out.contains("homebrew formula") {
        assert!(out.contains("ripgrep"), "formula metadata present");
    }
    // repo lane still wins (exact-name official-first)
    assert!(out.contains("chosen: repo (native mode)"), "out: {out}");
}

#[test]
fn get_from_brew_forces_lane() {
    // --from brew parses; network may fail gracefully with an actionable error
    let o = run(&["get", "ripgrep", "--from", "brew", "--explain"]);
    if !have_pacman() {
        // the brew lane is offered beside pacman only
        assert_unavailable_here(&o, "'brew' lane");
        return;
    }
    // --explain is read-only; forcing brew should print the formula table or a clean error
    if o.status.success() {
        assert!(stdout(&o).contains("Resolution: ripgrep"));
    } else {
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains("not found in lane") || err.contains("homebrew"),
            "err: {err}"
        );
    }
}

#[test]
fn plugin_lane_end_to_end_with_mock_adapter() {
    // mock community lane: a 3-line shell adapter speaking protocol v1
    let dir = std::env::temp_dir().join(format!("pc-plugin-mock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plugin = dir.join("yapacc-source-mocklane");
    std::fs::write(&plugin, "#!/bin/sh\ncase \"$1\" in --protocol) echo 1;; --probe) echo '{\"found\":true,\"source\":\"mock-store\",\"version\":\"1.0\",\"note\":\"mock lane hit\"}';; *) exit 1;; esac\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&plugin).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&plugin, perm).unwrap();
    let cfg = dir.join("config.toml");
    std::fs::write(&cfg, format!("[plugins]\ndirs = [\"{}\"]\n", dir.display())).unwrap();

    // plugins list shows it with protocol ok
    let list = run(&["--config", &cfg.display().to_string(), "plugins"]);
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let out = stdout(&list);
    assert!(out.contains("mocklane"), "out: {out}");
    assert!(out.contains("ok"), "out: {out}");

    // resolver probes the plugin lane before recipe
    let res = run(&[
        "--config",
        &cfg.display().to_string(),
        "get",
        "some-random-thing-xyz",
        "--explain",
    ]);
    assert!(res.status.success(), "stderr: {}", stderr(&res));
    let rout = stdout(&res);
    if have_pacman() {
        assert!(rout.contains("mocklane"), "lane missing: {rout}");
        assert!(rout.contains("mock-store"), "out: {rout}");
        assert!(rout.contains("chosen: mocklane"), "out: {rout}");
    } else {
        // Without pacman the resolver knows the repo and flatpak lanes
        // only: the adapter is listed above but not asked. The name exists
        // nowhere, so nothing is chosen.
        assert!(rout.contains("chosen: none"), "out: {rout}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_stdio_handshake_and_tool_call() {
    use std::io::Write as _;
    let mut child = bin()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn yapacc mcp");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
    )
    .unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"yapacc_install","arguments":{{"name":"zzz","confirm":false}}}}}}"#
    )
    .unwrap();
    drop(stdin); // EOF ends the serve loop
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() >= 2, "expected 2+ responses, got: {text}");
    let init: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "yapacc");
    let tools: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let names = tools["result"]["tools"].as_array().unwrap();
    assert!(names.iter().any(|t| t["name"] == "yapacc_search"));
    let install: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    let itext = install["result"]["content"][0]["text"].as_str().unwrap();
    assert!(itext.contains("PLAN ONLY"), "safety gate: {itext}");
}

#[test]
fn mcp_end_to_end_search_via_stdio() {
    if !have_pacman() {
        eprintln!("skipping: pacman not installed");
        return;
    }
    use std::io::Write as _;
    let mut child = bin()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"yapacc_search","arguments":{{"query":"ripgrep","limit":3}}}}}}"#
    )
    .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let resp: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let payload = resp["result"]["content"][0]["text"].as_str().unwrap();
    let results: serde_json::Value = serde_json::from_str(payload).expect("search JSON");
    assert!(
        results
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "ripgrep")
    );
}

#[test]
fn doctor_json_lists_the_checks_in_the_order_of_the_spec() {
    let o = run(&["--json", "doctor"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    let checks = v.as_array().expect("an array");
    let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "platform",
            "native-manager",
            "config",
            "component:yapacc-tui",
            "component:yapacc-mcp",
            "sandbox",
            "ledger:user",
            "ledger:system",
            "tool:flatpak",
            "tool:gpgv",
            "tool:bwrap",
            "repo-index",
            "tool:curl",
            "undo",
            "backend-maturity",
            "timeline",
            "privilege",
            "briefing",
        ]
    );
    let states: Vec<&str> = checks
        .iter()
        .map(|c| c["state"].as_str().unwrap())
        .collect();
    assert!(
        states
            .iter()
            .all(|s| ["ok", "warn", "fail", "n/a"].contains(s)),
        "{states:?}"
    );
    assert!(checks.iter().all(|c| c["detail"].is_string()));
    // exit status 1 exactly when a check is `fail`
    let expected = if states.contains(&"fail") { 1 } else { 0 };
    assert_eq!(o.status.code(), Some(expected), "states: {states:?}");
}

#[test]
fn doctor_creates_no_file() {
    let home = std::env::temp_dir().join(format!("yapacc-doctor-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let o = bin()
        .arg("doctor")
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .output()
        .expect("failed to run yapacc");
    let out = stdout(&o);
    assert_eq!(out.lines().count(), 18, "doctor output: {out}");
    assert!(
        out.contains("ledger:user           — no journal"),
        "doctor output: {out}"
    );
    let left: Vec<_> = std::fs::read_dir(&home).unwrap().flatten().collect();
    assert!(left.is_empty(), "doctor created {left:?}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn gate_without_consent_exits_3_and_changes_nothing() {
    // A gated command without --yes: the test runs it with no terminal on
    // stdin, so the gate cannot ask and must refuse.
    let dir = std::env::temp_dir().join(format!("yapacc-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let appimage_file = dir.join("gate.AppImage");
    std::fs::write(&appimage_file, "#!/bin/sh\necho gate\n").unwrap();
    let dest = dir.join("apps");
    let lroot = dir.join("ledger");
    let args = [
        "install",
        "appimage",
        "--name",
        "gate-app",
        "--source",
        &appimage_file.display().to_string(),
        "--dest",
        &dest.display().to_string(),
        "--ledger-path",
        &lroot.display().to_string(),
    ];
    let o = run(&args);
    assert_eq!(o.status.code(), Some(3), "a refused gate exits 3");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("stdin is not a terminal"), "stderr: {err}");
    assert!(!err.contains("REFUSED"), "the marker is not shown: {err}");
    assert!(!dest.exists(), "a refused install must not copy anything");
    assert!(
        !lroot.exists(),
        "a refused install must not create a ledger"
    );

    // --script without --yes: the same exit code
    let o = bin().arg("--script").args(args).output().unwrap();
    assert_eq!(o.status.code(), Some(3));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("--script never prompts"), "stderr: {err}");
    assert!(!dest.exists() && !lroot.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_safety_snapshot_value_is_rejected() {
    let dir = std::env::temp_dir().join(format!("yapacc-snapcfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("c.toml");
    std::fs::write(&cfg, "[safety]\nsnapshot = \"bogus\"\n").unwrap();
    let o = run(&["--config", cfg.to_str().unwrap(), "config"]);
    assert_eq!(o.status.code(), Some(2), "bad value is a usage error");
    assert!(String::from_utf8_lossy(&o.stderr).contains("snapshot"));
    std::fs::write(&cfg, "[safety]\nsnapshot_provider = \"bogus\"\n").unwrap();
    let o = run(&["--config", cfg.to_str().unwrap(), "config"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("snapshot_provider"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn dryrun_never_invokes_the_snapshot_tool_but_names_it() {
    use std::os::unix::fs::PermissionsExt;
    if !have_pacman() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("yapacc-snapdry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("called");
    let fake = dir.join("timeshift");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> {}\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = dir.join("c.toml");
    std::fs::write(&cfg, "[safety]\nsnapshot = \"auto\"\n").unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = bin()
        .env("PATH", path)
        .args(["--config", cfg.to_str().unwrap()])
        .args(["install", "repo", "tree", "--dryrun"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(!marker.exists(), "snapshot tool ran under --dryrun");
    if out.contains("Will run") {
        assert!(
            out.contains("snapshot will be taken through timeshift"),
            "stdout: {out}"
        );
        assert!(out.contains("DRY-RUN"), "stdout: {out}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The command surface as a whole: help, documentation, --json, exit codes.
// ---------------------------------------------------------------------------

use clap::CommandFactory;

/// Every command and subcommand, with the words that lead to it.
fn all_commands() -> Vec<(Vec<String>, clap::Command)> {
    fn walk(cmd: &clap::Command, path: &[String], out: &mut Vec<(Vec<String>, clap::Command)>) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let mut here = path.to_vec();
            here.push(sub.get_name().to_string());
            out.push((here.clone(), sub.clone()));
            walk(sub, &here, out);
        }
    }
    let mut out = Vec::new();
    walk(&yapacc::cli::Cli::command(), &[], &mut out);
    out
}

/// The native package manager `yapacc platform` detects here, if any.
fn native_manager() -> Option<String> {
    let o = run(&["--json", "platform"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).ok()?;
    v["native_pkg_manager"].as_str().map(str::to_string)
}

/// Finding 24 of the interface review: 60 subcommands and about 198 flags
/// had no description. Every command, argument and flag has one, and
/// `yapacc help …` prints it, with the global options under their own
/// heading instead of among the command's own.
#[test]
fn help_describes_every_command_argument_and_flag() {
    let commands = all_commands();
    assert!(
        commands.len() > 130,
        "only {} commands found",
        commands.len()
    );
    let mut missing = Vec::new();
    for (path, cmd) in &commands {
        let name = path.join(" ");
        let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
        if about.trim().is_empty() {
            missing.push(format!("`{name}` has no description"));
        }
        let mut own_flags = Vec::new();
        for arg in cmd.get_arguments() {
            let id = arg.get_id().as_str();
            if id == "help" || id == "version" {
                continue;
            }
            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            if help.trim().is_empty() {
                missing.push(format!("`{name}`: `{id}` has no description"));
            }
            if let Some(long) = arg.get_long() {
                own_flags.push(format!("--{long}"));
            }
        }
        // the same through the program, as a user asks for it
        let mut args = vec!["help".to_string()];
        args.extend(path.iter().cloned());
        let o = bin().args(&args).output().expect("failed to run yapacc");
        let out = stdout(&o);
        assert_eq!(o.status.code(), Some(0), "help {name}: {}", stderr(&o));
        assert!(
            out.contains(&format!("Usage: yapacc {name}")),
            "help {name}: {out}"
        );
        let first_words: Vec<&str> = about.split_whitespace().take(3).collect();
        assert!(
            out.starts_with(&first_words.join(" ")),
            "help {name} does not start with its description: {out}"
        );
        let (own, global) = out
            .split_once("\nGlobal options:\n")
            .unwrap_or_else(|| panic!("help {name} has no `Global options:` heading: {out}"));
        for flag in ["--script", "--noconfirm", "--config <PATH>", "--json"] {
            assert!(
                global.contains(flag),
                "help {name}: {flag} not under its heading"
            );
            let flag_line = format!("\n      {flag} ");
            assert!(
                !own.contains(&flag_line),
                "help {name}: {flag} is among the command's own options"
            );
        }
        for flag in own_flags {
            assert!(own.contains(&flag), "help {name} lacks {flag}: {out}");
        }
        // an option or argument line with nothing after its name. Such a
        // line is indented by two spaces (arguments, short options) or six
        // (long options); a wrapped description is indented further.
        let lines: Vec<&str> = out.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let entry = ["  <", "  [", "  -", "      --"]
                .iter()
                .any(|p| line.starts_with(p) && !line[p.len() - 1..].starts_with(' '));
            if !entry || line.trim().contains("  ") {
                continue;
            }
            // the name fills the line: the description follows, indented
            let next = lines.get(i + 1).copied().unwrap_or("");
            assert!(
                next.starts_with("          ") && !next.trim().is_empty(),
                "help {name}: `{}` has no description",
                line.trim()
            );
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn the_top_level_help_says_what_the_program_is() {
    let o = run(&["--help"]);
    let out = stdout(&o);
    assert!(
        out.starts_with("An optional companion to the package manager you already have"),
        "{out}"
    );
    for part in [
        "pacman, apt, dnf,",
        "Exit codes:",
        "0 ok",
        "3 refused",
        "6 something the command needs is unavailable",
        "`yapacc docs`",
        "https://ezechiel203.github.io/yapacc/",
        "Global options:",
    ] {
        assert!(out.contains(part), "--help lacks {part:?}: {out}");
    }
    // the gate flags cannot be given together, and say what they do
    let gate = stdout(&run(&["help", "upgrade"]));
    assert!(
        gate.contains("--dryrun") && gate.contains("change nothing"),
        "{gate}"
    );
    assert!(!gate.contains("never dry-run"), "{gate}");
    let both = run(&["refresh", "--dryrun", "--yes"]);
    assert_eq!(both.status.code(), Some(2), "stderr: {}", stderr(&both));
    assert!(
        stderr(&both).contains("cannot be used with"),
        "{}",
        stderr(&both)
    );
    // the lanes `get --from` accepts are the ones its help names
    let get = stdout(&run(&["help", "get"]));
    for lane in ["repo", "flatpak", "aur", "brew", "recipe"] {
        assert!(get.contains(lane), "help get lacks the lane {lane}: {get}");
    }
    // `index` keeps its file under the user's cache directory, not in /tmp
    for sub in ["build", "search"] {
        let index = stdout(&run(&["help", "index", sub]));
        assert!(index.contains("XDG_CACHE_HOME/yapacc/"), "{index}");
        assert!(!index.contains("/tmp"), "{index}");
    }
    // portable verbs are described without naming one package manager
    for path in [
        &["install", "repo"][..],
        &["remove", "repo"],
        &["upgrade"],
        &["refresh"],
    ] {
        let mut args = vec!["help"];
        args.extend(path);
        let text = stdout(&run(&args));
        let about = text.lines().next().unwrap_or("");
        assert!(!about.contains("pacman"), "{path:?}: {about}");
    }
}

/// Every `yapacc …` command line printed by the built-in topics and by the
/// EXAMPLES of the manual page is accepted by the program: it is run with
/// `--help` appended, which parses the line and changes nothing.
#[test]
fn every_documented_command_line_is_accepted_by_the_program() {
    let mut lines: Vec<(String, String)> = Vec::new();
    for (slug, _, _) in yapacc::docs::topics() {
        let text = yapacc::docs::render(slug).expect("a listed topic renders");
        // the same text reaches the user
        assert_eq!(stdout(&run(&["docs", slug])), text, "docs {slug}");
        for line in yapacc::docs::command_lines(&text) {
            lines.push((slug.to_string(), line));
        }
    }
    for line in yapacc::docs::command_lines(&yapacc::docs::index()) {
        lines.push(("(index)".into(), line));
    }
    let examples = yapacc::docs::man_examples();
    assert!(examples.len() >= 8, "{examples:?}");
    for line in examples {
        lines.push(("(manual page)".into(), line));
    }
    assert!(lines.len() > 60, "only {} command lines", lines.len());
    for (slug, line) in lines {
        let mut words = yapacc::docs::command_words(&line);
        assert_eq!(words.remove(0), "yapacc", "{slug}: {line}");
        if words.first().map(String::as_str) != Some("help") && !words.contains(&"--help".into()) {
            words.push("--help".into());
        }
        let o = bin().args(&words).output().expect("failed to run yapacc");
        assert_eq!(
            o.status.code(),
            Some(0),
            "{slug}: `{line}` is not accepted: {}",
            stderr(&o)
        );
        assert!(stdout(&o).contains("Usage: yapacc"), "{slug}: {line}");
    }
}

/// Arguments that make `cmd` parse: a value for each required argument.
fn required_args(cmd: &clap::Command) -> Vec<String> {
    let mut args = Vec::new();
    for arg in cmd.get_arguments() {
        if !arg.is_required_set() {
            continue;
        }
        let value = arg
            .get_possible_values()
            .first()
            .map(|v| v.get_name().to_string())
            .unwrap_or_else(|| "x".to_string());
        if let Some(long) = arg.get_long() {
            args.push(format!("--{long}"));
        }
        args.push(value);
    }
    args
}

/// Finding 21: `--json` was silently ignored by many commands, which then
/// printed prose with exit status 0. Every command is walked: one that is
/// not in the table of commands with a JSON form exits 2 and prints
/// nothing on standard output; one that is in it is never refused.
#[test]
fn json_is_refused_by_every_command_that_has_no_json_form() {
    let table: Vec<&str> = yapacc::cli::JSON_COMMANDS.iter().map(|(p, _)| *p).collect();
    let mut leaves = 0;
    let mut refused = 0;
    for (path, cmd) in all_commands() {
        if cmd.get_subcommands().next().is_some() {
            continue;
        }
        leaves += 1;
        let name = path.join(" ");
        if yapacc::cli::has_json(&name) {
            continue;
        }
        let mut args = vec!["--json".to_string()];
        args.extend(path.iter().cloned());
        args.extend(required_args(&cmd));
        let o = bin()
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("failed to run yapacc");
        assert_eq!(
            o.status.code(),
            Some(2),
            "--json {name}: stdout: {} stderr: {}",
            stdout(&o),
            stderr(&o)
        );
        assert!(
            stderr(&o).contains("no JSON output for this command"),
            "--json {name}: {}",
            stderr(&o)
        );
        assert!(
            stdout(&o).is_empty(),
            "--json {name} printed: {}",
            stdout(&o)
        );
        refused += 1;
    }
    assert_eq!(
        leaves,
        refused + table.len(),
        "a table entry is not a command"
    );
    assert!(refused > 80, "only {refused} commands were checked");

    // the conditions of the table
    for args in [
        &["--json", "docs", "daily"][..],
        &["--json", "docs", "--man"],
        &["--json", "get", "ripgrep"],
        &["--json", "get", "ripgrep", "--dryrun"],
    ] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("no JSON output for this command"),
            "{args:?}: {}",
            stderr(&o)
        );
        assert!(stdout(&o).is_empty(), "{args:?} printed: {}", stdout(&o));
    }
    let o = run(&["--json", "timeline", "--html"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stdout(&o).is_empty());

    // commands of the table that answer on any machine print JSON
    let dir = std::env::temp_dir().join(format!("yapacc-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display().to_string();
    for args in [
        &["platform"][..],
        &["doctor"],
        &["config"],
        &["docs"],
        &["capabilities"],
        &["external", "list"],
        &["foreign", "list", "--ledger-path", &d],
        &["ledger", "list", "--path", &d],
        &["ledger", "head", "--path", &d],
        &["ledger", "verify", "--path", &d],
        &["sources", "apt", "--root", &d],
        &["sources", "rpm", "--dir", &d],
    ] {
        assert!(
            table.contains(&args[..args.len().min(2)].join(" ").as_str())
                || table.contains(&args[0]),
            "{args:?} is not in the table"
        );
        let mut full = vec!["--json"];
        full.extend(args);
        let o = run(&full);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
        serde_json::from_str::<serde_json::Value>(&stdout(&o))
            .unwrap_or_else(|e| panic!("--json {args:?} is not JSON ({e}): {}", stdout(&o)));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finding 26: exit codes outside the documented contract.
#[test]
fn exit_codes_of_wrong_arguments_and_missing_things() {
    let code = |args: &[&str]| {
        let o = run(args);
        (o.status.code(), stderr(&o))
    };
    // 2: usage
    for args in [
        &["get"][..],
        &["ledger", "status", "--scope", "bogus"],
        &["completions", "tcsh"],
        &["--json", "plugins"],
        &["get", "ripgrep", "--from", "tap", "--explain"],
    ] {
        let (status, err) = code(args);
        assert_eq!(status, Some(2), "{args:?}: {err}");
    }
    // 4: an unknown topic
    let (status, err) = code(&["docs", "nosuch"]);
    assert_eq!(status, Some(4), "{err}");
    assert!(err.contains("unknown docs topic 'nosuch'"), "{err}");
    // 4: a category that does not exist
    assert_eq!(code(&["tree", "no-such-category"]).0, Some(4));
    // 4: a plan file that does not exist (where `plan` exists at all)
    let (status, err) = code(&["plan", "apply", "/nonexistent/yapacc/plan.json", "--dryrun"]);
    assert!(matches!(status, Some(4) | Some(6)), "{status:?}: {err}");

    // A name that is not a package name is a usage error on every package
    // manager, before any command is built from it.
    let bad = "a;b";
    for args in [
        &["info", bad][..],
        &["inspect", "files", bad],
        &["install", "repo", bad, "--dryrun"],
        &["remove", "repo", bad, "--dryrun"],
        &["get", bad, "--explain"],
    ] {
        let (status, err) = code(args);
        // on a machine with a package database but without its tool: 6
        assert!(
            matches!(status, Some(2) | Some(6)),
            "{args:?}: {status:?} {err}"
        );
        if status == Some(2) {
            assert!(
                err.contains("is not a valid package name"),
                "{args:?}: {err}"
            );
        }
    }

    // What follows needs a package manager to ask.
    let Some(manager) = native_manager() else {
        eprintln!("skipping the not-found checks: no native package manager detected");
        return;
    };
    for args in [
        &["info", "definitely-not-installed-xyz"][..],
        &["inspect", "files", "definitely-not-installed-xyz"],
        &["inspect", "owns", "/nonexistent/yapacc/no-such-file"],
    ] {
        let (status, err) = code(args);
        assert_eq!(status, Some(4), "{manager}: {args:?}: {err}");
        assert_eq!(
            err.matches("error:").count(),
            1,
            "{manager}: {args:?}: {err}"
        );
    }
    // a path that is not absolute is a usage error
    let (status, err) = code(&["inspect", "owns", "relative/path"]);
    assert_eq!(status, Some(2), "{manager}: {err}");
}

/// Finding 25: one wording for the last line of a dry run.
#[test]
fn every_dry_run_ends_with_the_same_line() {
    const FOOTER: &str = "DRY-RUN: nothing was changed. Run again without --dryrun to apply.";
    let dir = std::env::temp_dir().join(format!("yapacc-dry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display().to_string();
    // these two change nothing outside the user's own files
    let pacman_only = native_manager().is_some_and(|m| m != "pacman");
    for args in [
        &["monitor", "daemon-install", "--user", "--dryrun"][..],
        &[
            "install",
            "appimage",
            "--name",
            "x",
            "--source",
            "/nonexistent/x.AppImage",
            "--dest",
            &d,
            "--ledger-path",
            &d,
            "--dryrun",
        ],
    ] {
        let o = run(args);
        if args[0] == "monitor" && pacman_only {
            assert_unavailable_here(&o, "monitor");
            continue;
        }
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
        let out = stdout(&o);
        assert_eq!(
            out.trim_end().lines().last(),
            Some(FOOTER),
            "{args:?}: {out}"
        );
        assert_eq!(out.matches("DRY-RUN").count(), 1, "{args:?}: {out}");
    }
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "a dry run wrote a file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `plugins` printed "dirs =  + ~/.local/share/yapacc/plugins + PATH".
#[test]
fn plugins_lists_the_places_it_searched() {
    let o = run(&["plugins"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let first = out.lines().next().unwrap_or("");
    assert!(
        first.ends_with("Searched, in this order: ~/.local/share/yapacc/plugins, PATH"),
        "{first}"
    );
    assert!(!first.contains(" + ") && !first.contains(":  "), "{first}");
}
