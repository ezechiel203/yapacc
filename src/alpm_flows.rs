use alpm::{Alpm, SigLevel, TransFlag};
use anyhow::{Context, Result, anyhow};
use std::process::Command;

/// This build links libalpm (the `alpm` feature).
pub const COMPILED_IN: bool = true;

/// The command `commit_remove` hands the prepared removal to. Shown before
/// the gate by `remove alpm`. Package names follow `--`. Pure.
pub fn remove_commit_argv(targets: &[String]) -> Vec<String> {
    ["pacman", "-R", "--noconfirm", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(targets.iter().cloned())
        .collect()
}

/// Why an in-process transaction cannot run for this user, if it cannot:
/// libalpm takes the pacman database lock for the prepare step already, and
/// only root can. `verb` is the yapacc command, `instead` the one that
/// elevates only the pacman command. Pure.
pub fn needs_root(is_root: bool, verb: &str, instead: &str) -> Option<String> {
    if is_root {
        return None;
    }
    Some(format!(
        "`{verb}` prepares the transaction in this process through libalpm, which needs the pacman database lock: run as root (for example `sudo yapacc {verb} ...`). Nothing was changed. `yapacc {instead}` does the same change from a normal account and elevates only the pacman command."
    ))
}

fn repo_list() -> Vec<String> {
    Command::new("pacman-conf")
        .arg("--repo-list")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec!["core".into(), "extra".into(), "multilib".into()])
}

fn open_read() -> Result<Alpm> {
    let handle = Alpm::new("/", "/var/lib/pacman/").context("failed to init libalpm")?;
    // register syncdbs (read-only usage of existing sync db files)
    for repo in repo_list() {
        let _ = handle.register_syncdb(repo.as_bytes(), SigLevel::USE_DEFAULT);
    }
    Ok(handle)
}

#[derive(Clone, Debug)]
pub struct AlpmSearchHit {
    pub repo: String,
    pub name: String,
    pub version: String,
    pub desc: String,
    pub installed: bool,
}

fn search_match_rank(query: &str, name: &str, description: &str) -> u8 {
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    let d = description.to_lowercase();
    if n == q {
        0
    } else if n.contains(&q) {
        1
    } else if d.contains(&q) {
        2
    } else {
        3
    }
}

pub fn search(query: &str) -> Result<Vec<AlpmSearchHit>> {
    let h = open_read()?;
    let local = h.localdb();
    let q = query.to_lowercase();
    let mut hits = Vec::new();
    for db in h.syncdbs() {
        for pkg in db.pkgs() {
            let name = pkg.name();
            let desc = pkg.desc().unwrap_or("");
            if name.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q) {
                hits.push(AlpmSearchHit {
                    repo: db.name().to_string(),
                    name: name.to_string(),
                    version: pkg.version().as_str().to_string(),
                    desc: desc.to_string(),
                    installed: local.pkg(name).is_ok(),
                });
            }
        }
    }
    hits.sort_by(|a, b| {
        search_match_rank(query, &a.name, &a.desc)
            .cmp(&search_match_rank(query, &b.name, &b.desc))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.repo.cmp(&b.repo))
    });
    Ok(hits)
}

