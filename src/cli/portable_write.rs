// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Change verbs on systems whose native package manager is not pacman:
//! `install repo`, `remove repo`, `remove orphans`, `upgrade`, `refresh`
//! and `get` go through `crate::ops` (the native manager's own commands,
//! simulated, shown and gated). Also the guard that makes a pacman-only
//! verb say so on another system instead of failing inside pacman.

use super::{Gate, InstallVerb, RemoveVerb, Verb};
use crate::error::{Error, Result};
use crate::native::{self, Candidate};
use crate::ops::{self, Change, GateFlags};
use crate::platform::PkgManager;
use std::process::Command;

enum Job<'a> {
    Change(Change, &'a Gate),
    Orphans(&'a Gate),
    Refresh(&'a Gate),
    Get {
        names: &'a [String],
        from: Option<&'a str>,
        explain: bool,
        gate: &'a Gate,
    },
    AurOnly,
}

pub(super) fn route(
    verb: &Verb,
    noninteractive: bool,
    script: bool,
    json: bool,
) -> Option<Result<()>> {
    let job = match verb {
        Verb::Install {
            cmd: InstallVerb::Repo { pkgs, aur, gate },
        } => {
            if *aur {
                Job::AurOnly
            } else {
                Job::Change(Change::Install(pkgs.clone()), gate)
            }
        }
        Verb::Remove {
            cmd: RemoveVerb::Repo { pkgs, gate },
        } => Job::Change(Change::Remove(pkgs.clone()), gate),
        Verb::Remove {
            cmd: RemoveVerb::Orphans { gate },
        } => Job::Orphans(gate),
        Verb::Upgrade { aur, gate } => {
            if *aur {
                Job::AurOnly
            } else {
                Job::Change(Change::Upgrade, gate)
            }
        }
        Verb::Refresh { gate } => Job::Refresh(gate),
        Verb::Get {
            names,
            from,
            explain,
            gate,
            ..
        } => Job::Get {
            names,
            from: from.as_deref(),
            explain: *explain,
            gate,
        },
        _ => return None,
    };
    let (present, native) = crate::platform::native_manager();
    let Some(manager) = native else {
        // a package database without its tool on PATH: say so instead of
        // falling through to pacman
        if present.is_empty()
            && let Some((m, tool)) = crate::platform::missing_native_tool()
        {
            return Some(Err(Error::unavailable(
                crate::platform::missing_tool_message(m, tool),
            )));
        }
        return None;
    };
    if manager == PkgManager::Pacman {
        return None;
    }
    let flags = |g: &Gate| GateFlags {
        dryrun: g.dryrun,
        yes: g.yes,
        noninteractive,
        script,
    };
    Some(match job {
        Job::AurOnly => Err(Error::unavailable(format!(
            "--aur exists only on the Arch family; this system's package manager is {}",
            manager.name()
        ))),
        Job::Change(change, gate) => ops::apply(manager, &change, flags(gate)),
        Job::Refresh(gate) => ops::refresh(manager, flags(gate)),
        Job::Orphans(gate) => native::backend(manager).and_then(|b| {
            // "no orphan" is never concluded from a list that could not be
            // worked out
            if let Some(why) = super::portable::orphans_unknown(b.as_ref()) {
                return Err(Error::unavailable(format!(
                    "the packages nothing needs any more cannot be worked out: {why}. Nothing was changed."
                )));
            }
            let orphans = b.orphans()?;
            if orphans.is_empty() {
                println!("No orphan dependencies found. Nothing to remove.");
                return Ok(());
            }
            println!(
                "{} package(s) nothing needs any more: {}",
                orphans.len(),
                orphans.join(", ")
            );
            ops::apply(manager, &Change::Remove(orphans), flags(gate))
        }),
        Job::Get {
            names,
            from,
            explain,
            gate,
        } => get(manager, names, from, explain, json, flags(gate)),
    })
}

/// One row of the decision table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub lane: String,
    pub found: bool,
    pub what: String,
    pub note: String,
}

/// Parse `flatpak search --columns=application,name,version,remotes`:
/// rows whose name, or last part of the application id, is `query`.
pub fn flatpak_matches(text: &str, query: &str) -> Vec<(String, String, String)> {
    let q = query.to_lowercase();
    text.lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').map(str::trim).collect();
            (c.len() >= 4 && c[0].contains('.'))
                .then(|| (c[0].to_string(), c[1].to_string(), c[3].to_string()))
        })
        .filter(|(app, name, _)| {
            name.to_lowercase() == q
                || app.rsplit('.').next().map(str::to_lowercase) == Some(q.clone())
        })
        .collect()
}

