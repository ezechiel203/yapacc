// yapacc — `why`: where did this come from, and why is it here?
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Read-only, works on a machine that has never used yapacc before: native
//! facts come from the native package manager, flatpak facts from flatpak,
//! and the ledger adds what only yapacc knows (which interface installed
//! it, from which source, verified how). A package installed outside yapacc
//! is answered just as well — it simply has no ledger part.

use crate::error::{Error, Result};
use crate::ledger::{self, InstallRecord, Scope};
use crate::native::{self, NativeFact, Reason};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
pub struct FlatpakFact {
    pub application: String,
    pub name: String,
    pub version: String,
    pub origin: String,
    pub installation: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WhyReport {
    pub query: String,
    pub native: Option<NativeFact>,
    /// For a dependency: shortest paths from this package up to packages
    /// the user asked for (`[this, parent, …, explicit]`). Empty when the
    /// package is explicit, orphaned, or the search budget ran out.
    pub chains: Vec<Vec<String>>,
    /// For a dependency nothing installed depends on: whether the package
    /// manager's own orphan list (what `inspect orphans` prints) holds it.
    /// `None`: not such a package, or the list could not be read.
    pub orphan: Option<bool>,
    pub flatpak: Vec<FlatpakFact>,
    pub ledger: Vec<InstallRecord>,
    /// things that could not be checked (reported, never hidden)
    pub notes: Vec<String>,
}

impl WhyReport {
    pub fn found(&self) -> bool {
        self.native.is_some() || !self.flatpak.is_empty() || !self.ledger.is_empty()
    }
}

/// Parse `flatpak list --columns=application,name,version,origin,installation`.
pub fn parse_flatpak_list(text: &str, query: &str) -> Vec<FlatpakFact> {
    let q = query.to_lowercase();
    text.lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').map(|x| x.trim()).collect();
            (c.len() >= 5 && !c[0].is_empty()).then(|| FlatpakFact {
                application: c[0].into(),
                name: c[1].into(),
                version: c[2].into(),
                origin: c[3].into(),
                installation: c[4].into(),
            })
        })
        .filter(|f| {
            let app = f.application.to_lowercase();
            app == q || app.rsplit('.').next() == Some(q.as_str()) || f.name.to_lowercase() == q
        })
        .collect()
}

/// How many packages `chains` may look up, and how many paths it reports.
const CHAIN_BUDGET: usize = 40;
const CHAIN_WANT: usize = 3;
/// How many names of a long list are printed before "… and N more".
const LIST_SHOWN: usize = 12;

/// Shortest paths from `start` up to explicitly installed packages,
/// breadth first. `first` is what directly needs `start`; `parents` answers
/// the same question for any other package and is asked at most `budget`
/// times. Pure: the package manager is behind `parents`.
pub fn chains(
    start: &str,
    first: &[String],
    explicit: &HashSet<String>,
    mut parents: impl FnMut(&str) -> Vec<String>,
    mut budget: usize,
    want: usize,
) -> Vec<Vec<String>> {
    let mut found: Vec<Vec<String>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([start.to_string()]);
    let mut queue: VecDeque<Vec<String>> = VecDeque::new();
    for p in first {
        if seen.insert(p.clone()) {
            queue.push_back(vec![start.to_string(), p.clone()]);
        }
    }
    while let Some(path) = queue.pop_front() {
        if found.len() >= want {
            break;
        }
        let last = path.last().cloned().unwrap_or_default();
        if explicit.contains(&last) {
            found.push(path);
            continue;
        }
        if budget == 0 {
            continue;
        }
        budget -= 1;
        for q in parents(&last) {
            if seen.insert(q.clone()) {
                let mut next = path.clone();
                next.push(q);
                queue.push_back(next);
            }
        }
    }
    found
}

