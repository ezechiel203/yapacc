//! Resolution engine — the smart installer behind `yapacc get NAME`.
//!
//! What it does:
//!   1. Probe the lanes in the configured order (`[install] order`; the
//!      default is repo, aur, flatpak, recipe). Source plugin adapters are
//!      probed after them, and recipe is always last.
//!   2. The first lane that has the name is chosen; the decision table is
//!      shown before anything is applied (`--explain` shows it and stops).
//!   3. repo: an install through pacman, behind the gate. Nothing is
//!      recorded by yapacc for a native install.
//!      aur: a build behind the PKGBUILD review.
//!      flatpak: a system-wide flatpak install.
//!      brew: a Homebrew formula built through the recipe lane; only when
//!      it is in the order or forced with `--from brew`.
//!      tap: a name the table still knows; it never has anything, and
//!      `--from tap` is refused.
//!      recipe: a generated PKGBUILD, reviewed, built as an ordinary user
//!      and unpacked as an isolated install.

use serde::Serialize;

pub const LANES: &[&str] = &["repo", "aur", "flatpak", "brew", "tap", "recipe"];

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub lane: String,
    /// e.g. "extra", "aur", "flathub"; "" when unknown
    pub source: String,
    pub version: String,
    pub found: bool,
    /// why this lane was accepted/rejected
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Resolution {
    pub name: String,
    pub candidates: Vec<Candidate>,
    /// lane id of the winner (None only if every probe errored and recipe was disabled)
    pub chosen: Option<String>,
    pub runtime: String,
}

impl Resolution {
    /// Choose the first found candidate following `order`.
    /// `recipe` is always admissible (it is generation, not discovery).
    pub fn choose(order: &[String], candidates: &mut [Candidate]) -> Option<String> {
        for lane in order {
            if let Some(c) = candidates.iter_mut().find(|c| &c.lane == lane)
                && c.found
            {
                return Some(lane.clone());
            }
        }
        None
    }

    pub fn render(&self) -> String {
        let mut s = format!("== Resolution: {} ==\n", self.name);
        for c in &self.candidates {
            let mark = if Some(&c.lane) == self.chosen.as_ref() {
                "→"
            } else {
                " "
            };
            let status = if c.found {
                format!("{} {}", c.source, c.version)
            } else {
                "—".to_string()
            };
            s.push_str(&format!(
                " {mark} {:<8} {:<12} {:<24} {}\n",
                c.lane,
                if c.found { "found" } else { "missing" },
                tr(&status, 24),
                c.note
            ));
        }
        if let Some(chosen) = &self.chosen {
            s.push_str(&format!("chosen: {chosen}"));
            if chosen == "repo" || chosen == "aur" {
                s.push_str(" (native mode)");
            } else if chosen == "flatpak" {
                s.push_str(" (sandboxed)");
            } else if chosen == "recipe" {
                s.push_str(" (review-gated build into the yapacc prefix)");
            }
            s.push('\n');
        } else {
            s.push_str("chosen: none (all lanes exhausted)\n");
        }
        s
    }
}

