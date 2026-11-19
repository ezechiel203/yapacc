// yapacc — local tooling: CI runner, website, release archives.
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! `cargo xtask ci` runs the steps of `.github/workflows/ci.yml` on this
//! machine, so a change can be checked without the GitHub runner.
//!
//! Jobs: `portable`, `musl`, `arch` (needs podman or docker), `families`
//! (needs podman or docker). `cargo xtask ci` runs all four; name jobs to
//! run fewer. `--no-containers` skips `arch` and `families`.
//!
//! `cargo xtask site` builds the website into `target/site/` (see `site.rs`).
//!
//! `cargo xtask dist` builds the release archives into `target/dist/` (see
//! `dist.rs`); the release workflow runs the same command.
//! `cargo xtask release-notes [VERSION]` prints that version's section of
//! CHANGELOG.md, the text of the GitHub release.

mod dist;
mod site;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const MUSL_TARGET: &str = "x86_64-unknown-linux-musl";

/// (image, manager, package, refresh command) — the `families` matrix of ci.yml.
const FAMILIES: &[(&str, &str, &str, &str)] = &[
    ("debian:stable", "apt", "bash", ""),
    ("ubuntu:latest", "apt", "bash", ""),
    ("fedora:latest", "dnf", "bash", ""),
    ("archlinux:latest", "pacman", "bash", ""),
    (
        "opensuse/tumbleweed:latest",
        "zypper",
        "bash",
        "zypper --non-interactive refresh",
    ),
    ("alpine:latest", "apk", "busybox", "apk update"),
    (
        "ghcr.io/void-linux/void-glibc:latest",
        "xbps",
        "base-files",
        "xbps-install -S",
    ),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask has a workspace root two levels up")
        .to_path_buf()
}

/// Where cargo puts what it builds: `CARGO_TARGET_DIR` when it is set (a
/// relative value is relative to the workspace root, where every command
/// here runs), `target/` otherwise. A `build.target-dir` of a cargo
/// configuration file is not looked at.
fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if !dir.is_empty() => root.join(dir),
        _ => root.join("target"),
    }
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    println!("\n$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program} {}` failed: {status}", args.join(" ")))
    }
}