/// The table and the winner: the distribution's own repository wins when it
/// has the exact name; otherwise nothing is chosen for the user.
/// `missing_metadata` is the reason the repositories could not be asked (no
/// metadata on disk), if they could not: the repo row then says that, not
/// "no package of that name".
pub fn decide(
    manager: &str,
    repo: Option<&Candidate>,
    flatpaks: &[(String, String, String)],
    flatpak_present: bool,
    missing_metadata: Option<&str>,
) -> (Vec<Row>, Option<&'static str>) {
    let mut rows = vec![match repo {
        Some(c) => Row {
            lane: "repo".into(),
            found: true,
            what: format!("{} {} {}", c.repo, c.name, c.version)
                .trim()
                .to_string(),
            note: format!("exact name in the {manager} repositories"),
        },
        None => Row {
            lane: "repo".into(),
            found: false,
            what: "—".into(),
            note: match missing_metadata {
                Some(why) => format!("not known: {why}"),
                None => {
                    format!("no package of that exact name in the {manager} repositories on disk")
                }
            },
        },
    }];
    if !flatpak_present {
        rows.push(Row {
            lane: "flatpak".into(),
            found: false,
            what: "—".into(),
            note: "flatpak is not installed on this system".into(),
        });
    } else if flatpaks.is_empty() {
        rows.push(Row {
            lane: "flatpak".into(),
            found: false,
            what: "—".into(),
            note: "no application of that name on the configured remotes".into(),
        });
    } else {
        for (app, name, remote) in flatpaks {
            rows.push(Row {
                lane: "flatpak".into(),
                found: true,
                what: format!("{app} ({name})"),
                note: format!("remote '{remote}'"),
            });
        }
    }
    let chosen = repo.is_some().then_some("repo");
    (rows, chosen)
}

pub fn render(name: &str, rows: &[Row], chosen: Option<&str>) -> String {
    let mut s = format!("== Resolution: {name} ==\n");
    for r in rows {
        s.push_str(&format!(
            " {} {:<8} {:<8} {:<40} {}\n",
            if Some(r.lane.as_str()) == chosen && r.found {
                "→"
            } else {
                " "
            },
            r.lane,
            if r.found { "found" } else { "missing" },
            r.what,
            r.note
        ));
    }
    match chosen {
        Some(lane) => s.push_str(&format!(
            "chosen: {lane} (the distribution's own package wins when it exists)\n"
        )),
        None => s.push_str("chosen: none\n"),
    }
    s
}

/// The decision as `--json get NAME --explain` prints it: the same shape as
/// on the pacman lane (`resolve::Resolution`). Pure.
pub fn resolution(
    name: &str,
    repo: Option<&Candidate>,
    flatpaks: &[(String, String, String)],
    rows: &[Row],
    chosen: Option<&str>,
) -> crate::resolve::Resolution {
    let mut candidates = Vec::new();
    for row in rows {
        // one row per flatpak found: each is matched to its application below
        if row.lane == "flatpak" && row.found {
            continue;
        }
        candidates.push(crate::resolve::Candidate {
            lane: row.lane.clone(),
            source: repo
                .filter(|_| row.lane == "repo")
                .map(|c| c.repo.clone())
                .unwrap_or_default(),
            version: repo
                .filter(|_| row.lane == "repo")
                .map(|c| c.version.clone())
                .unwrap_or_default(),
            found: row.found,
            note: row.note.clone(),
        });
    }
    for (app, title, remote) in flatpaks {
        candidates.push(crate::resolve::Candidate {
            lane: "flatpak".into(),
            source: remote.clone(),
            version: String::new(),
            found: true,
            note: format!("{app} ({title})"),
        });
    }
    crate::resolve::Resolution {
        name: name.to_string(),
        candidates,
        chosen: chosen.map(str::to_string),
        runtime: String::new(),
    }
}

