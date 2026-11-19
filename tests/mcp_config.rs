//! `yapacc-mcp --config` is validated at startup (real process). More in
//! `tests/mcp_stdio.rs`.
use std::process::{Command, Stdio};

/// The exit code of a server started with `--config CONFIG`. Its home and
/// XDG directories point at a directory that does not exist, so nothing of
/// the developer's is read.
fn start(config: &str) -> Option<i32> {
    let nowhere = std::env::temp_dir().join(format!("yapacc-mcp-config-{}", std::process::id()));
    Command::new(env!("CARGO_BIN_EXE_yapacc-mcp"))
        .args(["--config", config])
        .env("HOME", &nowhere)
        .env("XDG_CONFIG_HOME", nowhere.join("config"))
        .env("XDG_DATA_HOME", nowhere.join("data"))
        .env("XDG_STATE_HOME", nowhere.join("state"))
        .env("XDG_CACHE_HOME", nowhere.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .code()
}

#[test]
fn missing_or_non_file_config_is_refused_at_startup() {
    // a usage error: exit 2
    assert_eq!(start("/nonexistent/yapacc.toml"), Some(2));
    assert_eq!(start("/tmp"), Some(2));
}