fn tr(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

fn pacman_available() -> bool {
    std::process::Command::new("pacman")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// What is said of a name no lane is asked about.
pub const INVALID_NAME_NOTE: &str = "not a valid package name: not looked up";

/// `flatpak remote-info --system flathub -- NAME`: the name after `--`, so
/// that it can never be read as an option (`--installation=…`).
fn flatpak_probe_args(name: &str) -> [&str; 5] {
    ["remote-info", "--system", "flathub", "--", name]
}

/// Probe one lane for an EXACT package-name match. Never fuzzy.
///
/// The name goes to command lines (pacman, flatpak) and into request URLs
/// (AUR, Homebrew), so one that is not a valid package name is not looked
/// up anywhere: every lane, recipe included, answers "not found".
pub fn probe_lane(lane: &str, name: &str) -> Candidate {
    let mut c = Candidate {
        lane: lane.to_string(),
        source: String::new(),
        version: String::new(),
        found: false,
        note: String::new(),
    };
    if !crate::util::valid_pkg_name(name) {
        c.note = INVALID_NAME_NOTE.into();
        return c;
    }
    match lane {
        "repo" => {
            if !pacman_available() {
                c.note = "pacman/ALPM unavailable".into();
                return c;
            }
            match crate::alpm_flows::query_sync(name) {
                Ok(Some((repo, _, ver, _))) => {
                    c.source = repo;
                    c.version = ver;
                    c.found = true;
                    c.note = "exact name in official sync dbs".into();
                }
                Ok(None) => c.note = "not in sync dbs".into(),
                Err(e) => c.note = format!("probe failed: {e}"),
            }
        }
        "aur" => match crate::aur_rpc::fetch_info(&[name.to_string()]) {
            Ok(infos) => {
                if let Some(i) = infos.into_iter().find(|i| i.name == name) {
                    c.source = "aur".into();
                    c.version = i.version;
                    c.found = true;
                    c.note = format!("AUR (votes: {})", i.num_votes);
                } else {
                    c.note = "not in AUR".into();
                }
            }
            Err(e) => c.note = format!("AUR probe failed: {e}"),
        },
        "flatpak" => {
            let out = std::process::Command::new("flatpak")
                .args(flatpak_probe_args(name))
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout).to_string();
                    let ver = text
                        .lines()
                        .find(|l| l.trim_start().starts_with("Version:"))
                        .and_then(|l| l.split(':').nth(1))
                        .map(|v| v.trim().to_string())
                        .unwrap_or_default();
                    c.source = "flathub".into();
                    c.version = ver;
                    c.found = true;
                    c.note = "flatpak remote exact match".into();
                }
                Ok(_) => c.note = "not on flathub".into(),
                Err(_) => c.note = "flatpak unavailable".into(),
            }
        }
        "brew" => match crate::brew::fetch_formula(name) {
            Ok(f) => {
                c.source = "homebrew-core".into();
                c.version = f.version.clone();
                c.found = true;
                c.note = format!(
                    "homebrew formula ({}, deps: {})",
                    f.classification(),
                    f.depends_on.len()
                );
            }
            Err(e) => c.note = format!("not in homebrew-core ({e})"),
        },
        "tap" => {
            c.note = "no such source in this version".into();
        }
        "recipe" => {
            c.found = true;
            c.source = "generate".into();
            c.version = "-".into();
            c.note = "fallback: generate a build recipe (review-gated)".into();
        }
        other => c.note = format!("unknown lane '{other}'"),
    }
    c
}