/// The chains for an installed dependency, asked of the native backend.
fn explain(b: &dyn native::NativeBackend, f: &NativeFact) -> Vec<Vec<String>> {
    if f.reason != Reason::Dependency || f.required_by.is_empty() {
        return vec![];
    }
    let Ok(explicit) = b.explicit() else {
        return vec![];
    };
    let explicit: HashSet<String> = explicit.into_iter().collect();
    chains(
        &f.name,
        &f.required_by,
        &explicit,
        // only "what requires it" is needed of an ancestor: the full
        // fact costs apt two cache loads per package
        |name| b.required_by(name).unwrap_or_default(),
        CHAIN_BUDGET,
        CHAIN_WANT,
    )
}

/// Is package `name` in the manager's orphan list? apt writes a package of
/// a foreign architecture there as `libdeflate0:i386`, and that spelling
/// is kept in the list because `remove orphans` passes it on. Pure.
pub fn in_orphans(orphans: &[String], name: &str) -> bool {
    orphans
        .iter()
        .any(|o| o == name || o.split_once(':').is_some_and(|(bare, _)| bare == name))
}

/// A long list of names, shortened for people: the first few, then a count.
pub fn short_list(names: &[String]) -> String {
    if names.len() <= LIST_SHOWN {
        return names.join(", ");
    }
    format!(
        "{} installed packages: {}, … and {} more (--json lists all)",
        names.len(),
        names[..LIST_SHOWN].join(", "),
        names.len() - LIST_SHOWN
    )
}

/// What a flatpak query of a read command may be limited to, given which
/// installations exist: `None`, do not ask (there is none); `--system` when
/// the user has none; no option when the user's exists. flatpak creates
/// the user's installation (`~/.local/share/flatpak/repo/...`) when a
/// command looks at it, and a read command writes nothing. Pure.
pub fn flatpak_scope(user: bool, system: bool) -> Option<&'static [&'static str]> {
    match (user, system) {
        (true, _) => Some(&[]),
        (false, true) => Some(&["--system"]),
        (false, false) => None,
    }
}

/// [`flatpak_scope`] for this machine and this user; `None` also when
/// `flatpak` is not installed.
pub fn flatpak_read_scope() -> Option<&'static [&'static str]> {
    let user = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .is_some_and(|d| d.join("flatpak").is_dir());
    let system = std::path::Path::new("/var/lib/flatpak").is_dir();
    flatpak_scope(user, system).filter(|_| crate::util::have("flatpak"))
}

pub fn collect(name: &str) -> WhyReport {
    let mut report = WhyReport {
        query: name.to_string(),
        native: None,
        chains: vec![],
        orphan: None,
        flatpak: vec![],
        ledger: vec![],
        notes: vec![],
    };
    match crate::platform::native_manager().1 {
        Some(m) => match native::backend(m) {
            Ok(b) => match b.fact(name) {
                Ok(mut f) => {
                    report.chains = f
                        .as_ref()
                        .map(|f| explain(b.as_ref(), f))
                        .unwrap_or_default();
                    if let Some(f) = f.as_mut() {
                        // zypper: where the package is offered from, asked
                        // once here (not for every package of a chain)
                        if m == crate::platform::PkgManager::Zypper && f.origin.is_empty() {
                            match native::zypper::origin(&f.name) {
                                Ok((origin, foreign)) => (f.origin, f.foreign) = (origin, foreign),
                                Err(e) => report.notes.push(e.message),
                            }
                        }
                        // "orphan" is what the manager's own list says, not
                        // what "nothing depends on it" suggests
                        if f.reason == Reason::Dependency
                            && f.required_by.is_empty()
                            && (!f.essential || f.base == "priority required")
                            && crate::cli::orphans_unknown(b.as_ref()).is_none()
                        {
                            report.orphan = b.orphans().ok().map(|list| in_orphans(&list, &f.name));
                        }
                    }
                    report.native = f;
                }
                Err(e) => report.notes.push(e.message),
            },
            Err(e) => report.notes.push(e.message),
        },
        None => report
            .notes
            .push(crate::platform::no_native_manager_message()),
    }
    let flatpak_args = [
        "list",
        "--columns=application,name,version,origin,installation",
    ];
    if let Some(scope) = flatpak_read_scope()
        && let Ok(out) = Command::new("flatpak")
            // the command first: `--system` is an option of `list`
            .args(flatpak_args)
            .args(scope)
            .env("LC_ALL", "C")
            .output()
        && out.status.success()
    {
        report.flatpak = parse_flatpak_list(&String::from_utf8_lossy(&out.stdout), name);
    }
    for scope in [Scope::User, Scope::System] {
        match ledger::list_records(&ledger::default_root(&scope)) {
            Ok(recs) => report.ledger.extend(
                recs.into_iter()
                    .filter(|r| r.status != "removed" && (r.name == name || r.id == name)),
            ),
            Err(e) => report.notes.push(format!("ledger unreadable: {e}")),
        }
    }
    report
}

