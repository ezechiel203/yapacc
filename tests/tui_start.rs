//! `yapacc-tui` when it cannot start: no terminal, a bad argument, a bad
//! configuration. (What it does on a terminal needs a pty, which these
//! tests do not have.) The home and XDG directories are temporary.
#![cfg(feature = "tui")]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "yapacc-tui-start-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tui(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yapacc-tui"))
        .args(args)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        // a core that must not be reached: nothing is run before the
        // terminal is known to be there
        .env("YAPACC_BIN", "/nonexistent/yapacc")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// Review finding 11: `yapacc-tui </dev/null` panicked inside the terminal
/// library and exited 101.
#[test]
fn without_a_terminal_it_says_so_and_exits_6() {
    let dir = home("noterm");
    let o = tui(&dir, &[]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(6), "{stderr}");
    assert!(
        stderr.starts_with("error: the interactive interface needs a terminal"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "one plain sentence: {stderr}");
    assert!(o.stdout.is_empty(), "nothing is drawn into a pipe");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review finding 14: a bare `--config` was silently ignored.
#[test]
fn a_bad_argument_or_configuration_is_exit_2() {
    let dir = home("badargs");
    let o = tui(&dir, &["--config"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--config needs a file"));

    let o = tui(&dir, &["--config", "/nonexistent/yapacc.toml"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot read config"));

    let config = dir.join("c.toml");
    std::fs::write(&config, "[mcp]\napply = \"perhaps\"\n").unwrap();
    let o = tui(&dir, &["--config", config.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("[mcp] apply"));

    let o = tui(&dir, &["--nope"]);
    assert_eq!(o.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}
