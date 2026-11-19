// yapacc — one-shot update check
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! `yapacc monitor check`: one report of pending updates (repositories, AUR,
//! flatpak), for a timer or a script. It changes nothing and applies
//! nothing. The timer that `yapacc monitor daemon-install` sets up runs
//! exactly this command.

use crate::{backend, safety};
use serde::Serialize;
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
pub struct MonitorReport {
    pub repo_updates: String,
    pub aur_updates: String,
    pub flatpak_updates: String,
    pub safety_note: String,
}

/// The columns of `flatpak remote-ls --updates`. `installation` is a
/// column of `flatpak list`, not of `remote-ls` (flatpak 1.18.4: "error:
/// Unknown column: installation"); with it the command failed and the
/// report said "(none or flatpak unavailable)" whatever was pending.
const FLATPAK_UPDATE_COLUMNS: &str = "--columns=application,branch,version,origin";

fn cmd_output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn run_check() -> Result<MonitorReport, String> {
    // structured update data (was: re-parsing the printed check_updates() text)
    let data = backend::check_updates_data();
    let repo_updates = if !data.repo_check_ok {
        "(repo check unavailable)".to_string()
    } else if data.repo_updates.is_empty() {
        "(none)".to_string()
    } else {
        data.repo_updates.join("\n")
    };
    let aur_updates = if !data.aur_check_ok {
        "(AUR check unavailable)".to_string()
    } else if data.aur_updates.is_empty() {
        "(none)".to_string()
    } else {
        data.aur_updates
            .iter()
            .map(|(n, cur, avail)| format!("{n} {cur} -> {avail}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let flatpak_updates = {
        // not asked about an installation that does not exist: flatpak
        // would create the user's (`~/.local/share/flatpak`)
        let out = match crate::why::flatpak_read_scope() {
            Some(scope) => {
                let mut args = vec!["remote-ls", "--updates", FLATPAK_UPDATE_COLUMNS];
                args.extend_from_slice(scope);
                cmd_output("flatpak", &args)
            }
            None => String::new(),
        };
        if out.is_empty() {
            "(none or flatpak unavailable)".to_string()
        } else {
            out
        }
    };
    let st = safety::detect_safety_capabilities();
    Ok(MonitorReport {
        repo_updates,
        aur_updates,
        flatpak_updates,
        safety_note: format!(
            "Nothing was changed: `monitor check` only reports. Snapshot tool detected: {} (available: {}). To upgrade, run `yapacc upgrade`: it shows the plan and asks.",
            st.snapshot_backend, st.snapshots_available
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `flatpak remote-ls --columns=help` (flatpak 1.18.4) lists: name,
    /// description, application, version, branch, arch, origin, ref, commit,
    /// runtime, installed-size, download-size, options.
    #[test]
    fn the_flatpak_update_columns_are_columns_of_remote_ls() {
        const KNOWN: [&str; 13] = [
            "name",
            "description",
            "application",
            "version",
            "branch",
            "arch",
            "origin",
            "ref",
            "commit",
            "runtime",
            "installed-size",
            "download-size",
            "options",
        ];
        let columns = FLATPAK_UPDATE_COLUMNS
            .strip_prefix("--columns=")
            .expect("a --columns option");
        for column in columns.split(',') {
            assert!(KNOWN.contains(&column), "{column}");
        }
    }
}
