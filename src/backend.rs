use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
pub struct PackageSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub installed_size: String,
    pub reason: String,
    pub repo: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub url: String,
    pub licenses: String,
    pub architecture: String,
    pub provides: String,
    pub depends: String,
    pub optional_deps: String,
    pub required_by: String,
    pub optional_for: String,
    pub installed_size: String,
    pub packager: String,
    pub build_date: String,
    pub install_date: String,
    pub install_reason: String,
    pub install_script: String,
    pub validated_by: String,
    pub is_foreign: bool,
    pub is_explicit: bool,
    pub is_leaf: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemovePlan {
    pub requested: Vec<String>,
    pub requested_found: Vec<String>,
    pub not_found: Vec<String>,
    pub direct_dependents: Vec<(String, Vec<String>)>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchResult {
    pub repo: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub installed_size: Option<String>,
    pub download_size: Option<String>,
    pub source: String,
    pub popularity: Option<String>,
    pub out_of_date: Option<String>,
    pub maintainer: Option<String>,
    pub purpose: String,
    pub kind: String,
    pub pkg_type: String,
    pub size_class: String,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

/// Structured update-check data (monitor/pipeline consume this; no text re-parsing).
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateCheckData {
    pub repo_updates: Vec<String>,
    pub aur_updates: Vec<(String, String, String)>, // (name, installed, available)
    pub repo_check_ok: bool,
    pub aur_check_ok: bool,
    /// why the repositories could not be asked, when no sync database is on
    /// disk (a fresh system before its first `pacman -Sy`); `pacman -Qu`
    /// then prints nothing, which is not "no update"
    #[serde(skip)]
    pub no_sync: Option<String>,
}

/// "failed to run X", and where X comes from when it is a tool Arch splits
/// out of the base install.
fn spawn_error(cmd: &str, e: &std::io::Error) -> String {
    let hint = match (cmd, e.kind()) {
        ("pactree", std::io::ErrorKind::NotFound) => {
            " (pactree is in the pacman-contrib package: `pacman -S pacman-contrib`)"
        }
        _ => "",
    };
    format!("failed to run {cmd}: {e}{hint}")
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| spawn_error(cmd, &e))?;
    if !out.status.success() && out.stdout.is_empty() {
        if is_empty_query_result(cmd, &out.stderr) {
            return Ok(String::new());
        }
        return Err(if out.stderr.is_empty() {
            format!("{cmd} exited with {}", out.status)
        } else {
            let msg = String::from_utf8_lossy(&out.stderr);
            let msg = msg.trim();
            // The caller's printer adds its own "error: ".
            msg.strip_prefix("error: ").unwrap_or(msg).to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
/// `pacman -Q...` and `-Ss` exit 1 with no output at all when nothing matches
/// (no foreign packages, no orphans, no hit). That is an empty answer, not a
/// failure: a real failure writes a message to stderr.
///
/// Before the first `pacman -Sy` (a fresh archlinux container) pacman also
/// writes, for every query, one line per repository:
/// `warning: database file for 'core' does not exist (use '-Sy' to download)`.
/// Warnings are not a failure either: `inspect orphans` with no orphan
/// ended with those lines as its error.
fn is_empty_query_result(cmd: &str, stderr: &[u8]) -> bool {
    cmd == "pacman"
        && String::from_utf8_lossy(stderr)
            .lines()
            .all(|l| l.trim().is_empty() || l.starts_with("warning: "))
}

/// The `reason` and `repo` columns of one row of `list`. `foreign` is
/// `None` when it cannot be told: without a sync database on disk
/// `pacman -Qm` lists every package, and none of them is called foreign
/// on that ground. Pure.
pub fn reason_and_repo(explicit: bool, foreign: Option<bool>) -> (String, String) {
    let reason = if explicit { "explicit" } else { "dep" };
    match foreign {
        Some(true) => (format!("{reason},foreign"), "local/aur".into()),
        Some(false) => (reason.into(), "repo".into()),
        None => (reason.into(), "unknown".into()),
    }
}

fn run_status(cmd: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    let out = Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| spawn_error(cmd, &e))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}
fn lines_set(cmd: &str, args: &[&str]) -> Result<HashSet<String>, String> {
    Ok(run(cmd, args)?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn parse_qi_blocks(text: &str) -> Vec<HashMap<String, String>> {
    let mut blocks = Vec::new();
    let mut cur: HashMap<String, String> = HashMap::new();
    let mut last_key: Option<String> = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key == "Name" && cur.contains_key("Name") {
                blocks.push(std::mem::take(&mut cur));
            }
            cur.insert(key.clone(), val);
            last_key = Some(key);
        } else if (line.starts_with(' ') || line.starts_with('\t')) && last_key.is_some() {
            if let Some(k) = &last_key {
                let e = cur.entry(k.clone()).or_default();
                if !e.is_empty() {
                    e.push(' ');
                }
                e.push_str(line.trim());
            }
        } else if line.trim().is_empty() {
            // ignore blank separators
        }
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    blocks
}

/// Is `name` a package group with at least one member installed
/// (`pacman -Qgq NAME`)? `pacman -Rs` takes such a name for its members.
pub fn installed_group(name: &str) -> bool {
    run_status("pacman", &["-Qgq", "--", name])
        .map(|(ok, out, _)| ok && !out.trim().is_empty())
        .unwrap_or(false)
}

/// Every installed package with its version (`pacman -Q`).
pub fn installed_version_map() -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    for line in run("pacman", &["-Q"])?.lines() {
        let mut it = line.split_whitespace();
        if let (Some(n), Some(v)) = (it.next(), it.next()) {
            m.insert(n.to_string(), v.to_string());
        }
    }
    Ok(m)
}

pub fn list_installed(
    foreign_only: bool,
    explicit_only: bool,
) -> Result<Vec<PackageSummary>, String> {
    // "foreign" is "in no sync database": not known while there is none
    let no_sync = crate::native::missing_metadata(crate::platform::PkgManager::Pacman);
    if foreign_only && let Some(why) = &no_sync {
        return Err(format!("which packages are foreign cannot be told: {why}"));
    }
    let foreign = match no_sync {
        Some(_) => None,
        None => Some(lines_set("pacman", &["-Qmq"])?),
    };
    let explicit = lines_set("pacman", &["-Qeq"])?;
    let mut out = Vec::new();
    for b in parse_qi_blocks(&run("pacman", &["-Qi"])?) {
        let name = b.get("Name").cloned().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let is_foreign = foreign.as_ref().map(|set| set.contains(&name));
        let is_explicit = explicit.contains(&name);
        if foreign_only && is_foreign != Some(true) {
            continue;
        }
        if explicit_only && !is_explicit {
            continue;
        }
        let (reason, repo) = reason_and_repo(is_explicit, is_foreign);
        out.push(PackageSummary {
            name,
            version: b.get("Version").cloned().unwrap_or_default(),
            description: b.get("Description").cloned().unwrap_or_default(),
            installed_size: b.get("Installed Size").cloned().unwrap_or_default(),
            reason,
            repo,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn list_leaves() -> Result<Vec<PackageSummary>, String> {
    let leaves = lines_set("pacman", &["-Qetq"])?;
    let mut out = Vec::new();
    for b in parse_qi_blocks(&run("pacman", &["-Qi"])?) {
        let name = b.get("Name").cloned().unwrap_or_default();
        if !leaves.contains(&name) {
            continue;
        }
        out.push(PackageSummary {
            name,
            version: b.get("Version").cloned().unwrap_or_default(),
            description: b.get("Description").cloned().unwrap_or_default(),
            installed_size: b.get("Installed Size").cloned().unwrap_or_default(),
            reason: "explicit,leaf".into(),
            repo: "".into(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn package_info(name: &str) -> Result<PackageInfo, String> {
    let foreign = lines_set("pacman", &["-Qmq"])?;
    let explicit = lines_set("pacman", &["-Qeq"])?;
    let leaves = lines_set("pacman", &["-Qetq"])?;
    let blocks = parse_qi_blocks(&run("pacman", &["-Qi", "--", name])?);
    let b = blocks
        .into_iter()
        .find(|b| b.get("Name").map(|s| s.as_str()) == Some(name))
        .ok_or_else(|| format!("package not found (installed): {name}"))?;
    Ok(PackageInfo {
        name: b.get("Name").cloned().unwrap_or_default(),
        version: b.get("Version").cloned().unwrap_or_default(),
        description: b.get("Description").cloned().unwrap_or_default(),
        url: b.get("URL").cloned().unwrap_or_default(),
        licenses: b.get("Licenses").cloned().unwrap_or_default(),
        architecture: b.get("Architecture").cloned().unwrap_or_default(),
        provides: b.get("Provides").cloned().unwrap_or_default(),
        depends: b.get("Depends On").cloned().unwrap_or_default(),
        optional_deps: b.get("Optional Deps").cloned().unwrap_or_default(),
        required_by: b.get("Required By").cloned().unwrap_or_default(),
        optional_for: b.get("Optional For").cloned().unwrap_or_default(),
        installed_size: b.get("Installed Size").cloned().unwrap_or_default(),
        packager: b.get("Packager").cloned().unwrap_or_default(),
        build_date: b.get("Build Date").cloned().unwrap_or_default(),
        install_date: b.get("Install Date").cloned().unwrap_or_default(),
        install_reason: b.get("Install Reason").cloned().unwrap_or_default(),
        install_script: b.get("Install Script").cloned().unwrap_or_default(),
        validated_by: b.get("Validated By").cloned().unwrap_or_default(),
        is_foreign: foreign.contains(name),
        is_explicit: explicit.contains(name),
        is_leaf: leaves.contains(name),
    })
}

pub fn remove_plan(targets: &[String]) -> Result<RemovePlan, String> {
    let installed = lines_set("pacman", &["-Qq"])?;
    let mut requested = targets.to_vec();
    requested.sort();
    requested.dedup();
    let mut requested_found = Vec::new();
    let mut not_found = Vec::new();
    for t in &requested {
        if installed.contains(t) {
            requested_found.push(t.clone());
        } else {
            not_found.push(t.clone());
        }
    }
    let mut direct_dependents = Vec::new();
    let mut warnings = Vec::new();
    for t in &requested_found {
        let blocks = parse_qi_blocks(&run("pacman", &["-Qi", "--", t])?);
        if let Some(b) = blocks
            .into_iter()
            .find(|b| b.get("Name").map(|s| s.as_str()) == Some(t.as_str()))
        {
            let req = b.get("Required By").cloned().unwrap_or_default();
            if req != "None" && !req.trim().is_empty() {
                let deps: Vec<String> = req.split_whitespace().map(|s| s.to_string()).collect();
                if !deps.is_empty() {
                    warnings.push(format!("{t} is required by {} package(s)", deps.len()));
                    direct_dependents.push((t.clone(), deps));
                }
            }
            let reason = b.get("Install Reason").cloned().unwrap_or_default();
            if reason.contains("dependency") {
                warnings.push(format!(
                    "{t} was installed as a dependency (review before removal)"
                ));
            }
        }
    }
    // Best-effort ALPM prepare impact (conflicts/dependency cascade visibility);
    // without libalpm or without root, pacman's own simulation shows it.
    if alpm_analysis_available() && not_found.is_empty() && !requested_found.is_empty() {
        match crate::alpm_flows::plan_remove(&requested_found) {
            Ok(plan) => {
                if !plan.to_remove.is_empty() {
                    warnings.push(format!(
                        "ALPM prepare impact: would remove {} package(s): {}",
                        plan.to_remove.len(),
                        plan.to_remove.join(", ")
                    ));
                }
                if !plan.to_install.is_empty() {
                    warnings.push(format!(
                        "ALPM prepare impact: would install/pull {} package(s): {}",
                        plan.to_install.len(),
                        plan.to_install.join(", ")
                    ));
                }
                for n in &plan.notes {
                    warnings.push(format!("ALPM: {n}"));
                }
            }
            Err(e) => {
                warnings.push(format!("ALPM prepare analysis unavailable/conflict: {e}"));
            }
        }
    }
    Ok(RemovePlan {
        requested,
        requested_found,
        not_found,
        direct_dependents,
        warnings,
    })
}

fn parse_size_to_mib(s: &str) -> Option<f64> {
    let mut it = s.split_whitespace();
    let num: f64 = it.next()?.parse().ok()?;
    let unit = it.next().unwrap_or("");
    let mib = match unit {
        "B" => num / (1024.0 * 1024.0),
        "KiB" => num / 1024.0,
        "MiB" => num,
        "GiB" => num * 1024.0,
        "TiB" => num * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(mib)
}
fn size_class_from(s: Option<&str>) -> String {
    let Some(v) = s.and_then(parse_size_to_mib) else {
        return "unknown".into();
    };
    if v < 0.1 {
        "tiny(<0.1MiB)".into()
    } else if v < 1.0 {
        "small(0.1-1MiB)".into()
    } else if v < 10.0 {
        "medium(1-10MiB)".into()
    } else if v < 100.0 {
        "large(10-100MiB)".into()
    } else {
        "huge(>100MiB)".into()
    }
}

/// Keyword-heuristic classification shared by search/index/facets.
pub fn classify(
    name: &str,
    desc: &str,
    source: &str,
    keywords: &[String],
    groups: &[String],
) -> (String, String, String, Vec<String>) {
    let lname = name.to_lowercase();
    let ldesc = desc.to_lowercase();
    let text = format!("{} {} {}", lname, ldesc, keywords.join(" ").to_lowercase());
    let mut cats: Vec<String> = Vec::new();
    let rules: &[(&str, &[&str])] = &[
        (
            "development",
            &[
                "compiler",
                "debug",
                "build",
                "sdk",
                "header",
                "devel",
                "cmake",
                "meson",
                "cargo",
                "rust",
                "python module",
                "toolchain",
                "llvm",
                "gcc",
                "linter",
                "formatter",
                "interpreter",
                "ide",
            ],
        ),
        (
            "library",
            &["library", "runtime library", "shared library", "bindings"],
        ),
        (
            "network",
            &[
                "http",
                "server",
                "proxy",
                "vpn",
                "dns",
                "network",
                "ssh",
                "ftp",
                "mail",
                "tcp",
                "wifi",
                "wireguard",
                "tailscale",
                "browser",
                "download",
            ],
        ),
        (
            "security",
            &[
                "crypto",
                "encryption",
                "password",
                "security",
                "firewall",
                "malware",
                "antivirus",
                "yara",
                "clamav",
                "signature",
                "auth",
                "tls",
                "ssl",
            ],
        ),
        (
            "multimedia",
            &[
                "audio", "video", "image", "codec", "ffmpeg", "mp3", "ogg", "png", "jpeg",
                "graphics", "3d", "viewer", "player", "photo",
            ],
        ),
        (
            "system",
            &[
                "kernel",
                "boot",
                "init",
                "daemon",
                "service",
                "systemd",
                "filesystem",
                "driver",
                "firmware",
                "monitor",
                "manager",
                "hardware",
            ],
        ),
        (
            "cli",
            &["command-line", "cli", "terminal", "shell", "console", "tui"],
        ),
        ("gui", &["gtk", "qt", "gui", "desktop", "x11", "wayland"]),
        (
            "database",
            &["database", "sql", "postgres", "mysql", "sqlite", "redis"],
        ),
        ("fonts", &["font", "typeface", "ttf", "otf"]),
        ("docs", &["documentation", "man pages", "manual"]),
        (
            "archive",
            &["archive", "compress", "zip", "tar", "extract", "backup"],
        ),
        (
            "containers",
            &["docker", "container", "podman", "oci", "virtualization"],
        ),
        ("games", &["game", "emulator"]),
        ("science", &["math", "scientific", "simulation", "analysis"]),
        (
            "office",
            &[
                "office",
                "document",
                "spreadsheet",
                "pdf",
                "word processor",
                "latex",
                "tex",
            ],
        ),
    ];
    for (cat, keys) in rules {
        if keys.iter().any(|k| text.contains(k)) {
            cats.push((*cat).to_string());
        }
    }
    for g in groups {
        if !g.is_empty() && g != "None" {
            cats.push(format!("group:{}", g.to_lowercase()));
        }
    }
    for k in keywords {
        if !k.is_empty() {
            cats.push(format!("kw:{}", k.to_lowercase()));
        }
    }
    if lname.ends_with("-git") {
        cats.push("variant:git".into());
    }
    if lname.ends_with("-bin") {
        cats.push("variant:bin".into());
    }
    if lname.ends_with("-appimage") {
        cats.push("variant:appimage".into());
    }
    if lname.starts_with("lib") {
        cats.push("type:library".into());
    }
    if lname.starts_with("ttf-") || lname.starts_with("otf-") {
        cats.push("type:font".into());
    }
    if source == "aur" {
        cats.push("source:aur".into());
    } else {
        cats.push("source:repo".into());
    }
    cats.sort();
    cats.dedup();
    if cats.is_empty() {
        cats.push("other".into());
    }
    let pkg_type = if lname.starts_with("lib")
        || ldesc.contains("library")
        || cats.iter().any(|c| c == "type:library")
    {
        "library".into()
    } else if cats.iter().any(|c| c == "type:font") {
        "font".into()
    } else if cats.iter().any(|c| c == "docs" || c == "type:docs") {
        "documentation".into()
    } else if lname.contains("kernel") || ldesc.contains("kernel") {
        "kernel/system".into()
    } else {
        "package".into()
    };
    let kind = if pkg_type == "library" {
        "library".into()
    } else if cats.iter().any(|c| c == "gui") {
        "gui-app".into()
    } else if cats.iter().any(|c| c == "cli") {
        "cli-tool".into()
    } else {
        "application/tool".into()
    };
    let purpose = if desc.is_empty() {
        "Unknown purpose".into()
    } else {
        desc.to_string()
    };
    (purpose, kind, pkg_type, cats)
}

/// Batched `pacman -Si` lookup for many names (one subprocess instead of one per result).
fn si_map_many(names: &[String]) -> HashMap<String, HashMap<String, String>> {
    if names.is_empty() {
        return HashMap::new();
    }
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let out = match Command::new("pacman")
        .args(["-Si", "--"])
        .args(&refs)
        .env("LC_ALL", "C")
        .output()
    {
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return HashMap::new(),
    };
    let mut m = HashMap::new();
    for b in parse_qi_blocks(&out) {
        if let Some(name) = b.get("Name") {
            m.insert(name.clone(), b);
        }
    }
    m
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

pub fn search_repo(query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let out = run("pacman", &["-Ss", "--", query])?;
    let installed = installed_version_map().unwrap_or_default();
    let mut results = Vec::new();
    let mut lines = out.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with(' ') && line.contains('/') {
            let mut parts = line.split_whitespace();
            let repo_name = parts.next().unwrap_or("");
            let version = parts.next().unwrap_or("").to_string();
            let rest: Vec<&str> = parts.collect();
            let installed_flag = rest.iter().any(|x| x.contains("installed"));
            let (repo, name) = repo_name.split_once('/').unwrap_or(("", repo_name));
            let mut desc = String::new();
            if let Some(dline) = lines.peek()
                && (dline.starts_with("    ") || dline.starts_with('\t'))
            {
                desc = dline.trim().to_string();
                lines.next();
            }
            let inst_ver = installed.get(name).cloned();
            results.push(SearchResult {
                repo: repo.to_string(),
                name: name.to_string(),
                version,
                description: desc,
                installed: installed_flag || inst_ver.is_some(),
                installed_version: inst_ver,
                installed_size: None,
                download_size: None,
                source: "repo".into(),
                popularity: None,
                out_of_date: None,
                maintainer: None,
                purpose: String::new(),
                kind: String::new(),
                pkg_type: String::new(),
                size_class: "unknown".into(),
                categories: vec![],
                keywords: vec![],
            });
        }
    }
    // enrich in one batched pacman -Si call (was: one subprocess per result)
    let names: Vec<String> = results.iter().map(|r| r.name.clone()).collect();
    let si = si_map_many(&names);
    for r in &mut results {
        if let Some(s) = si.get(&r.name) {
            let isize = s.get("Installed Size").cloned();
            let dsize = s.get("Download Size").cloned();
            let groups: Vec<String> = s
                .get("Groups")
                .cloned()
                .unwrap_or_default()
                .split_whitespace()
                .map(|x| x.to_string())
                .collect();
            let (purpose, kind, pkg_type, cats) =
                classify(&r.name, &r.description, "repo", &[], &groups);
            r.purpose = purpose;
            r.kind = kind;
            r.pkg_type = pkg_type;
            r.size_class = size_class_from(isize.as_deref().or(dsize.as_deref()));
            r.installed_size = isize;
            r.download_size = dsize;
            r.maintainer = s.get("Packager").cloned();
            r.categories = cats;
        } else {
            let (purpose, kind, pkg_type, cats) =
                classify(&r.name, &r.description, "repo", &[], &[]);
            r.purpose = purpose;
            r.kind = kind;
            r.pkg_type = pkg_type;
            r.categories = cats;
        }
    }
    results.sort_by(|a, b| {
        search_match_rank(query, &a.name, &a.description)
            .cmp(&search_match_rank(query, &b.name, &b.description))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    results.truncate(limit);
    Ok(results)
}

/// Smart search: direct query first; when there are no hits, deterministically
/// expand the query into a search goal (offline taxonomy intent map) and try
/// keyword variants + synonyms. Returns results plus a human explanation of
/// the expansion applied ("" when none needed). Fully offline/deterministic.
pub fn search_smart(query: &str, limit: usize) -> Result<(Vec<SearchResult>, String), String> {
    let direct = search_repo(query, limit).unwrap_or_default();
    if !direct.is_empty() {
        return Ok((direct, String::new()));
    }
    let goal = crate::taxonomy::parse_goal(query);
    if goal.keywords.is_empty() && goal.synonyms_applied.is_empty() {
        return Ok((
            Vec::new(),
            "no direct hits and no expansion applicable".into(),
        ));
    }
    // keyword variants: original significant tokens + intent synonyms
    let mut variants: Vec<String> = goal.keywords.clone();
    variants.extend(goal.synonyms_applied.iter().cloned());
    let mut merged: Vec<SearchResult> = Vec::new();
    for v in &variants {
        merged.extend(search_repo(v, limit).unwrap_or_default());
    }
    // category-guided preference: keep category-matching results first
    if !goal.categories.is_empty() {
        merged.sort_by_key(|r| {
            let hay = format!("{} {}", r.name, r.description).to_lowercase();
            let sub_hits = goal
                .categories
                .iter()
                .filter(|c| {
                    hay.contains(
                        c.rsplit('.')
                            .next()
                            .unwrap_or("")
                            .replace('-', " ")
                            .as_str(),
                    )
                })
                .count();
            std::cmp::Reverse(sub_hits)
        });
    }
    // dedup by repo/name
    let mut seen = std::collections::HashSet::new();
    merged.retain(|r| seen.insert(format!("{}/{}", r.repo, r.name)));
    merged.truncate(limit);
    let note = format!(
        "no direct hits for '{query}' — deterministic expansion: keywords [{}] synonyms [{}] facets [{}]",
        goal.keywords.join(","),
        goal.synonyms_applied.join(","),
        goal.categories.join(",")
    );
    Ok((merged, note))
}

/// AUR search via typed RPC client (serde_json; no jq dependency).
pub fn search_aur(query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let infos = crate::aur_rpc::fetch_search(query)?;
    let installed = installed_version_map().unwrap_or_default();
    let mut results = Vec::new();
    for i in infos {
        let inst_ver = installed.get(&i.name).cloned();
        let keywords = i.keywords.clone();
        let (purpose, kind, pkg_type, cats) =
            classify(&i.name, &i.description, "aur", &keywords, &[]);
        results.push(SearchResult {
            repo: "aur".into(),
            name: i.name.clone(),
            version: i.version.clone(),
            description: i.description.clone(),
            installed: inst_ver.is_some(),
            installed_version: inst_ver,
            installed_size: None,
            download_size: None,
            source: "aur".into(),
            popularity: Some(format!("{:.2}", i.popularity)),
            out_of_date: i.out_of_date.map(|t| t.to_string()),
            maintainer: if i.maintainer.is_empty() {
                None
            } else {
                Some(i.maintainer.clone())
            },
            purpose,
            kind,
            pkg_type,
            size_class: "unknown".into(),
            categories: cats,
            keywords,
        });
    }
    results.sort_by(|a, b| {
        search_match_rank(query, &a.name, &a.description)
            .cmp(&search_match_rank(query, &b.name, &b.description))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    results.truncate(limit);
    Ok(results)
}

pub fn si_repo(name: &str) -> Result<String, String> {
    let (ok, out, err) = run_status("pacman", &["-Si", "--", name])?;
    if ok && !out.trim().is_empty() {
        return Ok(out);
    }
    Err(if !err.trim().is_empty() {
        err.trim().to_string()
    } else {
        format!("package not found in sync repos: {name}")
    })
}

/// AUR info via typed RPC client (was: curl | jq, silently empty without jq).
pub fn aur_info(name: &str) -> Result<String, String> {
    let infos = crate::aur_rpc::fetch_info(&[name.to_string()])?;
    let Some(i) = infos.into_iter().find(|i| i.name == name) else {
        return Err(format!("not found in AUR: {name}"));
    };
    Ok(render_aur_info(&i))
}

/// The lines of `query NAME --aur` for one AUR package. Pure.
pub fn render_aur_info(i: &crate::aur_rpc::AurPkgInfo) -> String {
    format!(
        "Name: {}\nVersion: {}\nDescription: {}\nURL: {}\nLicenses: {}\nKeywords: {}\nMaintainer: {}\nPopularity: {:.2}\nVotes: {}\nOutOfDate: {}\nDepends: {}\nMakeDepends: {}\nPackageBase: {}\nSource: aur",
        i.name,
        i.version,
        i.description,
        i.url,
        i.license.join(", "),
        i.keywords.join(", "),
        if i.maintainer.is_empty() {
            "(orphan)"
        } else {
            &i.maintainer
        },
        i.popularity,
        i.num_votes,
        i.out_of_date
            .map(|t| t.to_string())
            .unwrap_or_else(|| "no".into()),
        i.depends.join(" "),
        i.make_depends.join(" "),
        i.package_base
    )
}

/// Structured update check: repo via pacman -Qu, AUR via RPC version comparison
/// (no yay dependency).
pub fn check_updates_data() -> UpdateCheckData {
    let mut data = UpdateCheckData {
        no_sync: crate::native::missing_metadata(crate::platform::PkgManager::Pacman),
        ..Default::default()
    };
    match run_status("pacman", &["-Qu"]) {
        Ok((_ok, out, _err)) => {
            // pacman -Qu exits 1 when there is nothing to do; stdout still valid
            data.repo_check_ok = true;
            data.repo_updates = out
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        }
        Err(_) => data.repo_check_ok = false,
    }
    // without a sync database `pacman -Qm` lists every installed package:
    // the AUR is not sent that list, and is not asked
    if data.no_sync.is_some() {
        return data;
    }
    match crate::aur_rpc::aur_update_check() {
        Ok(v) => {
            data.aur_check_ok = true;
            data.aur_updates = v;
        }
        Err(_) => data.aur_check_ok = false,
    }
    data
}

/// The pending updates as `--json check` prints them on every package
/// manager: an array of `{name, old_version, new_version, repo}`. A
/// repository update has an empty `repo` (pacman -Qu does not name it); an
/// AUR update has `repo` "aur". `Err` when pacman could not be asked; the
/// second value is a note when the AUR could not be (the list then holds
/// the repository updates only), or when no sync database is on disk (the
/// list is then empty because nothing could be asked). Pure.
pub fn check_updates_rows(
    d: &UpdateCheckData,
) -> Result<(Vec<crate::native::Update>, Option<String>), String> {
    if !d.repo_check_ok {
        return Err("pacman -Qu could not be run: the pending updates are not known".into());
    }
    let mut rows = crate::native::pacman::parse_updates(&d.repo_updates.join("\n"));
    rows.extend(
        d.aur_updates
            .iter()
            .map(|(name, old, new)| crate::native::Update {
                name: name.clone(),
                old_version: old.clone(),
                new_version: new.clone(),
                repo: "aur".into(),
            }),
    );
    let note = match (&d.no_sync, d.aur_check_ok) {
        (Some(why), _) => Some(why.clone()),
        (None, false) => Some(
            "the AUR could not be asked (is the network up?): the list holds the repository updates only"
                .to_string(),
        ),
        (None, true) => None,
    };
    Ok((rows, note))
}

pub fn check_updates() -> Result<String, String> {
    Ok(render_check(&check_updates_data()))
}

/// What `yapacc check` prints on Arch. With no sync database on disk the
/// two empty answers are replaced by notes: the repositories could not be
/// asked, and the AUR was not. Pure.
pub fn render_check(d: &UpdateCheckData) -> String {
    let repo = if let Some(why) = &d.no_sync {
        format!("note: {why}\n")
    } else if !d.repo_check_ok {
        "(repo update check unavailable)\n".to_string()
    } else if d.repo_updates.is_empty() {
        "(no repo updates)\n".to_string()
    } else {
        format!("{}\n", d.repo_updates.join("\n"))
    };
    let aur = if d.no_sync.is_some() {
        "note: not checked: without the repository databases yapacc cannot tell which installed packages come from the AUR\n".to_string()
    } else if !d.aur_check_ok {
        "(AUR update check unavailable; is the network up?)\n".to_string()
    } else if d.aur_updates.is_empty() {
        "(no AUR updates)\n".to_string()
    } else {
        d.aur_updates
            .iter()
            .map(|(n, cur, avail)| format!("{n} {cur} -> {avail}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    format!(
        "== Repo updates (pacman -Qu, as of the last repository refresh) ==\n{}== AUR updates (RPC vercmp) ==\n{aur}",
        repo
    )
}

/// What the AUR answered about one package name.
#[derive(Debug)]
pub enum AurLookup {
    Found(Box<crate::aur_rpc::AurPkgInfo>),
    /// the AUR answered, and has no package of that name
    NotInAur,
    /// the AUR could not be asked (no network, no curl, a refused query)
    Unreachable(String),
}

/// Read the answer of the AUR RPC `info` request for `name`. Pure.
pub fn aur_lookup_from(
    name: &str,
    answer: Result<Vec<crate::aur_rpc::AurPkgInfo>, String>,
) -> AurLookup {
    match answer {
        Err(e) => AurLookup::Unreachable(e),
        Ok(infos) => match infos.into_iter().find(|i| i.name == name) {
            Some(i) => AurLookup::Found(Box::new(i)),
            None => AurLookup::NotInAur,
        },
    }
}

/// Ask the AUR whether it has `name` (one RPC request through curl).
pub fn aur_lookup(name: &str) -> AurLookup {
    aur_lookup_from(name, crate::aur_rpc::fetch_info(&[name.to_string()]))
}

/// Why a PKGBUILD could not be printed.
#[derive(Debug, PartialEq, Eq)]
pub enum PkgbuildError {
    /// the AUR has no package of that name (exit 4)
    NotInAur(String),
    /// the AUR, or the file, could not be reached (exit 6)
    Unreachable(String),
}

impl std::fmt::Display for PkgbuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgbuildError::NotInAur(m) | PkgbuildError::Unreachable(m) => f.write_str(m),
        }
    }
}

/// "NAME is not in the AUR", with where to look next. Pure.
pub fn not_in_aur_message(name: &str) -> String {
    format!("{name} is not in the AUR (`yapacc search {name} --aur` looks for similar names there)")
}

/// "the AUR could not be reached", with what was being done and why. Pure.
pub fn aur_unreachable_message(name: &str, why: &str) -> String {
    format!(
        "the AUR (aur.archlinux.org) could not be reached, so {name} could not be looked up: {why}"
    )
}

/// The PKGBUILD of AUR package `name`, read from aur.archlinux.org with
/// curl. The AUR is asked first whether it has the name: a name it does not
/// have is said as that, not as a failed download. yay is not needed; when
/// it is installed it is the second way to the file.
pub fn fetch_pkgbuild(name: &str) -> Result<String, PkgbuildError> {
    let info = match aur_lookup(name) {
        AurLookup::Found(info) => info,
        AurLookup::NotInAur => return Err(PkgbuildError::NotInAur(not_in_aur_message(name))),
        AurLookup::Unreachable(e) => {
            return Err(PkgbuildError::Unreachable(aur_unreachable_message(
                name, &e,
            )));
        }
    };
    // a split package has one PKGBUILD, under the name of its base
    let base = if info.package_base.is_empty() {
        name
    } else {
        &info.package_base
    };
    let url = format!(
        "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h={}",
        crate::aur_rpc::url_encode(base)
    );
    let why = match Command::new("curl")
        .args(["-fsSL", "--max-time", "30", &url])
        .output()
    {
        Ok(out) if out.status.success() => {
            return Ok(String::from_utf8_lossy(&out.stdout).to_string());
        }
        Ok(out) => format!(
            "curl exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => format!("curl could not be run: {e}"),
    };
    // the second way, only when yay is there
    if crate::util::have("yay")
        && let Ok((true, o, _)) = run_status("yay", &["-Gp", "--", name])
        && !o.trim().is_empty()
    {
        return Ok(o);
    }
    Err(PkgbuildError::Unreachable(format!(
        "the PKGBUILD of {name} could not be fetched from aur.archlinux.org ({why})"
    )))
}

/// A pacman target: a package name, optionally as `repo/name`. Nothing that
/// passes can be read as an option or as a path.
pub fn valid_pacman_target(t: &str) -> bool {
    let name = t.split_once('/').map_or(t, |(repo, name)| {
        if crate::util::valid_pkg_name(repo) {
            name
        } else {
            ""
        }
    });
    crate::util::valid_pkg_name(name)
}

/// The privilege prefix for the pacman commands: `doas` when that is what the
/// machine has (it used to be a hard-coded `sudo`, which does not exist on a
/// doas-only Arch), `sudo` when neither is found (the plan text stays as
/// before; the run then fails with the real "not found"). Pure.
pub fn elevation_or_sudo(found: crate::error::Result<Vec<String>>) -> Vec<String> {
    match found {
        Ok(p) if !p.is_empty() => p,
        _ => vec!["sudo".to_string()],
    }
}

/// The privilege prefix of the pacman lane: empty for root, else `sudo` or
/// `doas`, with `-n` when the run must never wait for a password
/// (`never_ask`: --script/--noconfirm; also added when there is no terminal).
pub fn pacman_elevation(never_ask: bool) -> Vec<String> {
    if crate::util::is_root() {
        return Vec::new();
    }
    elevation_or_sudo(crate::ops::elevation_for(crate::ops::GateFlags {
        dryrun: false,
        yes: true,
        noninteractive: never_ask,
        script: never_ask,
    }))
}

/// Whether `op` with `use_yay` is handed to yay (which builds AUR code and
/// elevates by itself) instead of pacman. Pure.
pub fn goes_through_yay(op: &str, use_yay: bool) -> bool {
    use_yay && matches!(op, "install" | "upgrade")
}

/// The native command of a pacman-lane operation, without the privilege
/// prefix. Package arguments follow `--`. Pure.
///
/// - `remove` is `pacman -Rs`: the packages and the dependencies nothing else
///   needs. Not `-n`: configuration files that were modified are kept, as
///   `.pacsave` files.
/// - pacman is always given `--noconfirm`: the command runs after yapacc's
///   gate, where the plan and pacman's own simulation were shown and the
///   question was answered (at the terminal, or by `--yes`). pacman must not
///   ask "Proceed with installation?" a second time; apt, dnf, xbps and pkg
///   are run with `-y` for the same reason.
/// - yay is never given `--noconfirm`: it builds AUR code, and its own
///   PKGBUILD and diff questions are the review of that code. The answer at
///   yapacc's gate is not that review.
pub fn native_argv(op: &str, pkgs: &[String], use_yay: bool) -> Vec<String> {
    let yay = goes_through_yay(op, use_yay);
    let head: &[&str] = match (op, yay) {
        ("install", true) => &["yay", "-S"],
        ("install", false) => &["pacman", "-S"],
        ("install-local", _) => &["pacman", "-U"],
        ("remove", _) => &["pacman", "-Rs"],
        ("upgrade", true) => &["yay", "-Syu"],
        ("upgrade", false) => &["pacman", "-Syu"],
        ("update", _) => &["pacman", "-Sy"],
        _ => return Vec::new(),
    };
    let mut cmd: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    if !yay {
        cmd.push("--noconfirm".into());
    }
    if matches!(op, "install" | "remove" | "install-local") {
        cmd.push("--".into());
        cmd.extend(pkgs.iter().cloned());
    }
    cmd
}

/// pacman's own simulation of `native_argv(op, ..)`: the same command with
/// `--print`, which changes nothing and needs no root. `None` where pacman
/// cannot simulate (a database refresh; anything yay builds). For `upgrade`
/// the simulation is `-Su`, against the databases of the last refresh (`-y`
/// would need root and would change them). Pure.
pub fn preview_argv(op: &str, pkgs: &[String], use_yay: bool) -> Option<Vec<String>> {
    if goes_through_yay(op, use_yay) {
        return None;
    }
    let (flag, format, targets): (&str, &str, bool) = match op {
        "install" => ("-S", "%r/%n %v", true),
        "install-local" => ("-U", "%n %v", true),
        "remove" => ("-Rs", "%n %v", true),
        "upgrade" => ("-Su", "%r/%n %v", false),
        _ => return None,
    };
    let mut cmd: Vec<String> = ["pacman", flag, "--print", "--print-format", format]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if targets {
        cmd.push("--".into());
        cmd.extend(pkgs.iter().cloned());
    }
    Some(cmd)
}

/// Run a `preview_argv` simulation: its exit status, the lines it printed
/// and what it wrote to stderr. stdin is closed, so a question pacman would
/// ask takes its default instead of waiting. Changes nothing.
pub fn run_simulation(argv: &[String]) -> Result<(i32, Vec<String>, String), String> {
    let Some((prog, rest)) = argv.split_first() else {
        return Err("empty command".into());
    };
    let out = Command::new(prog)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| spawn_error(prog, &e))?;
    Ok((
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.trim().is_empty())
            .collect(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

/// The package lines of a simulation: what is neither a question nor a
/// message of pacman (`:: ...`, `warning: ...`). Pure.
pub fn simulation_packages(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with(':') && !l.contains(": "))
        .filter(|l| l.split_whitespace().count() == 2)
        .map(|l| l.to_string())
        .collect()
}

/// Whether the in-process libalpm analysis can run: compiled in, and root
/// (it takes the pacman database lock). Where it cannot, pacman's own
/// `--print` simulation is the plan.
pub fn alpm_analysis_available() -> bool {
    crate::alpm_flows::COMPILED_IN && crate::util::is_root()
}

/// `native_argv` behind the privilege prefix (none for root, none for yay).
pub fn build_command_for(op: &str, pkgs: &[String], use_yay: bool, never_ask: bool) -> Vec<String> {
    let native = native_argv(op, pkgs, use_yay);
    if native.is_empty() || goes_through_yay(op, use_yay) {
        return native;
    }
    let mut cmd = pacman_elevation(never_ask);
    cmd.extend(native);
    cmd
}

pub fn build_command(op: &str, pkgs: &[String], use_yay: bool) -> Vec<String> {
    build_command_for(op, pkgs, use_yay, false)
}

/// The command that takes back a failed install: `pacman -R`, the named
/// packages only. Not `-s` (their dependencies stay) and not `-n` (their
/// configuration stays). Pure.
pub fn rollback_remove_argv(names: &[String]) -> Vec<String> {
    ["pacman", "-R", "--noconfirm", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(names.iter().cloned())
        .collect()
}

/// When the sync databases were last refreshed: the newest modification
/// time among `/var/lib/pacman/sync/*.db`.
pub fn last_refresh() -> Option<std::time::SystemTime> {
    std::fs::read_dir("/var/lib/pacman/sync")
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "db"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// "3 d 4 h" / "5 h" / "12 min" for an age in seconds. Pure.
pub fn age_text(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h")
    } else {
        format!("{m} min")
    }
}

/// What `checkupdates` (pacman-contrib) answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckUpdates {
    /// the pending upgrades, against a private, freshly refreshed copy of
    /// the sync databases
    Current(Vec<String>),
    /// the tool is not installed
    Missing,
    /// the tool is installed and failed: why, in its own words
    Failed(String),
}

/// Read one run of `checkupdates`: status 0 lists the pending upgrades,
/// status 2 means none is pending, anything else is a failure whose reason
/// is the last line it wrote to standard error (`==> ERROR: Cannot find the
/// fakeroot binary.`). Pure.
pub fn read_checkupdates(code: Option<i32>, stdout: &str, stderr: &str) -> CheckUpdates {
    match code {
        Some(0) => CheckUpdates::Current(
            stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        ),
        Some(2) => CheckUpdates::Current(Vec::new()),
        other => {
            let said = stderr
                .lines()
                .map(|l| {
                    l.trim()
                        .trim_start_matches("==> ")
                        .trim_start_matches("ERROR: ")
                })
                .rfind(|l| !l.is_empty())
                .unwrap_or("")
                .trim_end_matches('.');
            let status = match other {
                Some(c) => format!("status {c}"),
                None => "killed by a signal".to_string(),
            };
            CheckUpdates::Failed(if said.is_empty() {
                status
            } else {
                format!("{status}: {said}")
            })
        }
    }
}

/// Run `checkupdates` (pacman-contrib). It asks the mirrors and works on a
/// private copy of the sync databases; the system's own are not touched.
pub fn current_pending_updates() -> CheckUpdates {
    if !crate::util::have("checkupdates") {
        return CheckUpdates::Missing;
    }
    match Command::new("checkupdates")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(out) => read_checkupdates(
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(e) => CheckUpdates::Failed(format!("it could not be started: {e}")),
    }
}

/// The installed package names (`pacman -Qq`). An error, not an empty set,
/// when pacman cannot be asked: callers decide what a rollback may touch
/// from this.
pub fn installed_names() -> Result<HashSet<String>, String> {
    let (ok, out, err) = run_status("pacman", &["-Qq"])?;
    if !ok {
        return Err(format!("pacman -Qq failed: {}", err.trim()));
    }
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Warning shown whenever a bare `pacman -Sy` (partial upgrade) is planned.
pub fn partial_upgrade_warning() -> &'static str {
    "WARNING: `pacman -Sy` alone creates a partial upgrade state, which is unsupported on Arch\nand can break the system. Prefer `upgrade` (pacman -Syu) unless you know why you need a\ndatabase-only refresh."
}

/// Orphans via one batched `pacman -Qi` call (was: one -Qi subprocess per orphan).
pub fn list_orphans() -> Result<Vec<PackageSummary>, String> {
    let names = run("pacman", &["-Qdtq"])?;
    let wanted: Vec<String> = names
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    let refs: Vec<&str> = wanted.iter().map(|s| s.as_str()).collect();
    let mut qi_args: Vec<&str> = vec!["-Qi", "--"];
    qi_args.extend(refs.iter());
    let qi = run("pacman", &qi_args)?;
    let blocks = parse_qi_blocks(&qi);
    let mut out = Vec::new();
    for name in &wanted {
        if let Some(b) = blocks
            .iter()
            .find(|b| b.get("Name").map(|s| s.as_str()) == Some(name.as_str()))
        {
            out.push(PackageSummary {
                name: name.clone(),
                version: b.get("Version").cloned().unwrap_or_default(),
                description: b.get("Description").cloned().unwrap_or_default(),
                installed_size: b.get("Installed Size").cloned().unwrap_or_default(),
                reason: "orphan-dep".into(),
                repo: "repo".into(),
            });
        }
    }
    Ok(out)
}

pub fn reverse_dep_tree(name: &str, depth: u32) -> Result<String, String> {
    let d = depth.to_string();
    let (ok, out, err) = run_status("pactree", &["-r", "-d", &d, "--", name])?;
    if ok && !out.trim().is_empty() {
        return Ok(out);
    }
    if !err.trim().is_empty() {
        return Err(err.trim().to_string());
    }
    Err(format!("no reverse dependency tree available for {name}"))
}

/// The two lines of the install plan for a target no sync database has.
/// Every command named works without a helper program: `search --all`
/// looks in the repositories and in the AUR, and `install aur` asks the
/// AUR whether it has the name before it plans anything. Pure.
pub fn not_in_sync_repos_hint(p: &str) -> String {
    format!(
        "- {p} [not in sync repos]\n  note: the repository databases on disk have no package of that name; it may be an AUR package, a group or a virtual name. `yapacc search {p} --all` looks in the repositories and in the AUR; an AUR package is installed with `yapacc install aur {p}`, which shows its PKGBUILD first.\n"
    )
}

/// The refusal of an install while no sync database is on disk (a fresh
/// Arch system before its first `pacman -Sy`): every target would be "not
/// in sync repos", and the user would be sent to the AUR for a package of
/// `core`. Pure.
pub fn install_without_sync_message(pkgs: &[String]) -> String {
    format!(
        "cannot install {}: no repository metadata on disk: run `pacman -Syu` as root first, or `yapacc upgrade`, which shows that command and asks. Nothing was changed.",
        pkgs.join(", ")
    )
}

/// The plan without its closing "Will run" lines, for callers that
/// print their own.
pub fn install_plan_body(pkgs: &[String], use_yay: bool) -> Result<String, String> {
    let mut s = String::new();
    s.push_str(&format!(
        "== Install plan (targets: {}) ==\nSource preference: {}\n",
        pkgs.join(", "),
        if use_yay {
            "yay/AUR-capable"
        } else {
            "pacman repos"
        }
    ));
    for p in pkgs {
        if let Ok(si) = si_repo(p) {
            let mut desc = "";
            let mut isz = "";
            let mut dsz = "";
            let mut deps = "";
            for l in si.lines() {
                if l.starts_with("Description") {
                    desc = l
                        .trim_start_matches("Description")
                        .trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                }
                if l.starts_with("Installed Size") {
                    isz = l.split_once(':').map(|x| x.1).unwrap_or("").trim();
                }
                if l.starts_with("Download Size") {
                    dsz = l.split_once(':').map(|x| x.1).unwrap_or("").trim();
                }
                if l.starts_with("Depends On") {
                    deps = l.split_once(':').map(|x| x.1).unwrap_or("").trim();
                }
            }
            s.push_str(&format!("- {p} [repo]\n  purpose: {desc}\n  size: download={dsz} installed={isz}\n  depends: {deps}\n"));
        } else {
            s.push_str(&not_in_sync_repos_hint(p));
        }
    }
    // Best-effort ALPM prepare analysis for provider/conflict/dependency/impact visibility
    if !use_yay && !alpm_analysis_available() {
        // no libalpm in this build, or no root for the database lock:
        // pacman's own simulation (printed by the caller) is the plan
    } else if !use_yay {
        match crate::alpm_flows::plan_install(pkgs) {
            Ok(plan) => {
                s.push_str("\n== ALPM prepare analysis (provider/conflict/dependency impact, no commit) ==\n");
                for n in &plan.notes {
                    s.push_str(&format!("  note: {n}\n"));
                }
                if !plan.to_install.is_empty() {
                    s.push_str(&format!(
                        "  to install ({}): {}\n",
                        plan.to_install.len(),
                        plan.to_install.join(", ")
                    ));
                }
                if !plan.to_remove.is_empty() {
                    s.push_str(&format!(
                        "  to remove ({}): {}\n",
                        plan.to_remove.len(),
                        plan.to_remove.join(", ")
                    ));
                }
            }
            Err(e) => {
                s.push_str(&format!(
                    "\n== ALPM prepare analysis ==\n  unavailable/conflict: {e}\n"
                ));
            }
        }
    } else {
        s.push_str("\n== ALPM prepare analysis ==\n  skipped for --aur/yay flow (AUR resolution handled by helper; repo/provider/conflict pre-prepare not fully modeled here).\n");
    }
    Ok(s)
}

pub fn classify_installed(name: &str) -> Result<String, String> {
    let info = package_info(name)?;
    let foreign = if info.is_foreign { "yes" } else { "no" };
    let explicit = if info.is_explicit {
        "explicit"
    } else {
        "dependency"
    };
    let leaf = if info.is_leaf { "yes" } else { "no" };
    let (purpose, kind, pkg_type, cats) = classify(
        &info.name,
        &info.description,
        if info.is_foreign { "aur" } else { "repo" },
        &[],
        &[],
    );
    let size_class = size_class_from(Some(&info.installed_size));
    Ok(format!(
        "Name: {}\nPurpose: {}\nKind: {}\nType: {}\nSize: {} ({})\nSource: {}\nInstall reason: {}  Foreign: {}  Explicit: {}  Leaf: {}\nGranular tags: {}\nDepends: {}\nRequired By: {}",
        info.name,
        purpose,
        kind,
        pkg_type,
        info.installed_size,
        size_class,
        if info.is_foreign {
            "local/aur"
        } else {
            "repo/sync"
        },
        info.install_reason,
        foreign,
        explicit,
        leaf,
        cats.join(", "),
        info.depends,
        info.required_by
    ))
}

pub fn aur_review(name: &str) -> Result<String, String> {
    let info = aur_info(name).unwrap_or_else(|e| format!("(AUR RPC info unavailable: {e})"));
    let pkgbuild = fetch_pkgbuild(name).unwrap_or_else(|e| format!("(PKGBUILD unavailable: {e})"));
    let mut s = String::new();
    s.push_str(&format!(
        "== AUR REVIEW: {name} ==\n\n{info}\n\n== PKGBUILD ==\n{pkgbuild}\n"
    ));
    s.push_str("\n== Confirm-gated build/install plan (NOT executed) ==\n");
    s.push_str("Options:\n");
    s.push_str(&format!("  1) yapacc install aur {name} --yes   # this tool (add --dryrun to plan only): clone -> review gate -> makepkg -> pacman -U\n"));
    s.push_str(&format!("  2) yay -S {name}                # preferred: helper handles deps/build/install with prompts\n"));
    s.push_str(&format!(
        "  3) git clone https://aur.archlinux.org/{name}.git && cd {name} && makepkg -si\n"
    ));
    s.push_str("Review notes: inspect PKGBUILD for malicious/suspicious commands, verify sources/checksums/PGP, review .install scripts, check maintainer/votes/out-of-date.\n");
    s.push_str("Nothing above was run. A build or install asks for confirmation first (`--dryrun` plans only).\n");
    Ok(s)
}

/// `makepkg --printsrcinfo` sources the PKGBUILD as bash, i.e. it runs
/// code, so it goes through the build sandbox (no network) like any build.
pub fn generate_srcinfo(dir: &str) -> Result<String, String> {
    let mut prepared = crate::sandbox::command(
        std::path::Path::new(dir),
        crate::sandbox::Net::Isolated,
        crate::sandbox::Policy::from_config(),
        None,
        &["makepkg", "--printsrcinfo"],
    )?;
    let out = prepared
        .command
        .output()
        .map_err(|e| format!("failed to run makepkg: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && !stdout.trim().is_empty() {
        return Ok(stdout);
    }
    if !stderr.trim().is_empty() {
        return Err(stderr.trim().to_string());
    }
    Err(
        "failed to generate .SRCINFO (is this a PKGBUILD directory and is makepkg installed?)"
            .into(),
    )
}

/// (repo, name, version, installed, kind, categories)
pub type RepoIndexRow = (String, String, String, String, String, String);

pub fn build_repo_index(limit: usize) -> Result<Vec<RepoIndexRow>, String> {
    // (repo,name,version,installed,kind_guess,categories_guess)
    let out = run("pacman", &["-Sl"])?;
    let installed = installed_version_map().unwrap_or_default();
    let mut rows = Vec::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let repo = it.next().unwrap_or("").to_string();
        let name = it.next().unwrap_or("").to_string();
        let ver = it.next().unwrap_or("").to_string();
        let inst = it.next().map(|s| s.contains("installed")).unwrap_or(false)
            || installed.contains_key(&name);
        if name.is_empty() {
            continue;
        }
        let (purpose, kind, _pkg_type, cats) = classify(&name, "", "repo", &[], &[]);
        let _ = purpose;
        rows.push((
            repo,
            name,
            ver,
            if inst { "yes".into() } else { "no".into() },
            kind,
            cats.join(","),
        ));
        if limit > 0 && rows.len() >= limit {
            break;
        }
    }
    Ok(rows)
}

pub fn search_flatpak(query: &str) -> Result<String, String> {
    let (ok, out, err) = run_status("flatpak", &["search", "--", query])?;
    if ok || !out.trim().is_empty() {
        return Ok(if out.trim().is_empty() {
            "(no results)\n".into()
        } else {
            out
        });
    }
    Err(if err.trim().is_empty() {
        "flatpak search failed".into()
    } else {
        err
    })
}

pub fn list_flatpak() -> Result<String, String> {
    let (ok, out, err) = run_status(
        "flatpak",
        &[
            "list",
            "--app",
            "--columns=application,name,version,branch,origin,installation",
        ],
    )?;
    if ok {
        return Ok(if out.trim().is_empty() {
            "(no flatpak apps)\n".into()
        } else {
            out
        });
    }
    Err(err)
}

pub fn flatpak_info(app: &str) -> Result<String, String> {
    let (ok, out, err) = run_status("flatpak", &["info", "--", app])?;
    if ok && !out.trim().is_empty() {
        return Ok(out);
    }
    let (ok2, out2, err2) = run_status(
        "flatpak",
        &["remote-info", "--system", "flathub", "--", app],
    )?;
    if ok2 && !out2.trim().is_empty() {
        return Ok(format!("(remote-info flathub)\n{out2}"));
    }
    let msg = if !err.trim().is_empty() {
        err
    } else if !err2.trim().is_empty() {
        err2
    } else {
        format!("no flatpak info found for {app}")
    };
    Err(msg)
}

pub fn package_files(name: &str) -> Result<String, String> {
    let (ok, out, err) = run_status("pacman", &["-Ql", "--", name])?;
    if ok {
        return Ok(out);
    }
    Err(if err.trim().is_empty() {
        format!("cannot list files for {name}")
    } else {
        err
    })
}
pub fn file_owner(path: &str) -> Result<String, String> {
    let (ok, out, err) = run_status("pacman", &["-Qo", "--", path])?;
    if ok {
        return Ok(out);
    }
    Err(if err.trim().is_empty() {
        format!("no owner found for {path}")
    } else {
        err
    })
}
pub fn dep_tree(name: &str, depth: u32) -> Result<String, String> {
    let d = depth.to_string();
    let (ok, out, err) = run_status("pactree", &["-d", &d, "--", name])?;
    if ok && !out.trim().is_empty() {
        return Ok(out);
    }
    if !err.trim().is_empty() {
        return Err(err.trim().to_string());
    }
    Err(format!("no dependency tree for {name}"))
}

pub fn build_enriched_index_file(path: &str, limit: usize) -> Result<String, String> {
    use std::fs;
    use std::path::Path;
    let rows = build_repo_index(limit)?;
    if let Some(p) = Path::new(path).parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut out = String::from("repo\tname\tversion\tinstalled\tkind\tcategories\n");
    for r in &rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            r.0, r.1, r.2, r.3, r.4, r.5
        ));
    }
    fs::write(path, &out).map_err(|e| e.to_string())?;
    Ok(format!("wrote {} rows to {}", rows.len(), path))
}

pub fn search_index_file(
    path: &str,
    query: &str,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<RepoIndexRow>, String> {
    use std::fs;
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read index {path}: {e} (run `yapacc index build` first)"))?;
    let q = query.to_lowercase();
    let cf = category.map(|s| s.to_lowercase());
    let mut rows = Vec::new();
    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 6 {
            continue;
        }
        let name = cols[1].to_string();
        let cats = cols[5].to_string();
        if !q.is_empty() && !name.to_lowercase().contains(&q) && !cats.to_lowercase().contains(&q) {
            continue;
        }
        if let Some(c) = &cf
            && !cats.to_lowercase().contains(c)
            && !name.to_lowercase().contains(c)
        {
            continue;
        }
        rows.push((
            cols[0].to_string(),
            name,
            cols[2].to_string(),
            cols[3].to_string(),
            cols[4].to_string(),
            cats,
        ));
    }
    let rank = |name: &str, cats: &str| -> u8 {
        if q.is_empty() {
            return 3;
        }
        let n = name.to_lowercase();
        let c = cats.to_lowercase();
        if n == q {
            0
        } else if n.contains(&q) {
            1
        } else if c.contains(&q) {
            2
        } else {
            3
        }
    };
    rows.sort_by(|a, b| {
        rank(&a.1, &a.5)
            .cmp(&rank(&b.1, &b.5))
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    rows.truncate(limit);
    Ok(rows)
}

fn flatpak_scope_arg(user: bool) -> &'static str {
    if user { "--user" } else { "--system" }
}

pub fn flatpak_install_plan(app: &str, user: bool) -> Result<String, String> {
    let (ok, out, _) = run_status("flatpak", &["search", "--", app])?;
    let info = if ok {
        out
    } else {
        "(flatpak search unavailable or no match)".into()
    };
    let scope = flatpak_scope_arg(user);
    Ok(format!(
        "== Flatpak install plan ==\nTarget: {app}\nScope: {} ({scope})\nSearch info:\n{info}\nWill run:\n  flatpak install {scope} -y -- {app}",
        if user { "user" } else { "system" },
    ))
}

pub fn flatpak_update_plan(target: Option<&str>, user: bool) -> String {
    let scope = flatpak_scope_arg(user);
    match target {
        Some(app) => format!(
            "== Flatpak update plan ==\nTarget: {app}\nScope: {} ({})\nWill run:\n  flatpak update {scope} -y -- {app}\n",
            if user { "user" } else { "system" },
            scope
        ),
        None => format!(
            "== Flatpak update plan ==\nTarget: <all>\nScope: {} ({})\nWill run:\n  flatpak update {scope} -y\n",
            if user { "user" } else { "system" },
            scope
        ),
    }
}

pub fn flatpak_remove_plan(app: &str, user: bool) -> Result<String, String> {
    let info = flatpak_info(app).unwrap_or_else(|e| format!("(info unavailable: {e})"));
    let scope = flatpak_scope_arg(user);
    Ok(format!(
        "== Flatpak remove plan ==\nTarget: {app}\nScope: {} ({scope})\nInfo:\n{info}\nWill run:\n  flatpak uninstall {scope} -y -- {app}",
        if user { "user" } else { "system" },
    ))
}

fn run_passthrough(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to execute {cmd}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(if stdout.trim().is_empty() && !stderr.trim().is_empty() {
            stderr
        } else {
            stdout
        })
    } else {
        Err(if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("{cmd} exited with {}", out.status)
        })
    }
}

/// Whether `app` is installed in the given flatpak installation.
pub fn flatpak_installed(app: &str, user: bool) -> bool {
    run_status("flatpak", &["info", flatpak_scope_arg(user), "--", app])
        .map(|(ok, _, _)| ok)
        .unwrap_or(false)
}

pub fn flatpak_install(app: &str, user: bool) -> Result<String, String> {
    let scope = flatpak_scope_arg(user);
    run_passthrough("flatpak", &["install", scope, "-y", "--", app])
}
pub fn flatpak_remove(app: &str, user: bool) -> Result<String, String> {
    let scope = flatpak_scope_arg(user);
    run_passthrough("flatpak", &["uninstall", scope, "-y", "--", app])
}
pub fn flatpak_update(target: Option<&str>, user: bool) -> Result<String, String> {
    let scope = flatpak_scope_arg(user);
    match target {
        Some(app) => run_passthrough("flatpak", &["update", scope, "-y", "--", app]),
        None => run_passthrough("flatpak", &["update", scope, "-y"]),
    }
}

/// Run `prefix + argv` and return what it printed; no shell.
fn run_elevated(prefix: &[String], argv: &[String]) -> Result<String, String> {
    let full: Vec<&str> = prefix.iter().chain(argv).map(|s| s.as_str()).collect();
    let Some((prog, rest)) = full.split_first() else {
        return Err("empty command".into());
    };
    run_passthrough(prog, rest)
}

/// `pacman -D --asexplicit|--asdeps -- NAMES`. Pure.
pub fn mark_argv(targets: &[String], as_explicit: bool) -> Vec<String> {
    let mode = if as_explicit {
        "--asexplicit"
    } else {
        "--asdeps"
    };
    ["pacman", "-D", mode, "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(targets.iter().cloned())
        .collect()
}

/// `prefix` is the privilege prefix the command runs behind
/// (`pacman_elevation`), so the plan shows the command as it is run.
pub fn mark_plan(targets: &[String], as_explicit: bool, prefix: &[String]) -> String {
    format!(
        "== Mark install reason plan ==\nTargets: {}\nSet reason: {}\nWill run:\n  {}\n",
        targets.join(", "),
        if as_explicit {
            "explicit"
        } else {
            "dependency"
        },
        crate::ops::shown(prefix, &mark_argv(targets, as_explicit))
    )
}

pub fn mark_reason(
    targets: &[String],
    as_explicit: bool,
    prefix: &[String],
) -> Result<String, String> {
    run_elevated(prefix, &mark_argv(targets, as_explicit))
}

/// `pacman -Sw --noconfirm -- TARGETS`: download into the package cache,
/// install nothing. Pure.
pub fn download_argv(targets: &[String]) -> Vec<String> {
    ["pacman", "-Sw", "--noconfirm", "--"]
        .iter()
        .map(|s| s.to_string())
        .chain(targets.iter().cloned())
        .collect()
}

pub fn download_plan(targets: &[String], prefix: &[String]) -> String {
    format!(
        "== Download (cache-only) plan ==\nTargets: {}\nWill run:\n  {}\n",
        targets.join(", "),
        crate::ops::shown(prefix, &download_argv(targets))
    )
}

pub fn download(targets: &[String], prefix: &[String]) -> Result<String, String> {
    run_elevated(prefix, &download_argv(targets))
}

/// File-ownership search against sync file databases (`pacman -F`).
pub fn file_search(path: &str, refresh: bool) -> Result<String, String> {
    if refresh {
        let _ = run_status("pacman", &["-Fy"]);
    }
    let (ok, out, err) = run_status("pacman", &["-F", "--", path])?;
    if ok && !out.trim().is_empty() {
        return Ok(out);
    }
    Err(if !err.trim().is_empty() {
        err.trim().to_string()
    } else {
        format!(
            "no owning package found in file databases for {path} (run with --refresh to update the -F databases first)"
        )
    })
}

/// Package groups: all groups, or members of one group (`pacman -Sg`).
pub fn groups(group: Option<&str>) -> Result<String, String> {
    match group {
        Some(g) => {
            let (ok, out, err) = run_status("pacman", &["-Sg", "--", g])?;
            if ok && !out.trim().is_empty() {
                Ok(out)
            } else {
                Err(if !err.trim().is_empty() {
                    err.trim().to_string()
                } else {
                    format!("group not found: {g}")
                })
            }
        }
        None => {
            let (ok, out, err) = run_status("pacman", &["-Sg"])?;
            if ok && !out.trim().is_empty() {
                Ok(out)
            } else {
                Err(if !err.trim().is_empty() {
                    err.trim().to_string()
                } else {
                    "no groups available (sync dbs initialized?)".into()
                })
            }
        }
    }
}

/// `pacman -Sc --noconfirm`, or `pacman -Scc` for the whole cache.
///
/// `-Scc` is not given `--noconfirm`: pacman's default answer to "remove ALL
/// files from cache?" is no, so with `--noconfirm` it removes nothing and
/// exits 0 (observed: 89 files before, 89 after). `cache_clean` answers its
/// questions with `y` on stdin instead. Pure.
pub fn cache_clean_argv(all: bool) -> Vec<String> {
    let parts: &[&str] = if all {
        &["pacman", "-Scc"]
    } else {
        &["pacman", "-Sc", "--noconfirm"]
    };
    parts.iter().map(|s| s.to_string()).collect()
}

pub fn cache_clean_plan(all: bool, prefix: &[String]) -> String {
    format!(
        "== Cache clean plan ==\nMode: {}\nWill run:\n  {}\n{}",
        if all {
            "full (-Scc): remove ALL cached packages (including installed versions)."
        } else {
            "standard (-Sc): remove outdated/uninstalled cached packages and unused sync dbs."
        },
        crate::ops::shown(prefix, &cache_clean_argv(all)),
        if all {
            "pacman's questions are answered y on its input (its own default for removing the whole cache is no, so --noconfirm would remove nothing).\n"
        } else {
            ""
        }
    )
}

pub fn cache_clean(all: bool, prefix: &[String]) -> Result<String, String> {
    let argv = cache_clean_argv(all);
    if !all {
        return run_elevated(prefix, &argv);
    }
    // answer every question of `pacman -Scc` (one pair per cache directory) with y
    use std::io::Write;
    let full: Vec<&String> = prefix.iter().chain(argv.iter()).collect();
    let mut child = Command::new(full[0])
        .args(&full[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to execute {}: {e}", full[0]))?;
    if let Some(mut stdin) = child.stdin.take() {
        // a closed pipe only means pacman stopped asking
        let _ = stdin.write_all("y\n".repeat(64).as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for {}: {e}", full[0]))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Err(stderr)
    } else {
        Err(format!("pacman -Scc exited with {}", out.status))
    }
}

/// Fetch recent Arch Linux news headlines (RSS) — check before upgrading.
pub fn arch_news(limit: usize) -> Result<Vec<(String, String)>, String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "20",
            "https://www.archlinux.org/feeds/news/",
        ])
        .output()
        .map_err(|e| format!("failed to fetch news feed (curl): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "news feed request failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let xml = String::from_utf8_lossy(&out.stdout).to_string();
    let mut items = Vec::new();
    let mut rest = xml.as_str();
    while let Some(start) = rest.find("<item>") {
        let after = &rest[start..];
        let Some(end) = after.find("</item>") else {
            break;
        };
        let block = &after[..end];
        let title = block
            .find("<title>")
            .and_then(|t| {
                block[t..]
                    .find("</title>")
                    .map(|e| block[t + 7..t + e].to_string())
            })
            .unwrap_or_default();
        let date = block
            .find("<pubDate>")
            .and_then(|t| {
                block[t..]
                    .find("</pubDate>")
                    .map(|e| block[t + 9..t + e].to_string())
            })
            .unwrap_or_default();
        if !title.is_empty() {
            items.push((decode_xml_entities(&title), decode_xml_entities(&date)));
        }
        rest = &after[end..];
        if items.len() >= limit {
            break;
        }
    }
    Ok(items)
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Find .pacnew/.pacsave/.orig config merge candidates under a root (pacdiff workflow).
pub fn pacdiff_candidates(root: &str) -> Result<Vec<String>, String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) -> Result<(), String> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()), // unreadable dirs (permissions) are skipped
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_symlink() {
                continue;
            }
            if p.is_dir() {
                walk(&p, out)?;
            } else if let Some(n) = p.file_name().and_then(|s| s.to_str())
                && (n.ends_with(".pacnew") || n.ends_with(".pacsave") || n.ends_with(".orig"))
            {
                out.push(p.display().to_string());
            }
        }
        Ok(())
    }
    let root_path = std::path::Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("scan root is not a directory: {root}"));
    }
    let mut out = Vec::new();
    walk(root_path, &mut out)?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// archlinux:latest container, before any `pacman -Sy` (2026-11-19):
    /// `yapacc inspect orphans` ended with pacman's two warnings as its
    /// error, and `yapacc list` showed every package as `dep,foreign`.
    #[test]
    fn a_missing_sync_database_is_neither_an_error_nor_a_foreign_mark() {
        let warnings = b"warning: database file for 'core' does not exist (use '-Sy' to download)\nwarning: database file for 'extra' does not exist (use '-Sy' to download)\n";
        assert!(is_empty_query_result("pacman", warnings));
        assert!(is_empty_query_result("pacman", b""));
        assert!(!is_empty_query_result(
            "pacman",
            b"error: failed to initialize alpm library\n"
        ));
        assert!(!is_empty_query_result("pactree", b""));
        assert_eq!(
            reason_and_repo(false, None),
            ("dep".to_string(), "unknown".to_string())
        );
        assert_eq!(
            reason_and_repo(true, Some(true)),
            ("explicit,foreign".to_string(), "local/aur".to_string())
        );
        assert_eq!(
            reason_and_repo(true, Some(false)),
            ("explicit".to_string(), "repo".to_string())
        );
    }

    #[test]
    fn elevation_prefix_follows_the_tool_the_machine_has() {
        assert_eq!(elevation_or_sudo(Ok(vec!["doas".into()])), vec!["doas"]);
        assert_eq!(
            elevation_or_sudo(Ok(vec!["doas".into(), "-n".into()])),
            vec!["doas", "-n"]
        );
        assert_eq!(elevation_or_sudo(Ok(vec!["sudo".into()])), vec!["sudo"]);
        // neither tool installed: the plan keeps saying sudo
        assert_eq!(
            elevation_or_sudo(Err(crate::error::Error::unavailable("none"))),
            vec!["sudo"]
        );
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pacman_targets() {
        for ok in ["htop", "extra/htop", "lib32-glibc", "g++", "python-3.14_x"] {
            assert!(valid_pacman_target(ok), "{ok}");
        }
        for bad in [
            "",
            "a;id",
            "-Syu",
            "--config=/tmp/evil.conf",
            "/htop",
            "extra/",
            "a/b/c",
            "x y",
            "$(id)",
        ] {
            assert!(!valid_pacman_target(bad), "{bad}");
        }
    }

    /// A7: `--json check` was an object on Arch and an array elsewhere.
    #[test]
    fn json_check_has_the_shape_of_the_other_managers() {
        let d = UpdateCheckData {
            repo_updates: vec!["cargo-deny 0.20.2-1 -> 0.21.0-1".into()],
            aur_updates: vec![("yay".into(), "12.5.0-1".into(), "12.5.2-1".into())],
            repo_check_ok: true,
            aur_check_ok: true,
            no_sync: None,
        };
        let (rows, note) = check_updates_rows(&d).unwrap();
        assert_eq!(note, None);
        assert_eq!(
            serde_json::to_string(&rows).unwrap(),
            r#"[{"name":"cargo-deny","old_version":"0.20.2-1","new_version":"0.21.0-1","repo":""},{"name":"yay","old_version":"12.5.0-1","new_version":"12.5.2-1","repo":"aur"}]"#
        );
        // the AUR did not answer: the repository rows, and a note
        let offline = UpdateCheckData {
            aur_check_ok: false,
            aur_updates: vec![],
            ..d.clone()
        };
        let (rows, note) = check_updates_rows(&offline).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(note.unwrap().contains("AUR could not be asked"));
        // pacman did not answer: an error, never an empty list
        let broken = UpdateCheckData {
            repo_check_ok: false,
            ..d
        };
        assert!(check_updates_rows(&broken).is_err());
        // nothing pending: an empty array
        let none = UpdateCheckData {
            repo_check_ok: true,
            aur_check_ok: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&check_updates_rows(&none).unwrap().0).unwrap(),
            "[]"
        );
    }

    /// VM round of 2026-11-19, Arch with no sync database (a fresh
    /// `archlinux` container before `pacman -Sy`): `check` printed "(no
    /// repo updates)" and "(no AUR updates)", and `--json check` printed
    /// `[]` with no note.
    #[test]
    fn check_without_a_sync_database_says_so_instead_of_no_updates() {
        let why = crate::native::no_metadata_message(crate::platform::PkgManager::Pacman, &[]);
        assert_eq!(
            why,
            "no repository metadata on disk: run `pacman -Syu` (yapacc does not refresh on a read command)"
        );
        // what `check_updates_data` gives in that state: pacman -Qu printed
        // nothing, and the AUR was not asked
        let d = UpdateCheckData {
            repo_check_ok: true,
            no_sync: Some(why.clone()),
            ..Default::default()
        };
        let text = render_check(&d);
        assert_eq!(
            text,
            format!(
                "== Repo updates (pacman -Qu, as of the last repository refresh) ==\nnote: {why}\n== AUR updates (RPC vercmp) ==\nnote: not checked: without the repository databases yapacc cannot tell which installed packages come from the AUR\n"
            )
        );
        assert!(!text.contains("(no repo updates)") && !text.contains("(no AUR updates)"));
        // --json: the same empty array, and the sentence as the note
        let (rows, note) = check_updates_rows(&d).unwrap();
        assert_eq!(serde_json::to_string(&rows).unwrap(), "[]");
        assert_eq!(note.as_deref(), Some(why.as_str()));
        // the field is not part of any JSON
        assert!(!serde_json::to_string(&d).unwrap().contains("no_sync"));
        // with the databases on disk nothing changes
        let synced = UpdateCheckData {
            repo_check_ok: true,
            aur_check_ok: true,
            ..Default::default()
        };
        assert_eq!(
            render_check(&synced),
            "== Repo updates (pacman -Qu, as of the last repository refresh) ==\n(no repo updates)\n== AUR updates (RPC vercmp) ==\n(no AUR updates)\n"
        );
        // an install in that state is refused with the command that
        // downloads the databases, not sent to the AUR
        let refusal = install_without_sync_message(&["figlet".to_string()]);
        assert_eq!(
            refusal,
            "cannot install figlet: no repository metadata on disk: run `pacman -Syu` as root first, or `yapacc upgrade`, which shows that command and asks. Nothing was changed."
        );
        assert!(!refusal.contains("AUR"));
    }

    /// VM round of 2026-11-19: the hint for a target no repository has
    /// named `yapacc aur pkgbuild NAME`, which failed with "failed to run
    /// yay: No such file or directory" (exit 1) on a system without yay;
    /// `query NAME --aur` exited 1 for a name the AUR does not have.
    #[test]
    fn an_aur_name_is_found_not_found_or_not_known() {
        let info = |name: &str| crate::aur_rpc::AurPkgInfo {
            name: name.to_string(),
            package_base: name.to_string(),
            ..Default::default()
        };
        assert!(matches!(
            aur_lookup_from("yay", Ok(vec![info("yay-bin"), info("yay")])),
            AurLookup::Found(i) if i.name == "yay"
        ));
        // the AUR answered `{"resultcount":0,"results":[]}`
        assert!(matches!(
            aur_lookup_from("no-such-package-xyz", Ok(vec![])),
            AurLookup::NotInAur
        ));
        // only another name came back: still not this one
        assert!(matches!(
            aur_lookup_from("yay", Ok(vec![info("yay-bin")])),
            AurLookup::NotInAur
        ));
        // curl's words when there is no network
        let offline = "AUR RPC request failed (curl exit exit status: 6): curl: (6) Could not resolve host: aur.archlinux.org";
        assert!(matches!(
            aur_lookup_from("yay", Err(offline.into())),
            AurLookup::Unreachable(e) if e == offline
        ));
        assert_eq!(
            not_in_aur_message("no-such-package-xyz"),
            "no-such-package-xyz is not in the AUR (`yapacc search no-such-package-xyz --aur` looks for similar names there)"
        );
        assert_eq!(
            aur_unreachable_message("yay", "curl: (6) Could not resolve host: aur.archlinux.org"),
            "the AUR (aur.archlinux.org) could not be reached, so yay could not be looked up: curl: (6) Could not resolve host: aur.archlinux.org"
        );
        // the hint names commands that need no helper program
        let hint = not_in_sync_repos_hint("no-such-package-xyz");
        assert!(hint.starts_with("- no-such-package-xyz [not in sync repos]\n  note: "));
        assert!(hint.contains("`yapacc search no-such-package-xyz --all`"));
        assert!(hint.contains("`yapacc install aur no-such-package-xyz`"));
        assert!(
            !hint.contains("aur pkgbuild") && !hint.contains("query"),
            "{hint}"
        );
    }

    /// A6: with pacman-contrib installed and `checkupdates` failing (no
    /// fakeroot), the note still said "Install pacman-contrib".
    #[test]
    fn a_failed_checkupdates_is_told_apart_from_a_missing_one() {
        assert_eq!(
            read_checkupdates(Some(0), "tree 2.2.1-1 -> 2.3.1-1\n\n", ""),
            CheckUpdates::Current(vec!["tree 2.2.1-1 -> 2.3.1-1".into()])
        );
        // status 2: nothing pending
        assert_eq!(
            read_checkupdates(Some(2), "", ""),
            CheckUpdates::Current(vec![])
        );
        // the real line of checkupdates without fakeroot
        assert_eq!(
            read_checkupdates(Some(1), "", "==> ERROR: Cannot find the fakeroot binary.\n"),
            CheckUpdates::Failed("status 1: Cannot find the fakeroot binary".into())
        );
        assert_eq!(
            read_checkupdates(Some(1), "", ""),
            CheckUpdates::Failed("status 1".into())
        );
        assert_eq!(
            read_checkupdates(None, "", ""),
            CheckUpdates::Failed("killed by a signal".into())
        );
    }

    #[test]
    fn package_arguments_always_follow_a_separator() {
        let hostile = names(&["htop", "--config=/tmp/evil.conf"]);
        for op in ["install", "remove", "install-local"] {
            for yay in [false, true] {
                let cmd = native_argv(op, &hostile, yay);
                let dash = cmd.iter().position(|a| a == "--").expect("a -- separator");
                let first = cmd.iter().position(|a| a == "htop").unwrap();
                assert!(dash < first, "{cmd:?}");
                assert_eq!(&cmd[dash + 1..], &hostile[..], "{cmd:?}");
            }
        }
        assert_eq!(
            mark_argv(&hostile, false),
            [
                "pacman",
                "-D",
                "--asdeps",
                "--",
                "htop",
                "--config=/tmp/evil.conf"
            ]
        );
        assert_eq!(
            download_argv(&names(&["htop"])),
            ["pacman", "-Sw", "--noconfirm", "--", "htop"]
        );
    }

    #[test]
    fn a_default_removal_keeps_modified_configuration_files() {
        // -Rs, not -Rns: -n would delete what pacman otherwise keeps as .pacsave
        assert_eq!(
            native_argv("remove", &names(&["htop"]), false),
            ["pacman", "-Rs", "--noconfirm", "--", "htop"]
        );
        for cmd in [
            native_argv("remove", &names(&["htop"]), false),
            preview_argv("remove", &names(&["htop"]), false).unwrap(),
        ] {
            assert!(
                !cmd.iter().any(|a| a.contains("Rns") || a == "-n"),
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn yay_is_never_told_noconfirm() {
        // the answer at yapacc's gate is not the review of AUR build code
        assert_eq!(
            native_argv("install", &names(&["foo"]), true),
            ["yay", "-S", "--", "foo"]
        );
        assert_eq!(native_argv("upgrade", &[], true), ["yay", "-Syu"]);
        // ops that never reach yay ignore the flag
        assert!(!goes_through_yay("remove", true));
        assert_eq!(native_argv("remove", &names(&["x"]), true)[0], "pacman");
        // yay elevates by itself: no prefix in front of it
        assert_eq!(build_command("install", &names(&["foo"]), true)[0], "yay");
    }

    /// A1: after `y` at yapacc's gate pacman asked its own "Proceed with
    /// installation? [Y/n]", because `--noconfirm` was passed only with
    /// --yes/--script. Consent is given once, at the gate.
    #[test]
    fn pacman_never_asks_again_after_the_gate() {
        let pkgs = names(&["tree"]);
        assert_eq!(
            native_argv("install", &pkgs, false),
            ["pacman", "-S", "--noconfirm", "--", "tree"]
        );
        assert_eq!(
            native_argv("install-local", &names(&["/tmp/x.pkg.tar.zst"]), false),
            ["pacman", "-U", "--noconfirm", "--", "/tmp/x.pkg.tar.zst"]
        );
        assert_eq!(
            native_argv("upgrade", &[], false),
            ["pacman", "-Syu", "--noconfirm"]
        );
        assert_eq!(
            native_argv("update", &[], false),
            ["pacman", "-Sy", "--noconfirm"]
        );
        for op in ["install", "remove", "install-local", "upgrade", "update"] {
            let cmd = native_argv(op, &pkgs, false);
            assert_eq!(
                cmd.iter().filter(|a| *a == "--noconfirm").count(),
                1,
                "{cmd:?}"
            );
            // an option, so before the `--` that ends the options
            if let Some(dash) = cmd.iter().position(|a| a == "--") {
                assert!(cmd.iter().position(|a| a == "--noconfirm").unwrap() < dash);
            }
        }
    }

    #[test]
    fn the_simulation_is_the_command_with_print() {
        let pkgs = names(&["htop", "extra/tree"]);
        for op in ["install", "remove", "install-local"] {
            let real = native_argv(op, &pkgs, false);
            let sim = preview_argv(op, &pkgs, false).unwrap();
            assert!(sim.iter().any(|i| i == "--print"), "{sim:?}");
            // same program, same operation flag, same targets after `--`
            assert_eq!(sim[..2], real[..2], "{op}");
            let cut = |c: &[String]| c[c.iter().position(|a| a == "--").unwrap()..].to_vec();
            assert_eq!(cut(&sim), cut(&real), "{op}");
            assert!(!sim.iter().any(|i| i == "--noconfirm"));
        }
        // an upgrade is simulated without the refresh, which needs root
        assert_eq!(
            preview_argv("upgrade", &[], false).unwrap()[..3],
            ["pacman", "-Su", "--print"]
        );
        // nothing to simulate: a refresh, and what yay builds
        assert!(preview_argv("update", &[], false).is_none());
        assert!(preview_argv("install", &pkgs, true).is_none());
        assert!(preview_argv("upgrade", &[], true).is_none());
        assert!(native_argv("no-such-op", &pkgs, false).is_empty());
    }

    #[test]
    fn plans_show_the_command_behind_its_privilege_prefix() {
        let sudo = names(&["sudo", "-n"]);
        let p = mark_plan(&names(&["htop"]), true, &sudo);
        assert!(
            p.contains("Will run:\n  sudo -n pacman -D --asexplicit -- htop\n"),
            "{p}"
        );
        let p = download_plan(&names(&["htop"]), &sudo);
        assert!(
            p.contains("Will run:\n  sudo -n pacman -Sw --noconfirm -- htop\n"),
            "{p}"
        );
        // -Scc --noconfirm takes pacman's default "no" and removes nothing:
        // the whole-cache clean is run without it and answered on stdin
        let p = cache_clean_plan(true, &[]);
        assert!(p.contains("Will run:\n  pacman -Scc\n"), "{p}");
        assert!(p.contains("answered y"), "{p}");
        assert_eq!(cache_clean_argv(true), ["pacman", "-Scc"]);
        assert_eq!(cache_clean_argv(false), ["pacman", "-Sc", "--noconfirm"]);
        assert!(cache_clean_plan(false, &sudo).contains("sudo -n pacman -Sc --noconfirm"));
        // a plan is not a dry-run report: the caller says which one it is
        assert!(!p.contains("DRY-RUN"));
    }

    #[test]
    fn a_missing_pactree_says_which_package_has_it() {
        let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(spawn_error("pactree", &gone).contains("pacman-contrib"));
        assert!(!spawn_error("pacman", &gone).contains("pacman-contrib"));
    }

    #[test]
    fn silent_pacman_failure_is_an_empty_answer() {
        assert!(is_empty_query_result("pacman", b""));
        assert!(is_empty_query_result("pacman", b"\n"));
        assert!(!is_empty_query_result(
            "pacman",
            b"error: package 'x' was not found\n"
        ));
        assert!(!is_empty_query_result("apk", b""));
    }

    #[test]
    fn parse_size_to_mib_units() {
        assert!((parse_size_to_mib("2.00 MiB").unwrap() - 2.0).abs() < 1e-9);
        assert!((parse_size_to_mib("1.00 GiB").unwrap() - 1024.0).abs() < 1e-9);
        assert!((parse_size_to_mib("512.00 KiB").unwrap() - 0.5).abs() < 1e-9);
        assert!(parse_size_to_mib("nonsense").is_none());
    }

    #[test]
    fn size_classes() {
        assert_eq!(size_class_from(Some("0.05 MiB")), "tiny(<0.1MiB)");
        assert_eq!(size_class_from(Some("5.00 MiB")), "medium(1-10MiB)");
        assert_eq!(size_class_from(Some("2.00 GiB")), "huge(>100MiB)");
        assert_eq!(size_class_from(None), "unknown");
    }

    #[test]
    fn classify_tags() {
        let (_p, kind, _t, cats) = classify("ttf-fira-code", "font for coding", "repo", &[], &[]);
        assert!(cats.iter().any(|c| c == "type:font"), "cats={cats:?}");
        assert_eq!(kind, "application/tool"); // fonts are not libraries/cli/gui kinds
        let (_p2, _k2, _t2, cats2) = classify(
            "docker-compose",
            "container tool",
            "aur",
            &["docker".to_string()],
            &[],
        );
        assert!(cats2.iter().any(|c| c == "containers"));
        assert!(cats2.iter().any(|c| c == "kw:docker"));
        assert!(cats2.iter().any(|c| c == "source:aur"));
    }

    #[test]
    fn search_rank_ordering() {
        assert!(
            search_match_rank("ripgrep", "ripgrep", "x")
                < search_match_rank("ripgrep", "ripgrep-git", "x")
        );
        assert!(
            search_match_rank("ripgrep", "ripgrep-git", "x")
                < search_match_rank("ripgrep", "other", "ripgrep tool")
        );
    }

    #[test]
    fn qi_block_parsing_multi_package() {
        let text = "Name           : foo\nVersion        : 1.0\nDescription    : a foo\n\nName           : bar\nVersion        : 2.0\nDescription    : a bar\nDepends On     : none\n               also-foo\n";
        let blocks = parse_qi_blocks(text);
        assert_eq!(blocks.len(), 2, "blocks: {blocks:?}");
        assert_eq!(blocks[0].get("Name").unwrap(), "foo");
        assert_eq!(blocks[1].get("Name").unwrap(), "bar");
        assert_eq!(blocks[1].get("Depends On").unwrap(), "none also-foo"); // continuation folded
    }
}
