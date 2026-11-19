// yapacc — shared by the integration tests.
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Every yapacc process a test starts gets a private, empty home: `HOME` and
//! the four XDG base directories point below one directory made for this
//! test run. No test reads the developer's configuration or ledger, and none
//! leaves a file below the real home directory.

// Each test file is its own crate and uses a different part of this module.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The home directory of this test run. One per test binary, below cargo's
/// scratch directory for integration tests (`target/tmp`), named after the
/// process so that two runs at the same time do not share it.
pub fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        sweep(base);
        let dir = base.join(format!("test-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["config", "data", "state", "cache"] {
            std::fs::create_dir_all(dir.join(sub)).expect("create the test home");
        }
        dir
    })
}

/// `$XDG_DATA_HOME` of the test run.
pub fn data_home() -> PathBuf {
    home().join("data")
}

/// Remove the homes of earlier runs whose process is gone. There is no hook
/// at the end of a test binary, so the next run cleans up.
fn sweep(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_string_lossy()
            .strip_prefix("test-home-")
            .map(String::from)
        else {
            continue;
        };
        if !Path::new("/proc").join(&pid).exists() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Point `cmd` at the private home, and drop the variables through which the
/// developer's environment could reach the process under test.
pub fn isolate(cmd: &mut Command) -> &mut Command {
    let home = home();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("YAPACC_CONTRIB_DIR")
        .env_remove("YAPACC_FOREIGN_ROOT")
        .env_remove("PACKAGE_CLEANER_FOREIGN_ROOT")
        .env_remove("YAPACC_VIA")
}

/// A command for `program`, isolated.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    isolate(&mut cmd);
    cmd
}
