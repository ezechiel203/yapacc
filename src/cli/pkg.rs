// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Package lifecycle helpers and monitor unit installation.

use super::*;

pub(super) fn cmd_scaffold(name: &str, dir: Option<&str>) -> Result<(), String> {
    let dir = dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("./{name}-pkg"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pkgbuild = format!(
        r#"# Maintainer: Your Name <you@example.com>
pkgname={name}
pkgver=0.1.0
pkgrel=1
pkgdesc="TODO: description"
arch=('x86_64')
url="https://example.invalid/$pkgname"
license=('MIT')
depends=()
makedepends=()
source=("$pkgname-$pkgver.tar.gz::https://example.invalid/$pkgname-$pkgver.tar.gz")
sha256sums=('SKIP')

build() {{ cd "$srcdir/$pkgname-$pkgver" || return 0; }}
package() {{ cd "$srcdir/$pkgname-$pkgver" || return 0; install -Dm644 /dev/null "$pkgdir/usr/share/doc/$pkgname/README.placeholder"; }}
"#
    );
    fs::write(Path::new(&dir).join("PKGBUILD"), pkgbuild).map_err(|e| e.to_string())?;
    fs::write(Path::new(&dir).join(".SRCINFO"), format!("pkgbase = {name}\n\tpkgdesc = TODO\n\tpkgver = 0.1.0\n\tpkgrel = 1\n\tarch = x86_64\n\npkgname = {name}\n")).map_err(|e| e.to_string())?;
    println!("Scaffolded package at {dir} (PKGBUILD + .SRCINFO placeholder)");
    Ok(())
}
pub(super) fn cmd_validate(target: &str) -> Result<(), String> {
    let path = Path::new(target);
    let pb = if path.is_dir() {
        path.join("PKGBUILD")
    } else {
        path.to_path_buf()
    };
    let content =
        fs::read_to_string(&pb).map_err(|e| format!("cannot read {}: {e}", pb.display()))?;
    println!("Validating {} (basic checks)", pb.display());
    let mut errs = 0;
    let mut warn = 0;
    for k in ["pkgname", "pkgver", "pkgrel", "pkgdesc", "arch", "license"] {
        if !content.contains(&format!("{k}=")) && !content.contains(&format!("{k} =")) {
            println!("ERROR: missing '{k}'");
            errs += 1;
        }
    }
    if content.contains("SKIP") {
        println!("WARN: checksums may be SKIP");
        warn += 1;
    }
    if content.contains("TODO") || content.contains("example.invalid") {
        println!("WARN: placeholders remain");
        warn += 1;
    }
    println!(
        "Result: {} (errors={}, warnings={})",
        if errs == 0 { "PASS-basic" } else { "FAIL" },
        errs,
        warn
    );
    // namcap lint when available (deeper PKGBUILD compliance checks)
    if Command::new("namcap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        println!("\n== namcap ==");
        let out = Command::new("namcap").arg(&pb).output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout);
            if !s.trim().is_empty() {
                print!("{s}");
            }
            if !o.status.success() {
                let e = String::from_utf8_lossy(&o.stderr);
                if !e.trim().is_empty() {
                    println!("namcap stderr: {e}");
                }
            }
        }
    } else {
        println!("namcap not installed; skipping deeper lint (pacman -S namcap)");
    }
    Ok(())
}

/// Install systemd monitor units from contrib/systemd templates:
/// rewrite ExecStart to this binary's absolute path, write service+timer to
/// the user (default) or system unit dir, and (gated) enable+start the timer.
pub(super) fn daemon_install_flow(
    user: bool,
    execute: bool,
    yes: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<(), String> {
    // The unit templates are compiled in, so an installed binary needs no
    // file beside it.
    let template = |name: &str| {
        safety::embedded("systemd", name)
            .ok_or_else(|| format!("unit template {name} is not built into this binary"))
    };
    let exe = crate::util::core_exe().display().to_string();
    let service = template("yapacc-monitor.service")?.replace(
        "/usr/bin/yapacc monitor check",
        &format!("{exe} monitor check"),
    );
    let timer = template("yapacc-monitor.timer")?.to_string();

    let (unit_dir, systemctl_args) = if user {
        let base = crate::config::xdg_base("XDG_CONFIG_HOME", ".config").ok_or_else(|| {
            "neither XDG_CONFIG_HOME nor HOME names a directory for user units".to_string()
        })?;
        (base.join("systemd/user"), vec!["--user".to_string()])
    } else {
        (
            std::path::PathBuf::from("/etc/systemd/system"),
            vec!["--system".to_string()],
        )
    };
    let service_path = unit_dir.join("yapacc-monitor.service");
    let timer_path = unit_dir.join("yapacc-monitor.timer");
    println!("== daemon install (systemd units) ==");
    println!("ExecStart: {exe} monitor check");
    println!(
        "units -> {} and {}",
        service_path.display(),
        timer_path.display()
    );
    println!(
        "then: systemctl {} enable --now yapacc-monitor.timer",
        systemctl_args.join(" ")
    );
    if !execute {
        println!("{}", prompt::DRY_RUN_FOOTER);
        return Ok(());
    }
    if !confirm_gate(yes, noninteractive, script_mode)? {
        println!("Aborted.");
        return Ok(());
    }
    if !user {
        ensure_privileges("install", true, false, noninteractive, script_mode)?;
    }
    fs::create_dir_all(&unit_dir).map_err(|e| format!("create {}: {e}", unit_dir.display()))?;
    fs::write(&service_path, &service)
        .map_err(|e| format!("write {}: {e}", service_path.display()))?;
    fs::write(&timer_path, &timer).map_err(|e| format!("write {}: {e}", timer_path.display()))?;
    let mut cmd = Command::new("systemctl");
    cmd.args(&systemctl_args)
        .args(["enable", "--now", "yapacc-monitor.timer"]);
    let out = cmd
        .output()
        .map_err(|e| format!("systemctl failed to start: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "units installed but enabling failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    println!(
        "Monitor timer installed and started (runs `monitor check` every 30min; never auto-applies)."
    );
    Ok(())
}
