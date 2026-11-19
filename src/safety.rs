//! Safety / update / rollback subsystem.
//! Detects snapshot capabilities and provides a host-oriented safe-update plan.
//! On btrfs hosts without snapper/timeshift, yapacc ships a helper
//! hook for read-only snapshots and explicit rollback guidance.

use std::path::PathBuf;

#[derive(Clone, Debug, serde::Serialize)]
pub struct SafetyStatus {
    pub snapshots_available: bool,
    pub snapshot_backend: String,
    pub boot_rollback_hint: String,
    pub notes: String,
}

fn has(cmd: &str) -> bool {
    crate::util::have(cmd)
}

fn root_is_btrfs() -> bool {
    crate::platform::root_fstype() == "btrfs"
}

pub fn detect_safety_capabilities() -> SafetyStatus {
    let snapper = has("snapper");
    let timeshift = has("timeshift");
    let btrfs = has("btrfs");
    let grub_btrfs = std::path::Path::new("/etc/grub.d/41_snapshots-btrfs").exists();
    let root_btrfs = root_is_btrfs();
    let backend = if snapper {
        "snapper".to_string()
    } else if timeshift {
        "timeshift".to_string()
    } else if btrfs && root_btrfs {
        "btrfs (yapacc snapshot-hook helper)".to_string()
    } else if btrfs {
        "btrfs (manual/tooling)".to_string()
    } else {
        "none-detected".to_string()
    };
    let snapshots_available = snapper || timeshift || btrfs;
    let hint = if grub_btrfs || snapper {
        "Snapshot boot entries may be available via GRUB/snapper integration; verify before relying on rollback.".to_string()
    } else if root_btrfs {
        "Root is on btrfs: read-only snapshots can be created, but live-root rollback requires booting into rescue/another subvolume and manual subvolume swap/default change. See `yapacc plan safety`.".to_string()
    } else {
        "No snapshot-boot integration detected; ensure a known-good kernel/boot path and backups before risky updates.".to_string()
    };
    let notes = if snapper || timeshift {
        "`yapacc snapshot create` and `[safety] snapshot` take snapshots through that tool. yapacc never restores a snapshot: use the tool itself for that.".to_string()
    } else if btrfs && root_btrfs {
        "`yapacc safety apply --snapshot-tag TAG` takes a read-only btrfs snapshot with the built-in snapshot hook (default target: /, stored under /.snapshots/yapacc); `yapacc snapshot list` shows them. `yapacc snapshot create` and `[safety] snapshot` do not use plain btrfs: they need snapper, timeshift or zfs. yapacc never restores a snapshot: on a live root that is done by hand, from a rescue system.".to_string()
    } else {
        "No snapshot tool was found: yapacc takes no snapshot before a change. `yapacc undo` restores package versions, not configuration or data.".to_string()
    };
    SafetyStatus {
        snapshots_available,
        snapshot_backend: backend,
        boot_rollback_hint: hint,
        notes,
    }
}

/// The two scripts `safety apply` runs. They are compiled into the binary:
/// what runs is what was built, from any directory, on any installed system.
/// A file is never looked up relative to the current directory (a cloned
/// repository or a shared directory could plant one there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Helper {
    /// `snapshot-hook.sh`: creates a read-only btrfs snapshot (`create TAG`).
    SnapshotHook,
    /// `pretest-transaction.sh`: read-only checks before a transaction.
    Pretest,
}

impl Helper {
    pub fn file_name(self) -> &'static str {
        match self {
            Helper::SnapshotHook => "snapshot-hook.sh",
            Helper::Pretest => "pretest-transaction.sh",
        }
    }

    /// The script as built into this binary.
    pub fn text(self) -> &'static str {
        match self {
            Helper::SnapshotHook => include_str!("../contrib/safety/snapshot-hook.sh"),
            Helper::Pretest => include_str!("../contrib/safety/pretest-transaction.sh"),
        }
    }
}

/// The files under `contrib/` that are compiled into the binary, by
/// `<category>/<name>`: the two helper scripts and the systemd unit templates.
pub fn embedded(category: &str, name: &str) -> Option<&'static str> {
    match (category, name) {
        ("safety", "snapshot-hook.sh") => Some(Helper::SnapshotHook.text()),
        ("safety", "pretest-transaction.sh") => Some(Helper::Pretest.text()),
        ("systemd", "yapacc-monitor.service") => {
            Some(include_str!("../contrib/systemd/yapacc-monitor.service"))
        }
        ("systemd", "yapacc-monitor.timer") => {
            Some(include_str!("../contrib/systemd/yapacc-monitor.timer"))
        }
        _ => None,
    }
}