/// The "why" line of a native package. `orphan` is whether the package
/// manager's own orphan list holds it (`None`: not asked or not known).
/// Pure.
pub fn reason_line(f: &NativeFact, orphan: Option<bool>) -> String {
    match (&f.reason, f.required_by.is_empty()) {
        // apt: `Essential: yes` is protected by the package manager;
        // `Priority: required` alone is not, and `apt-get autoremove`
        // removes such a package when it lists it as an orphan
        _ if f.essential && f.base == "priority required" && orphan == Some(true) => {
            "installed as a dependency, but nothing needs it any more (orphan; its priority `required` does not protect it from `apt-get autoremove`)".to_string()
        }
        _ if f.essential && f.base == "priority required" => {
            "part of the base system (priority required)".to_string()
        }
        _ if f.essential => "part of the base system (the package manager protects it)".to_string(),
        (Reason::Explicit, _) => {
            "you (or a tool acting for you) installed it explicitly".to_string()
        }
        (Reason::Dependency, false) => "pulled in as a dependency".to_string(),
        // nothing depends on it, and the manager would still not remove it:
        // not an orphan (`inspect orphans` does not list it either)
        (Reason::Dependency, true) if orphan == Some(false) => format!(
            "installed as a dependency; nothing depends on it, but {} keeps it ({})",
            f.manager,
            if f.manager == "apt" {
                "another package recommends or suggests it"
            } else {
                "it is not in the list of `yapacc inspect orphans`"
            }
        ),
        (Reason::Dependency, true) => {
            "installed as a dependency, but nothing needs it any more (orphan)".to_string()
        }
        (Reason::Unknown, _) => format!("{} does not record an install reason here", f.manager),
    }
}

