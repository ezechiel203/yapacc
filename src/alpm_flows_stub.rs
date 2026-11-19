// yapacc — libalpm transactions, stub for builds without the `alpm` feature
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Same surface as `alpm_flows.rs`; see `alpm_stub.rs`. Sync-database lookups
//! fall back to the pacman CLI; transactions report that native support is
//! not compiled in.

const MSG: &str =
    "this build of yapacc has no native libalpm support (built without the `alpm` feature)";

/// This build does not link libalpm.
pub const COMPILED_IN: bool = false;

/// Same surface as the libalpm build; never run here (`commit_remove` fails
/// first). Pure.
pub fn remove_commit_argv(targets: &[String]) -> Vec<String> {
    ["pacman", "-R", "--noconfirm", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(targets.iter().cloned())
        .collect()
}

/// Without libalpm there is no in-process transaction for any user: the
/// answer is the same for root, and names the command that works. Pure.
pub fn needs_root(_is_root: bool, verb: &str, instead: &str) -> Option<String> {
    Some(format!(
        "`{verb}` needs native libalpm support, and {MSG}. Nothing was changed. `yapacc {instead}` does the same change through the pacman command."
    ))
}

#[derive(Clone, Debug)]
pub struct AlpmSearchHit {
    pub repo: String,
    pub name: String,
    pub version: String,
    pub desc: String,
    pub installed: bool,
}

#[derive(Clone, Debug)]
pub struct AlpmTxPlan {
    pub to_install: Vec<String>,
    pub to_remove: Vec<String>,
    pub notes: Vec<String>,
}

pub fn search(_query: &str) -> Result<Vec<AlpmSearchHit>, String> {
    Err(MSG.into())
}

/// Parse `pacman -Si NAME` (C locale) into (repo, name, version, description).
pub fn parse_pacman_si(text: &str) -> Option<(String, String, String, String)> {
    let field = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(" : ")?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    Some((
        field("Repository")?,
        field("Name")?,
        field("Version")?,
        field("Description").unwrap_or_default(),
    ))
}

/// Without libalpm the sync databases are queried through the pacman CLI,
/// so the resolver's repository lane keeps working in the portable build.
pub fn query_sync(name: &str) -> Result<Option<(String, String, String, String)>, String> {
    if !crate::util::valid_pkg_name(name) {
        return Err(format!("'{name}' is not a valid package name"));
    }
    let out = std::process::Command::new("pacman")
        .args(["-Si", "--"])
        .arg(name)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("failed to run pacman: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(parse_pacman_si(&String::from_utf8_lossy(&out.stdout)).filter(|(_, n, _, _)| n == name))
}

pub fn plan_install(_targets: &[String]) -> Result<AlpmTxPlan, String> {
    Err(MSG.into())
}

pub fn plan_install_with_providers(
    _targets: &[String],
    _providers: &[(String, String)],
) -> Result<AlpmTxPlan, String> {
    Err(MSG.into())
}

pub fn commit_install_with_providers(
    _targets: &[String],
    _providers: &[(String, String)],
) -> Result<AlpmTxPlan, String> {
    Err(MSG.into())
}

pub fn plan_remove(_targets: &[String]) -> Result<AlpmTxPlan, String> {
    Err(MSG.into())
}

pub fn commit_remove(_targets: &[String]) -> Result<AlpmTxPlan, String> {
    Err(MSG.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacman_si_fields() {
        let text = "Repository      : extra\nName            : ripgrep\nVersion         : 14.1.1-1\nDescription     : A search tool\n";
        assert_eq!(
            parse_pacman_si(text),
            Some((
                "extra".into(),
                "ripgrep".into(),
                "14.1.1-1".into(),
                "A search tool".into()
            ))
        );
        assert!(parse_pacman_si("error: package 'x' was not found\n").is_none());
    }

    #[test]
    fn without_libalpm_the_alpm_verbs_name_the_command_that_works() {
        // the same answer for root: becoming root would not help
        for root in [true, false] {
            let why = needs_root(root, "remove alpm", "remove repo").unwrap();
            assert!(why.contains("no native libalpm support"), "{why}");
            assert!(why.contains("`yapacc remove repo`"), "{why}");
            assert!(!why.contains("run as root"), "{why}");
        }
        assert_eq!(
            remove_commit_argv(&["htop".to_string()]),
            ["pacman", "-R", "--noconfirm", "--", "htop"]
        );
    }
}