/// Where a helper script comes from for this run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperSource {
    /// the script compiled into the binary (the default)
    Embedded,
    /// a replacement the user named with `YAPACC_CONTRIB_DIR`
    Override(PathBuf),
}

/// Whether a replacement script may be run: owned by root or by the user
/// running yapacc, and writable by nobody else. Pure.
pub fn override_trusted(owner: u32, mode: u32, euid: u32) -> bool {
    (owner == 0 || owner == euid) && mode & 0o022 == 0
}

/// Effective uid without libc: `/proc/self/status`, else `id -u`.
fn effective_uid() -> Option<u32> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                // fields: real, effective, saved, fs
                .and_then(|l| l.split_whitespace().nth(2)?.parse().ok())
        })
        .or_else(|| {
            let out = std::process::Command::new("id").arg("-u").output().ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        })
}

/// `helper_source` with the override directory given (`None`: not set). A
/// replacement that exists but is not trusted is an error, never a silent
/// fall back to something else.
pub fn helper_source_in(h: Helper, contrib_dir: Option<&str>) -> Result<HelperSource, String> {
    use std::os::unix::fs::MetadataExt;
    let Some(dir) = contrib_dir.filter(|d| !d.is_empty()) else {
        return Ok(HelperSource::Embedded);
    };
    let path = PathBuf::from(dir).join("safety").join(h.file_name());
    let Ok(meta) = std::fs::metadata(&path) else {
        // the directory may hold other files only (unit templates)
        return Ok(HelperSource::Embedded);
    };
    if !meta.is_file() {
        return Err(format!(
            "YAPACC_CONTRIB_DIR: {} is not a regular file; unset YAPACC_CONTRIB_DIR to use the built-in script",
            path.display()
        ));
    }
    let euid = effective_uid().ok_or_else(|| {
        "cannot tell which user is running yapacc (no /proc, no `id`)".to_string()
    })?;
    if !override_trusted(meta.uid(), meta.mode(), euid) {
        return Err(format!(
            "YAPACC_CONTRIB_DIR: {} is owned by uid {} with mode {:o}; a replacement script must be owned by root or by you and not be writable by group or others. Nothing was run. Fix the file, or unset YAPACC_CONTRIB_DIR to use the built-in script.",
            path.display(),
            meta.uid(),
            meta.mode() & 0o7777
        ));
    }
    Ok(HelperSource::Override(path))
}

/// The built-in script, unless `YAPACC_CONTRIB_DIR` names a trusted replacement.
pub fn helper_source(h: Helper) -> Result<HelperSource, String> {
    helper_source_in(h, std::env::var("YAPACC_CONTRIB_DIR").ok().as_deref())
}

/// The argument vector that runs a helper. The built-in script is handed to
/// bash as the `-c` text (a compile-time constant; the arguments stay
/// separate `argv` elements, nothing is interpolated). Not `bash -s`: a
/// script read from stdin is eaten by any child that reads stdin. Pure.
pub fn helper_argv(h: Helper, source: &HelperSource, args: &[String]) -> Vec<String> {
    let mut v: Vec<String> = vec!["bash".into()];
    match source {
        HelperSource::Embedded => {
            v.push("-c".into());
            v.push(h.text().to_string());
            // $0 of the script
            v.push(h.file_name().to_string());
        }
        HelperSource::Override(path) => {
            v.push("--".into());
            v.push(path.to_string_lossy().to_string());
        }
    }
    v.extend(args.iter().cloned());
    v
}

/// How a helper invocation is shown: the script by name, never its text.
pub fn helper_shown(h: Helper, source: &HelperSource, args: &[String]) -> String {
    let what = match source {
        HelperSource::Embedded => format!("{} (built into yapacc)", h.file_name()),
        HelperSource::Override(p) => format!("{} (from YAPACC_CONTRIB_DIR)", p.display()),
    };
    if args.is_empty() {
        what
    } else {
        format!("{what} {}", crate::ops::shown(&[], args))
    }
}