fn get(
    manager: PkgManager,
    names: &[String],
    from: Option<&str>,
    explain: bool,
    json: bool,
    flags: GateFlags,
) -> Result<()> {
    if names.is_empty() {
        return Err(Error::usage("get needs a package name"));
    }
    if let Some(lane) = from
        && lane != "repo"
        && lane != "flatpak"
    {
        // a lane of the pacman lane is unavailable here (6); anything else
        // is not a lane at all (2), as it is on Arch
        if !crate::resolve::LANES.contains(&lane) || lane == "tap" {
            return Err(Error::usage(format!(
                "invalid --from lane '{lane}' (expected: repo | flatpak; on Arch also aur | brew | recipe)"
            )));
        }
        return Err(Error::unavailable(format!(
            "the '{lane}' lane does not exist with {}; lanes here: repo, flatpak",
            manager.name()
        )));
    }
    let backend = native::backend(manager)?;
    let flatpak_present = crate::util::have("flatpak");
    let mut from_repo = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for name in names {
        let repo = if from == Some("flatpak") {
            None
        } else {
            backend.candidate(name)?
        };
        let flatpaks = if flatpak_present && from != Some("repo") {
            Command::new("flatpak")
                .args([
                    "search",
                    "--columns=application,name,version,remotes",
                    "--",
                    name,
                ])
                .env("LC_ALL", "C")
                .output()
                .map(|o| flatpak_matches(&String::from_utf8_lossy(&o.stdout), name))
                .unwrap_or_default()
        } else {
            vec![]
        };
        let missing = if repo.is_none() && from != Some("flatpak") {
            backend.missing_metadata()
        } else {
            None
        };
        let (rows, chosen) = decide(
            manager.name(),
            repo.as_ref(),
            &flatpaks,
            flatpak_present,
            missing.as_deref(),
        );
        if json {
            // `--json get` exists only with --explain (checked in `run`)
            super::print_json(&resolution(name, repo.as_ref(), &flatpaks, &rows, chosen));
        } else {
            print!("{}", render(name, &rows, chosen));
        }
        if explain {
            // the table was printed; a name no source has is still "not
            // found" (exit 4), so that a caller cannot take it for a plan
            if chosen.is_none() {
                unresolved.push(name.clone());
            }
            continue;
        }
        match chosen {
            Some(_) => from_repo.push(name.clone()),
            None if flatpaks.len() == 1 => {
                return Err(Error::not_found(format!(
                    "'{name}' is not in the {} repositories. One flatpak matches; yapacc does not choose a sandboxed application for you: `yapacc install flatpak {}` installs it.",
                    manager.name(),
                    flatpaks[0].0
                )));
            }
            None => {
                return Err(Error::not_found(format!(
                    "'{name}' was found in no source on this system{}",
                    if flatpaks.is_empty() {
                        "; `yapacc search` looks for similar names".to_string()
                    } else {
                        format!(
                            "'s repositories; {} flatpaks match — pick one with `yapacc install flatpak APP`",
                            flatpaks.len()
                        )
                    }
                )));
            }
        }
    }
    if !unresolved.is_empty() {
        return Err(Error::not_found(explain_not_found(&unresolved)));
    }
    if explain || from_repo.is_empty() {
        return Ok(());
    }
    ops::apply(manager, &Change::Install(from_repo), flags)
}

