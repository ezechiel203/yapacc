// yapacc — small shared helpers
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Helpers that used to be re-implemented per module, several of them by
//! spawning `sh -c`. Nothing here starts a shell.

use std::path::{Path, PathBuf};

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Locate an executable on `PATH` without spawning a shell.
pub fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(cmd))
        .find(|p| is_executable(p))
}

pub fn have(cmd: &str) -> bool {
    which(cmd).is_some()
}

/// Effective-uid check. Linux needs neither libc nor a subprocess; where
/// `/proc` is not mounted (FreeBSD by default) it asks `id -u`, because
/// answering "not root" for root would put `doas` in front of every command.
pub fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                // fields: real, effective, saved, fs
                .map(|l| l.split_whitespace().nth(2) == Some("0"))
        })
        .unwrap_or_else(|| {
            std::process::Command::new("id")
                .arg("-u")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .is_some_and(|o| uid_output_is_root(&String::from_utf8_lossy(&o.stdout)))
        })
}

/// Whether the output of `id -u` names uid 0. Pure.
fn uid_output_is_root(out: &str) -> bool {
    out.trim() == "0"
}

/// Real uid of this process (used to mask `/run/user/<uid>` in sandboxes).
pub fn real_uid() -> Option<u32> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|l| l.starts_with("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// `YYYY-MM-DD HH:MM UTC` for a unix timestamp, without a date library
/// (civil-from-days, proleptic Gregorian).
pub fn fmt_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Strict package-name check, applied before a name taken from untrusted
/// text (build output, plugin answers, MCP arguments) is handed to a
/// package manager. Mirrors the Arch `pkgname` rule: ASCII alphanumerics
/// plus `@ . _ + -`, not starting with `-` or `.`. A name that fails this
/// can never be mistaken for a command-line option or a path.
pub fn valid_pkg_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'+' | b'-'))
}

/// Path of the `yapacc` core binary, for components that drive it.
/// Order: `YAPACC_BIN`, this executable if it is the core, a sibling named
/// `yapacc`, then `PATH`.
pub fn core_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("YAPACC_BIN").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let me = std::env::current_exe().ok();
    if let Some(me) = &me {
        if me.file_name().and_then(|n| n.to_str()) == Some("yapacc") {
            return me.clone();
        }
        if let Some(sibling) = me.parent().map(|d| d.join("yapacc"))
            && is_executable(&sibling)
        {
            return sibling;
        }
    }
    which("yapacc")
        .or(me)
        .unwrap_or_else(|| PathBuf::from("yapacc"))
}

/// Locate a first-party component (`yapacc-tui`, `yapacc-mcp`, ...).
///
/// Components are looked up only next to the running binary and in the
/// yapacc libexec directories — never on the whole `PATH` — so that a stray
/// `yapacc-something` earlier on `PATH` cannot be run under yapacc's name.
/// (Community adapters are a separate mechanism with its own listing and
/// opt-out; see `plugin.rs`.)
pub fn component_exe(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("YAPACC_LIBEXEC").filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
    {
        dirs.push(bin.to_path_buf());
        dirs.push(bin.join("../lib/yapacc"));
        dirs.push(bin.join("../libexec/yapacc"));
    }
    dirs.push(PathBuf::from("/usr/lib/yapacc"));
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_output_root_detection() {
        assert!(uid_output_is_root("0\n"));
        assert!(!uid_output_is_root("1001\n"));
        assert!(!uid_output_is_root(""));
        assert!(!uid_output_is_root("00x"));
    }

    #[test]
    fn which_finds_sh_and_rejects_nonsense() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-command-xyz").is_none());
        assert!(which("").is_none());
    }

    #[test]
    fn utc_formatting_matches_known_dates() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(fmt_utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(fmt_utc(1_714_000_000), "2024-04-24 23:06 UTC");
    }

    #[test]
    fn pkg_names_are_strict() {
        for ok in [
            "ripgrep",
            "gtk3",
            "lib32-glibc",
            "c++utilities",
            "r8168-dkms",
            "foo@1.2",
        ] {
            assert!(valid_pkg_name(ok), "{ok} should be accepted");
        }
        for bad in [
            "",
            "-Syu",
            "--overwrite=*",
            ".hidden",
            "a b",
            "a/b",
            "a;rm",
            "$(id)",
            "x\ny",
            "naïve",
        ] {
            assert!(!valid_pkg_name(bad), "{bad:?} should be rejected");
        }
    }
}