pub fn query_sync(name: &str) -> Result<Option<(String, String, String, String)>> {
    let h = open_read()?;
    for db in h.syncdbs() {
        if let Ok(pkg) = db.pkg(name) {
            return Ok(Some((
                db.name().to_string(),
                pkg.name().to_string(),
                pkg.version().as_str().to_string(),
                pkg.desc().unwrap_or("").to_string(),
            )));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug)]
pub struct AlpmTxPlan {
    pub to_install: Vec<String>,
    pub to_remove: Vec<String>,
    pub notes: Vec<String>,
}

fn collect_plan(h: &alpm::Alpm) -> AlpmTxPlan {
    let mut plan = AlpmTxPlan {
        to_install: vec![],
        to_remove: vec![],
        notes: vec![],
    };
    for p in h.trans_add() {
        plan.to_install
            .push(format!("{} {}", p.name(), p.version()));
    }
    for p in h.trans_remove() {
        plan.to_remove.push(p.name().to_string());
    }
    plan
}

pub fn plan_install(targets: &[String]) -> Result<AlpmTxPlan> {
    plan_install_with_providers(targets, &[])
}

/// Install plan with user-preselected providers for virtual targets:
/// `providers` maps virtual name -> concrete package name.
pub fn plan_install_with_providers(
    targets: &[String],
    providers: &[(String, String)],
) -> Result<AlpmTxPlan> {
    let mut h = open_read()?;
    let flags = TransFlag::NONE;
    if let Err(e) = h.trans_init(flags) {
        return Err(anyhow!(
            "ALPM transaction init failed (lock/permissions?): {e}. Run with appropriate privileges for real prepare/commit."
        ));
    }
    let mut resolved_notes = Vec::new();
    for t in targets {
        let mut found = None;
        for db in h.syncdbs() {
            if let Ok(pkg) = db.pkg(t.as_str()) {
                found = Some(pkg);
                break;
            }
        }
        let (pkg, via_provider) = if let Some(pkg) = found {
            (pkg, false)
        } else if let Some((_, chosen)) = providers.iter().find(|(v, _)| v == t) {
            // user-preselected provider for a virtual target
            let mut pkg_opt = None;
            for db in h.syncdbs() {
                if let Ok(p) = db.pkg(chosen.as_str()) {
                    pkg_opt = Some(p);
                    break;
                }
            }
            let pkg = match pkg_opt {
                Some(p) => p,
                None => {
                    h.trans_release().ok();
                    return Err(anyhow!(
                        "--provider {t}={chosen}: package {chosen} not found in sync dbs"
                    ));
                }
            };
            (pkg, true)
        } else if let Some(pkg) = h.syncdbs().find_satisfier(t.as_str()) {
            (pkg, true)
        } else {
            h.trans_release().ok();
            return Err(anyhow!(
                "target not found in sync dbs and no provider satisfies: {t} (may be AUR/missing; try query --aur/yay flows)"
            ));
        };
        if via_provider {
            resolved_notes.push(format!("resolved provider/virtual: {t} -> {}", pkg.name()));
        }
        h.trans_add_pkg(pkg)
            .map_err(|e| anyhow!("failed to add target {t}: {e}"))?;
    }
    let prep = h.trans_prepare().map_err(|e| e.to_string());
    match prep {
        Ok(()) => {
            let mut plan = collect_plan(&h);
            plan.notes.extend(resolved_notes);
            plan.notes
                .push("ALPM prepare succeeded (no commit performed).".into());
            h.trans_release().ok();
            Ok(plan)
        }
        Err(msg) => {
            h.trans_release().ok();
            Err(anyhow!(
                "ALPM prepare failed (conflict/provider/dependency issue?): {msg}"
            ))
        }
    }
}

/// Commit an install with user-preselected providers (see plan_install_with_providers).
pub fn commit_install_with_providers(
    targets: &[String],
    providers: &[(String, String)],
) -> Result<AlpmTxPlan> {
    let mut h = open_read()?;
    let flags = TransFlag::NONE;
    if let Err(e) = h.trans_init(flags) {
        return Err(anyhow!(
            "ALPM transaction init failed (lock/permissions?): {e}"
        ));
    }
    let mut resolved_notes = Vec::new();
    // map virtual -> concrete for the final pacman invocation (non-interactive)
    let mut commit_targets: Vec<String> = Vec::new();
    for t in targets {
        let mut found = None;
        for db in h.syncdbs() {
            if let Ok(pkg) = db.pkg(t.as_str()) {
                found = Some(pkg);
                break;
            }
        }
        let (pkg, via_provider) = if let Some(pkg) = found {
            (pkg, false)
        } else if let Some((_, chosen)) = providers.iter().find(|(v, _)| v == t) {
            let mut pkg_opt = None;
            for db in h.syncdbs() {
                if let Ok(p) = db.pkg(chosen.as_str()) {
                    pkg_opt = Some(p);
                    break;
                }
            }
            let pkg = match pkg_opt {
                Some(p) => p,
                None => {
                    h.trans_release().ok();
                    return Err(anyhow!(
                        "--provider {t}={chosen}: package {chosen} not found in sync dbs"
                    ));
                }
            };
            (pkg, true)
        } else if let Some(pkg) = h.syncdbs().find_satisfier(t.as_str()) {
            (pkg, true)
        } else {
            h.trans_release().ok();
            return Err(anyhow!(
                "target not found in sync dbs and no provider satisfies: {t} (may be AUR/missing; try query --aur/yay flows)"
            ));
        };
        if via_provider {
            resolved_notes.push(format!("resolved provider/virtual: {t} -> {}", pkg.name()));
        }
        commit_targets.push(pkg.name().to_string());
        h.trans_add_pkg(pkg)
            .map_err(|e| anyhow!("failed to add target {t}: {e}"))?;
    }
    let prep = h.trans_prepare().map_err(|e| e.to_string());
    if let Err(msg) = prep {
        h.trans_release().ok();
        return Err(anyhow!(
            "ALPM prepare failed (conflict/provider/dependency issue?): {msg}"
        ));
    }
    let mut plan = collect_plan(&h);
    plan.notes.extend(resolved_notes);
    h.trans_release().ok();
    // Commit via pacman after ALPM prepare validation (libalpm commit requires full server/cache configuration).
    let mut cmd = Command::new("pacman");
    cmd.args(["-S", "--noconfirm", "--needed", "--"]);
    for t in &commit_targets {
        cmd.arg(t);
    }
    let out = cmd
        .output()
        .map_err(|e| anyhow!("failed to execute pacman commit: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(anyhow!("commit failed after ALPM prepare: {msg}"));
    }
    plan.notes.push(
        "ALPM prepare validated; commit executed via `pacman -S --noconfirm --needed -- NAMES`."
            .into(),
    );
    Ok(plan)
}

pub fn plan_remove(targets: &[String]) -> Result<AlpmTxPlan> {
    let mut h = open_read()?;
    let flags = TransFlag::NONE;
    if let Err(e) = h.trans_init(flags) {
        return Err(anyhow!(
            "ALPM transaction init failed (lock/permissions?): {e}."
        ));
    }
    let local = h.localdb();
    for t in targets {
        match local.pkg(t.as_str()) {
            Ok(pkg) => {
                h.trans_remove_pkg(pkg)
                    .map_err(|e| anyhow!("failed to mark {t} for removal: {e}"))?;
            }
            Err(_) => {
                h.trans_release().ok();
                return Err(anyhow!("installed package not found: {t}"));
            }
        }
    }
    let prep = h.trans_prepare().map_err(|e| e.to_string());
    match prep {
        Ok(()) => {
            let mut plan = collect_plan(&h);
            plan.notes
                .push("ALPM prepare succeeded (no commit performed).".into());
            h.trans_release().ok();
            Ok(plan)
        }
        Err(msg) => {
            h.trans_release().ok();
            Err(anyhow!(
                "ALPM prepare failed (dependency/conflict issue?): {msg}"
            ))
        }
    }
}

pub fn commit_remove(targets: &[String]) -> Result<AlpmTxPlan> {
    let mut h = open_read()?;
    let flags = TransFlag::NONE;
    if let Err(e) = h.trans_init(flags) {
        return Err(anyhow!(
            "ALPM transaction init failed (lock/permissions?): {e}"
        ));
    }
    let local = h.localdb();
    for t in targets {
        match local.pkg(t.as_str()) {
            Ok(pkg) => {
                h.trans_remove_pkg(pkg)
                    .map_err(|e| anyhow!("failed to mark {t} for removal: {e}"))?;
            }
            Err(_) => {
                h.trans_release().ok();
                return Err(anyhow!("installed package not found: {t}"));
            }
        }
    }
    let prep = h.trans_prepare().map_err(|e| e.to_string());
    if let Err(msg) = prep {
        h.trans_release().ok();
        return Err(anyhow!(
            "ALPM prepare failed (dependency/conflict issue?): {msg}"
        ));
    }
    let mut plan = collect_plan(&h);
    h.trans_release().ok();
    let argv = remove_commit_argv(targets);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let out = cmd
        .output()
        .map_err(|e| anyhow!("failed to execute pacman commit: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(anyhow!("commit failed after ALPM prepare: {msg}"));
    }
    plan.notes.push(
        "ALPM prepare validated; commit executed via `pacman -R --noconfirm -- NAMES`.".into(),
    );
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_in_process_transaction_is_refused_up_front_without_root() {
        assert!(needs_root(true, "remove alpm", "remove repo").is_none());
        let why = needs_root(false, "remove alpm", "remove repo").unwrap();
        assert!(why.contains("run as root"), "{why}");
        assert!(why.contains("Nothing was changed"), "{why}");
        assert!(why.contains("`yapacc remove repo`"), "{why}");
    }

    #[test]
    fn the_commit_passes_names_after_a_separator() {
        let names = vec!["htop".to_string(), "--dbpath=/tmp/x".to_string()];
        assert_eq!(
            remove_commit_argv(&names),
            [
                "pacman",
                "-R",
                "--noconfirm",
                "--",
                "htop",
                "--dbpath=/tmp/x"
            ]
        );
    }
}