pub fn render(r: &WhyReport) -> String {
    let mut s = String::new();
    if let Some(f) = &r.native {
        s.push_str(&format!(
            "{} {}  — native package ({})\n",
            f.name, f.version, f.manager
        ));
        if !f.description.is_empty() {
            s.push_str(&format!("  what         {}\n", f.description));
        }
        s.push_str(&format!("  why          {}\n", reason_line(f, r.orphan)));
        for (i, chain) in r.chains.iter().enumerate() {
            s.push_str(&format!(
                "  {:<12} {}  (you installed {})\n",
                if i == 0 { "because" } else { "" },
                chain.join(" ← "),
                chain.last().map(String::as_str).unwrap_or("")
            ));
        }
        s.push_str(&format!(
            "  needed by    {}\n",
            if f.required_by.is_empty() {
                "nothing installed".to_string()
            } else {
                short_list(&f.required_by)
            }
        ));
        // never an empty value: say that it is not known
        if f.origin.is_empty() {
            s.push_str(&format!(
                "  from         unknown: {} did not say where it came from\n",
                f.manager
            ));
        } else {
            s.push_str(&format!("  from         {}\n", f.origin));
        }
        if !f.installed.is_empty() {
            s.push_str(&format!("  installed    {}\n", f.installed));
        }
        if !f.verified.is_empty() {
            s.push_str(&format!("  verified     {}\n", f.verified));
        }
        if !f.packager.is_empty() {
            s.push_str(&format!("  packager     {}\n", f.packager));
        }
    }
    for f in &r.flatpak {
        s.push_str(&format!(
            "{} {}  — flatpak ({})\n  from         remote '{}', {} installation\n",
            f.application, f.version, f.name, f.origin, f.installation
        ));
    }
    for rec in &r.ledger {
        s.push_str(&format!(
            "{} {}  — yapacc ledger record ({}, {} scope)\n  from         {}\n  into         {}\n",
            rec.name, rec.version, rec.format, rec.scope, rec.source, rec.install_root
        ));
        if let Some(via) = rec.metadata.get("via") {
            s.push_str(&format!("  installed by yapacc through: {via}\n"));
        }
        for key in ["verified", "sha256", "signature", "lane"] {
            if let Some(v) = rec.metadata.get(key) {
                s.push_str(&format!("  {key:<12} {v}\n"));
            }
        }
    }
    if r.found() && r.ledger.is_empty() {
        s.push_str(
            "  ledger       no yapacc record (installed outside yapacc, or by a native lane) — the facts above come straight from the package manager\n",
        );
    }
    for n in &r.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

pub fn run(name: &str, json: bool) -> Result<()> {
    if !crate::util::valid_pkg_name(name) {
        return Err(Error::usage(format!(
            "'{name}' is not a valid package name"
        )));
    }
    let report = collect(name);
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => return Err(Error::from(format!("json serialization failed: {e}"))),
        }
    } else if report.found() {
        print!("{}", render(&report));
    }
    if report.found() {
        Ok(())
    } else {
        Err(Error::not_found(format!(
            "'{name}' is not installed as a native package, a flatpak, or a ledger-tracked install{}",
            if report.notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", report.notes.join("; "))
            }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_rows_match_id_suffix_or_name() {
        let text = "org.mozilla.firefox\tFirefox\t131.0\tflathub\tsystem\norg.gimp.GIMP\tGNU Image Manipulation Program\t2.10\tflathub\tuser\n";
        assert_eq!(parse_flatpak_list(text, "firefox").len(), 1);
        assert_eq!(
            parse_flatpak_list(text, "org.gimp.GIMP")[0].installation,
            "user"
        );
        assert_eq!(parse_flatpak_list(text, "gimp").len(), 1);
        assert!(parse_flatpak_list(text, "vlc").is_empty());
    }

    #[test]
    fn chains_reach_the_nearest_explicit_packages() {
        let explicit: HashSet<String> = ["base", "firefox"].map(String::from).into();
        let parents = |n: &str| -> Vec<String> {
            match n {
                "glib2" => vec!["gtk3".into()],
                "gtk3" => vec!["firefox".into(), "glib2".into()],
                _ => vec![],
            }
        };
        let got = chains(
            "bash",
            &["base".into(), "glib2".into()],
            &explicit,
            parents,
            40,
            3,
        );
        assert_eq!(
            got,
            vec![
                vec!["bash".to_string(), "base".into()],
                vec![
                    "bash".into(),
                    "glib2".into(),
                    "gtk3".into(),
                    "firefox".into()
                ],
            ]
        );
        // no budget: only what is already known is reported
        assert_eq!(
            chains("bash", &["glib2".into()], &explicit, parents, 0, 3),
            Vec::<Vec<String>>::new()
        );
        // a cycle ends
        let loops = |_: &str| vec!["a".to_string(), "b".to_string()];
        assert!(chains("x", &["a".into()], &HashSet::new(), loops, 40, 3).is_empty());
    }

    /// On apt the full fact of a package costs two loads of apt's cache;
    /// `why libc6` in a debian:stable container (no binary cache) asked it
    /// of forty ancestors and took 68 seconds. The walk up the chain asks
    /// only what requires each one.
    #[test]
    fn the_chain_walk_asks_only_what_requires_each_ancestor() {
        use crate::error::Error;
        use crate::native::{Candidate, Installed, NativeBackend, Update};
        struct Walk;
        impl NativeBackend for Walk {
            fn manager(&self) -> crate::platform::PkgManager {
                crate::platform::PkgManager::Apt
            }
            fn installed(&self) -> std::result::Result<Vec<Installed>, Error> {
                Ok(vec![])
            }
            fn explicit(&self) -> std::result::Result<Vec<String>, Error> {
                Ok(vec!["tree".into()])
            }
            fn fact(&self, name: &str) -> std::result::Result<Option<NativeFact>, Error> {
                panic!("the full fact of {name} was asked for");
            }
            fn required_by(&self, name: &str) -> std::result::Result<Vec<String>, Error> {
                Ok(match name {
                    "libb" => vec!["tree".into()],
                    _ => vec![],
                })
            }
            fn candidate(&self, _: &str) -> std::result::Result<Option<Candidate>, Error> {
                Ok(None)
            }
            fn search(&self, _: &str) -> std::result::Result<Vec<Candidate>, Error> {
                Ok(vec![])
            }
            fn owner_of(&self, _: &str) -> std::result::Result<Option<String>, Error> {
                Ok(None)
            }
            fn files(&self, _: &str) -> std::result::Result<Vec<String>, Error> {
                Ok(vec![])
            }
            fn orphans(&self) -> std::result::Result<Vec<String>, Error> {
                Ok(vec![])
            }
            fn updates(&self) -> std::result::Result<Vec<Update>, Error> {
                Ok(vec![])
            }
        }
        let mut f = apt_fact("liba", Reason::Dependency, false, "");
        f.required_by = vec!["libb".into()];
        assert_eq!(explain(&Walk, &f), vec![vec!["liba", "libb", "tree"]]);
    }

    /// `yapacc clean` with a HOME that had no flatpak directory left
    /// `~/.local/share/flatpak/repo/{config,objects,refs,...}` behind.
    #[test]
    fn flatpak_is_not_asked_about_an_installation_that_does_not_exist() {
        assert_eq!(flatpak_scope(false, false), None);
        assert_eq!(flatpak_scope(false, true), Some(&["--system"][..]));
        assert_eq!(flatpak_scope(true, true), Some(&[][..]));
        assert_eq!(flatpak_scope(true, false), Some(&[][..]));
    }

    /// debian:stable with i386 added: `apt-get -s autoremove` named
    /// `libdeflate0:i386`, and `why libdeflate0` said "apt keeps it (another
    /// package recommends or suggests it)".
    #[test]
    fn an_orphan_of_a_foreign_architecture_is_recognised() {
        let orphans = vec!["libdeflate0:i386".to_string(), "liblerc4".to_string()];
        assert!(in_orphans(&orphans, "libdeflate0"));
        assert!(in_orphans(&orphans, "liblerc4"));
        assert!(!in_orphans(&orphans, "libdeflate"));
        assert!(!in_orphans(&orphans, "i386"));
    }

    #[test]
    fn long_lists_are_shortened() {
        let few: Vec<String> = (0..3).map(|i| format!("p{i}")).collect();
        assert_eq!(short_list(&few), "p0, p1, p2");
        let many: Vec<String> = (0..120).map(|i| format!("p{i}")).collect();
        let s = short_list(&many);
        assert!(s.starts_with("120 installed packages: p0, p1"), "{s}");
        assert!(s.ends_with("and 108 more (--json lists all)"), "{s}");
    }

    #[test]
    fn orphan_dependency_is_called_out() {
        let r = WhyReport {
            query: "libfoo".into(),
            native: Some(NativeFact {
                manager: "pacman".into(),
                name: "libfoo".into(),
                version: "1-1".into(),
                description: String::new(),
                reason: Reason::Dependency,
                required_by: vec![],
                origin: "extra (sync repository)".into(),
                foreign: Some(false),
                essential: false,
                base: String::new(),
                installed: String::new(),
                verified: "Signature".into(),
                packager: String::new(),
            }),
            chains: vec![],
            orphan: Some(true),
            flatpak: vec![],
            ledger: vec![],
            notes: vec![],
        };
        let out = render(&r);
        assert!(out.contains("orphan"), "{out}");
        assert!(out.contains("no yapacc record"), "{out}");
        // not known whether the manager lists it: the old wording stays
        let unknown = WhyReport {
            orphan: None,
            ..r.clone()
        };
        assert!(render(&unknown).contains("(orphan)"));
    }

    fn apt_fact(name: &str, reason: Reason, essential: bool, base: &str) -> NativeFact {
        NativeFact {
            manager: "apt".into(),
            name: name.into(),
            version: "1".into(),
            description: String::new(),
            reason,
            required_by: vec![],
            origin: String::new(),
            foreign: None,
            essential,
            base: base.into(),
            installed: String::new(),
            verified: String::new(),
            packager: String::new(),
        }
    }

    /// P2: `why perl` said "(orphan)" for a package `apt-get -s autoremove`
    /// keeps and `inspect orphans` does not list.
    #[test]
    fn a_dependency_the_manager_keeps_is_not_called_an_orphan() {
        let perl = apt_fact("perl", Reason::Dependency, false, "");
        assert_eq!(
            reason_line(&perl, Some(false)),
            "installed as a dependency; nothing depends on it, but apt keeps it (another package recommends or suggests it)"
        );
        assert_eq!(
            reason_line(&perl, Some(true)),
            "installed as a dependency, but nothing needs it any more (orphan)"
        );
        let mut rpm = perl.clone();
        rpm.manager = "dnf".into();
        assert_eq!(
            reason_line(&rpm, Some(false)),
            "installed as a dependency; nothing depends on it, but dnf keeps it (it is not in the list of `yapacc inspect orphans`)"
        );
        // something needs it: the orphan list is not consulted
        let mut needed = perl.clone();
        needed.required_by = vec!["cowsay".into()];
        assert_eq!(
            reason_line(&needed, Some(false)),
            "pulled in as a dependency"
        );
    }

    /// P2: `why libtext-charwidth-perl` (Priority: required, not Essential)
    /// said "the package manager protects it".
    #[test]
    fn priority_required_is_not_called_protected() {
        let required = apt_fact(
            "libtext-charwidth-perl",
            Reason::Dependency,
            true,
            "priority required",
        );
        assert_eq!(
            reason_line(&required, None),
            "part of the base system (priority required)"
        );
        assert_eq!(
            reason_line(&required, Some(false)),
            "part of the base system (priority required)"
        );
        // Debian 13, after `apt-get remove cowsay`: `apt-get -s autoremove`
        // lists it, and so does `inspect orphans`
        assert_eq!(
            reason_line(&required, Some(true)),
            "installed as a dependency, but nothing needs it any more (orphan; its priority `required` does not protect it from `apt-get autoremove`)"
        );
        let essential = apt_fact("bash", Reason::Explicit, true, "essential");
        assert_eq!(
            reason_line(&essential, None),
            "part of the base system (the package manager protects it)"
        );
    }

    /// Z5: an empty `from` line.
    #[test]
    fn an_origin_that_is_not_known_is_said_not_left_empty() {
        let r = WhyReport {
            query: "x".into(),
            native: Some(apt_fact("x", Reason::Explicit, false, "")),
            chains: vec![],
            orphan: None,
            flatpak: vec![],
            ledger: vec![],
            notes: vec![],
        };
        let out = render(&r);
        assert!(
            out.contains("  from         unknown: apt did not say where it came from\n"),
            "{out}"
        );
        assert!(!out.contains("  from         \n"), "{out}");
    }
}