/// Resolve a name across all configured lanes (built-ins + plugin source
/// adapters probed just before recipe) and choose a winner.
pub fn resolve(name: &str, order: &[String]) -> Resolution {
    let plugins: Vec<crate::plugin::PluginAdapter> = crate::plugin::discover()
        .into_iter()
        .filter(|a| a.kind == crate::plugin::PluginKind::Source)
        .collect();
    let plugin_lanes: Vec<String> = plugins.iter().map(|a| a.name.clone()).collect();
    let mut full_order: Vec<String> = order.iter().filter(|l| *l != "recipe").cloned().collect();
    full_order.extend(plugin_lanes.iter().filter(|p| !order.contains(p)).cloned());
    full_order.push("recipe".into());
    let mut candidates: Vec<Candidate> = full_order
        .iter()
        .map(|l| {
            if plugin_lanes.contains(l) {
                let a = plugins.iter().find(|a| &a.name == l).unwrap();
                crate::plugin::probe_source(a, name)
            } else {
                probe_lane(l, name)
            }
        })
        .collect();
    let chosen = Resolution::choose(&full_order, &mut candidates);
    Resolution {
        name: name.to_string(),
        candidates,
        chosen,
        runtime: crate::config::get().install.runtime_default.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(lane: &str, found: bool) -> Candidate {
        Candidate {
            lane: lane.into(),
            source: "s".into(),
            version: "1.0".into(),
            found,
            note: String::new(),
        }
    }

    #[test]
    fn choose_follows_order() {
        let order: Vec<String> = ["repo", "aur", "flatpak", "tap", "recipe"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // repo missing, aur found -> aur wins even though flatpak also found
        let mut cs = vec![
            cand("repo", false),
            cand("aur", true),
            cand("flatpak", true),
            cand("tap", false),
            cand("recipe", true),
        ];
        assert_eq!(Resolution::choose(&order, &mut cs).as_deref(), Some("aur"));
        // everything missing except recipe -> recipe
        let mut cs = vec![
            cand("repo", false),
            cand("aur", false),
            cand("flatpak", false),
            cand("tap", false),
            cand("recipe", true),
        ];
        assert_eq!(
            Resolution::choose(&order, &mut cs).as_deref(),
            Some("recipe")
        );
    }

    #[test]
    fn choose_respects_custom_order() {
        let order: Vec<String> = vec!["flatpak".to_string(), "repo".to_string()];
        let mut cs = vec![cand("repo", true), cand("flatpak", true)];
        assert_eq!(
            Resolution::choose(&order, &mut cs).as_deref(),
            Some("flatpak")
        );
    }

    #[test]
    fn table_marks_chosen_and_lane() {
        let r = Resolution {
            name: "hello".into(),
            candidates: vec![
                Candidate {
                    lane: "repo".into(),
                    source: "extra".into(),
                    version: "2.10-3".into(),
                    found: true,
                    note: "exact name in official sync dbs".into(),
                },
                Candidate {
                    lane: "recipe".into(),
                    source: "generate".into(),
                    version: "-".into(),
                    found: true,
                    note: "fallback".into(),
                },
            ],
            chosen: Some("repo".into()),
            runtime: "container".into(),
        };
        let text = r.render();
        assert!(text.contains("→ repo"), "got: {text}");
        assert!(text.contains("(native mode)"));
        assert!(text.contains("extra"));
        // missing lane marked
        let r2 = Resolution {
            name: "x".into(),
            candidates: vec![Candidate {
                lane: "aur".into(),
                source: String::new(),
                version: String::new(),
                found: false,
                note: "not in AUR".into(),
            }],
            chosen: None,
            runtime: "container".into(),
        };
        assert!(r2.render().contains("missing"));
        assert!(r2.render().contains("chosen: none"));
    }

    #[test]
    fn recipe_lane_is_always_found() {
        let c = probe_lane("recipe", "whatever");
        assert!(c.found);
        let c2 = probe_lane("tap", "whatever");
        assert!(!c2.found);
        assert!(c2.note.contains("no such source in this version"));
    }

    #[test]
    fn the_flatpak_probe_puts_the_name_after_the_separator() {
        assert_eq!(
            flatpak_probe_args("org.gnome.Gedit"),
            [
                "remote-info",
                "--system",
                "flathub",
                "--",
                "org.gnome.Gedit"
            ]
        );
    }

    /// A name that is not a package name reaches no command line and no
    /// URL: every lane says so without asking anything, recipe included.
    #[test]
    fn an_invalid_name_is_not_looked_up_in_any_lane() {
        for bad in [
            "--installation=x",
            "-y",
            "--config=/x",
            "a b",
            "a\nb",
            "x;id",
            "../x",
            "",
        ] {
            for lane in LANES {
                let c = probe_lane(lane, bad);
                assert!(!c.found, "{lane} {bad:?}");
                assert_eq!(c.note, INVALID_NAME_NOTE, "{lane} {bad:?}");
            }
        }
        // nothing is chosen for it, so nothing is installed or generated
        let order: Vec<String> = LANES.iter().map(|l| l.to_string()).collect();
        let mut cs: Vec<Candidate> = LANES.iter().map(|l| probe_lane(l, "-y")).collect();
        assert_eq!(Resolution::choose(&order, &mut cs), None);
    }
}
