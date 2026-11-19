//! Serializable action plans (Plan-as-data): install/remove/upgrade planning
//! produces a structured, JSON-serializable plan that can be inspected
//! (`--json`), saved (`--out <file>`), and later applied (`plan apply FILE`)
//! or consumed by the TUI/daemon.
//!
//! `plan apply` takes the operation, the source and the targets from the
//! file, validates them (`ActionPlan::validate`), and rebuilds and simulates
//! the command itself. `planned_command`, `to_install`, `to_remove`,
//! `warnings` and `notes` are a record of the planning run for the reader;
//! they are never executed and never shown as what will happen.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PLAN_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionPlan {
    pub schema: u32,
    /// install | remove | upgrade | update | install-local
    pub op: String,
    pub targets: Vec<String>,
    /// repo | aur | auto
    pub source: String,
    pub planned_command: Vec<String>,
    pub to_install: Vec<String>,
    pub to_remove: Vec<String>,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
    pub created_at: u64,
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ActionPlan {
    pub fn new(op: &str, targets: &[String], source: &str) -> Self {
        ActionPlan {
            schema: PLAN_SCHEMA,
            op: op.to_string(),
            targets: targets.to_vec(),
            source: source.to_string(),
            planned_command: vec![],
            to_install: vec![],
            to_remove: vec![],
            warnings: vec![],
            notes: vec![],
            created_at: now_ts(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)
                .map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        let data =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize plan: {e}"))?;
        std::fs::write(path, data + "\n").map_err(|e| format!("write plan {}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<ActionPlan, String> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("read plan {}: {e}", path.display()))?;
        let plan: ActionPlan = serde_json::from_str(&data)
            .map_err(|e| format!("parse plan {}: {e}", path.display()))?;
        if plan.schema != PLAN_SCHEMA {
            return Err(format!(
                "plan schema {} not supported (expected {PLAN_SCHEMA}): {}",
                plan.schema,
                path.display()
            ));
        }
        Ok(plan)
    }

    /// Age in seconds since the plan was created.
    pub fn age_secs(&self) -> u64 {
        now_ts().saturating_sub(self.created_at)
    }

    /// Whether this plan may be applied now. See `validate_at`.
    pub fn validate(&self, force: bool) -> Result<(), String> {
        self.validate_at(now_ts(), force)
    }

    /// Check what `plan apply` takes from the file: the operation, the
    /// source, the targets and the creation time. A plan file is data from
    /// outside: a target must be a package name (never an option), and
    /// nothing else in the file is trusted. `force` waives the two
    /// freshness rules only. Pure.
    pub fn validate_at(&self, now: u64, force: bool) -> Result<(), String> {
        match self.op.as_str() {
            "install" | "remove" => {
                if self.targets.is_empty() {
                    return Err(format!("a plan to {} names no package", self.op));
                }
                for t in &self.targets {
                    if !crate::backend::valid_pacman_target(t) {
                        return Err(format!("target '{t}' is not a valid package name"));
                    }
                }
            }
            "install-local" => {
                if self.targets.is_empty() {
                    return Err("a plan to install local files names no file".into());
                }
            }
            "upgrade" | "update" => {
                if !self.targets.is_empty() {
                    return Err(format!(
                        "a plan to {} takes no targets, and this one names: {}",
                        self.op,
                        self.targets.join(", ")
                    ));
                }
            }
            other => return Err(format!("plan op '{other}' is not applicable")),
        }
        if !matches!(self.source.as_str(), "repo" | "aur" | "auto") {
            return Err(format!(
                "plan source '{}' is not one of repo, aur, auto",
                self.source
            ));
        }
        if force {
            return Ok(());
        }
        // a few minutes of difference between two clocks is not a forgery
        if self.created_at > now + 300 {
            return Err(format!(
                "its creation time is {} in the future, so its age cannot be checked; create the plan again, or pass --force to apply it anyway",
                crate::backend::age_text(self.created_at - now)
            ));
        }
        let age = now.saturating_sub(self.created_at);
        if age > 48 * 3600 {
            return Err(format!(
                "the plan is stale ({}h old); create it again, or pass --force to apply it anyway",
                age / 3600
            ));
        }
        Ok(())
    }

    /// Human-oriented rendering (parity with the legacy text plans).
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "== Plan ({}) ==\nTargets: {}\nSource: {}\n",
            self.op,
            if self.targets.is_empty() {
                "<none>".to_string()
            } else {
                self.targets.join(", ")
            },
            self.source
        ));
        if !self.to_install.is_empty() {
            s.push_str(&format!(
                "To install ({}): {}\n",
                self.to_install.len(),
                self.to_install.join(", ")
            ));
        }
        if !self.to_remove.is_empty() {
            s.push_str(&format!(
                "To remove ({}): {}\n",
                self.to_remove.len(),
                self.to_remove.join(", ")
            ));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        for n in &self.notes {
            s.push_str(&format!("note: {n}\n"));
        }
        if !self.planned_command.is_empty() {
            s.push_str(&format!(
                "\nWill run:\n  {}\n",
                self.planned_command.join(" ")
            ));
        }
        s.push_str(crate::prompt::PLAN_ONLY_FOOTER);
        s.push('\n');
        s
    }
}

/// A plan is only written for names that can be applied: the same rule
/// `plan apply` enforces when it reads the file back.
fn check_names(pkgs: &[String]) -> Result<(), String> {
    match pkgs
        .iter()
        .find(|p| !crate::backend::valid_pacman_target(p))
    {
        Some(bad) => Err(format!("'{bad}' is not a valid package name")),
        None => Ok(()),
    }
}

/// Fill the plan from pacman's own `--print` simulation: what a normal
/// user and a build without libalpm get instead of the in-process analysis.
fn simulate_into(plan: &mut ActionPlan, op: &str, pkgs: &[String]) {
    let Some(sim) = crate::backend::preview_argv(op, pkgs, false) else {
        return;
    };
    let shown = crate::ops::shown(&[], &sim);
    match crate::backend::run_simulation(&sim) {
        Ok((0, lines, _)) => {
            let listed = crate::backend::simulation_packages(&lines);
            match op {
                "remove" => plan.to_remove = listed,
                _ => plan.to_install = listed,
            }
            plan.notes.push(format!("package list from: {shown}"));
        }
        Ok((code, _, stderr)) => plan.warnings.push(format!(
            "{shown} failed (status {code}): {}",
            stderr.lines().next().unwrap_or("no message")
        )),
        Err(e) => plan.warnings.push(format!("no simulation: {e}")),
    }
}

/// Build an install plan: repo metadata + best-effort ALPM prepare analysis.
pub fn build_install_plan(pkgs: &[String], use_yay: bool) -> Result<ActionPlan, String> {
    check_names(pkgs)?;
    let mut plan = ActionPlan::new("install", pkgs, if use_yay { "aur" } else { "repo" });
    for p in pkgs {
        match crate::backend::si_repo(p) {
            Ok(si) => {
                let desc = si
                    .lines()
                    .find(|l| l.starts_with("Description"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default();
                plan.to_install.push(p.clone());
                plan.notes.push(format!("{p} [repo]: {desc}"));
            }
            Err(e) => {
                plan.warnings.push(format!(
                    "{p} not in sync repos ({e}); may be AUR/group/virtual — use --aur or query first"
                ));
            }
        }
    }
    if !use_yay && !crate::backend::alpm_analysis_available() {
        simulate_into(&mut plan, "install", pkgs);
    } else if !use_yay {
        match crate::alpm_flows::plan_install(pkgs) {
            Ok(alpm_plan) => {
                plan.to_install = alpm_plan.to_install.clone();
                plan.to_remove = alpm_plan.to_remove.clone();
                for n in &alpm_plan.notes {
                    plan.notes.push(format!("ALPM: {n}"));
                }
            }
            Err(e) => plan
                .warnings
                .push(format!("ALPM prepare analysis unavailable/conflict: {e}")),
        }
    } else {
        plan.notes
            .push("ALPM prepare analysis skipped for --aur/yay flow".into());
    }
    plan.planned_command = crate::backend::build_command("install", pkgs, use_yay);
    Ok(plan)
}

/// Build a remove plan: impact analysis + ALPM prepare.
pub fn build_remove_plan(pkgs: &[String]) -> Result<ActionPlan, String> {
    check_names(pkgs)?;
    let mut plan = ActionPlan::new("remove", pkgs, "repo");
    let impact = crate::backend::remove_plan(pkgs)?;
    plan.to_remove = impact.requested_found.clone();
    if impact.not_found.is_empty() && !impact.requested_found.is_empty() {
        // the whole transaction (`-Rs` also removes unneeded dependencies)
        simulate_into(&mut plan, "remove", pkgs);
    }
    if !impact.not_found.is_empty() {
        plan.warnings
            .push(format!("not installed: {}", impact.not_found.join(", ")));
    }
    for (p, deps) in &impact.direct_dependents {
        plan.warnings.push(format!(
            "{p} is required by {}: {}",
            deps.len(),
            deps.join(", ")
        ));
    }
    plan.warnings
        .extend(impact.warnings.iter().map(|w| format!("impact: {w}")));
    plan.planned_command = crate::backend::build_command("remove", pkgs, false);
    Ok(plan)
}

/// Build an upgrade plan (pending updates snapshot).
pub fn build_upgrade_plan(aur: bool) -> Result<ActionPlan, String> {
    let mut plan = ActionPlan::new("upgrade", &[], if aur { "aur" } else { "repo" });
    let data = crate::backend::check_updates_data();
    plan.to_install = data.repo_updates.clone();
    for (n, cur, avail) in &data.aur_updates {
        plan.notes.push(format!("{n} {cur} -> {avail} (AUR)"));
    }
    if !data.repo_check_ok {
        plan.warnings.push("repo update check unavailable".into());
    }
    if aur && !data.aur_check_ok {
        plan.warnings.push("AUR update check unavailable".into());
    }
    plan.planned_command = crate::backend::build_command("upgrade", &[], aur);
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_roundtrip() {
        let mut plan = ActionPlan::new("install", &["ripgrep".to_string()], "repo");
        plan.to_install.push("ripgrep 14.1.0-1".into());
        plan.warnings.push("none".into());
        plan.planned_command = vec![
            "sudo".into(),
            "pacman".into(),
            "-S".into(),
            "ripgrep".into(),
        ];
        let dir = std::env::temp_dir().join(format!("pc-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("plan.json");
        plan.save(&p).unwrap();
        let loaded = ActionPlan::load(&p).unwrap();
        assert_eq!(loaded.op, "install");
        assert_eq!(loaded.targets, vec!["ripgrep".to_string()]);
        assert_eq!(loaded.to_install, plan.to_install);
        assert_eq!(loaded.planned_command, plan.planned_command);
        assert!(loaded.age_secs() < 60);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan_with(op: &str, targets: &[&str], source: &str) -> ActionPlan {
        let t: Vec<String> = targets.iter().map(|s| s.to_string()).collect();
        let mut p = ActionPlan::new(op, &t, source);
        p.created_at = 1_000_000;
        p
    }

    #[test]
    fn a_plan_file_cannot_smuggle_options_as_targets() {
        // the review's file: one honest target, two pacman options
        let mut p = plan_with(
            "install",
            &[
                "htop",
                "--config=/tmp/evil-pacman.conf",
                "--hookdir=/tmp/hooks",
            ],
            "repo",
        );
        // what the file claims will run is not consulted at all
        p.planned_command = vec!["sudo".into(), "pacman".into(), "-S".into(), "htop".into()];
        let e = p.validate_at(1_000_000, false).unwrap_err();
        assert!(
            e.contains("'--config=/tmp/evil-pacman.conf' is not a valid package name"),
            "{e}"
        );
        // --force waives freshness, never validation
        assert!(p.validate_at(1_000_000, true).is_err());
        for bad in ["-Syu", "a;id", "x y", "$(id)", ""] {
            assert!(
                plan_with("remove", &[bad], "repo")
                    .validate_at(1_000_000, true)
                    .is_err()
            );
        }
        assert!(
            plan_with("install", &["htop", "extra/tree"], "repo")
                .validate_at(1_000_000, false)
                .is_ok()
        );
    }

    #[test]
    fn plan_ops_sources_and_shapes_are_checked() {
        let now = 1_000_000;
        assert!(
            plan_with("upgrade", &[], "repo")
                .validate_at(now, false)
                .is_ok()
        );
        assert!(
            plan_with("upgrade", &[], "aur")
                .validate_at(now, false)
                .is_ok()
        );
        // an upgrade with "targets" would otherwise be shown and then dropped
        let e = plan_with("upgrade", &["htop"], "repo")
            .validate_at(now, false)
            .unwrap_err();
        assert!(e.contains("takes no targets"), "{e}");
        assert!(
            plan_with("install", &[], "repo")
                .validate_at(now, false)
                .is_err()
        );
        assert!(
            plan_with("install-local", &[], "repo")
                .validate_at(now, false)
                .is_err()
        );
        let e = plan_with("exec", &["x"], "repo")
            .validate_at(now, false)
            .unwrap_err();
        assert!(e.contains("not applicable"), "{e}");
        let e = plan_with("install", &["htop"], "yay --noconfirm")
            .validate_at(now, false)
            .unwrap_err();
        assert!(e.contains("is not one of repo, aur, auto"), "{e}");
    }

    #[test]
    fn stale_and_future_plans_need_force() {
        let p = plan_with("install", &["htop"], "repo");
        // 49 hours later
        let e = p.validate_at(1_000_000 + 49 * 3600, false).unwrap_err();
        assert!(e.contains("stale (49h old)"), "{e}");
        assert!(p.validate_at(1_000_000 + 49 * 3600, true).is_ok());
        // created "tomorrow": its age would read as zero for ever
        let e = p.validate_at(1_000_000 - 86_400, false).unwrap_err();
        assert!(e.contains("in the future"), "{e}");
        assert!(p.validate_at(1_000_000 - 86_400, true).is_ok());
        // two clocks a minute apart
        assert!(p.validate_at(1_000_000 - 60, false).is_ok());
    }

    #[test]
    fn planners_refuse_names_they_could_not_apply() {
        for bad in ["--config=/tmp/x", "-Syu", "a;b"] {
            let names = vec!["htop".to_string(), bad.to_string()];
            let e = build_install_plan(&names, false).unwrap_err();
            assert!(e.contains("is not a valid package name"), "{e}");
            let e = build_remove_plan(&names).unwrap_err();
            assert!(e.contains("is not a valid package name"), "{e}");
        }
    }

    #[test]
    fn plan_schema_mismatch_rejected() {
        let dir = std::env::temp_dir().join(format!("pc-plan2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.json");
        std::fs::write(&p, r#"{"schema":99,"op":"install","targets":[],"source":"repo","planned_command":[],"to_install":[],"to_remove":[],"warnings":[],"notes":[],"created_at":0}"#).unwrap();
        assert!(ActionPlan::load(&p).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_contains_core_sections() {
        let mut plan = ActionPlan::new("remove", &["foo".to_string()], "repo");
        plan.to_remove.push("foo".into());
        plan.warnings.push("foo is required by bar".into());
        let text = plan.render();
        assert!(text.contains("To remove (1): foo"));
        assert!(text.contains("warning: foo is required by bar"));
        assert!(text.contains("PLAN ONLY: nothing was changed."));
    }
}
