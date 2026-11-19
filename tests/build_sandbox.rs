//! End-to-end: a real `makepkg` build inside the sandbox.
//!
//! The PKGBUILD below behaves like a hostile recipe: it tries to read a
//! file from the invoking user's home, looks for inherited secrets in the
//! environment, tries to write outside the build directory, looks for the
//! sockets and removable disks under `/run`, and checks for network
//! interfaces during build(). The build must SUCCEED (the sandbox must not
//! break ordinary packaging, fakeroot included) while every one of those
//! attempts fails.
//!
//! Self-skips where makepkg or a working bubblewrap is unavailable
//! (non-Arch hosts, containers without user namespaces).

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use yapacc::repair::{DepConsent, RepairPolicy, build_with_repair, makepkg_attempt};
use yapacc::sandbox;

fn scratch() -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "yapacc-build-sandbox-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn can_build() -> bool {
    let ok = yapacc::util::have("makepkg") && sandbox::probe(None).is_ok();
    if !ok {
        eprintln!("skipped: needs makepkg and a working bubblewrap");
    }
    ok
}

fn policy(build_net: bool) -> RepairPolicy {
    RepairPolicy {
        enabled: false,
        max_attempts: 1,
        allow_checksum_refresh: false,
        interactive: false,
        record_fix: false,
        lane: "recipe".into(),
        pkg: "sandbox-probe".into(),
        sandbox: sandbox::Policy::Require,
        build_net,
        dep_consent: DepConsent::Deny,
    }
}

/// What must not be reachable under `/run`: the session directory, the
/// system bus, removable disks, container engine sockets.
const RUN_PROBE: &str = r#"for p in /run/user /run/media /run/dbus /run/docker.sock /run/podman /run/containerd /var/run/docker.sock /var/run/dbus; do
    [ -e "$p" ] && echo "LEAK-RUN $p" >> "$_report"
  done"#;

fn pkgbuild(canary: &std::path::Path) -> String {
    format!(
        r#"pkgname=sandbox-probe
pkgver=1
pkgrel=1
pkgdesc="yapacc sandbox probe"
arch=('any')
license=('GPL-2.0-only')
options=('!debug')

_report="$startdir/report.txt"

prepare() {{
  : > "$_report"
  cat '{canary}' >/dev/null 2>&1 && echo LEAK-HOME >> "$_report"
  [ -n "${{YAPACC_TEST_SECRET:-}}" ] && echo LEAK-ENV >> "$_report"
  touch /usr/yapacc-probe 2>/dev/null && echo LEAK-WRITE >> "$_report"
  {RUN_PROBE}
  echo "prepare-home=$HOME" >> "$_report"
  # what the fetch phase leaves in $HOME (a cargo or go cache) ...
  echo fetched > "$HOME/phase-marker"
  return 0
}}

build() {{
  n=$(tail -n +3 /proc/net/dev | grep -vc '^ *lo:' || true)
  echo "build-ifaces=$n" >> "$_report"
  {RUN_PROBE}
  echo "build-run-entries=$(ls -A /run | wc -l)" >> "$_report"
  # ... must still be there in the build phase
  echo "build-home-marker=$(cat "$HOME/phase-marker" 2>/dev/null)" >> "$_report"
}}

package() {{
  install -Dm755 /dev/null "$pkgdir/usr/bin/sandbox-probe"
  echo "package-uid=$(id -u)" >> "$_report"
}}
"#,
        canary = canary.display()
    )
}