/// What `get NAME --explain` ends with (exit 4) when it chose no source for
/// a name: the table above it says where it looked. Pure.
pub fn explain_not_found(names: &[String]) -> String {
    format!(
        "no source was chosen for {}: see the table above; nothing would be installed",
        names
            .iter()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A verb that only exists for pacman, asked on another system: say so.
pub(super) fn pacman_only(verb: &Verb) -> Result<()> {
    let what = match verb {
        Verb::Query { .. } => "query",
        Verb::News { .. } => "news",
        Verb::Aur { .. } => "aur",
        Verb::Package { .. } => "package",
        Verb::Mark { .. } => "mark",
        Verb::Download { .. } => "download",
        Verb::Cache { .. } => "cache",
        Verb::Plan { .. } => "plan",
        Verb::Safety { .. } => "safety",
        Verb::Inspect { .. } => "inspect",
        Verb::Index { .. } => "index",
        // the check, and the timer that would run it, read pacman and the AUR
        Verb::Monitor { .. } => "monitor",
        Verb::Install {
            cmd: InstallVerb::Aur { .. },
        } => "install aur",
        Verb::Install {
            cmd: InstallVerb::Local { .. },
        } => "install local",
        Verb::Install {
            cmd: InstallVerb::Alpm { .. },
        } => "install alpm",
        Verb::Remove {
            cmd: RemoveVerb::Alpm { .. },
        } => "remove alpm",
        // the list of categories is built in; the packages of one come
        // from pacman
        Verb::Tree { path: Some(p), .. } if p.contains('.') => "tree CATEGORY.SUBCATEGORY",
        _ => return Ok(()),
    };
    match crate::platform::native_manager().1 {
        Some(PkgManager::Pacman) | None => Ok(()),
        Some(m) => Err(Error::unavailable(format!(
            "`yapacc {what}` exists only for pacman; this system's package manager is {}. `yapacc docs commands` lists what works everywhere.",
            m.name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand() -> Candidate {
        Candidate {
            name: "tree".into(),
            version: "2.2.1-1".into(),
            repo: "main".into(),
            description: "lists directories".into(),
        }
    }

    #[test]
    fn the_distribution_package_wins() {
        let fp = vec![(
            "org.example.Tree".to_string(),
            "Tree".to_string(),
            "flathub".to_string(),
        )];
        let (rows, chosen) = decide("apt", Some(&cand()), &fp, true, None);
        assert_eq!(chosen, Some("repo"));
        assert_eq!(rows.len(), 2);
        let out = render("tree", &rows, chosen);
        assert!(out.contains("→ repo"), "{out}");
        assert!(out.contains("chosen: repo"), "{out}");
    }

    #[test]
    fn nothing_is_chosen_for_the_user_outside_the_repositories() {
        let fp = vec![(
            "org.example.Tree".to_string(),
            "Tree".to_string(),
            "flathub".to_string(),
        )];
        let (rows, chosen) = decide("dnf", None, &fp, true, None);
        assert_eq!(chosen, None);
        assert!(rows[1].found);
        let (rows, _) = decide("dnf", None, &[], false, None);
        assert!(rows[1].note.contains("not installed"), "{rows:?}");
        assert!(render("x", &rows, None).contains("chosen: none"));
    }

    /// M1: `get nosuchpackage-xyz --explain` printed "chosen: none" and
    /// exited 0, which the MCP server took for a plan.
    #[test]
    fn an_explanation_that_chose_nothing_is_not_found() {
        let e = Error::not_found(explain_not_found(&["nosuchpackage-xyz".to_string()]));
        assert_eq!(e.kind.exit_code(), 4);
        assert_eq!(
            e.to_string(),
            "no source was chosen for 'nosuchpackage-xyz': see the table above; nothing would be installed"
        );
        // without repository metadata the repo row says why it is unknown
        let why = native::no_metadata_message(PkgManager::Dnf, &[]);
        let (rows, chosen) = decide("dnf", None, &[], false, Some(&why));
        assert_eq!(chosen, None);
        assert!(
            rows[0]
                .note
                .starts_with("not known: no repository metadata on disk"),
            "{rows:?}"
        );
    }

    #[test]
    fn the_json_decision_has_the_shape_of_the_pacman_lane() {
        let fp = vec![(
            "org.example.Tree".to_string(),
            "Tree".to_string(),
            "flathub".to_string(),
        )];
        let c = cand();
        let (rows, chosen) = decide("apt", Some(&c), &fp, true, None);
        let r = resolution("tree", Some(&c), &fp, &rows, chosen);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["name"], "tree");
        assert_eq!(v["chosen"], "repo");
        let cs = v["candidates"].as_array().unwrap();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0]["lane"], "repo");
        assert_eq!(cs[0]["source"], "main");
        assert_eq!(cs[0]["version"], "2.2.1-1");
        assert_eq!(cs[0]["found"], true);
        assert_eq!(cs[1]["lane"], "flatpak");
        assert_eq!(cs[1]["source"], "flathub");
        assert_eq!(cs[1]["note"], "org.example.Tree (Tree)");
        // nothing found: the lanes are still listed, and nothing is chosen
        let (rows, chosen) = decide("dnf", None, &[], false, None);
        let v = serde_json::to_value(resolution("x", None, &[], &rows, chosen)).unwrap();
        assert!(v["chosen"].is_null());
        assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(v["candidates"][1]["found"], false);
    }

    #[test]
    fn flatpak_rows_match_exactly() {
        let text = "org.mozilla.firefox\tFirefox\t131.0\tflathub\norg.example.FirefoxHelper\tFirefox Helper\t1\tflathub\nbroken line\n";
        let m = flatpak_matches(text, "firefox");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, "org.mozilla.firefox");
        assert_eq!(m[0].2, "flathub");
    }
}
