//! `yapacc timeline` on the machine that runs the tests: a JSON array when
//! a package log exists here, exit code 6 when none does.

mod common;

use std::process::Command;

/// The core, with a private home (see `common`).
fn yapacc() -> Command {
    common::command(env!("CARGO_BIN_EXE_yapacc"))
}

#[test]
fn timeline_json_is_an_array_or_exit_6() {
    let o = yapacc()
        .args(["--json", "timeline", "--limit", "5"])
        .output()
        .expect("failed to run yapacc");
    if o.status.success() {
        let text = String::from_utf8_lossy(&o.stdout);
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        let arr = v.as_array().expect("array");
        assert!(arr.len() <= 5);
        for e in arr {
            assert!(e["ts"].is_u64() && e["action"].is_string(), "entry: {e}");
        }
    } else {
        assert_eq!(o.status.code(), Some(6));
        assert!(String::from_utf8_lossy(&o.stderr).contains("/var/log/pacman.log"));
    }
}

#[test]
fn timeline_html_is_one_document_or_exit_6() {
    let o = yapacc()
        .args(["timeline", "--html", "--limit", "5"])
        .output()
        .expect("failed to run yapacc");
    if o.status.success() {
        let text = String::from_utf8_lossy(&o.stdout);
        assert!(text.starts_with("<!DOCTYPE html>"), "{text}");
        assert!(text.trim_end().ends_with("</html>"), "{text}");
        assert!(!text.contains("<script"), "{text}");
    } else {
        assert_eq!(o.status.code(), Some(6));
    }
}

#[test]
fn timeline_html_and_json_together_is_a_usage_error() {
    let o = yapacc()
        .args(["--json", "timeline", "--html"])
        .output()
        .expect("failed to run yapacc");
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn timeline_since_boot_names_its_instant_or_exit_6() {
    let o = yapacc()
        .args(["timeline", "--since-boot", "--limit", "5"])
        .output()
        .expect("failed to run yapacc");
    if o.status.success() {
        let text = String::from_utf8_lossy(&o.stdout);
        let first = text.lines().next().unwrap_or("");
        assert!(
            first.starts_with("Changes since the start of the "),
            "{text}"
        );
        assert!(first.contains(" boot: "), "{text}");
    } else {
        // no package log, or no boot time, could be read on this machine
        assert_eq!(o.status.code(), Some(6));
    }
}