#[test]
fn hostile_recipe_builds_but_cannot_reach_anything() {
    if !can_build() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let canary = home.join(format!(".yapacc-build-canary-{}", std::process::id()));
    std::fs::write(&canary, "secret").unwrap();
    let dir = scratch();
    std::fs::write(dir.join("PKGBUILD"), pkgbuild(&canary)).unwrap();

    // SAFETY: test-only; set before any thread of this test reads the env.
    unsafe { std::env::set_var("YAPACC_TEST_SECRET", "hunter2") };
    let res = makepkg_attempt(&dir, None, &policy(false));
    let _ = std::fs::remove_file(&canary);

    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap_or_default();
    assert!(
        res.is_ok(),
        "sandboxed build failed: {res:?}\nreport:\n{report}"
    );
    assert!(!report.contains("LEAK"), "sandbox leaked:\n{report}");
    assert!(
        report.contains(&format!("prepare-home={}", sandbox::SANDBOX_HOME_PATH)),
        "HOME was not redirected:\n{report}"
    );
    assert!(
        report.contains("build-home-marker=fetched"),
        "the fetch phase and the build phase must share one $HOME:\n{report}"
    );
    assert!(
        !dir.join(sandbox::SANDBOX_HOME).exists(),
        "the throwaway home must not be created in the build directory"
    );
    assert!(
        report.contains("build-ifaces=0"),
        "build() must have no network:\n{report}"
    );
    assert!(
        report.contains("build-run-entries=0"),
        "/run must be empty in build():\n{report}"
    );
    assert!(
        report.contains("package-uid=0"),
        "fakeroot must work inside the sandbox:\n{report}"
    );
    let built = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".pkg.tar"));
    assert!(built, "no package artifact produced");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_net_flag_shares_the_network_for_build() {
    if !can_build() {
        return;
    }
    let dir = scratch();
    std::fs::write(
        dir.join("PKGBUILD"),
        pkgbuild(std::path::Path::new("/nonexistent/canary")),
    )
    .unwrap();
    let res = makepkg_attempt(&dir, None, &policy(true));
    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap_or_default();
    assert!(res.is_ok(), "{res:?}\n{report}");
    assert!(
        !report.contains("LEAK"),
        "sandbox leaked with --build-net:\n{report}"
    );
    // host networking is shared: unless this host has no interface at all,
    // build() sees more than loopback
    let host_ifaces = std::fs::read_to_string("/proc/net/dev")
        .map(|t| {
            t.lines()
                .skip(2)
                .filter(|l| !l.trim_start().starts_with("lo:"))
                .count()
        })
        .unwrap_or(0);
    if host_ifaces > 0 {
        assert!(!report.contains("build-ifaces=0"), "{report}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn mode(p: &Path) -> u32 {
    std::fs::symlink_metadata(p).unwrap().mode() & 0o7777
}

/// Finding 4 of the 2026-11 review, through the public entry point every
/// caller uses (`package srcinfo`, `install aur`, `get --from recipe`, the
/// checksum refresh): a build directory that carries
/// `.yapacc-home/.gnupg/pubring.kbx -> <victim file>` or
/// `.yapacc-home/.gnupg -> <victim dir>` must leave both victims alone.
/// The code before the fix chmod'ed the victim directory to 0700 and copied
/// the public keyring over the victim file.
#[test]
fn links_planted_in_the_build_directory_are_never_followed() {
    if sandbox::probe(None).is_err() {
        eprintln!("skipped: needs a working bubblewrap");
        return;
    }
    let root = scratch();
    let victim_file = root.join("victim-id_ed25519");
    std::fs::write(&victim_file, "PRIVATE KEY").unwrap();
    std::fs::set_permissions(&victim_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let victim_dir = root.join("victim-dir");
    std::fs::create_dir(&victim_dir).unwrap();
    std::fs::write(victim_dir.join("keep.txt"), "keep").unwrap();
    std::fs::set_permissions(&victim_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let file_case = root.join("build-file-link");
    let planted_file = file_case
        .join(sandbox::SANDBOX_HOME)
        .join(".gnupg/pubring.kbx");
    std::fs::create_dir_all(planted_file.parent().unwrap()).unwrap();
    symlink(&victim_file, &planted_file).unwrap();

    let dir_case = root.join("build-dir-link");
    let planted_dir = dir_case.join(sandbox::SANDBOX_HOME).join(".gnupg");
    std::fs::create_dir_all(planted_dir.parent().unwrap()).unwrap();
    symlink(&victim_dir, &planted_dir).unwrap();

    for dir in [&file_case, &dir_case] {
        // twice, as a build does (fetch phase, then build phase)
        for net in [sandbox::Net::Shared, sandbox::Net::Isolated] {
            let mut p = sandbox::command(
                dir,
                net,
                sandbox::Policy::Require,
                None,
                &["bash", "-c", "echo \"home=$HOME\""],
            )
            .unwrap();
            assert!(p.sandboxed);
            let host_home = p.home.as_ref().unwrap().path().to_path_buf();
            assert!(!host_home.starts_with(&root), "{}", host_home.display());
            let out = p.command.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                format!("home={}", sandbox::SANDBOX_HOME_PATH)
            );
            drop(p);
            assert!(!host_home.exists(), "the home is removed after the command");
        }
        assert_eq!(
            std::fs::read_to_string(&victim_file).unwrap(),
            "PRIVATE KEY",
            "victim file content changed"
        );
        assert_eq!(mode(&victim_file), 0o644, "victim file mode changed");
        assert_eq!(mode(&victim_dir), 0o755, "victim directory mode changed");
        let names: Vec<String> = std::fs::read_dir(&victim_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["keep.txt"], "victim directory gained entries");
    }
    // the planted links themselves are untouched
    for link in [&planted_file, &planted_dir] {
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Finding 14: with `/run` emptied, the fetch phase must still resolve
/// names. Self-skips when this host cannot resolve the name itself.
#[test]
fn fetch_phase_resolves_names_with_run_hidden() {
    if sandbox::probe(None).is_err() || !yapacc::util::have("getent") {
        eprintln!("skipped: needs a working bubblewrap and getent");
        return;
    }
    let host = "archlinux.org";
    let host_resolves = std::process::Command::new("getent")
        .args(["hosts", host])
        .output()
        .is_ok_and(|o| o.status.success());
    if !host_resolves {
        eprintln!("skipped: this host cannot resolve {host} (offline?)");
        return;
    }
    let dir = scratch();
    let script = format!(
        "getent hosts {host} >/dev/null && echo RESOLVED\n\
         for p in /run/user /run/media /run/dbus /run/docker.sock /run/podman /run/containerd; do\n\
           [ -e \"$p\" ] && echo \"LEAK-RUN $p\"\n\
         done\n\
         echo \"sockets=$(find /run -type s 2>/dev/null | wc -l)\"\n"
    );
    let mut p = sandbox::command(
        &dir,
        sandbox::Net::Shared,
        sandbox::Policy::Require,
        None,
        &["bash", "-c", &script],
    )
    .unwrap();
    let out = p.command.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("RESOLVED"),
        "name resolution is broken in the fetch phase: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!stdout.contains("LEAK"), "{stdout}");
    assert!(stdout.contains("sockets=0"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A real download through the sandboxed fetch phase (`makepkg --nobuild`
/// with curl), then a build without network. Needs the Internet, so it is
/// not part of the default run:
/// `cargo test --test build_sandbox -- --ignored real_download`.
#[test]
#[ignore = "downloads a file from www.gnu.org"]
fn real_download_in_the_fetch_phase() {
    if !can_build() {
        return;
    }
    let dir = scratch();
    std::fs::write(
        dir.join("PKGBUILD"),
        r#"pkgname=sandbox-fetch
pkgver=1
pkgrel=1
pkgdesc="yapacc sandbox fetch probe"
arch=('any')
license=('GPL-2.0-only')
options=('!debug')
source=('https://www.gnu.org/licenses/gpl-2.0.txt')
sha256sums=('SKIP')

package() {
  install -Dm644 "$srcdir/gpl-2.0.txt" "$pkgdir/usr/share/doc/sandbox-fetch/gpl-2.0.txt"
}
"#,
    )
    .unwrap();
    let mut pol = policy(false);
    pol.pkg = "sandbox-fetch".into();
    let res = makepkg_attempt(&dir, None, &pol);
    assert!(res.is_ok(), "{res:?}");
    let fetched = std::fs::read_to_string(dir.join("gpl-2.0.txt")).unwrap_or_default();
    assert!(
        fetched.contains("GNU GENERAL PUBLIC LICENSE"),
        "the source was not downloaded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const REFRESH_PKGBUILD: &str = r#"pkgname=sandbox-refresh
pkgver=1
pkgrel=1
pkgdesc="yapacc checksum refresh probe"
arch=('any')
license=('GPL-2.0-only')
options=('!debug')
source=('hello.txt')
sha512sums=('00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000')

package() {
  install -Dm644 "$srcdir/hello.txt" "$pkgdir/usr/share/sandbox-refresh/hello.txt"
}
"#;

/// Finding 19: `--allow-checksum-refresh` must rewrite the PKGBUILD of the
/// directory that is being built (the AUR lane builds in `./NAME-aur-build`),
/// not `<recipes root>/NAME/PKGBUILD`. The build directory here is nowhere
/// near the recipes root, and the array is `sha512sums`, as in most AUR
/// packages.
#[test]
fn checksum_refresh_rewrites_the_pkgbuild_that_is_being_built() {
    if !can_build() {
        return;
    }
    let dir = scratch();
    std::fs::write(dir.join("PKGBUILD"), REFRESH_PKGBUILD).unwrap();
    std::fs::write(dir.join("hello.txt"), "hello\n").unwrap();
    let elsewhere = yapacc::recipe::recipes_root().join("sandbox-refresh");
    let elsewhere_before = elsewhere.exists();

    // without the flag the mismatch stops the build and changes nothing
    let mut pol = policy(false);
    pol.pkg = "sandbox-refresh".into();
    pol.enabled = true;
    pol.max_attempts = 2;
    let refused = build_with_repair(&dir, None, &pol);
    assert!(refused.is_err(), "a checksum mismatch must fail the build");
    assert_eq!(
        std::fs::read_to_string(dir.join("PKGBUILD")).unwrap(),
        REFRESH_PKGBUILD,
        "no refresh without --allow-checksum-refresh"
    );

    pol.allow_checksum_refresh = true;
    let built = build_with_repair(&dir, None, &pol);
    assert!(built.is_ok(), "{built:?}");
    let after = std::fs::read_to_string(dir.join("PKGBUILD")).unwrap();
    // sha512 of "hello\n"
    assert!(
        after.contains("sha512sums=('e7c22b994c59d9cf2b48e549b1e24666636045930d3da7c1acb299d1c3b7f931f94aae41edda2c2b207a36e10f8bcb8d45223e54878f5b316e7ce3b6bc019629')"),
        "the PKGBUILD in the build directory was not refreshed:\n{after}"
    );
    assert!(!after.contains("0000000000"), "{after}");
    assert!(after.contains("package() {"), "the rest is kept:\n{after}");
    assert_eq!(
        elsewhere.exists(),
        elsewhere_before,
        "nothing may be created under the recipes root"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finding 25: what goes on to `pacman -U` is the package of this build,
/// not everything whose name contains `.pkg.tar`.
#[test]
fn only_the_package_of_this_build_is_returned() {
    if !can_build() {
        return;
    }
    let dir = scratch();
    std::fs::write(
        dir.join("PKGBUILD"),
        pkgbuild(std::path::Path::new("/nonexistent/canary")),
    )
    .unwrap();
    // left over from an earlier version, a detached signature, a partial
    // download, and a link named like a package
    for stale in [
        "sandbox-probe-0.9-1-any.pkg.tar.zst",
        "sandbox-probe-1-1-any.pkg.tar.zst.sig",
        "sandbox-probe-1-1-any.pkg.tar.zst.part",
        "other-1-1-any.pkg.tar.zst",
    ] {
        std::fs::write(dir.join(stale), "stale").unwrap();
    }
    symlink("/etc/hostname", dir.join("linked-1-1-any.pkg.tar.zst")).unwrap();

    let built = build_with_repair(&dir, None, &policy(false)).unwrap();
    let names: Vec<String> = built
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with("sandbox-probe-1-1-any.pkg.tar")
            && yapacc::repair::is_package_file_name(&names[0]),
        "{names:?}"
    );
    assert_eq!(built[0].parent().unwrap(), dir);
    assert!(
        std::fs::read(&built[0]).unwrap() != b"stale",
        "the artifact is the freshly built file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