/// Run a helper and capture what it printed. stdin is closed: the helpers
/// never ask anything.
pub fn run_helper(
    h: Helper,
    source: &HelperSource,
    args: &[String],
) -> Result<std::process::Output, String> {
    let argv = helper_argv(h, source, args);
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("failed to run {} (bash): {e}", h.file_name()))
}

/// Resolve a data file shipped under contrib/<category>/ (the systemd unit
/// templates) on disk:
/// 1. YAPACC_CONTRIB_DIR env override
/// 2. executable-relative (installed layout: <bin>/../contrib and <bin>/../share/yapacc/contrib)
/// 3. debug builds only: the source tree the binary was built from
///
/// Never the current directory, and no builder path in a release binary. The
/// helper scripts do not go through this: see `Helper`.
pub fn contrib_script(category: &str, name: &str) -> Result<PathBuf, String> {
    let rel = format!("{category}/{name}");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("YAPACC_CONTRIB_DIR")
        && !dir.is_empty()
    {
        candidates.push(PathBuf::from(dir).join(&rel));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin_dir) = exe.parent()
    {
        candidates.push(bin_dir.join("../contrib").join(&rel));
        candidates.push(bin_dir.join("../share/yapacc/contrib").join(&rel));
        candidates.push(bin_dir.join("contrib").join(&rel));
    }
    #[cfg(debug_assertions)]
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join(&rel),
    );
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    Err(format!(
        "contrib file '{rel}' not found (searched: {}). Install contrib/ next to the binary, or set YAPACC_CONTRIB_DIR to the directory that holds it.",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub fn plan_update_with_safety(targets: &[String]) -> String {
    let st = detect_safety_capabilities();
    let mut s = String::new();
    s.push_str("== SAFE UPDATE PLAN ==\n");
    s.push_str(&format!(
        "Targets: {}\n",
        if targets.is_empty() {
            "<system/selected>".into()
        } else {
            targets.join(", ")
        }
    ));
    s.push_str(&format!(
        "Snapshot backend: {} (available={})\n",
        st.snapshot_backend, st.snapshots_available
    ));
    s.push_str(&format!("Boot/rollback hint: {}\n", st.boot_rollback_hint));
    s.push_str(&format!("Notes: {}\n", st.notes));
    s.push_str("\nStages of `yapacc safety apply`:\n");
    s.push_str("1) Plan: the preview of the change, as the package manager or flatpak gives it\n");
    s.push_str("2) Pre-check: read-only checks of the transaction (skipped with --skip-pretest, or with [safety] pretest_enabled = false)\n");
    s.push_str("3) Snapshot: only with --snapshot-tag TAG, a read-only btrfs snapshot, taken after you consent and before the change\n");
    if st.snapshot_backend.contains("snapshot-hook helper") {
        s.push_str("   - through the btrfs helper built into yapacc; default target: / (override with SNAP_TARGETS, base with SNAP_BASE)\n");
    }
    s.push_str(
        "4) Apply: the change, behind the gate (--dryrun stops before it; --yes consents)\n",
    );
    s.push_str("5) Health check: only with --health-check COMMAND; a non-zero exit counts as a failed change\n");
    s.push_str("6) On failure: with --auto-rollback, the targets this run installed are removed again; nothing else is rolled back\n");
    s.push_str("\nyapacc never restores a snapshot. On a live btrfs root that is done by hand, from a rescue system.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A fresh private directory under the system temp directory.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yapacc-safety-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("safety")).unwrap();
        dir
    }

    #[test]
    fn helper_scripts_are_compiled_in() {
        for h in [Helper::SnapshotHook, Helper::Pretest] {
            assert!(h.text().starts_with("#!/usr/bin/env bash"), "{h:?}");
            assert_eq!(embedded("safety", h.file_name()), Some(h.text()));
        }
        assert!(Helper::Pretest.text().contains("RESULT: PASS"));
        assert!(
            Helper::SnapshotHook
                .text()
                .contains("btrfs subvolume snapshot -r")
        );
        assert!(embedded("systemd", "yapacc-monitor.service").is_some());
        assert!(embedded("systemd", "yapacc-monitor.timer").is_some());
        assert!(embedded("safety", "no-such-script.sh").is_none());
    }

    #[test]
    fn the_built_in_script_is_the_default_and_no_path_is_involved() {
        assert_eq!(
            helper_source_in(Helper::Pretest, None),
            Ok(HelperSource::Embedded)
        );
        assert_eq!(
            helper_source_in(Helper::Pretest, Some("")),
            Ok(HelperSource::Embedded)
        );
        let args = strs(&["arch-install", "a b", "$(id)"]);
        let argv = helper_argv(Helper::Pretest, &HelperSource::Embedded, &args);
        // bash -c <constant text> <$0> <arguments, one element each>
        assert_eq!(argv[..2], ["bash", "-c"]);
        assert_eq!(argv[2], Helper::Pretest.text());
        assert_eq!(argv[3], "pretest-transaction.sh");
        assert_eq!(argv[4..], args[..]);
        // the script text never contains an argument
        assert!(!argv[2].contains("$(id)"));
        // nothing in the command names a file: no directory can supply one
        assert!(
            !argv
                .iter()
                .any(|a| a.starts_with("contrib") || a.starts_with("./"))
        );
    }

    #[test]
    fn a_script_planted_in_the_current_directory_is_never_run() {
        if !crate::util::have("bash") {
            return; // the helpers need bash; nothing to run here without it
        }
        // the layout the old lookup accepted: ./contrib/safety/<name> under the cwd
        let dir = scratch("planted");
        let marker = dir.join("PLANTED-RAN");
        let planted = dir.join("contrib/safety");
        std::fs::create_dir_all(&planted).unwrap();
        for h in [Helper::Pretest, Helper::SnapshotHook] {
            std::fs::write(
                planted.join(h.file_name()),
                format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
            )
            .unwrap();
        }
        // run the pretest the way `safety apply` does, from inside that directory
        let argv = helper_argv(
            Helper::Pretest,
            &helper_source_in(Helper::Pretest, None).unwrap(),
            &strs(&["no-such-op"]),
        );
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            said.contains("[pretest]"),
            "the built-in script ran: {said}"
        );
        assert!(!marker.exists(), "the planted script was executed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_replacement_script_must_be_owned_and_not_writable_by_others() {
        assert!(override_trusted(0, 0o100755, 1000));
        assert!(override_trusted(1000, 0o100700, 1000));
        // another user's file
        assert!(!override_trusted(1001, 0o100755, 1000));
        // group- or world-writable
        assert!(!override_trusted(1000, 0o100775, 1000));
        assert!(!override_trusted(0, 0o100757, 0));
    }

    #[test]
    fn the_override_directory_is_honoured_only_for_a_trusted_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("override");
        let d = dir.to_str().unwrap();
        // no such file there: the built-in script
        assert_eq!(
            helper_source_in(Helper::Pretest, Some(d)),
            Ok(HelperSource::Embedded)
        );
        let path = dir.join("safety/pretest-transaction.sh");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            helper_source_in(Helper::Pretest, Some(d)),
            Ok(HelperSource::Override(path.clone()))
        );
        let argv = helper_argv(
            Helper::Pretest,
            &HelperSource::Override(path.clone()),
            &strs(&["arch-upgrade"]),
        );
        assert_eq!(argv[..2], ["bash", "--"]);
        assert_eq!(argv[3], "arch-upgrade");
        // writable by others: refused, and not replaced by anything else
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
        let e = helper_source_in(Helper::Pretest, Some(d)).unwrap_err();
        assert!(e.contains("Nothing was run") && e.contains("777"), "{e}");
        // a directory in its place
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(helper_source_in(Helper::Pretest, Some(d)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_helper_is_shown_by_name() {
        let s = helper_shown(
            Helper::SnapshotHook,
            &HelperSource::Embedded,
            &strs(&["create", "pre upgrade"]),
        );
        assert_eq!(
            s,
            "snapshot-hook.sh (built into yapacc) create 'pre upgrade'"
        );
    }

    #[test]
    fn contrib_lookup_never_uses_the_current_directory() {
        // a data file that exists nowhere: the error lists what was searched
        let e = contrib_script("safety", "no-such-script.sh").unwrap_err();
        assert!(e.contains("not found"), "got: {e}");
        let searched = e.split("searched: ").nth(1).unwrap_or_default();
        let searched = searched.split("). ").next().unwrap_or_default();
        for c in searched.split(", ") {
            assert!(
                c.starts_with('/'),
                "a candidate relative to the current directory: {c}"
            );
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn unit_templates_resolve_from_the_source_tree_in_debug_builds() {
        let p = contrib_script("systemd", "yapacc-monitor.service").unwrap();
        assert!(p.is_file(), "got {}", p.display());
    }
}