fn engine() -> Option<String> {
    let want = std::env::var("CONTAINER_ENGINE").ok();
    let candidates: Vec<String> = match want {
        Some(e) if !e.is_empty() => vec![e],
        _ => vec!["podman".into(), "docker".into()],
    };
    candidates.into_iter().find(|e| {
        Command::new(e)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn portable(root: &Path) -> Result<(), String> {
    run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    let features = ["--no-default-features", "--features", "tui"];
    let mut clippy = vec!["clippy", "--locked", "--workspace"];
    clippy.extend(features);
    clippy.extend(["--all-targets", "--", "-D", "warnings"]);
    run(root, "cargo", &clippy)?;
    run(
        root,
        "cargo",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--no-default-features",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    let mut test = vec!["test", "--locked", "--workspace"];
    test.extend(features);
    test.push("--all-targets");
    run(root, "cargo", &test)?;
    run(
        root,
        "cargo",
        &[
            "build",
            "--release",
            "--locked",
            "--no-default-features",
            "--bin",
            "yapacc",
        ],
    )
}

fn musl_binary(root: &Path) -> PathBuf {
    target_dir(root)
        .join(MUSL_TARGET)
        .join("release")
        .join("yapacc")
}

fn musl(root: &Path) -> Result<(), String> {
    run(
        root,
        "cargo",
        &[
            "build",
            "--release",
            "--locked",
            "--no-default-features",
            "--target",
            MUSL_TARGET,
            "--bin",
            "yapacc",
        ],
    )?;
    let bin = musl_binary(root);
    let bin_s = bin.to_string_lossy().to_string();
    let file = Command::new("file")
        .arg(&bin)
        .output()
        .map_err(|e| format!("cannot run file: {e}"))?;
    let described = String::from_utf8_lossy(&file.stdout).to_string();
    println!("\n{}", described.trim());
    if !(described.contains("statically linked") || described.contains("static-pie linked")) {
        return Err(format!("{bin_s} is not statically linked"));
    }
    // ldd prints "=>" lines only for a dynamic executable
    let ldd = Command::new("ldd").arg(&bin).output();
    if let Ok(o) = ldd {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        if text.contains("=>") {
            return Err(format!("{bin_s} has dynamic dependencies:\n{text}"));
        }
    }
    run(root, &bin_s, &["--version"])?;
    let size = std::fs::metadata(&bin)
        .map_err(|e| format!("cannot stat {bin_s}: {e}"))?
        .len();
    let limit = dist::CORE_SIZE_LIMIT;
    println!("core binary: {size} bytes (limit {limit})");
    if size > limit {
        return Err(format!(
            "core binary is {size} bytes, over the {limit} budget"
        ));
    }
    Ok(())
}

/// The `arch` job: clippy, the full tests and the read-only command surface
/// inside `archlinux:base-devel`, on a copy of the tracked files so the
/// container's target directory never lands in the checkout. Both rows of
/// the workflow's matrix run in the one container: the default features
/// (what the Arch package ships: the pacman command-line lane), then
/// `--features alpm` (the local-build libalpm lane, kept from rotting).
fn arch(root: &Path, engine: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("yapacc-xtask-arch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let script = format!(
        "git -C '{r}' ls-files -z | xargs -0 tar -C '{r}' -cf - | tar -xf - -C '{t}'",
        r = root.display(),
        t = tmp.display()
    );
    let copied = run(root, "sh", &["-c", &script]);
    let result = copied.and_then(|()| {
        let mount = format!("{}:/src:Z", tmp.display());
        run(
            root,
            engine,
            &[
                "run",
                "--rm",
                "-v",
                &mount,
                "-w",
                "/src",
                "archlinux:base-devel",
                "sh",
                "-ec",
                ARCH_SCRIPT,
            ],
        )
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

const ARCH_SCRIPT: &str = r#"
pacman -Syu --noconfirm git rust curl jq procps-ng which bubblewrap
for features in "" "--features alpm"; do
echo "== arch: features: ${features:-default}"
cargo clippy --locked --workspace --all-targets $features -- -D warnings
cargo test --locked --workspace --all-targets $features
./target/debug/yapacc --help >/dev/null
./target/debug/yapacc config
./target/debug/yapacc --json platform | jq -e '.native_pkg_manager == "pacman"' >/dev/null
./target/debug/yapacc --json why pacman | jq -e '.native.manager == "pacman"' >/dev/null
./target/debug/yapacc --json capabilities | jq -e '.capabilities' >/dev/null
./target/debug/yapacc --json list --limit 3 | jq -e 'type == "array"' >/dev/null
./target/debug/yapacc inspect orphans >/dev/null
./target/debug/yapacc remove orphans --dryrun >/dev/null
done
"#;

/// One row of the `families` matrix: the static binary, mounted read-only.
fn family(root: &Path, engine: &str, row: &(&str, &str, &str, &str)) -> Result<(), String> {
    let (image, manager, pkg, refresh) = *row;
    let bin = musl_binary(root);
    let mount = format!("{}:/yapacc:ro,Z", bin.display());
    let refresh_step = if refresh.is_empty() {
        String::new()
    } else {
        format!("{refresh}\n")
    };
    let orphans = if manager == "pacman" {
        String::new()
    } else {
        "/yapacc inspect orphans\n".to_string()
    };
    let script = format!(
        r#"set -e
/yapacc platform
/yapacc --json platform | grep -q '"native_pkg_manager": "{manager}"'
/yapacc --json why {pkg} | grep -q '"manager": "{manager}"'
rc=0; /yapacc why definitely-not-installed-xyz || rc=$?; test "$rc" -eq 4
{refresh_step}/yapacc list --limit 3
/yapacc --json list --limit 3 | grep -q '^\['
/yapacc search bash --limit 3
/yapacc inspect owns /usr/bin/env
/yapacc check
{orphans}"#
    );
    println!("\n== families: {image} ({manager})");
    run(
        root,
        engine,
        &["run", "--rm", "-v", &mount, image, "sh", "-c", &script],
    )
}

fn families(root: &Path, engine: &str) -> Result<(), String> {
    if !musl_binary(root).exists() {
        musl(root)?;
    }
    let mut failed = vec![];
    for row in FAMILIES {
        if let Err(e) = family(root, engine, row) {
            eprintln!("FAILED {}: {e}", row.0);
            failed.push(row.0);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("families failed on: {}", failed.join(", ")))
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("site") {
        return match site::build(&root(), &args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("dist") {
        let root = root();
        return match dist::run(&root, &target_dir(&root), &args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("release-notes") {
        return match dist::release_notes(&root(), &args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) != Some("ci") {
        eprintln!(
            "usage: cargo xtask ci [portable] [musl] [arch] [families] [--no-containers]\n       cargo xtask site [--out DIR]\n       cargo xtask dist [--require-aarch64] [--allow-dirty] [--out DIR]\n       cargo xtask release-notes [VERSION]"
        );
        return ExitCode::from(2);
    }
    args.remove(0);
    let no_containers = args.iter().any(|a| a == "--no-containers");
    let mut jobs: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();
    if jobs.is_empty() {
        jobs = ["portable", "musl", "arch", "families"]
            .map(String::from)
            .to_vec();
    }
    let root = root();
    let engine = engine();
    let mut results: Vec<(String, Result<(), String>)> = vec![];
    for job in &jobs {
        println!("\n######## job: {job}");
        let needs_engine = matches!(job.as_str(), "arch" | "families");
        let outcome = match (job.as_str(), needs_engine, &engine) {
            (_, true, _) if no_containers => {
                println!("skipped (--no-containers)");
                continue;
            }
            (_, true, None) => Err("needs podman or docker (set CONTAINER_ENGINE)".to_string()),
            ("portable", _, _) => portable(&root),
            ("musl", _, _) => musl(&root),
            ("arch", _, Some(e)) => arch(&root, e),
            ("families", _, Some(e)) => families(&root, e),
            (other, _, _) => Err(format!("unknown job `{other}`")),
        };
        results.push((job.clone(), outcome));
    }
    println!("\n######## summary");
    let mut bad = false;
    for (job, r) in &results {
        match r {
            Ok(()) => println!("PASS {job}"),
            Err(e) => {
                bad = true;
                println!("FAIL {job}: {e}");
            }
        }
    }
    if bad {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FAMILIES` is written by hand beside the matrix of the workflow; this
    /// is what notices when one of the two changes alone.
    #[test]
    fn the_families_are_those_of_the_workflow() {
        let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
        let rows: Vec<&str> = workflow
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("- { image: "))
            .collect();
        assert_eq!(rows.len(), FAMILIES.len(), "{rows:#?}");
        for (image, manager, pkg, refresh) in FAMILIES {
            let start = format!("- {{ image: \"{image}\", manager: {manager}, pkg: {pkg}");
            let row = rows
                .iter()
                .find(|row| row.starts_with(&start))
                .unwrap_or_else(|| panic!("no row of ci.yml starts with `{start}`"));
            if refresh.is_empty() {
                assert!(!row.contains("refresh:"), "{row}");
            } else {
                assert!(row.contains(&format!("refresh: \"{refresh}\"")), "{row}");
            }
        }
    }

    #[test]
    fn the_target_directory_follows_the_environment_of_cargo() {
        // Only the default is checked here: the variable belongs to the
        // process and other tests run beside this one.
        if std::env::var_os("CARGO_TARGET_DIR").is_none() {
            assert_eq!(target_dir(Path::new("/w")), Path::new("/w/target"));
        }
    }
}
