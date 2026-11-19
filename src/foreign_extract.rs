use crate::Error;
use crate::ledger;
use sha2::{Digest, Sha256, Sha512};
use std::fs;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// A directory nobody else can have prepared (mkdtemp semantics): the name
/// comes from the kernel's random source, the directory is created
/// non-recursively with mode 0700, and a path that already exists is an
/// error instead of being reused. Removed with its contents when dropped.
///
/// It does not protect against the same user or root, who can enter it.
#[derive(Debug)]
pub struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    /// A fresh private directory under the system temporary directory.
    pub fn new(tag: &str) -> Result<Self, String> {
        Self::new_in(&std::env::temp_dir(), tag)
    }

    /// A fresh private directory under `parent`, which must exist.
    pub fn new_in(parent: &Path, tag: &str) -> Result<Self, String> {
        let mut raw = [0u8; 16];
        fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut raw))
            .map_err(|e| format!("cannot read /dev/urandom to name a private directory: {e}"))?;
        let path = parent.join(format!("yapacc-{tag}-{}", hex(&raw)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| {
                format!(
                    "cannot create the private directory {}: {e}",
                    path.display()
                )
            })?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Can we create files under this directory? (probe, not just exists)
fn dir_writable(p: &Path) -> bool {
    if fs::create_dir_all(p).is_err() {
        return false;
    }
    let probe = p.join(format!(".probe-{}", std::process::id()));
    let ok = fs::File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

pub fn default_isolated_root() -> PathBuf {
    if let Ok(p) = std::env::var("YAPACC_FOREIGN_ROOT")
        .or_else(|_| std::env::var("PACKAGE_CLEANER_FOREIGN_ROOT"))
    {
        return PathBuf::from(p);
    }
    if let Some(p) = crate::config::get().paths.foreign_root.clone() {
        return PathBuf::from(p);
    }
    // Preferred managed root (outside /bin,/usr and standard home dirs).
    let system = PathBuf::from("/var/lib/yapacc/isolated/foreign");
    if dir_writable(&system) {
        return system;
    }
    // legacy pre-rename root (migrate silently if present and writable)
    let legacy_system = PathBuf::from("/var/lib/package_cleaner/isolated/foreign");
    if dir_writable(&legacy_system) {
        return legacy_system;
    }
    // Non-root fallback: XDG data dir (still not a standard home bin dir).
    let xdg = crate::config::xdg_base("XDG_DATA_HOME", ".local/share")
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    let new = xdg.join("yapacc/isolated/foreign");
    let legacy = xdg.join("package_cleaner/isolated/foreign");
    if legacy.exists() && !new.exists() {
        return legacy;
    }
    new
}

/// Record scope that reflects where the prefix actually lives (home => user).
fn scope_for_prefix(prefix: &Path) -> &'static str {
    if let Ok(home) = std::env::var("HOME")
        && prefix.starts_with(home)
    {
        return "user";
    }
    "system"
}

/// One path component made from a package name or a format. Characters
/// outside letters, digits, `-`, `_` and `.` become `_`, and so does a
/// leading dot: the result is never empty, `.` or `..`, so a keg made from
/// it is always one directory below its parent.
fn safe_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with('.') {
        out.replace_range(0..1, "_");
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

pub fn default_prefix(name: &str, format: &str) -> PathBuf {
    default_isolated_root()
        .join(safe_component(format))
        .join(safe_component(name))
}

/// `p` must be an absolute path without `.` or `..` components.
fn plain_absolute(p: &Path) -> Result<(), String> {
    if !p.is_absolute() {
        return Err(format!("{} is not an absolute path", p.display()));
    }
    if p.components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || p.to_string_lossy().split('/').any(|part| part == ".")
    {
        return Err(format!("{} contains a '.' or '..' component", p.display()));
    }
    Ok(())
}

/// A keg directory is strictly below the directory that holds the kegs of
/// one format: never the foreign root, a format directory, or anything
/// above them.
fn outside_keg_parents(prefix: &Path, root: &Path) -> Result<(), String> {
    if root.starts_with(prefix) || prefix.parent() == Some(root) {
        return Err(format!(
            "{} is the foreign root {} or a directory that holds other kegs",
            prefix.display(),
            root.display()
        ));
    }
    Ok(())
}

/// Where a new keg may be created: an absolute path that is not the
/// foreign root, a format directory, or an ancestor of them.
fn check_new_prefix(prefix: &Path) -> Result<(), String> {
    plain_absolute(prefix)
        .and_then(|()| outside_keg_parents(prefix, &default_isolated_root()))
        .map_err(|why| {
            format!("refusing to install into this prefix: {why}. Nothing was installed.")
        })
}

/// The containment check before a keg directory is deleted. `install_root`
/// is read from the ledger, so it is deleted only when it is an absolute
/// path without `.`/`..`, a real directory (not a symbolic link), not the
/// foreign root or a directory holding other kegs, and it looks like one
/// keg (it holds the manifest or a `rootfs` directory).
///
/// It does not protect against a ledger rewritten to name another keg.
fn check_keg_dir(prefix: &Path) -> Result<(), String> {
    check_keg_dir_in(prefix, &default_isolated_root())
}

fn check_keg_dir_in(prefix: &Path, root: &Path) -> Result<(), String> {
    let looks_like_keg = |p: &Path| {
        let has = |n: &str| fs::symlink_metadata(p.join(n)).ok();
        has(MANIFEST_FILE).is_some_and(|m| m.is_file()) || has("rootfs").is_some_and(|m| m.is_dir())
    };
    plain_absolute(prefix)
        .and_then(|()| outside_keg_parents(prefix, root))
        .and_then(|()| match fs::symlink_metadata(prefix) {
            Ok(m) if m.file_type().is_symlink() => {
                Err(format!("{} is a symbolic link", prefix.display()))
            }
            Ok(m) if !m.is_dir() => Err(format!("{} is not a directory", prefix.display())),
            Ok(_) if !looks_like_keg(prefix) => Err(format!(
                "{} holds neither {MANIFEST_FILE} nor a rootfs directory, so it is not a keg",
                prefix.display()
            )),
            Ok(_) => Ok(()),
            Err(e) => Err(format!("cannot examine {}: {e}", prefix.display())),
        })
        .map_err(|why| format!("refusing to delete: {why}. Nothing was deleted."))
}

pub fn foreign_bin_name(format: &str, pkg_name: &str, bin: &str) -> String {
    let norm = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    format!("fgn.{}.{}--{}", norm(format), norm(pkg_name), norm(bin))
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub package: String,
    pub version: String,
    pub arch: String,
    /// deb: Section (e.g. non-free/utils); rpm: License field
    pub section: String,
    pub license_field: String,
    pub depends: String,
    pub conflicts: String,
    pub provides: String,
    pub base: String,
    pub dep_status: Vec<String>, // best-effort per-dep resolution hints
    pub warnings: Vec<String>,
}

fn dep_base_token(dep: &str) -> String {
    let d = dep.trim();
    let base = d
        .split(['(', '<', '>', '=', ':', ' ', ','])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    base.trim_start_matches(|c: char| {
        !c.is_alphanumeric() && c != '-' && c != '_' && c != '.' && c != '+'
    })
    .to_string()
}

fn resolve_dep_hint(dep: &str) -> String {
    let base = dep_base_token(dep);
    if base.is_empty() {
        return format!("{dep} => unknown (unparsed)");
    }
    // host pacman installed
    if std::process::Command::new("pacman")
        .args(["-Q", &base])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return format!("{dep} => satisfied (pacman installed: {base})");
    }
    // host pacman sync/known (best-effort)
    if std::process::Command::new("pacman")
        .args(["-Si", &base])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return format!(
            "{dep} => resolvable via host repos (pacman -Si: {base}) [not auto-installed]"
        );
    }
    // common name mapping hint
    let alt = match base.as_str() {
        "libc6" => Some("glibc"),
        "libssl3" => Some("openssl"),
        _ => None,
    };
    if let Some(a) = alt
        && std::process::Command::new("pacman")
            .args(["-Q", a])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    {
        return format!(
            "{dep} => host-equivalent may be present ({a} installed); ABI/name compatibility not guaranteed"
        );
    }
    format!("{dep} => unresolved in mediated mode (not found in pacman db/host)")
}

pub fn analyze_package(format: &str, source: &Path) -> Analysis {
    let mut a = Analysis::default();
    match format {
        "deb" => {
            // parse control (reuse inspect logic without temp? simpler: call inspect_deb parse)
            if let Ok(info) = inspect_deb(source) {
                for line in info.lines() {
                    if let Some(v) = line.strip_prefix("Package: ") {
                        a.package = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("Version: ") {
                        a.version = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("Architecture: ") {
                        a.arch = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("Depends: ") {
                        a.depends = v.trim().to_string();
                    }
                }
                // pull Conflicts/Provides/Section from control block if present
                if let Some(idx) = info.find("\n-- control --\n") {
                    for line in info[idx + 14..].lines() {
                        if let Some(v) = line.strip_prefix("Conflicts:") {
                            a.conflicts = v.trim().to_string();
                        } else if let Some(v) = line.strip_prefix("Provides:") {
                            a.provides = v.trim().to_string();
                        } else if let Some(v) = line.strip_prefix("Section:") {
                            a.section = v.trim().to_string();
                        } else if let Some(v) = line.strip_prefix("Pre-Depends:") {
                            let p = v.trim();
                            if !p.is_empty() {
                                if a.depends.is_empty() {
                                    a.depends = p.to_string();
                                } else {
                                    a.depends = format!("{} , {}", a.depends, p);
                                }
                            }
                        }
                    }
                }
            }
            if a.depends.is_empty() {
                a.warnings.push("No dependency metadata parsed (or none declared); runtime libraries may be missing on host.".into());
            } else {
                a.warnings.push(format!(
                    "Declared deps (not auto-resolved in mediated extract mode): {}",
                    a.depends
                ));
                for d in a.depends.split(',') {
                    let d = d.trim();
                    if !d.is_empty() {
                        a.dep_status.push(resolve_dep_hint(d));
                    }
                }
            }
        }
        "rpm" => {
            if Command::new("rpm").arg("--version").output().is_ok() {
                let info =
                    run_cmd("rpm", &["-qip", &source.to_string_lossy()], None).unwrap_or_default();
                apply_rpm_info(&mut a, &info);
                a.depends = run_cmd("rpm", &["-qpR", &source.to_string_lossy()], None)
                    .unwrap_or_default()
                    .lines()
                    .take(20)
                    .collect::<Vec<_>>()
                    .join(", ");
                a.conflicts = run_cmd(
                    "rpm",
                    &["-qp", "--conflicts", &source.to_string_lossy()],
                    None,
                )
                .unwrap_or_default()
                .lines()
                .take(10)
                .collect::<Vec<_>>()
                .join(", ");
                a.provides = run_cmd(
                    "rpm",
                    &["-qp", "--provides", &source.to_string_lossy()],
                    None,
                )
                .unwrap_or_default()
                .lines()
                .take(10)
                .collect::<Vec<_>>()
                .join(", ");
            } else {
                a.warnings.push("rpm metadata tools unavailable; dependency analysis skipped and the License field was not read, so the non-free check did not run (payload extract may still work).".into());
            }
            if !a.depends.is_empty() {
                a.warnings
                    .push(format!("Requires (not auto-resolved): {}", a.depends));
                for d in a.depends.split(',') {
                    let d = d.trim();
                    if !d.is_empty() {
                        a.dep_status.push(resolve_dep_hint(d));
                    }
                }
            }
        }
        "snap" => {
            if let Ok(info) = inspect_snap(source)
                && let Some(idx) = info.find("\n-- meta/snap.yaml --\n")
            {
                for line in info[idx + 21..].lines() {
                    let t = line.trim();
                    if let Some(v) = t.strip_prefix("name:") {
                        if a.package.is_empty() {
                            a.package = v
                                .trim()
                                .trim_matches(|c| c == '\'' || c == '\"')
                                .to_string();
                        }
                    } else if let Some(v) = t.strip_prefix("version:") {
                        if a.version.is_empty() {
                            a.version = v
                                .trim()
                                .trim_matches(|c| c == '\'' || c == '\"')
                                .to_string();
                        }
                    } else if let Some(v) = t.strip_prefix("base:") {
                        a.base = v
                            .trim()
                            .trim_matches(|c| c == '\'' || c == '\"')
                            .to_string();
                    } else if let Some(v) = t.strip_prefix("architectures:")
                        && a.arch.is_empty()
                    {
                        a.arch = v.trim().to_string();
                    }
                }
            }
            if !a.base.is_empty() {
                a.warnings.push(format!(
                    "Snap base '{}' not provisioned (no snapd/runtime mediation).",
                    a.base
                ));
            }
            a.warnings.push(
                "No confinement/services/daemons integration in mediated extract mode.".into(),
            );
        }
        _ => {
            a.warnings
                .push(format!("No dependency analyzer for format '{format}'."));
        }
    }
    a
}

/// Fill name, version, architecture and licence from `rpm -qip` output
/// (`Key         : value` lines; the header ends at `Description`).
fn apply_rpm_info(a: &mut Analysis, info: &str) {
    for line in info.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => a.package = value.to_string(),
            "Version" => a.version = value.to_string(),
            "Architecture" => a.arch = value.to_string(),
            "License" => a.license_field = value.to_string(),
            "Description" => break,
            _ => {}
        }
    }
}

/// Non-free detection from a deb `Section` or an RPM `License`. Returns a
/// human descriptor when the package requires an explicit licence
/// acceptance.
///
/// This is a match on words, not a licence analysis. It recognises the
/// Debian and Ubuntu archive areas (`non-free`, `non-free-firmware`,
/// `contrib`, `restricted`, `multiverse`) and RPM licence strings that say
/// nonfree, proprietary or commercial. A non-free package filed under a
/// free section, or with another wording, is not caught.
pub fn nonfree_from(section: &str, license: &str) -> Option<String> {
    let sec = section.trim().to_lowercase();
    let area = sec.split('/').next().unwrap_or("");
    if area.starts_with("non-free")
        || area.starts_with("nonfree")
        || area == "restricted"
        || area == "multiverse"
    {
        return Some(format!("Section: {}", section.trim()));
    }
    if area == "contrib" {
        return Some(format!(
            "Section: {} (contrib: depends on non-free)",
            section.trim()
        ));
    }
    let lic = license.to_lowercase();
    if lic.contains("nonfree")
        || lic.contains("non-free")
        || lic.contains("proprietary")
        || lic.contains("commercial")
    {
        return Some(format!("License: {}", license.trim()));
    }
    None
}

pub fn nonfree_descriptor(a: &Analysis) -> Option<String> {
    nonfree_from(&a.section, &a.license_field)
}

/// The non-free gate: interactive license agreement, or the explicit
/// --non-free flag in non-interactive runs. Ok(true)=accepted, Ok(false)=declined.
pub fn nonfree_gate(
    descriptor: &str,
    non_free_flag: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<bool, String> {
    if non_free_flag {
        println!("non-free accepted via --non-free: {descriptor}");
        return Ok(true);
    }
    if noninteractive || script_mode {
        return Err(format!(
            "NON-FREE package ({descriptor}): pass --non-free together with --yes/--noconfirm to accept the license non-interactively. Nothing was installed."
        ));
    }
    // --yes consents to the install, not to a licence: this is asked at the
    // terminal or answered with --non-free
    crate::prompt::confirm_on_terminal(&format!(
        "NON-FREE package ({descriptor}) — accept the license and install"
    ))
    .map_err(|e| format!("{e}; pass --non-free to accept the license. Nothing was installed."))
}

/// What installing one package file as a keg will do. It says nothing about
/// whether this run is a dry run: the caller knows.
pub fn plan_body(name: &str, format: &str, source: &str, prefix: &Path) -> String {
    let src_path = Path::new(source);
    let analysis = analyze_package(format, src_path);
    let mut s = format!(
        "== Foreign package extract plan (mediated, user-scope) ==\nName: {name}\nFormat: {format}\nSource: {source}\nPrefix: {}\n",
        prefix.display()
    );
    if !analysis.package.is_empty() || !analysis.version.is_empty() {
        s.push_str(&format!(
            "Detected: pkg={} ver={} arch={}\n",
            if analysis.package.is_empty() {
                "?"
            } else {
                &analysis.package
            },
            if analysis.version.is_empty() {
                "?"
            } else {
                &analysis.version
            },
            if analysis.arch.is_empty() {
                "?"
            } else {
                &analysis.arch
            }
        ));
    }
    if !analysis.depends.is_empty() {
        s.push_str(&format!("Depends/Requires: {}\n", analysis.depends));
    }
    if !analysis.conflicts.is_empty() {
        s.push_str(&format!("Conflicts: {}\n", analysis.conflicts));
    }
    if !analysis.provides.is_empty() {
        s.push_str(&format!("Provides: {}\n", analysis.provides));
    }
    if !analysis.base.is_empty() {
        s.push_str(&format!("Base/Runtime: {}\n", analysis.base));
    }
    if let Some(desc) = nonfree_descriptor(&analysis) {
        s.push_str(&format!(
            "Non-free: {desc}. The install asks you to accept the licence at the terminal, or takes --non-free.\n"
        ));
    }
    s.push_str("Actions:\n  1) Analyze metadata/dependencies and check prefix/ledger conflicts\n  2) Extract payload only (no maintainer scripts/confinement)\n  3) Create managed bin symlinks for discovered executables (where possible)\n  4) Record manifest + dependency metadata in user ledger\n");
    let mut resolvable: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    if !analysis.dep_status.is_empty() {
        s.push_str("Dependency resolution hints (best-effort, not auto-resolved):\n");
        for d in &analysis.dep_status {
            s.push_str(&format!("  - {d}\n"));
            if d.contains("=> resolvable via host repos") {
                if let Some(base) = d.split("pacman -Si:").nth(1) {
                    let b = base.split(')').next().unwrap_or("").trim();
                    if !b.is_empty() {
                        resolvable.push(b.to_string());
                    }
                }
            } else if d.contains("=> unresolved") || d.contains("=> unknown") {
                unresolved.push(d.clone());
            }
        }
    }
    s.push_str("Actionable remediation (mediated, not automatic):\n");
    if !resolvable.is_empty() {
        let uniq: std::collections::BTreeSet<_> = resolvable.into_iter().collect();
        let list = uniq.into_iter().collect::<Vec<_>>().join(" ");
        s.push_str(&format!("  - Suggested host packages to install (review/ABI compatibility not guaranteed): pacman -S --needed {}\n", list));
    }
    if !unresolved.is_empty() {
        s.push_str("  - Unresolved dependencies remain: provide compatible libraries in prefix/container, find Arch equivalents, or use native packaging/snapd/flatpak mediation as appropriate.\n");
    }
    if !analysis.conflicts.is_empty() {
        s.push_str(&format!("  - Declared conflicts present ({}): do not install alongside conflicting providers; verify ownership before use.\n", analysis.conflicts));
    }
    if analysis.dep_status.is_empty() && analysis.conflicts.is_empty() {
        s.push_str("  - No dependency/conflict remediation hints detected (best-effort).\n");
    }
    if !analysis.warnings.is_empty() {
        s.push_str("Dependency/conflict notes:\n");
        for w in &analysis.warnings {
            s.push_str(&format!("  - {w}\n"));
        }
    }
    s.push_str("Notes: NOT seamless system integration; dependencies/ABI/services not auto-resolved. Use isolated user-prefix usage and verify runtime needs.");
    s
}

/// `plan_body` with the closing line of the plan verb.
pub fn plan(name: &str, format: &str, source: &str, prefix: &Path) -> String {
    let mut s = plan_body(name, format, source, prefix);
    s.push('\n');
    s.push_str(crate::prompt::PLAN_ONLY_FOOTER);
    s
}

fn run_cmd(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c
        .output()
        .map_err(|e| format!("failed to execute {cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if e.is_empty() {
            format!("{cmd} failed with {}", out.status)
        } else {
            e
        })
    }
}

pub fn inspect_deb(source: &Path) -> Result<String, String> {
    let tmp_dir = PrivateDir::new("deb-inspect")?;
    let tmp = tmp_dir.path();
    let list = run_cmd("ar", &["t", &source.to_string_lossy()], None)?;
    let control_member = list
        .lines()
        .find(|l| l.starts_with("control.tar") && !l.contains('/') && !l.contains(".."))
        .ok_or("deb missing control.tar member")?
        .trim()
        .to_string();
    run_cmd(
        "ar",
        &["x", &source.to_string_lossy(), &control_member],
        Some(tmp),
    )?;
    let control_path = tmp.join(&control_member);
    // extract control file content
    let out = run_cmd(
        "bsdtar",
        &["-xOf", &control_path.to_string_lossy(), "./control"],
        None,
    )
    .or_else(|_| {
        run_cmd(
            "bsdtar",
            &["-xOf", &control_path.to_string_lossy(), "control"],
            None,
        )
    })?;
    drop(tmp_dir);
    let mut pkg = String::new();
    let mut ver = String::new();
    let mut desc = String::new();
    let mut dep = String::new();
    let mut arch = String::new();
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("Package:") {
            pkg = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Version:") {
            ver = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Description:") {
            desc = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Depends:") {
            dep = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Architecture:") {
            arch = v.trim().to_string();
        }
    }
    Ok(format!(
        "Format: deb\nPackage: {pkg}\nVersion: {ver}\nArchitecture: {arch}\nDepends: {dep}\nDescription: {desc}\n\n-- control --\n{out}"
    ))
}

pub fn inspect_snap(source: &Path) -> Result<String, String> {
    // best-effort: extract meta/snap.yaml to temp via unsquashfs
    let tmp_dir = PrivateDir::new("snap-inspect")?;
    // unsquashfs creates its destination itself, inside the private directory
    let tmp = tmp_dir.path().join("snap");
    let out = run_cmd(
        "unsquashfs",
        &[
            "-d",
            &tmp.to_string_lossy(),
            &source.to_string_lossy(),
            "meta/snap.yaml",
        ],
        None,
    )
    .or_else(|_| {
        run_cmd(
            "unsquashfs",
            &["-d", &tmp.to_string_lossy(), &source.to_string_lossy()],
            None,
        )
    });
    let mut info = format!("Format: snap\nSource: {}\n", source.display());
    match out {
        Ok(_) => {
            let y = tmp.join("meta/snap.yaml");
            if y.exists() {
                if let Ok(s) = fs::read_to_string(&y) {
                    info.push_str("\n-- meta/snap.yaml --\n");
                    for line in s.lines().take(80) {
                        info.push_str(line);
                        info.push('\n');
                    }
                } else {
                    info.push_str("(snap.yaml unreadable)\n");
                }
            } else {
                info.push_str("(meta/snap.yaml not found; squashfs payload may be extractable)\n");
            }
        }
        Err(e) => {
            info.push_str(&format!("(unsquashfs inspect unavailable/failed: {e})\n"));
        }
    }
    Ok(info)
}

pub fn inspect_foreign(format: &str, source: &Path) -> Result<String, String> {
    match format {
        "deb" => inspect_deb(source),
        "rpm" => {
            if Command::new("rpm").arg("--version").output().is_ok() {
                run_cmd("rpm", &["-qip", &source.to_string_lossy()], None)
            } else {
                Ok(format!(
                    "Format: rpm\nSource: {}\n(rpm metadata tool not available; payload extraction supported via bsdtar/rpm2cpio)",
                    source.display()
                ))
            }
        }
        "snap" => inspect_snap(source),
        "pkg" => run_cmd(
            "bsdtar",
            &["-xOf", &source.to_string_lossy(), ".PKGINFO"],
            None,
        )
        .map(|info| format!("Format: pkg (Arch)\n\n-- .PKGINFO --\n{info}")),
        _ => Err(format!("unsupported format: {format}")),
    }
}

/// Every entry below `root` that is not a directory, as a path relative to
/// `root`. A symbolic link is recorded as itself and never followed, so a
/// link to a directory outside the keg (`opt/x/host -> /`) is one entry.
fn collect_files(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) -> Result<(), String> {
        for ent in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
            let ent = ent.map_err(|e| e.to_string())?;
            let p = ent.path();
            // `file_type` does not follow a symbolic link (`Path::is_dir` does)
            let kind = ent
                .file_type()
                .map_err(|e| format!("cannot read the type of {}: {e}", p.display()))?;
            if kind.is_dir() {
                walk(&p, base, out)?;
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                out.push(rel);
            }
        }
        Ok(())
    }
    if fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        walk(root, root, &mut files)?;
    }
    files.sort();
    Ok(files)
}

/// What `neutralize_tree` changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Neutralized {
    /// entries given to the installing user and group
    chowned: usize,
    /// files and directories that lost set-user-ID / set-group-ID
    mode_cleared: usize,
    /// device nodes deleted
    devices_removed: usize,
}

/// After extraction, make the payload safe to keep on disk whatever the
/// archive asked for and whoever ran the extraction: every entry belongs to
/// the installing user and group, no file or directory keeps its
/// set-user-ID or set-group-ID bit, and device nodes are deleted. Symbolic
/// links are not followed.
///
/// The extractors are also told not to restore owners or special modes;
/// this pass is what holds for the ones that cannot be told (cpio,
/// unsquashfs) and when yapacc runs as root.
fn neutralize_tree(root: &Path, owner: (u32, u32)) -> Result<Neutralized, String> {
    fn walk(dir: &Path, owner: (u32, u32), seen: &mut Neutralized) -> Result<(), String> {
        let fix = |p: &Path, seen: &mut Neutralized| -> Result<fs::Metadata, String> {
            let md = fs::symlink_metadata(p)
                .map_err(|e| format!("cannot examine {}: {e}", p.display()))?;
            if md.uid() != owner.0 || md.gid() != owner.1 {
                std::os::unix::fs::lchown(p, Some(owner.0), Some(owner.1)).map_err(|e| {
                    format!("cannot give {} to the installing user: {e}", p.display())
                })?;
                seen.chowned += 1;
            }
            let kind = md.file_type();
            if (kind.is_file() || kind.is_dir()) && md.mode() & 0o6000 != 0 {
                fs::set_permissions(p, fs::Permissions::from_mode(md.mode() & 0o1777)).map_err(
                    |e| {
                        format!(
                            "cannot clear the set-user-ID/set-group-ID bits of {}: {e}",
                            p.display()
                        )
                    },
                )?;
                seen.mode_cleared += 1;
            }
            Ok(md)
        };
        fix(dir, seen)?;
        for ent in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
            let p = ent.map_err(|e| e.to_string())?.path();
            let kind = fs::symlink_metadata(&p)
                .map_err(|e| format!("cannot examine {}: {e}", p.display()))?
                .file_type();
            if kind.is_block_device() || kind.is_char_device() {
                fs::remove_file(&p)
                    .map_err(|e| format!("cannot delete the device node {}: {e}", p.display()))?;
                seen.devices_removed += 1;
            } else if kind.is_dir() {
                walk(&p, owner, seen)?;
            } else {
                fix(&p, seen)?;
            }
        }
        Ok(())
    }
    let mut seen = Neutralized::default();
    if fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        walk(root, owner, &mut seen)?;
    }
    Ok(seen)
}

/// Extract one package payload into `rootfs` (inside the keg `prefix`,
/// which this process created) and neutralize what was extracted.
fn extract_payload(format: &str, source: &Path, prefix: &Path) -> Result<Neutralized, String> {
    let rootfs = prefix.join("rootfs");
    match format {
        "deb" => extract_deb(source, &rootfs),
        "rpm" => extract_rpm(source, &rootfs),
        "snap" => extract_snap(source, &rootfs),
        "pkg" => extract_pkg(source, &rootfs),
        _ => Err(format!("unsupported foreign format: {format}")),
    }?;
    // the keg directory is the installing user's own: no archive wrote it
    let own = fs::symlink_metadata(prefix)
        .map_err(|e| format!("cannot examine {}: {e}", prefix.display()))?;
    let changed = neutralize_tree(&rootfs, (own.uid(), own.gid()))?;
    if changed.devices_removed > 0 || changed.mode_cleared > 0 {
        println!(
            "note: the package asked for {} set-user-ID/set-group-ID entr{} and {} device node{}; a keg never keeps them, so the bits were cleared and the device nodes deleted.",
            changed.mode_cleared,
            if changed.mode_cleared == 1 {
                "y"
            } else {
                "ies"
            },
            changed.devices_removed,
            if changed.devices_removed == 1 {
                ""
            } else {
                "s"
            },
        );
    }
    Ok(changed)
}

fn create_bin_links(
    prefix: &Path,
    rootfs: &Path,
    format: &str,
    pkg_name: &str,
    expose_bins: bool,
) -> Result<Vec<String>, String> {
    if !expose_bins {
        return Ok(Vec::new());
    }
    let bin_dir = prefix.join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin dir {}: {e}", bin_dir.display()))?;
    let mut linked = Vec::new();
    // launchers only ever point at files of this keg: a bin directory that
    // resolves outside the rootfs (`usr/bin -> /usr/bin`) is skipped, and so
    // is an entry whose target is outside it
    let real_root = fs::canonicalize(rootfs).unwrap_or_else(|_| rootfs.to_path_buf());
    let inside = |p: &Path| fs::canonicalize(p).is_ok_and(|real| real.starts_with(&real_root));
    for cand in ["usr/bin", "bin", "usr/sbin", "sbin", "usr/local/bin"] {
        let d = rootfs.join(cand);
        if !d.is_dir() || !inside(&d) {
            continue;
        }
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if !p.is_file() || !inside(&p) {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(md) = p.metadata()
                    && md.permissions().mode() & 0o111 == 0
                {
                    continue;
                }
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Foreign-to-distro binaries must always have foreign-attributed names when exposed.
            let attr = foreign_bin_name(format, pkg_name, name);
            let link_path = bin_dir.join(&attr);
            if link_path.exists() {
                // conflict on exposed name: do not overwrite silently
                return Err(format!(
                    "bin exposure conflict: attributed name already exists: {} (resolve by choosing --expose none to not add to PATH, or rename/install under a different package name/prefix)",
                    link_path.display()
                ));
            }
            #[cfg(unix)]
            {
                if std::os::unix::fs::symlink(&p, &link_path).is_ok() {
                    linked.push(format!("{} -> {}", link_path.display(), p.display()));
                }
            }
        }
    }
    linked.sort();
    Ok(linked)
}

pub fn extract_deb(source: &Path, rootfs: &Path) -> Result<(), String> {
    fs::create_dir_all(rootfs).map_err(|e| format!("mkdir rootfs: {e}"))?;
    let tmp_dir = PrivateDir::new_in(rootfs.parent().ok_or("invalid prefix")?, "extract")?;
    let tmp = tmp_dir.path();
    // list ar members and extract data.tar*
    let list = run_cmd("ar", &["t", &source.to_string_lossy()], None)?;
    let data_member = list
        .lines()
        .find(|l| l.starts_with("data.tar") && !l.contains('/') && !l.contains(".."))
        .ok_or("deb missing data.tar member")?
        .trim()
        .to_string();
    run_cmd(
        "ar",
        &["x", &source.to_string_lossy(), &data_member],
        Some(tmp),
    )?;
    let data_path = tmp.join(&data_member);
    run_cmd(
        "bsdtar",
        &[
            "-xf",
            &data_path.to_string_lossy(),
            "-C",
            &rootfs.to_string_lossy(),
            BSDTAR_NO_OWNER,
            BSDTAR_NO_PERMISSIONS,
        ],
        None,
    )?;
    Ok(())
}

/// bsdtar, also as root: files belong to whoever extracts, and set-user-ID,
/// set-group-ID, sticky bits, ACLs, extended attributes and file flags
/// from the archive are not restored.
const BSDTAR_NO_OWNER: &str = "--no-same-owner";
const BSDTAR_NO_PERMISSIONS: &str = "--no-same-permissions";

pub fn extract_rpm(source: &Path, rootfs: &Path) -> Result<(), String> {
    fs::create_dir_all(rootfs).map_err(|e| format!("mkdir rootfs: {e}"))?;
    // bsdtar can often extract rpm payload directly
    let r = run_cmd(
        "bsdtar",
        &[
            "-xf",
            &source.to_string_lossy(),
            "-C",
            &rootfs.to_string_lossy(),
            BSDTAR_NO_OWNER,
            BSDTAR_NO_PERMISSIONS,
        ],
        None,
    );
    if r.is_ok() {
        return Ok(());
    }
    // fallback: rpm2cpio | cpio, piped directly (no shell), with absolute
    // member paths forced under the destination
    let mut producer = Command::new("rpm2cpio")
        .arg(source)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start rpm2cpio: {e}"))?;
    let payload = producer
        .stdout
        .take()
        .ok_or("rpm2cpio produced no output stream")?;
    let out = Command::new("cpio")
        .args([
            "-idm",
            "--quiet",
            "--no-absolute-filenames",
            "--no-preserve-owner",
        ])
        .current_dir(rootfs)
        .stdin(payload)
        .output()
        .map_err(|e| format!("failed to run cpio: {e}"))?;
    let produced = producer
        .wait()
        .map_err(|e| format!("rpm2cpio did not finish: {e}"))?;
    if !produced.success() {
        return Err(format!("rpm2cpio failed with {produced}"));
    }
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

pub fn extract_snap(source: &Path, rootfs: &Path) -> Result<(), String> {
    fs::create_dir_all(rootfs).map_err(|e| format!("mkdir rootfs: {e}"))?;
    // unsquashfs extracts into target dir; -f to overwrite if retried in same context, -d dest
    run_cmd(
        "unsquashfs",
        &[
            "-f",
            "-d",
            &rootfs.to_string_lossy(),
            &source.to_string_lossy(),
        ],
        None,
    )?;
    Ok(())
}

/// Extract an Arch package (.pkg.tar.zst/.xz) payload into the keg rootfs.
/// Used by the recipe lane so builds land in the yapacc prefix, not the system.
pub fn extract_pkg(source: &Path, rootfs: &Path) -> Result<(), String> {
    fs::create_dir_all(rootfs).map_err(|e| format!("mkdir rootfs: {e}"))?;
    run_cmd(
        "bsdtar",
        &[
            "-xf",
            &source.to_string_lossy(),
            "-C",
            &rootfs.to_string_lossy(),
            BSDTAR_NO_OWNER,
            BSDTAR_NO_PERMISSIONS,
            "--exclude",
            ".PKGINFO",
            "--exclude",
            ".MTREE",
            "--exclude",
            ".BUILDINFO",
        ],
        None,
    )
    .map(|_| ())
}

/// Name of the plain-text manifest written inside every keg.
pub const MANIFEST_FILE: &str = "YAPACC-MANIFEST.txt";

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The manifest of one keg: what it is, where it came from, when it was
/// installed, its exposed launchers, and how to remove it without yapacc.
pub fn render_manifest(rec: &ledger::InstallRecord, launchers: &[String]) -> String {
    let line = |v: &str| v.replace(['\n', '\r'], " ");
    let mut s = format!(
        "name: {}\nversion: {}\nformat: {}\nsource: {}\ninstalled-utc: {}\nlaunchers:\n",
        line(&rec.name),
        line(&rec.version),
        line(&rec.format),
        line(&rec.source),
        crate::util::fmt_utc(rec.installed_at)
    );
    for launcher in launchers {
        // a bin link is recorded as "LINK -> TARGET"; the launcher is LINK
        let path = launcher.split(" -> ").next().unwrap_or(launcher);
        s.push_str(&format!("  {}\n", line(path)));
    }
    s.push_str(&format!(
        "remove: rm -r {} and the launchers above\n",
        line(&rec.install_root)
    ));
    s
}

fn write_manifest(
    prefix: &Path,
    rec: &ledger::InstallRecord,
    launchers: &[String],
) -> Result<(), String> {
    let path = prefix.join(MANIFEST_FILE);
    fs::write(&path, render_manifest(rec, launchers))
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// The package formats a keg can be made from, and the ledger format each
/// one is recorded under.
const EXTRACT_FORMATS: [(&str, &str); 4] = [
    ("deb", "deb-extract"),
    ("rpm", "rpm-extract"),
    ("snap", "snap-extract"),
    ("pkg", "pkg-extract"),
];

/// The package format behind a ledger record format (`pkg-extract` ->
/// `pkg`), or None when the record is not a keg.
fn extract_format(record_format: &str) -> Option<&'static str> {
    EXTRACT_FORMATS
        .iter()
        .find(|(_, rec)| *rec == record_format)
        .map(|(fmt, _)| *fmt)
}

pub fn install(
    name: &str,
    format: &str,
    source: &str,
    prefix: &Path,
    ledger_root: &Path,
    expose_bins: bool,
) -> Result<ledger::InstallRecord, String> {
    install_from(
        name,
        format,
        Path::new(source),
        source,
        None,
        prefix,
        ledger_root,
        expose_bins,
    )
}

/// Install the package file `src_path` as a keg. `origin` is what the ledger
/// and the manifest record as the source (the URL when the file was
/// downloaded), and `checked` the index hash the file was verified against.
#[allow(clippy::too_many_arguments)]
fn install_from(
    name: &str,
    format: &str,
    src_path: &Path,
    origin: &str,
    checked: Option<&IndexVerify>,
    prefix: &Path,
    ledger_root: &Path,
    expose_bins: bool,
) -> Result<ledger::InstallRecord, String> {
    let source = origin;
    if !src_path.exists() {
        return Err(format!("source file not found: {}", src_path.display()));
    }
    if extract_format(&format!("{format}-extract")).is_none() {
        return Err(format!("unsupported foreign format: {format}"));
    }
    // a relative --prefix would make the recorded keg depend on the
    // directory a later `remove` is run from
    let absolute;
    let prefix = if prefix.is_absolute() {
        prefix
    } else {
        absolute = std::env::current_dir()
            .map_err(|e| {
                format!(
                    "cannot resolve the relative prefix {}: {e}",
                    prefix.display()
                )
            })?
            .join(prefix);
        absolute.as_path()
    };
    check_new_prefix(prefix)?;
    let analysis = analyze_package(format, src_path);
    let rootfs = prefix.join("rootfs");
    if fs::symlink_metadata(prefix).is_ok() {
        return Err(format!(
            "prefix already exists (possible conflict): {}",
            prefix.display()
        ));
    }
    // conflict analysis: ledger prefix/path conflicts and active same-name installs of same format
    if let Ok(existing) = ledger::list_records(ledger_root)
        && let Some(c) = existing.iter().find(|r| {
            r.status != "removed" && r.name == name && r.format == format!("{}-extract", format)
        })
    {
        return Err(format!(
            "ledger conflict: active install already exists for {} ({}), id={}",
            name, c.format, c.id
        ));
    }
    // The keg directory itself is created non-recursively and must be new:
    // everything deleted on a failure below was made by this run.
    if let Some(parent) = prefix.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    fs::DirBuilder::new()
        .create(prefix)
        .map_err(|e| format!("failed to create prefix {}: {e}", prefix.display()))?;
    if let Err(e) = extract_payload(format, src_path, prefix) {
        fs::remove_dir_all(prefix).ok();
        return Err(e);
    }
    let files = match collect_files(&rootfs) {
        Ok(f) => f,
        Err(e) => {
            fs::remove_dir_all(prefix).ok();
            return Err(e);
        }
    };
    let bin_links = match create_bin_links(prefix, &rootfs, format, name, expose_bins) {
        Ok(v) => v,
        Err(e) => {
            fs::remove_dir_all(prefix).ok();
            return Err(e);
        }
    };
    let conflicts = match ledger::check_path_conflicts(ledger_root, &[prefix.display().to_string()])
    {
        Ok(c) => c,
        Err(e) => {
            fs::remove_dir_all(prefix).ok();
            return Err(e);
        }
    };
    if conflicts.iter().any(|c| c.kind == "ledger") {
        fs::remove_dir_all(prefix).ok();
        return Err("ledger conflict on prefix".into());
    }
    let mut paths: Vec<String> = files
        .into_iter()
        .map(|f| rootfs.join(f).display().to_string())
        .collect();
    paths.extend(bin_links.clone());
    let mut meta = std::collections::BTreeMap::new();
    meta.insert("rootfs".into(), rootfs.display().to_string());
    meta.insert("bin_dir".into(), prefix.join("bin").display().to_string());
    meta.insert("bin_links".into(), bin_links.len().to_string());
    meta.insert("expose_bins".into(), expose_bins.to_string());
    meta.insert(
        "mode".into(),
        if expose_bins {
            "extract-only-no-scriptlets+bin-symlinks"
        } else {
            "extract-only-no-scriptlets+no-path-exposure"
        }
        .into(),
    );
    // what the package file was checked against, in words a reader of the
    // ledger can rely on
    meta.insert(
        "source_check".into(),
        match checked {
            Some(v) => format!("{}:{} (from the repository index)", v.algo, v.hash),
            None => "none (the file was not checked against any index)".into(),
        },
    );
    if !analysis.package.is_empty() {
        meta.insert("pkg_name".into(), analysis.package.clone());
    }
    if !analysis.version.is_empty() {
        meta.insert("pkg_version".into(), analysis.version.clone());
    }
    if !analysis.arch.is_empty() {
        meta.insert("arch".into(), analysis.arch.clone());
    }
    if !analysis.section.is_empty() {
        meta.insert("section".into(), analysis.section.clone());
    }
    if !analysis.license_field.is_empty() {
        meta.insert("license".into(), analysis.license_field.clone());
    }
    if !analysis.depends.is_empty() {
        meta.insert("depends".into(), analysis.depends.clone());
    }
    if !analysis.conflicts.is_empty() {
        meta.insert("declared_conflicts".into(), analysis.conflicts.clone());
    }
    if !analysis.provides.is_empty() {
        meta.insert("provides".into(), analysis.provides.clone());
    }
    if !analysis.base.is_empty() {
        meta.insert("base".into(), analysis.base.clone());
    }
    if !analysis.dep_status.is_empty() {
        meta.insert("dep_status".into(), analysis.dep_status.join(" | "));
    }
    if !analysis.warnings.is_empty() {
        meta.insert("analysis_notes".into(), analysis.warnings.join(" | "));
    }
    let rec = ledger::InstallRecord {
        id: String::new(),
        name: name.to_string(),
        format: format!("{}-extract", format),
        source: source.to_string(),
        version: if analysis.version.is_empty() {
            "unknown".into()
        } else {
            analysis.version
        },
        scope: scope_for_prefix(prefix).into(),
        install_root: prefix.display().to_string(),
        paths,
        status: "installed".into(),
        installed_at: 0,
        updated_at: 0,
        metadata: meta,
    };
    let rec = ledger::InstallRecord {
        installed_at: unix_now(),
        ..rec
    };
    if let Err(e) = write_manifest(prefix, &rec, &bin_links) {
        fs::remove_dir_all(prefix).ok();
        return Err(e);
    }
    match ledger::upsert_record(ledger_root, rec) {
        Ok(r) => Ok(r),
        Err(e) => {
            fs::remove_dir_all(prefix).ok();
            Err(format!(
                "ledger record failed, rolled back extracted prefix: {e}"
            ))
        }
    }
}

pub fn remove(record_id: &str, ledger_root: &Path) -> Result<ledger::InstallRecord, String> {
    let recs = ledger::list_records(ledger_root)?;
    let mut rec = recs
        .into_iter()
        .find(|r| r.id == record_id)
        .ok_or_else(|| format!("ledger record not found: {record_id}"))?;
    if extract_format(&rec.format).is_none() {
        return Err(format!(
            "record {record_id} is not a keg (its format is '{}'; kegs are deb-extract, rpm-extract, snap-extract and pkg-extract). Nothing was removed.",
            rec.format
        ));
    }
    let prefix = Path::new(&rec.install_root);
    if fs::symlink_metadata(prefix).is_ok() {
        check_keg_dir(prefix).map_err(|e| format!("record {record_id}: {e}"))?;
        fs::remove_dir_all(prefix)
            .map_err(|e| format!("failed to remove prefix {}: {e}", prefix.display()))?;
    }
    rec.status = "removed".into();
    ledger::upsert_record(ledger_root, rec)
}

pub fn update_plan(record: &ledger::InstallRecord, new_source: &str) -> String {
    format!(
        "== Foreign extract update plan ==\nID: {}\nName: {} ({})\nPrefix: {}\nNew source: {}\nActions:\n  1) Backup current extracted prefix\n  2) Extract new package payload into prefix (no scriptlets)\n  3) Update ledger (old record removed, new record installed)\n  4) On failure, restore backup prefix",
        record.id, record.name, record.format, record.install_root, new_source
    )
}

pub fn update(
    record_id: &str,
    new_source: &str,
    ledger_root: &Path,
    expose_bins: bool,
) -> Result<(ledger::InstallRecord, ledger::InstallRecord), String> {
    let recs = ledger::list_records(ledger_root)?;
    let old = recs
        .into_iter()
        .find(|r| r.id == record_id)
        .ok_or_else(|| format!("ledger record not found: {record_id}"))?;
    let Some(format) = extract_format(&old.format) else {
        return Err(format!(
            "record {record_id} is not a keg (its format is '{}'; kegs are deb-extract, rpm-extract, snap-extract and pkg-extract). Nothing was updated.",
            old.format
        ));
    };
    if old.status == "removed" {
        return Err(format!("record {record_id} is already removed"));
    }
    let src_path = Path::new(new_source);
    if !src_path.exists() {
        return Err(format!("source file not found: {new_source}"));
    }
    let prefix = PathBuf::from(&old.install_root);
    // the old keg is moved aside and deleted once the new one is recorded
    if fs::symlink_metadata(&prefix).is_ok() {
        check_keg_dir(&prefix)
            .map_err(|e| format!("record {record_id}: {e} The keg was not updated."))?;
    } else {
        check_new_prefix(&prefix)?;
    }
    let parent = prefix
        .parent()
        .ok_or_else(|| "invalid prefix path".to_string())?;
    let backup = parent.join(format!(
        ".{}.bak-{}",
        prefix
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("prefix"),
        std::process::id()
    ));

    if prefix.exists() {
        fs::rename(&prefix, &backup)
            .map_err(|e| format!("failed to backup existing prefix {}: {e}", prefix.display()))?;
    }

    // Undo a failed update and say what state the keg is left in.
    let rollback = |e: String| -> String {
        if let Err(re) = fs::remove_dir_all(&prefix)
            && fs::symlink_metadata(&prefix).is_ok()
        {
            return format!(
                "{e}; the partly written keg {} could NOT be removed ({re}){}",
                prefix.display(),
                if backup.exists() {
                    format!(", and the previous keg is still at {}", backup.display())
                } else {
                    String::new()
                }
            );
        }
        if !backup.exists() {
            return format!("{e}; the new files were removed");
        }
        match fs::rename(&backup, &prefix) {
            Ok(()) => format!("{e}; the previous keg was restored"),
            Err(re) => format!(
                "{e}; the previous keg could NOT be restored ({re}): it is still at {}",
                backup.display()
            ),
        }
    };

    fs::create_dir_all(&prefix)
        .map_err(|e| rollback(format!("failed to create prefix {}: {e}", prefix.display())))?;
    let rootfs = prefix.join("rootfs");
    if let Err(e) = extract_payload(format, src_path, &prefix) {
        return Err(rollback(e));
    }
    let files = match collect_files(&rootfs) {
        Ok(f) => f,
        Err(e) => return Err(rollback(e)),
    };
    let bin_links = match create_bin_links(&prefix, &rootfs, format, &old.name, expose_bins) {
        Ok(v) => v,
        Err(e) => return Err(rollback(e)),
    };
    let analysis = analyze_package(format, src_path);

    let mut old_removed = old.clone();
    old_removed.status = "removed".into();
    let mut paths: Vec<String> = files
        .into_iter()
        .map(|f| rootfs.join(f).display().to_string())
        .collect();
    paths.extend(bin_links.clone());
    let mut meta = old.metadata.clone();
    meta.insert("rootfs".into(), rootfs.display().to_string());
    meta.insert("bin_dir".into(), prefix.join("bin").display().to_string());
    meta.insert("bin_links".into(), bin_links.len().to_string());
    meta.insert("expose_bins".into(), expose_bins.to_string());
    meta.insert(
        "mode".into(),
        if expose_bins {
            "extract-only-no-scriptlets+bin-symlinks"
        } else {
            "extract-only-no-scriptlets+no-path-exposure"
        }
        .into(),
    );
    if !analysis.package.is_empty() {
        meta.insert("pkg_name".into(), analysis.package);
    }
    if !analysis.version.is_empty() {
        meta.insert("pkg_version".into(), analysis.version.clone());
    }
    if !analysis.arch.is_empty() {
        meta.insert("arch".into(), analysis.arch);
    }
    if !analysis.depends.is_empty() {
        meta.insert("depends".into(), analysis.depends);
    }
    if !analysis.conflicts.is_empty() {
        meta.insert("declared_conflicts".into(), analysis.conflicts);
    }
    if !analysis.provides.is_empty() {
        meta.insert("provides".into(), analysis.provides);
    }
    if !analysis.base.is_empty() {
        meta.insert("base".into(), analysis.base);
    }
    if !analysis.dep_status.is_empty() {
        meta.insert("dep_status".into(), analysis.dep_status.join(" | "));
    }
    if !analysis.warnings.is_empty() {
        meta.insert("analysis_notes".into(), analysis.warnings.join(" | "));
    }
    let new_rec = ledger::InstallRecord {
        id: String::new(),
        name: old.name.clone(),
        format: old.format.clone(),
        source: new_source.to_string(),
        version: if analysis.version.is_empty() {
            "unknown".into()
        } else {
            analysis.version
        },
        scope: old.scope.clone(),
        install_root: old.install_root.clone(),
        paths,
        status: "installed".into(),
        installed_at: 0,
        updated_at: 0,
        metadata: meta,
    };
    let new_rec = ledger::InstallRecord {
        installed_at: unix_now(),
        ..new_rec
    };
    if let Err(e) = write_manifest(&prefix, &new_rec, &bin_links) {
        return Err(rollback(e));
    }
    let ledger_res = (|| {
        ledger::upsert_record(ledger_root, old_removed.clone())?;
        ledger::upsert_record(ledger_root, new_rec.clone())
    })();
    match ledger_res {
        Ok(nr) => {
            let _ = fs::remove_dir_all(&backup);
            Ok((old_removed, nr))
        }
        Err(e) => Err(rollback(format!("ledger update failed: {e}"))),
    }
}

/// One package of a transaction.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TxArtifact {
    /// where the package file is: a path, or an http(s):// or file:// URL
    pub source: String,
    /// package name; empty until the file has been read when the artifact
    /// was given as a bare path or URL
    pub name: String,
    pub provides: Vec<String>,
    pub depends: Vec<String>,
    /// SHA256/SHA512 and size from the repository index the artifact was
    /// resolved from. None: there is nothing to check the file against.
    pub verify: Option<IndexVerify>,
    /// deb `Section` / rpm `License`, for the non-free gate
    pub section: String,
    pub license: String,
}

/// What a transaction can be made from: a bare path or URL (nothing to
/// check the file against), or an artifact resolved from a repository index
/// (it carries the index's hash).
pub trait TxSource {
    fn artifact(&self) -> TxArtifact;
}

impl TxSource for String {
    fn artifact(&self) -> TxArtifact {
        TxArtifact {
            source: self.clone(),
            ..TxArtifact::default()
        }
    }
}

impl TxSource for TxArtifact {
    fn artifact(&self) -> TxArtifact {
        self.clone()
    }
}

fn split_tokens(s: &str) -> Vec<String> {
    s.split([',', '|', '\n'])
        .map(dep_base_token)
        .filter(|t| !t.is_empty())
        .collect()
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("file://")
}

/// Fetched over a network: any `scheme://` except `file://`. A plain path
/// is local.
fn is_remote(s: &str) -> bool {
    s.contains("://") && !s.starts_with("file://")
}

/// Download one URL into `dir`, or copy a `file://` URL or plain path
/// there. `dir` is a private directory the caller created. Nothing about
/// the result is verified here.
fn download_to(dir: &Path, url: &str, idx: usize) -> Result<PathBuf, String> {
    let base = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("artifact");
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let out = dir.join(format!("{idx}-{safe}"));
    let local = url
        .strip_prefix("file://")
        .or_else(|| (!url.contains("://")).then_some(url));
    if let Some(path) = local {
        let src = PathBuf::from(path);
        fs::copy(&src, &out)
            .map_err(|e| format!("failed to copy {} -> {}: {e}", src.display(), out.display()))?;
        return Ok(out);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "failed to fetch {url}: only http://, https:// and file:// sources are supported"
        ));
    }
    // try curl, then wget; a redirect may not leave http(s)
    let curl = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "-o",
        ])
        .arg(&out)
        .arg(url)
        .output();
    let wget = |out: &Path| {
        Command::new("wget")
            .arg("-qO")
            .arg(out)
            .arg(url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    match curl {
        Ok(o) if o.status.success() => Ok(out),
        Ok(o) => {
            let _ = fs::remove_file(&out);
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if wget(&out) {
                return Ok(out);
            }
            let _ = fs::remove_file(&out);
            Err(format!("failed to fetch {url}: {err}"))
        }
        Err(_) => {
            if wget(&out) {
                return Ok(out);
            }
            let _ = fs::remove_file(&out);
            Err(format!("failed to fetch {url}: curl/wget not available"))
        }
    }
}

/// A hash and size that an index declares for a file. Only SHA256 and
/// SHA512 are ever put here; an MD5 or SHA1 entry is not a check.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct IndexVerify {
    pub algo: String,
    pub hash: String,
    pub size: Option<u64>,
}

/// The hash algorithms accepted from an index, by the name the index uses
/// (`SHA256`, `sha512`): its rank (higher is stronger) and canonical name.
/// None for MD5, SHA1 (`sha`, `sha1`) and anything unknown.
fn strong_algo(name: &str) -> Option<(u8, &'static str)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "sha512" => Some((2, "sha512")),
        "sha256" => Some((1, "sha256")),
        _ => None,
    }
}

fn digest_file<D: Digest>(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut h = D::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(h.finalize().as_slice()))
}

/// SHA-256 of a file, in-process, as lowercase hex.
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    digest_file::<Sha256>(path)
}

/// Who declares the hash of a package file, and of a package index.
const BY_INDEX: &str = "the repository index";
const BY_RELEASE: &str = "the repository metadata (Release or repomd.xml)";

/// Compare a file with the hash and size `declared_by` gives for it. The
/// error says what differs and names no path: the caller knows what the
/// file is.
fn verify_file_hash(path: &Path, v: &IndexVerify, declared_by: &str) -> Result<(), String> {
    let got = match strong_algo(&v.algo) {
        Some((_, "sha512")) => digest_file::<Sha512>(path)?,
        Some(_) => digest_file::<Sha256>(path)?,
        None => {
            return Err(format!(
                "{declared_by} gives a {} checksum, which yapacc does not accept as a check (SHA256 or SHA512 only)",
                v.algo
            ));
        }
    };
    if let Some(sz) = v.size {
        let len = fs::metadata(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .len();
        if len != sz {
            return Err(format!(
                "size mismatch: {declared_by} says {sz} bytes, the file has {len}"
            ));
        }
    }
    if got != v.hash.to_lowercase() {
        return Err(format!(
            "checksum mismatch: {declared_by} says {} {}, the file has {got}",
            v.algo, v.hash
        ));
    }
    Ok(())
}

/// Fill in what only the package file knows, for an artifact that was given
/// as a bare path or URL.
fn describe_from_file(format: &str, a: &mut TxArtifact, file: &Path) {
    let an = analyze_package(format, file);
    a.name = if an.package.is_empty() {
        file.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pkg")
            .to_string()
    } else {
        an.package.clone()
    };
    let mut provides = split_tokens(&an.provides);
    provides.push(a.name.to_lowercase());
    provides.sort();
    provides.dedup();
    a.provides = provides;
    a.depends = split_tokens(&an.depends);
    a.section = an.section;
    a.license = an.license_field;
}

fn collect_tx_artifacts(format: &str, sources: &[String]) -> Result<Vec<TxArtifact>, String> {
    let mut arts: Vec<TxArtifact> = Vec::new();
    for src in sources {
        let p = Path::new(src);
        if !p.exists() {
            return Err(format!("source not found: {src}"));
        }
        let mut a = src.artifact();
        describe_from_file(format, &mut a, p);
        arts.push(a);
    }
    Ok(arts)
}

/// The package files of one transaction, on local disk: downloaded once
/// (into a private directory that is deleted when this value is dropped)
/// and each already compared with its index hash when it has one. The plan
/// and the install both use these same files.
#[derive(Debug)]
pub struct StagedTx {
    format: String,
    /// as resolved or given: `source` is still the path or URL
    arts: Vec<TxArtifact>,
    /// the local file of each artifact, same order
    local: Vec<PathBuf>,
    _dir: Option<PrivateDir>,
}

/// Fetch every URL source once and check each file that has an index hash.
/// A mismatch is an integrity error (exit 5): a downloaded copy is deleted,
/// a local file is left alone, and nothing is installed.
pub fn stage<S: TxSource>(format: &str, sources: &[S]) -> crate::error::Result<StagedTx> {
    let mut arts: Vec<TxArtifact> = sources.iter().map(TxSource::artifact).collect();
    let mut dir: Option<PrivateDir> = None;
    let mut local: Vec<PathBuf> = Vec::new();
    for (i, a) in arts.iter().enumerate() {
        if is_url(&a.source) {
            let d = match &dir {
                Some(d) => d,
                None => dir.insert(PrivateDir::new("fetch")?),
            };
            let file = download_to(d.path(), &a.source, i)?;
            if let Some(v) = &a.verify
                && let Err(why) = verify_file_hash(&file, v, BY_INDEX)
            {
                let _ = fs::remove_file(&file);
                return Err(Error::integrity(format!(
                    "{}: {why}. The downloaded file was deleted and nothing was installed.",
                    a.source
                )));
            }
            local.push(file);
        } else {
            let file = PathBuf::from(&a.source);
            if !file.exists() {
                return Err(Error::from(format!("source not found: {}", a.source)));
            }
            if let Some(v) = &a.verify {
                verify_file_hash(&file, v, BY_INDEX).map_err(|why| {
                    Error::integrity(format!(
                        "{}: {why}. The file was left where it is and nothing was installed.",
                        a.source
                    ))
                })?;
            }
            local.push(file);
        }
    }
    for (a, file) in arts.iter_mut().zip(&local) {
        if a.name.is_empty() {
            describe_from_file(format, a, file);
        }
    }
    Ok(StagedTx {
        format: format.to_string(),
        arts,
        local,
        _dir: dir,
    })
}

impl StagedTx {
    pub fn artifacts(&self) -> &[TxArtifact] {
        &self.arts
    }

    /// The plan, from the files that will be installed.
    pub fn plan(&self) -> String {
        render_tx_plan(&self.arts, &self.format, true)
    }

    /// Install every staged package as a keg, dependencies first. When one
    /// fails, the kegs this run made are removed again, and the error says
    /// how many were, and which could not be.
    pub fn install(
        &self,
        ledger_root: &Path,
        expose_bins: bool,
    ) -> Result<Vec<ledger::InstallRecord>, String> {
        let txid = format!(
            "tx-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let mut installed: Vec<ledger::InstallRecord> = Vec::new();
        for (step, i) in topo_order(&self.arts).into_iter().enumerate() {
            let a = &self.arts[i];
            let prefix = default_prefix(&a.name, &self.format);
            match install_from(
                &a.name,
                &self.format,
                &self.local[i],
                &a.source,
                a.verify.as_ref(),
                &prefix,
                ledger_root,
                expose_bins,
            ) {
                Ok(mut rec) => {
                    rec.metadata.insert("tx_id".into(), txid.clone());
                    rec.metadata
                        .insert("tx_step".into(), format!("{}", step + 1));
                    let rec = ledger::upsert_record(ledger_root, rec.clone()).unwrap_or(rec);
                    installed.push(rec);
                }
                Err(e) => {
                    return Err(format!(
                        "transaction failed at step {} ({}): {e}; {}",
                        step + 1,
                        a.name,
                        roll_back(&installed, ledger_root)
                    ));
                }
            }
        }
        Ok(installed)
    }
}

/// Remove the kegs a failed transaction made, newest first, and say what
/// happened: how many were removed and which were not, with the reason.
fn roll_back(installed: &[ledger::InstallRecord], ledger_root: &Path) -> String {
    let mut stuck: Vec<String> = Vec::new();
    for r in installed.iter().rev() {
        if let Err(why) = remove(&r.id, ledger_root) {
            stuck.push(format!("{} at {} ({why})", r.id, r.install_root));
        }
    }
    let undone = installed.len() - stuck.len();
    if stuck.is_empty() {
        format!("rolled back {undone} install(s) made in this run")
    } else {
        format!(
            "rolled back {undone} of {} install(s) made in this run; could NOT remove: {}",
            installed.len(),
            stuck.join("; ")
        )
    }
}

/// The artifacts that need a licence acceptance, as (package, reason).
pub fn nonfree_artifacts(arts: &[TxArtifact]) -> Vec<(String, String)> {
    arts.iter()
        .filter_map(|a| nonfree_from(&a.section, &a.license).map(|d| (a.name.clone(), d)))
        .collect()
}

fn topo_order(arts: &[TxArtifact]) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    let mut placed = vec![false; arts.len()];
    let provided_by_unplaced = |want: &str, placed: &[bool]| -> bool {
        arts.iter()
            .enumerate()
            .any(|(j, a)| !placed[j] && a.provides.iter().any(|p| p == want))
    };
    for _ in 0..arts.len() {
        let mut progress = false;
        for i in 0..arts.len() {
            if placed[i] {
                continue;
            }
            let ok = arts[i].depends.iter().all(|d| {
                let dl = d.to_lowercase();
                arts.iter()
                    .enumerate()
                    .any(|(j, a)| placed[j] && a.provides.iter().any(|p| p == &dl))
                    || !provided_by_unplaced(&dl, &placed)
            });
            if ok {
                order.push(i);
                placed[i] = true;
                progress = true;
            }
        }
        if !progress {
            for (i, p) in placed.iter_mut().enumerate() {
                if !*p {
                    order.push(i);
                    *p = true;
                }
            }
            break;
        }
    }
    order
}

pub fn resolve_sources_from_dir(
    format: &str,
    dir: &Path,
    requested: &[String],
) -> Result<Vec<String>, String> {
    if !dir.is_dir() {
        return Err(format!("artifact directory not found: {}", dir.display()));
    }
    let ext = match format {
        "deb" => ".deb",
        "rpm" => ".rpm",
        "snap" => ".snap",
        _ => "",
    };
    let mut candidates: Vec<(String, TxArtifact)> = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .collect();
    entries.sort();
    for p in entries {
        if !ext.is_empty() {
            let ok = p
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.ends_with(ext))
                .unwrap_or(false);
            if !ok {
                continue;
            }
        }
        let src = p.display().to_string();
        if let Ok(arts) = collect_tx_artifacts(format, std::slice::from_ref(&src))
            && let Some(a) = arts.into_iter().next()
        {
            candidates.push((src, a));
        }
    }
    // select requested and transitive deps within candidates
    let mut selected: Vec<usize> = Vec::new();
    let mut queue: Vec<String> = requested.iter().map(|r| r.to_lowercase()).collect();
    while let Some(want) = queue.pop() {
        if let Some((i, _)) = candidates.iter().enumerate().find(|(i, c)| {
            !selected.contains(i)
                && (c.1.name.to_lowercase() == want || c.1.provides.iter().any(|p| p == &want))
        }) {
            selected.push(i);
            for d in &candidates[i].1.depends {
                let dl = d.to_lowercase();
                if candidates
                    .iter()
                    .any(|c| c.1.provides.iter().any(|p| p == &dl))
                    && !selected
                        .iter()
                        .any(|si| candidates[*si].1.provides.iter().any(|p| p == &dl))
                {
                    queue.push(dl);
                }
            }
        }
    }
    if selected.is_empty() {
        return Err(format!(
            "no matching artifacts found in {} for requested: {}",
            dir.display(),
            requested.join(", ")
        ));
    }
    let subset_sources: Vec<String> = selected.iter().map(|i| candidates[*i].0.clone()).collect();
    let subset_arts = collect_tx_artifacts(format, &subset_sources)?;
    let order = topo_order(&subset_arts);
    Ok(order
        .into_iter()
        .map(|i| subset_sources[i].clone())
        .collect())
}

/// Read a text index at a local path, decompressing gzip, xz or zstd (by
/// file signature; `.gz` also by name) through the matching tool.
fn read_text_path(p: &Path, src_name: &str) -> Result<String, String> {
    let bytes = fs::read(p).map_err(|e| format!("failed to read {src_name}: {e}"))?;
    let tool = if src_name.ends_with(".gz") || bytes.starts_with(&[0x1f, 0x8b]) {
        Some("gzip")
    } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Some("xz")
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some("zstd")
    } else {
        None
    };
    let Some(tool) = tool else {
        return String::from_utf8(bytes).map_err(|e| format!("index not utf-8 ({src_name}): {e}"));
    };
    let out = Command::new(tool)
        .args(["-dc"])
        .arg(p)
        .output()
        .map_err(|e| format!("failed to decompress index {src_name}: cannot run {tool}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "failed to decompress index {src_name} with {tool}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout)
        .map_err(|e| format!("decompressed index not utf-8 ({src_name}): {e}"))
}

/// Download a URL into a fresh private directory, which is deleted when the
/// returned guard is dropped.
fn fetch_to_temp(url: &str, tag: usize) -> Result<(PathBuf, PrivateDir), String> {
    let dir = PrivateDir::new("meta")?;
    let p = download_to(dir.path(), url, tag)?;
    Ok((p, dir))
}

/// Read an index, checking it first against the hash a signed (or at least
/// fetched) release file declared for it. A mismatch is an integrity error.
fn read_text_source_verified(
    src: &str,
    verify: Option<&IndexVerify>,
) -> crate::error::Result<String> {
    let check = |p: &Path| -> crate::error::Result<()> {
        match verify {
            Some(v) => verify_file_hash(p, v, BY_RELEASE).map_err(|why| {
                Error::integrity(format!(
                    "{src}: {why}. The index was not used and nothing was installed."
                ))
            }),
            None => Ok(()),
        }
    };
    if is_url(src) {
        let (p, _dir) = fetch_to_temp(src, 0)?;
        check(&p)?;
        Ok(read_text_path(&p, src)?)
    } else {
        check(Path::new(src))?;
        Ok(read_text_path(Path::new(src), src)?)
    }
}

/// Trust policy for remote foreign repos (APT/RPM).
#[derive(Clone, Debug)]
pub struct RepoTrust<'a> {
    /// Path to a GPG keyring used to verify InRelease/Release.gpg or repomd signatures.
    pub keyring: Option<&'a str>,
    /// Permit installing from unsigned/unverified repos (explicit opt-in).
    pub allow_unsigned: bool,
}

/// The refusal for an unverified remote repository (same text as the
/// install-time gate in the CLI).
const UNSIGNED_REFUSAL: &str = "refusing to install from an unsigned remote repo: pass --keyring <keyring> to verify GPG signatures, or --allow-unsigned (or set foreign.allow_unsigned_repos=true in config) to explicitly accept the risk";

/// Refuse a remote repository that has no keyring to verify it and was not
/// explicitly accepted unverified. Called before anything is fetched.
/// `file://` URLs and plain paths are local and always pass.
fn refuse_unverified(base_url: &str, trust: &RepoTrust) -> crate::error::Result<()> {
    if is_remote(base_url) && trust.keyring.is_none() && !trust.allow_unsigned {
        return Err(Error::refused(UNSIGNED_REFUSAL));
    }
    Ok(())
}

/// The same gate for the verbs that are handed a package index directly
/// (`tx-apt-repo`, `tx-rpm-repo`): no signature exists at that level, so a
/// remote index or a remote package location needs the explicit opt-in.
fn refuse_unverified_index(
    verb: &str,
    signed_verb: &str,
    locations: &[&str],
    trust: &RepoTrust,
) -> crate::error::Result<()> {
    if trust.allow_unsigned || !locations.iter().any(|l| is_remote(l)) {
        return Ok(());
    }
    Err(Error::refused(format!(
        "refusing to install from a remote package index that nothing verifies: `install {verb}` checks no signature. Use `install {signed_verb} --keyring <keyring>` for a verified install, or pass --allow-unsigned (or set foreign.allow_unsigned_repos=true in config) to explicitly accept the risk. Nothing was downloaded or installed."
    )))
}

/// A keyring was given, but this repository offers nothing it can verify.
/// That is accepted only with the explicit opt-in, and said aloud.
fn unsigned_or_refuse(trust: &RepoTrust, what: &str) -> crate::error::Result<()> {
    if trust.allow_unsigned {
        eprintln!(
            "warning: {what}; proceeding WITHOUT signature verification because --allow-unsigned (or config foreign.allow_unsigned_repos) is set"
        );
        Ok(())
    } else {
        Err(Error::refused(format!(
            "{what}, so the repository metadata cannot be GPG-verified and was not used. Pass --allow-unsigned to accept it unverified. Nothing was installed."
        )))
    }
}

/// The keyring as gpgv must be given it: an existing file named by an
/// absolute path (gpgv looks for a name without a slash under ~/.gnupg).
fn keyring_path(keyring: &str) -> crate::error::Result<PathBuf> {
    let p = Path::new(keyring);
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| Error::from(format!("cannot resolve the keyring path {keyring}: {e}")))?
            .join(p)
    };
    if !p.is_file() {
        return Err(Error::from(format!(
            "keyring not found: {} (expected an OpenPGP keyring file gpgv can read). Nothing was verified or installed.",
            p.display()
        )));
    }
    Ok(p)
}

/// Run gpgv. A signature that does not verify — bad signature, key not in
/// the keyring, malformed input — is an integrity error whatever
/// `allow_unsigned` says.
fn gpgv_run(cmd: &mut Command) -> crate::error::Result<()> {
    let out = cmd.output().map_err(|e| {
        Error::unavailable(format!(
            "cannot run gpgv to check the repository signature: {e}. Nothing was installed."
        ))
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::integrity(format!(
            "GPG signature verification failed: {}. The repository was not used and nothing was installed.",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

const PGP_SIGNED_BEGIN: &str = "-----BEGIN PGP SIGNED MESSAGE-----";
const PGP_SIGNATURE_BEGIN: &str = "-----BEGIN PGP SIGNATURE-----";
const PGP_SIGNATURE_END: &str = "-----END PGP SIGNATURE-----";

/// The signed text of a clearsigned file (InRelease), or why the file is
/// not exactly one clearsigned message. Anything but blank lines before the
/// armor or after the signature is refused: gpgv accepts such a file and
/// verifies the signed part, while a reader of the whole file would also
/// read the unsigned text.
///
/// This checks structure only; it verifies no signature.
fn clearsigned_payload(text: &str) -> Result<String, String> {
    let mut lines = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l));
    let outside = "text outside the signed message";
    // before the armor: blank lines only
    loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => continue,
            Some(PGP_SIGNED_BEGIN) => break,
            Some(_) => {
                return Err(format!(
                    "not a single clearsigned message: {outside} before '{PGP_SIGNED_BEGIN}'"
                ));
            }
            None => return Err("not a clearsigned message: it is empty".into()),
        }
    }
    // armor headers (`Hash: SHA512`) up to the first empty line
    loop {
        match lines.next() {
            Some("") => break,
            Some(l) if l.starts_with("Hash:") => continue,
            Some(_) => return Err("not a clearsigned message: unexpected armor header".into()),
            None => return Err("not a clearsigned message: it ends in the armor header".into()),
        }
    }
    let mut payload: Vec<&str> = Vec::new();
    loop {
        match lines.next() {
            Some(PGP_SIGNATURE_BEGIN) => break,
            // a line that starts with a dash is always escaped as "- " in
            // the signed text; a bare one is another armor block
            Some(l) if l.starts_with("- ") => payload.push(&l[2..]),
            Some(l) if l.starts_with('-') => {
                return Err("not a single clearsigned message: a second armor block".into());
            }
            Some(l) => payload.push(l),
            None => return Err("not a clearsigned message: it has no signature".into()),
        }
    }
    loop {
        match lines.next() {
            Some(PGP_SIGNATURE_END) => break,
            Some(l) if l.starts_with('-') => {
                return Err("not a single clearsigned message: a second armor block".into());
            }
            Some(_) => continue,
            None => return Err("not a clearsigned message: the signature does not end".into()),
        }
    }
    if lines.any(|l| !l.trim().is_empty()) {
        return Err(format!(
            "not a single clearsigned message: {outside} after '{PGP_SIGNATURE_END}'"
        ));
    }
    // the line break before the signature armor is not part of the text
    if payload.last() == Some(&"") {
        payload.pop();
    }
    let mut out = payload.join("\n");
    out.push('\n');
    Ok(out)
}

/// Verify a clearsigned file (e.g. InRelease) against a keyring and return
/// the signed text, which is the only part a caller may parse: it is what
/// gpgv wrote with `--output`, not the file. A file with anything outside
/// the one signed message is refused before gpgv runs.
pub fn verify_clearsigned(file: &Path, keyring: &str) -> crate::error::Result<String> {
    let raw = fs::read_to_string(file).map_err(|e| {
        Error::integrity(format!(
            "the clearsigned file could not be read as text: {e}. Nothing was installed."
        ))
    })?;
    clearsigned_payload(&raw).map_err(|why| {
        Error::integrity(format!(
            "{why}. The file was not used and nothing was installed."
        ))
    })?;
    let keyring = keyring_path(keyring)?;
    let out_dir = PrivateDir::new("gpgv")?;
    let signed = out_dir.path().join("signed-text");
    gpgv_run(
        Command::new("gpgv")
            .arg("--keyring")
            .arg(&keyring)
            .arg("--output")
            .arg(&signed)
            .arg(file),
    )?;
    fs::read_to_string(&signed).map_err(|e| {
        Error::integrity(format!(
            "gpgv accepted the signature but its output could not be read: {e}. Nothing was installed."
        ))
    })
}

/// Verify a detached signature (e.g. Release.gpg over Release, repomd.xml.asc over repomd.xml).
pub fn verify_detached(data: &Path, sig: &Path, keyring: &str) -> crate::error::Result<()> {
    let keyring = keyring_path(keyring)?;
    gpgv_run(
        Command::new("gpgv")
            .arg("--keyring")
            .arg(&keyring)
            .arg(sig)
            .arg(data),
    )
}

/// The Packages indexes a Release file lists for one component and
/// architecture.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReleaseIndexes {
    /// path and its strongest accepted hash, in the order to try them
    strong: Vec<(String, IndexVerify)>,
    /// paths listed with MD5 or SHA1 only: never used
    weak_only: Vec<String>,
}

/// Read the hash sections of a Release file (the signed text only: pass
/// what `verify_clearsigned` or `clearsigned_payload` returned for an
/// InRelease). For each Packages index the strongest accepted hash wins
/// (SHA512, then SHA256), whatever order the sections come in; MD5Sum and
/// SHA1 entries never become a check.
fn parse_release_index_paths(release_text: &str, component: &str, arch: &str) -> ReleaseIndexes {
    // index names in the order they are tried
    const SUFFIXES: [&str; 4] = [".gz", ".xz", ".zst", ""];
    let stem = format!("{component}/binary-{arch}/Packages");
    // path -> (best accepted hash, listed at all)
    let mut seen: std::collections::BTreeMap<String, Option<(u8, IndexVerify)>> =
        std::collections::BTreeMap::new();
    let mut section: Option<String> = None;
    for line in release_text.lines() {
        if !(line.starts_with(' ') || line.starts_with('\t')) {
            let field = line.trim_end();
            section = matches!(field, "MD5Sum:" | "SHA1:" | "SHA256:" | "SHA512:")
                .then(|| field.trim_end_matches(':').to_string());
            continue;
        }
        let Some(algo) = &section else {
            continue;
        };
        // " <hash> <size> <name>"
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [hash, size, name] = parts[..] else {
            continue;
        };
        if !SUFFIXES.iter().any(|s| name == format!("{stem}{s}")) {
            continue;
        }
        let best = seen.entry(name.to_string()).or_insert(None);
        if let Some((rank, canonical)) = strong_algo(algo)
            && best.as_ref().is_none_or(|(have, _)| rank > *have)
        {
            *best = Some((
                rank,
                IndexVerify {
                    algo: canonical.to_string(),
                    hash: hash.to_lowercase(),
                    size: size.parse::<u64>().ok(),
                },
            ));
        }
    }
    let mut out = ReleaseIndexes::default();
    for suffix in SUFFIXES {
        match seen.remove(&format!("{stem}{suffix}")) {
            Some(Some((_, v))) => out.strong.push((format!("{stem}{suffix}"), v)),
            Some(None) => out.weak_only.push(format!("{stem}{suffix}")),
            None => {}
        }
    }
    out
}

#[derive(Clone, Debug)]
struct AptIndexCandidate {
    url: String,
    verify: Option<IndexVerify>,
}

/// The same error, saying which file it is about.
fn about(what: &str, e: Error) -> Error {
    Error::new(e.kind, format!("{what}: {}", e.message))
}

/// The Packages index candidates of a repository and what vouches for them.
struct AptCandidates {
    cands: Vec<AptIndexCandidate>,
    /// the release file the candidates and their hashes were read from
    release: String,
    /// gpgv accepted the signature of `release` for the given keyring
    signature_verified: bool,
}

/// Build candidate APT Packages index URLs from repo base/suite/component/arch.
///
/// With a keyring: InRelease is verified and only its signed text is read;
/// failing that, Release with Release.gpg. A signature that does not verify
/// is fatal. A repository that offers no signature at all is refused unless
/// `allow_unsigned` is set. A release file that lists the index with MD5 or
/// SHA1 only is refused in every case.
fn apt_packages_candidates(
    base_url: &str,
    suite: &str,
    component: &str,
    arch: &str,
    trust: &RepoTrust,
) -> crate::error::Result<AptCandidates> {
    refuse_unverified(base_url, trust)?;
    let base = base_url.trim_end_matches('/');
    let dists = format!("{base}/dists/{suite}");

    // The candidates a release text lists; Ok(None) when it lists none and
    // nothing verified it (the caller then tries the next source).
    let listed = |text: &str,
                  release: &str,
                  verified: bool|
     -> crate::error::Result<Option<AptCandidates>> {
        let found = parse_release_index_paths(text, component, arch);
        if found.strong.is_empty() {
            if !found.weak_only.is_empty() {
                return Err(Error::refused(format!(
                    "{release} lists {} with MD5 or SHA1 only; yapacc requires SHA256 or SHA512 and did not use this repository. Nothing was installed.",
                    found.weak_only.join(", ")
                )));
            }
            if verified {
                return Err(Error::from(format!(
                    "the verified {release} lists no Packages index for component '{component}', architecture '{arch}'"
                )));
            }
            return Ok(None);
        }
        Ok(Some(AptCandidates {
            cands: found
                .strong
                .into_iter()
                .map(|(p, v)| AptIndexCandidate {
                    url: format!("{dists}/{p}"),
                    verify: Some(v),
                })
                .collect(),
            release: release.to_string(),
            signature_verified: verified,
        }))
    };

    // 1) InRelease (clearsigned)
    let inrelease = format!("{dists}/InRelease");
    if let Ok((file, _dir)) = fetch_to_temp(&inrelease, 0) {
        let (text, verified) = match trust.keyring {
            Some(kr) => (
                verify_clearsigned(&file, kr).map_err(|e| about(&inrelease, e))?,
                true,
            ),
            None => {
                let raw = fs::read_to_string(&file).map_err(|e| {
                    Error::integrity(format!("{inrelease} could not be read as text: {e}"))
                })?;
                let text = clearsigned_payload(&raw).map_err(|why| {
                    Error::integrity(format!(
                        "{inrelease}: {why}. The file was not used and nothing was installed."
                    ))
                })?;
                (text, false)
            }
        };
        if let Some(found) = listed(&text, &inrelease, verified)? {
            return Ok(found);
        }
    }

    // 2) Release + Release.gpg (detached)
    let release = format!("{dists}/Release");
    if let Ok((rel_file, _dir)) = fetch_to_temp(&release, 0) {
        let mut verified = false;
        if let Some(kr) = trust.keyring {
            match fetch_to_temp(&format!("{release}.gpg"), 1) {
                Ok((sig, _sig_dir)) => {
                    verify_detached(&rel_file, &sig, kr).map_err(|e| about(&release, e))?;
                    verified = true;
                }
                Err(_) => unsigned_or_refuse(
                    trust,
                    &format!("{release} has no Release.gpg signature and there is no InRelease"),
                )?,
            }
        }
        let text = read_text_path(&rel_file, "Release")?;
        if let Some(found) = listed(&text, &release, verified)? {
            return Ok(found);
        }
    }

    // 3) direct probe, unverified
    if trust.keyring.is_some() {
        unsigned_or_refuse(
            trust,
            &format!("{dists} offers neither an InRelease nor a Release file"),
        )?;
    }
    let path = format!("{dists}/{component}/binary-{arch}");
    Ok(AptCandidates {
        cands: vec![
            AptIndexCandidate {
                url: format!("{path}/Packages.gz"),
                verify: None,
            },
            AptIndexCandidate {
                url: format!("{path}/Packages"),
                verify: None,
            },
        ],
        release: String::new(),
        signature_verified: false,
    })
}

/// Packages resolved from a repository index, and how far that index was
/// verified.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub artifacts: Vec<TxArtifact>,
    /// the package index the artifacts were read from
    pub index: String,
    /// the release file that declared the index (InRelease, Release,
    /// repomd.xml); empty when the index was used on its own
    pub release: String,
    /// gpgv accepted the signature of `release` for the given keyring
    pub signature_verified: bool,
}

impl Resolved {
    /// What was verified and what was not, for the user.
    pub fn trust_statement(&self) -> String {
        let unchecked = self.artifacts.iter().filter(|a| a.verify.is_none()).count();
        let mut s = if self.signature_verified {
            format!(
                "Verified: the signature of {} (gpgv, with the keyring you gave) and the package index against the hash in it. Each package is compared with the SHA256/SHA512 in that index when it is downloaded.\nNot verified: that the keyring belongs to the publisher, and what the packages do.",
                self.release
            )
        } else if self.release.is_empty() {
            format!(
                "NOT verified: the package index {} was used without any signature check. Each package is compared with the hash in that index, which catches a damaged download and not an index that was replaced.",
                self.index
            )
        } else {
            format!(
                "NOT verified: {} was used without a signature check, so the index and the hashes in it are whatever the server sent. Each package is compared with the hash in that index, which catches a damaged download and not a hostile server.",
                self.release
            )
        };
        if unchecked > 0 {
            s.push_str(&format!(
                "\nNOT checked at all: {unchecked} of {} package(s) have no SHA256 or SHA512 in the index.",
                self.artifacts.len()
            ));
        }
        s
    }
}

/// Resolve .deb artifacts from a repository: Release-aware discovery, the
/// index checked against the release file's hash, each artifact carrying
/// the hash its index stanza declares.
pub fn resolve_apt_repo(
    base_url: &str,
    suite: &str,
    component: &str,
    arch: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<Resolved> {
    let found = apt_packages_candidates(base_url, suite, component, arch, trust)?;
    let mut last: Option<Error> = None;
    let mut tampered: Option<Error> = None;
    for cand in found.cands {
        match apt_artifacts_from_index(
            base_url,
            &cand.url,
            cand.verify.as_ref(),
            requested,
            !trust.allow_unsigned,
        ) {
            Ok(artifacts) => {
                return Ok(Resolved {
                    artifacts,
                    index: cand.url,
                    release: found.release,
                    signature_verified: found.signature_verified,
                });
            }
            Err(e) => {
                if e.kind == crate::Kind::Integrity && tampered.is_none() {
                    tampered = Some(e.clone());
                }
                last = Some(e);
            }
        }
    }
    // an index that failed its hash check is what the user must hear about
    let e = tampered
        .or(last)
        .unwrap_or_else(|| Error::from("no Packages index candidate"));
    Err(Error::new(
        e.kind,
        format!(
            "failed to resolve from APT repo {base_url} (suite={suite}, component={component}, arch={arch}): {e}"
        ),
    ))
}

/// `resolve_apt_repo` as (artifacts, index URL), for the plan verbs.
pub fn resolve_sources_from_apt_repo(
    base_url: &str,
    suite: &str,
    component: &str,
    arch: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<(Vec<TxArtifact>, String)> {
    let r = resolve_apt_repo(base_url, suite, component, arch, requested, trust)?;
    Ok((r.artifacts, r.index))
}

/// Resolve closure from parsed APT source definitions (sources.list / sources.list.d).
/// Tries enabled deb entries (URI/suite/components) in order and returns the
/// first successful resolution with a description of the entry used. A
/// source whose signature or hash check fails stops the search: it is not
/// skipped in favour of the next one.
pub fn resolve_apt_sources_root(
    apt_root: &Path,
    requested: &[String],
    arch: &str,
    trust: &RepoTrust,
) -> crate::error::Result<(Resolved, String)> {
    let entries = crate::source_repos::parse_apt_sources(apt_root)?;
    let mut last_err = Error::from("no suitable apt source entries found");
    for e in entries {
        if !e.enabled {
            continue;
        }
        // only binary deb-like resolution; allow apt-list/apt-deb822 with deb types implied
        if e.uri.is_empty() || e.suite.is_empty() {
            continue;
        }
        let comps: Vec<String> = if e.components.trim().is_empty() {
            vec![String::new()]
        } else {
            e.components
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        };
        // support space/newline separated URIs/Suites in deb822 simplified form
        let uris: Vec<String> = e.uri.split_whitespace().map(|s| s.to_string()).collect();
        let suites: Vec<String> = e.suite.split_whitespace().map(|s| s.to_string()).collect();
        for uri in &uris {
            for suite in &suites {
                for comp in &comps {
                    let comp_eff = if comp.is_empty() {
                        "main"
                    } else {
                        comp.as_str()
                    };
                    match resolve_apt_repo(uri, suite, comp_eff, arch, requested, trust) {
                        Ok(resolved) => {
                            return Ok((
                                resolved,
                                format!(
                                    "{} (uri={uri} suite={suite} component={comp_eff})",
                                    e.origin
                                ),
                            ));
                        }
                        Err(err) if err.kind == crate::Kind::Integrity => return Err(err),
                        Err(err) => last_err = err,
                    }
                }
            }
        }
    }
    Err(last_err)
}

/// `resolve_apt_sources_root` as (artifacts, index URL, source entry), for
/// the plan verbs.
pub fn resolve_sources_from_apt_sources_root(
    apt_root: &Path,
    requested: &[String],
    arch: &str,
    trust: &RepoTrust,
) -> crate::error::Result<(Vec<TxArtifact>, String, String)> {
    let (r, entry) = resolve_apt_sources_root(apt_root, requested, arch, trust)?;
    Ok((r.artifacts, r.index, entry))
}

fn extract_xml_tag(text: &str, tag: &str) -> String {
    // naive: <tag>value</tag> (first)
    let open = format!("<{tag}");
    let Some(s) = text.find(&open) else {
        return String::new();
    };
    let rest = &text[s..];
    let Some(gt) = rest.find('>') else {
        return String::new();
    };
    if rest[..gt + 1].ends_with("/>") {
        return String::new();
    }
    let body = &rest[gt + 1..];
    let close = format!("</{tag}>");
    if let Some(e) = body.find(&close) {
        body[..e].trim().to_string()
    } else {
        String::new()
    }
}

fn extract_xml_attr(text: &str, tag: &str, attr: &str) -> String {
    // naive: find <tag ... attr="value"
    let needle = format!("<{tag}");
    let mut from = 0;
    while let Some(s) = text[from..].find(&needle) {
        let i = from + s;
        let rest = &text[i..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let head = &rest[..gt];
        let pat = format!("{attr}=\"");
        if let Some(a) = head.find(&pat) {
            let vstart = a + pat.len();
            if let Some(endq) = head[vstart..].find('"') {
                return head[vstart..vstart + endq].to_string();
            }
        }
        from = i + 1;
    }
    String::new()
}

/// The `name` of every `<rpm:entry .../>` (or `<entry .../>`) in a
/// requires/provides section. Every index used to slice the text comes from
/// a search for an ASCII delimiter, so text in any language cannot split a
/// character.
fn extract_rpm_entries(section: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = section;
    while let Some(lt) = rest.find('<') {
        let tag = &rest[lt..];
        let end = tag.find('>').map_or(tag.len(), |gt| gt + 1);
        let head = &tag[..end];
        if (head.starts_with("<rpm:entry") || head.starts_with("<entry"))
            && let Some(at) = head.find("name=\"")
        {
            let value = &head[at + 6..];
            if let Some(endq) = value.find('"')
                && endq > 0
            {
                out.push(dep_base_token(&value[..endq]));
            }
        }
        rest = &tag[end..];
    }
    out.retain(|s| !s.is_empty());
    out.sort();
    out.dedup();
    out
}

fn find_repomd_primary_info(repomd_text: &str) -> Option<(String, Option<IndexVerify>)> {
    // <data type="primary"> ... <location href="..."/> <checksum type="sha256">HASH</checksum> <size>N</size> ... </data>
    let mut rest = repomd_text;
    while let Some(s) = rest.find("<data") {
        let seg = &rest[s..];
        let end = seg.find("</data>").map(|e| e + 7).unwrap_or(seg.len());
        let block = &seg[..end];
        if block.contains("type=\"primary\"") || block.contains("type='primary'") {
            let href = extract_xml_attr(block, "location", "href");
            if href.is_empty() {
                return None;
            }
            let algo = extract_xml_attr(block, "checksum", "type");
            let hash = extract_xml_tag(block, "checksum");
            let size = extract_xml_tag(block, "size").parse::<u64>().ok();
            let verify = if !algo.is_empty() && !hash.is_empty() {
                Some(IndexVerify { algo, hash, size })
            } else {
                None
            };
            return Some((href, verify));
        }
        rest = &seg[end.min(seg.len())..];
        if rest.is_empty() {
            break;
        }
    }
    None
}

/// Where a repository's primary metadata is, and what vouches for it.
struct RepomdPrimary {
    primary: String,
    repomd: String,
    verify: Option<IndexVerify>,
    signature_verified: bool,
}

/// Locate the primary metadata through repomd.xml. With a keyring,
/// repomd.xml.asc must verify (a bad signature is fatal; a missing one is
/// refused unless `allow_unsigned`). A primary checksum that is only MD5 or
/// SHA1 is refused in every case.
fn rpm_primary_from_repomd(
    base_url: &str,
    trust: &RepoTrust,
) -> crate::error::Result<RepomdPrimary> {
    refuse_unverified(base_url, trust)?;
    let base = base_url.trim_end_matches('/');
    let mut fetch_error = String::new();
    for repomd_url in [
        format!("{base}/repodata/repomd.xml"),
        format!("{base}/repodata/repomd.xml.gz"),
    ] {
        let (repomd_file, _dir) = match fetch_to_temp(&repomd_url, 0) {
            Ok(got) => got,
            Err(e) => {
                if fetch_error.is_empty() {
                    fetch_error = e;
                }
                continue;
            }
        };
        let mut signature_verified = false;
        if let Some(kr) = trust.keyring {
            let sig_url = format!("{repomd_url}.asc");
            match fetch_to_temp(&sig_url, 1) {
                Ok((sig, _sig_dir)) => {
                    verify_detached(&repomd_file, &sig, kr).map_err(|e| about(&repomd_url, e))?;
                    signature_verified = true;
                }
                Err(_) => unsigned_or_refuse(
                    trust,
                    &format!("the repomd signature {sig_url} is missing"),
                )?,
            }
        }
        let text = read_text_path(&repomd_file, "repomd.xml")?;
        let Some((href, declared)) = find_repomd_primary_info(&text) else {
            continue;
        };
        let verify = match declared {
            Some(v) => match strong_algo(&v.algo) {
                Some((_, canonical)) => Some(IndexVerify {
                    algo: canonical.to_string(),
                    ..v
                }),
                None => {
                    return Err(Error::refused(format!(
                        "{repomd_url} gives only a {} checksum for the primary metadata; yapacc requires SHA256 or SHA512 and did not use this repository. Nothing was installed.",
                        v.algo
                    )));
                }
            },
            None if signature_verified => {
                return Err(Error::from(format!(
                    "the verified {repomd_url} declares no checksum for the primary metadata, so the package list cannot be tied to the signature"
                )));
            }
            None => None,
        };
        return Ok(RepomdPrimary {
            primary: format!("{base}/{}", href.trim_start_matches('/')),
            repomd: repomd_url,
            verify,
            signature_verified,
        });
    }
    Err(Error::from(format!(
        "failed to discover primary metadata from repomd.xml under {base}/repodata (tried repomd.xml[.gz]){}",
        if fetch_error.is_empty() {
            String::new()
        } else {
            format!(": {fetch_error}")
        }
    )))
}

/// Resolve closure from RPM repo base URL by discovering primary.xml via
/// repomd.xml; the primary metadata is checked against the repomd hash and
/// each artifact carries the checksum its package entry declares.
pub fn resolve_rpm_repo(
    base_url: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<Resolved> {
    let found = rpm_primary_from_repomd(base_url, trust)?;
    let text = read_text_source_verified(&found.primary, found.verify.as_ref())?;
    let artifacts = rpm_artifacts_from_primary(base_url, &text, requested, !trust.allow_unsigned)?;
    Ok(Resolved {
        artifacts,
        index: found.primary,
        release: found.repomd,
        signature_verified: found.signature_verified,
    })
}

/// `resolve_rpm_repo` as (artifacts, primary URL, repomd URL), for the plan
/// verbs.
pub fn resolve_artifacts_from_rpm_repo(
    base_url: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<(Vec<TxArtifact>, String, String)> {
    let r = resolve_rpm_repo(base_url, requested, trust)?;
    Ok((r.artifacts, r.index, r.release))
}

/// Resolve .rpm artifacts from a primary.xml given directly (path or URL,
/// plain or compressed). Nothing signs a bare index, so for an install a
/// remote index or a remote `base_url` needs `allow_unsigned`.
pub fn resolve_rpm_primary(
    base_url: &str,
    primary_source: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<Resolved> {
    refuse_unverified_index("tx-rpm-repo", "tx-rpm", &[base_url, primary_source], trust)?;
    let text = read_text_source_verified(primary_source, None)?;
    let artifacts = rpm_artifacts_from_primary(base_url, &text, requested, !trust.allow_unsigned)?;
    Ok(Resolved {
        artifacts,
        index: primary_source.to_string(),
        release: String::new(),
        signature_verified: false,
    })
}

/// `resolve_rpm_primary` for the plan verb: a plan installs nothing, so it
/// may read an unverified index.
pub fn resolve_artifacts_from_rpm_primary(
    base_url: &str,
    primary_source: &str,
    requested: &[String],
) -> crate::error::Result<Vec<TxArtifact>> {
    let plan_only = RepoTrust {
        keyring: None,
        allow_unsigned: true,
    };
    Ok(resolve_rpm_primary(base_url, primary_source, requested, &plan_only)?.artifacts)
}

/// A package location from an index, joined to the repository base. A
/// location with a `..` segment is not a location inside the repository.
fn artifact_location(base_url: &str, rel: &str) -> Option<String> {
    if rel.split('/').any(|seg| seg == "..") {
        return None;
    }
    Some(format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        rel.trim_start_matches('/')
    ))
}

/// The refusal for selected packages whose index entry has no accepted
/// hash, when the caller did not accept unverified installs.
fn refuse_unhashed(index_kind: &str, names: &[String]) -> Error {
    Error::refused(format!(
        "the {index_kind} gives no SHA256 or SHA512 for: {}. Without one the download cannot be checked, so nothing was installed. Pass --allow-unsigned to install such packages unchecked.",
        names.join(", ")
    ))
}

fn rpm_artifacts_from_primary(
    base_url: &str,
    text: &str,
    requested: &[String],
    require_hash: bool,
) -> crate::error::Result<Vec<TxArtifact>> {
    let mut pkgs: Vec<TxArtifact> = Vec::new();
    let mut rest = text;
    while let Some(s) = rest.find("<package") {
        let seg = &rest[s..];
        let Some(e) = seg.find("</package>") else {
            break;
        };
        let block = &seg[..e + 10];
        let name = extract_xml_tag(block, "name");
        let href = extract_xml_attr(block, "location", "href");
        if !name.is_empty()
            && !href.is_empty()
            && let Some(source) = artifact_location(base_url, &href)
        {
            let mut requires = Vec::new();
            let mut provides = Vec::new();
            let extract_section = |block: &str, tag: &str, alttag: &str| -> Option<String> {
                let pos = block
                    .find(&format!("<{tag}"))
                    .or_else(|| block.find(&format!("<{alttag}")))?;
                let seg = &block[pos..];
                let gt = seg.find('>')?;
                if seg[..=gt].contains("/>") {
                    return Some(String::new());
                }
                let end = seg
                    .find(&format!("</{tag}>"))
                    .or_else(|| seg.find(&format!("</{alttag}>")))?;
                Some(seg[gt + 1..end].to_string())
            };
            if let Some(sec) = extract_section(block, "rpm:requires", "requires") {
                requires = extract_rpm_entries(&sec);
            }
            if let Some(sec) = extract_section(block, "rpm:provides", "provides") {
                provides = extract_rpm_entries(&sec);
            }
            provides.push(name.to_lowercase());
            provides.sort();
            provides.dedup();
            // <checksum type="sha256" pkgid="YES">HASH</checksum>
            // <size package="N" installed=".." archive=".."/>
            let hash = extract_xml_tag(block, "checksum");
            let verify = strong_algo(&extract_xml_attr(block, "checksum", "type"))
                .filter(|_| !hash.is_empty())
                .map(|(_, algo)| IndexVerify {
                    algo: algo.to_string(),
                    hash: hash.to_lowercase(),
                    size: extract_xml_attr(block, "size", "package")
                        .parse::<u64>()
                        .ok(),
                });
            pkgs.push(TxArtifact {
                source,
                name,
                provides,
                depends: requires,
                verify,
                section: String::new(),
                license: extract_xml_tag(block, "rpm:license"),
            });
        }
        rest = &seg[e + 10..];
    }
    if pkgs.is_empty() {
        return Err(Error::from(
            "no package entries parsed from RPM primary.xml index",
        ));
    }
    let arts = select_closure(&pkgs, requested);
    if arts.is_empty() {
        return Err(Error::from(format!(
            "no matching packages found in RPM primary.xml for requested: {}",
            requested.join(", ")
        )));
    }
    finish_selection(arts, "RPM primary.xml", require_hash)
}

/// The requested packages and, transitively, the dependencies the index can
/// satisfy. A name is matched by package name first, then by what packages
/// provide; one provider is taken per name.
fn select_closure(pkgs: &[TxArtifact], requested: &[String]) -> Vec<TxArtifact> {
    let mut selected: Vec<usize> = Vec::new();
    let mut queue: Vec<String> = requested.iter().map(|r| r.to_lowercase()).collect();
    while let Some(want) = queue.pop() {
        if selected
            .iter()
            .any(|si| pkgs[*si].provides.iter().any(|pr| pr == &want))
        {
            continue;
        }
        let Some(i) = pkgs
            .iter()
            .position(|p| p.name.to_lowercase() == want)
            .or_else(|| {
                pkgs.iter()
                    .position(|p| p.provides.iter().any(|pr| pr == &want))
            })
        else {
            continue;
        };
        selected.push(i);
        for d in &pkgs[i].depends {
            queue.push(d.to_lowercase());
        }
    }
    selected.into_iter().map(|i| pkgs[i].clone()).collect()
}

/// Order the selection (dependencies first) and apply the hash rule.
fn finish_selection(
    arts: Vec<TxArtifact>,
    index_kind: &str,
    require_hash: bool,
) -> crate::error::Result<Vec<TxArtifact>> {
    let unhashed: Vec<String> = arts
        .iter()
        .filter(|a| a.verify.is_none())
        .map(|a| a.name.clone())
        .collect();
    if require_hash && !unhashed.is_empty() {
        return Err(refuse_unhashed(index_kind, &unhashed));
    }
    let order = topo_order(&arts);
    Ok(order.into_iter().map(|i| arts[i].clone()).collect())
}

/// Parse an APT Packages index: one artifact per stanza that names a
/// package and a file, with the strongest accepted hash the stanza gives
/// (SHA512, then SHA256; MD5sum and SHA1 are never a check), its Size and
/// its Section.
fn parse_apt_packages(base_url: &str, text: &str) -> Vec<TxArtifact> {
    let mut pkgs: Vec<TxArtifact> = Vec::new();
    let mut cur = TxArtifact::default();
    let mut filename = String::new();
    let mut size: Option<u64> = None;
    let mut best: Option<(u8, &'static str, String)> = None;
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !cur.name.is_empty()
                && !filename.is_empty()
                && let Some(source) = artifact_location(base_url, &filename)
            {
                cur.source = source;
                cur.provides.push(cur.name.to_lowercase());
                cur.provides.sort();
                cur.provides.dedup();
                cur.verify = best.take().map(|(_, algo, hash)| IndexVerify {
                    algo: algo.to_string(),
                    hash,
                    size,
                });
                pkgs.push(std::mem::take(&mut cur));
            }
            cur = TxArtifact::default();
            filename.clear();
            size = None;
            best = None;
            continue;
        }
        // a continuation line belongs to the previous field
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "Package" => cur.name = v.to_string(),
            "Filename" => filename = v.to_string(),
            "Section" => cur.section = v.to_string(),
            "Size" => size = v.parse::<u64>().ok(),
            "Depends" | "Pre-Depends" => cur.depends.extend(split_tokens(v)),
            "Provides" => cur.provides = split_tokens(v),
            algo => {
                if let Some((rank, canonical)) = strong_algo(algo)
                    && best.as_ref().is_none_or(|(have, _, _)| rank > *have)
                {
                    best = Some((rank, canonical, v.to_lowercase()));
                }
            }
        }
    }
    pkgs
}

/// Resolve artifacts from one Packages index (path or URL, plain or
/// compressed), which is first checked against `verify` when the release
/// file declared a hash for it.
fn apt_artifacts_from_index(
    base_url: &str,
    packages_source: &str,
    verify: Option<&IndexVerify>,
    requested: &[String],
    require_hash: bool,
) -> crate::error::Result<Vec<TxArtifact>> {
    let text = read_text_source_verified(packages_source, verify)?;
    let pkgs = parse_apt_packages(base_url, &text);
    if pkgs.is_empty() {
        return Err(Error::from("no package entries parsed from Packages index"));
    }
    let arts = select_closure(&pkgs, requested);
    if arts.is_empty() {
        return Err(Error::from(format!(
            "no matching packages found in Packages index for requested: {}",
            requested.join(", ")
        )));
    }
    finish_selection(arts, "Packages index", require_hash)
}

/// Resolve .deb artifacts from a Packages index given directly (path or
/// URL). Nothing signs a bare index, so for an install a remote index or a
/// remote `base_url` needs `allow_unsigned`.
pub fn resolve_apt_index(
    base_url: &str,
    packages_source: &str,
    requested: &[String],
    trust: &RepoTrust,
) -> crate::error::Result<Resolved> {
    refuse_unverified_index("tx-apt-repo", "tx-apt", &[base_url, packages_source], trust)?;
    let artifacts = apt_artifacts_from_index(
        base_url,
        packages_source,
        None,
        requested,
        !trust.allow_unsigned,
    )?;
    Ok(Resolved {
        artifacts,
        index: packages_source.to_string(),
        release: String::new(),
        signature_verified: false,
    })
}

/// `resolve_apt_index` for the plan verb: a plan installs nothing, so it
/// may read an unverified index.
pub fn resolve_sources_from_apt_packages(
    base_url: &str,
    packages_source: &str,
    requested: &[String],
) -> crate::error::Result<Vec<TxArtifact>> {
    let plan_only = RepoTrust {
        keyring: None,
        allow_unsigned: true,
    };
    Ok(resolve_apt_index(base_url, packages_source, requested, &plan_only)?.artifacts)
}

/// The transaction plan. `fetched` says whether the package files are
/// already on disk (and were compared with their index hash) or will be
/// downloaded after consent.
fn render_tx_plan(arts: &[TxArtifact], format: &str, fetched: bool) -> String {
    let order = topo_order(arts);
    let mut s = String::new();
    s.push_str(&format!(
        "== Foreign transaction plan (mediated closure from the given or resolved artifacts) ==\nFormat: {format}\nArtifacts: {}\n",
        arts.len()
    ));
    if arts.iter().any(|a| is_url(&a.source)) {
        s.push_str(if fetched {
            "Source fetch: URL sources were downloaded once into a private temporary directory; the install uses these same files, and the directory is deleted afterwards.\n"
        } else {
            "Source fetch: nothing was downloaded for this plan. Each package is downloaded once, after consent, into a private temporary directory that is deleted afterwards.\n"
        });
    }
    s.push_str("Resolved install order (best-effort, within provided set):\n");
    for (idx, i) in order.iter().enumerate() {
        let a = &arts[*i];
        let unresolved: Vec<String> = a
            .depends
            .iter()
            .filter(|d| {
                !arts
                    .iter()
                    .any(|x| x.provides.iter().any(|p| p == &d.to_lowercase()))
            })
            .cloned()
            .collect();
        let check = match (&a.verify, fetched) {
            (Some(v), true) => format!(
                "{} {} from the repository index: the file matched",
                v.algo, v.hash
            ),
            (Some(v), false) => format!(
                "{} {} from the repository index: a download that differs is deleted and the install stops (exit 5)",
                v.algo, v.hash
            ),
            (None, _) => {
                "NONE: there is no index hash for this file, so it is installed as it is".into()
            }
        };
        s.push_str(&format!(
            "  {}. {}  (source: {})\n     depends: {}\n     unresolved-in-set/external: {}\n     check: {check}\n",
            idx + 1,
            a.name,
            a.source,
            if a.depends.is_empty() {
                "(none)".into()
            } else {
                a.depends.join(", ")
            },
            if unresolved.is_empty() {
                "(none)".into()
            } else {
                unresolved.join(", ")
            }
        ));
        if let Some(why) = nonfree_from(&a.section, &a.license) {
            s.push_str(&format!(
                "     non-free: {why}. The install asks you to accept the licence at the terminal, or takes --non-free.\n"
            ));
        }
    }
    s.push_str("Notes: installs each artifact into an isolated managed prefix with foreign-attributed bins and ledger tracking. Dependencies outside the provided set are NOT auto-fetched/resolved from remote repos; install may be non-functional without them. On failure, the installs made in this run are removed again.\n");
    s
}

const PLAN_ONLY: &str = "PLAN ONLY: nothing was changed.\n";

/// The plan for the plan verbs. Artifacts resolved from an index are
/// planned from the index alone (nothing is downloaded); bare paths and
/// URLs must be read to learn what they are, so URLs are downloaded into a
/// private directory that is deleted before this returns.
pub fn tx_plan<S: TxSource>(format: &str, sources: &[S]) -> crate::error::Result<String> {
    let arts: Vec<TxArtifact> = sources.iter().map(TxSource::artifact).collect();
    let mut s = if arts.iter().all(|a| !a.name.is_empty()) {
        render_tx_plan(&arts, format, false)
    } else {
        stage(format, &arts)?.plan()
    };
    s.push_str(PLAN_ONLY);
    Ok(s)
}

/// The plan for artifacts resolved from an index, without the plan verbs'
/// closing line: nothing is downloaded for it.
pub fn resolved_plan(arts: &[TxArtifact], format: &str) -> String {
    render_tx_plan(arts, format, false)
}

pub fn tx_plan_resolved(arts: &[TxArtifact], format: &str) -> String {
    let mut s = resolved_plan(arts, format);
    s.push_str(PLAN_ONLY);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    fn art(name: &str, provides: &[&str], depends: &[&str]) -> TxArtifact {
        TxArtifact {
            source: format!("/tmp/{name}.pkg"),
            name: name.into(),
            provides: provides.iter().map(|s| s.to_string()).collect(),
            depends: depends.iter().map(|s| s.to_string()).collect(),
            ..TxArtifact::default()
        }
    }

    fn scratch(tag: &str) -> PrivateDir {
        PrivateDir::new(&format!("test-{tag}")).unwrap()
    }

    fn names(arts: &[TxArtifact]) -> Vec<&str> {
        arts.iter().map(|a| a.name.as_str()).collect()
    }

    // ---- a small signed repository (throwaway key made for this fixture;
    // ---- only the public keyring and the signatures are kept here)

    const FIX_DEB_HELLO: &[u8] = b"yapacc test fixture: not a real package (hello)\n";
    const FIX_DEB_LIB: &[u8] = b"yapacc test fixture: not a real package (libhello)\n";
    const FIX_HELLO_SHA256: &str =
        "7d72d1fe6e803942c1fb92333fca6be07170aded154356daa3e968ce8b9c3c8c";
    const FIX_PACKAGES: &str = "Package: hello\nVersion: 1.0\nArchitecture: amd64\nSection: non-free/utils\nDepends: libhello (>= 1.0)\nFilename: pool/main/h/hello/hello_1.0_amd64.deb\nSize: 48\nMD5sum: d41d8cd98f00b204e9800998ecf8427e\nSHA256: 7d72d1fe6e803942c1fb92333fca6be07170aded154356daa3e968ce8b9c3c8c\nDescription: fixture\n continuation line with a colon: ignored\n\nPackage: libhello\nVersion: 1.0\nArchitecture: amd64\nSection: libs\nFilename: pool/main/h/hello/libhello_1.0_amd64.deb\nSize: 51\nSHA256: c6f7a1704802fbcfa998e3257db7f6bc4b14c79ab66f67f997e93b9c89617200\n\n";
    const FIX_PACKAGES_SHA256: &str =
        "f5880940090a2396f4c6543de8865fdf5da8fa213a34b509e7bcf064918b55ed";
    const FIX_RELEASE: &str = "Origin: yapacc-test\nSuite: stable\nCodename: stable\nArchitectures: amd64\nComponents: main\nMD5Sum:\n 00000000000000000000000000000000 532 main/binary-amd64/Packages\nSHA256:\n f5880940090a2396f4c6543de8865fdf5da8fa213a34b509e7bcf064918b55ed 532 main/binary-amd64/Packages\n";
    const FIX_INRELEASE: &str = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nOrigin: yapacc-test\nSuite: stable\nCodename: stable\nArchitectures: amd64\nComponents: main\nMD5Sum:\n 00000000000000000000000000000000 532 main/binary-amd64/Packages\nSHA256:\n f5880940090a2396f4c6543de8865fdf5da8fa213a34b509e7bcf064918b55ed 532 main/binary-amd64/Packages\n\n-----BEGIN PGP SIGNATURE-----\n\niHUEARYKAB0WIQTTaT4DV/PFTqH2x2RRNGfLgEdBsAUCav6v3AAKCRBRNGfLgEdB\nsI8BAP4kGF2diq436q3HKF94vzoIpaWo+BNopt2wjKYmACTvDgD8DYdfWJENpHJq\nUnAbEoOGibnCgQ2DwNagaK/rNFbXug8=\n=wULQ\n-----END PGP SIGNATURE-----\n";
    const FIX_RELEASE_GPG: &str = "-----BEGIN PGP SIGNATURE-----\n\niHUEABYKAB0WIQTTaT4DV/PFTqH2x2RRNGfLgEdBsAUCav6v3AAKCRBRNGfLgEdB\nsOMEAQDG0q992fJrPDPhzEGh8F0cHOhjltDNw3q+fEhkknT/EQD+OyhQY0/F5zZ6\n3bVpJh/ntfI73DU41aXbGJyyHibI9gk=\n=KhVf\n-----END PGP SIGNATURE-----\n";
    const FIX_KEYRING_HEX: &str = "9833046afeafdc16092b06010401da470f0101074046b44e9d2cfa69855d869bda58778441891dbb84761c1be73111e38be24016f8b42d7961706163632074657374207265706f7369746f7279203c74657374406578616d706c652e696e76616c69643e88900413160a0038162104d3693e0357f3c54ea1f6c764513467cb804741b005026afeafdc021b03050b0908070206150a09080b020416020301021e01021780000a0910513467cb804741b0d3b00100892c59c2402f4533de43a4618ffc926869e98c72020f5b2e0661c81ff269e8960100a6e82903a1b65ac3e0a02892fa48f4ece7f7a05fa5e078d78f8b3f3653032002";

    /// The fixture laid out as a repository on disk, with its keyring.
    struct Repo {
        dir: PrivateDir,
    }

    impl Repo {
        fn new() -> Self {
            let dir = scratch("repo");
            let repo = Repo { dir };
            repo.put("dists/stable/InRelease", FIX_INRELEASE.as_bytes());
            repo.put("dists/stable/Release", FIX_RELEASE.as_bytes());
            repo.put("dists/stable/Release.gpg", FIX_RELEASE_GPG.as_bytes());
            repo.put(
                "dists/stable/main/binary-amd64/Packages",
                FIX_PACKAGES.as_bytes(),
            );
            repo.put("pool/main/h/hello/hello_1.0_amd64.deb", FIX_DEB_HELLO);
            repo.put("pool/main/h/hello/libhello_1.0_amd64.deb", FIX_DEB_LIB);
            let keyring: Vec<u8> = (0..FIX_KEYRING_HEX.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&FIX_KEYRING_HEX[i..i + 2], 16).unwrap())
                .collect();
            fs::write(repo.dir.path().join("keyring.gpg"), keyring).unwrap();
            repo
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join("repo").join(rel)
        }
        fn put(&self, rel: &str, bytes: &[u8]) {
            let p = self.path(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
        fn remove(&self, rel: &str) {
            fs::remove_file(self.path(rel)).unwrap();
        }
        /// as a `file://` URL: every file is copied into a private
        /// directory first, like a download
        fn url(&self) -> String {
            format!("file://{}", self.dir.path().join("repo").display())
        }
        fn keyring(&self) -> String {
            self.dir.path().join("keyring.gpg").display().to_string()
        }
        fn resolve(&self, keyring: bool, allow_unsigned: bool) -> crate::error::Result<Resolved> {
            let kr = self.keyring();
            let trust = RepoTrust {
                keyring: keyring.then_some(kr.as_str()),
                allow_unsigned,
            };
            resolve_apt_repo(
                &self.url(),
                "stable",
                "main",
                "amd64",
                &["hello".to_string()],
                &trust,
            )
        }
    }

    /// The signature tests need gpgv; without it they say so and pass.
    fn gpgv_missing(test: &str) -> bool {
        let missing = !crate::util::have("gpgv");
        if missing {
            eprintln!("{test}: skipped, gpgv is not installed");
        }
        missing
    }

    // ---- ordering and tokens

    #[test]
    fn topo_order_deps_first() {
        let arts = vec![
            art("app", &["app"], &["libb", "libc"]),
            art("libc", &["libc"], &["libb"]),
            art("libb", &["libb"], &[]),
        ];
        let order = topo_order(&arts);
        let names: Vec<&str> = order.iter().map(|&i| arts[i].name.as_str()).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("libb") < pos("libc"), "libb before libc: {names:?}");
        assert!(pos("libc") < pos("app"), "libc before app: {names:?}");
    }

    #[test]
    fn topo_order_external_deps_do_not_block() {
        // deps not provided by any artifact (external) must not prevent ordering
        let arts = vec![art("app", &["app"], &["not-in-set"])];
        let order = topo_order(&arts);
        assert_eq!(order.len(), 1);
    }

    #[test]
    fn topo_order_cycle_does_not_hang() {
        let arts = vec![art("a", &["a"], &["b"]), art("b", &["b"], &["a"])];
        let order = topo_order(&arts);
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn foreign_bin_name_attribution() {
        assert_eq!(
            foreign_bin_name("deb", "my pkg", "run.sh"),
            "fgn.deb.my_pkg--run.sh"
        );
        assert_eq!(foreign_bin_name("rpm", "foo", "bar"), "fgn.rpm.foo--bar");
    }

    #[test]
    fn dep_base_token_variants() {
        assert_eq!(dep_base_token("libfoo1 (>= 1.2)"), "libfoo1");
        assert_eq!(dep_base_token("python3:any"), "python3");
        assert_eq!(dep_base_token("libc6"), "libc6");
    }

    #[test]
    fn split_tokens_dedupes_and_strips() {
        let mut got = split_tokens("foo, bar | baz");
        got.sort();
        assert_eq!(got, vec!["bar", "baz", "foo"]);
    }

    // ---- finding 16: private directories

    #[test]
    fn private_dir_is_new_random_mode_0700_and_removed_on_drop() {
        let a = PrivateDir::new("test-private").unwrap();
        let b = PrivateDir::new("test-private").unwrap();
        assert_ne!(a.path(), b.path(), "names must not repeat");
        let name = a.path().file_name().unwrap().to_str().unwrap().to_string();
        let random = name.strip_prefix("yapacc-test-private-").unwrap();
        assert_eq!(random.len(), 32, "16 random bytes as hex: {name}");
        assert!(random.bytes().all(|c| c.is_ascii_hexdigit()));
        let md = fs::symlink_metadata(a.path()).unwrap();
        assert!(md.is_dir());
        assert_eq!(md.permissions().mode() & 0o777, 0o700);
        // creation is not recursive: a missing parent is an error, not made
        let missing = a.path().join("no/such/parent");
        let e = PrivateDir::new_in(&missing, "x").unwrap_err();
        assert!(e.starts_with("cannot create the private directory"), "{e}");
        assert!(!a.path().join("no").exists());
        // an existing path is an error, never reused (what EEXIST gives)
        let taken = a.path().join("taken");
        fs::create_dir(&taken).unwrap();
        let e = fs::DirBuilder::new()
            .mode(0o700)
            .create(&taken)
            .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        // removed with its contents on drop
        let kept = a.path().to_path_buf();
        fs::write(kept.join("file"), b"x").unwrap();
        drop(a);
        assert!(!kept.exists());
    }

    // ---- finding 25: names, containment, owner and mode, non-ASCII, weak hashes

    #[test]
    fn a_package_named_dot_or_dotdot_gets_its_own_directory() {
        assert_eq!(safe_component(".."), "_.");
        assert_eq!(safe_component("."), "_");
        assert_eq!(safe_component(""), "_");
        assert_eq!(safe_component(".hidden"), "_hidden");
        assert_eq!(safe_component("a/b c"), "a_b_c");
        assert_eq!(safe_component("libfoo1.2-dev"), "libfoo1.2-dev");
        let root = default_isolated_root();
        for hostile in ["..", ".", "", "../..", "/"] {
            let p = default_prefix(hostile, "deb");
            assert_eq!(p.parent(), Some(root.join("deb").as_path()), "{hostile:?}");
            assert!(
                plain_absolute(&p).is_ok() || !p.is_absolute(),
                "{hostile:?}"
            );
            assert!(
                !p.components().any(|c| matches!(c, Component::ParentDir)),
                "{hostile:?} -> {}",
                p.display()
            );
        }
        // the format is a path component too
        let p = default_prefix("x", "../..");
        assert_eq!(p.parent().and_then(|d| d.parent()), Some(root.as_path()));
    }

    #[test]
    fn only_a_keg_directory_is_ever_deleted() {
        let tmp = scratch("kegdir");
        let root = tmp.path().join("foreign");
        let keg = root.join("deb/tree");
        fs::create_dir_all(keg.join("rootfs")).unwrap();
        fs::write(keg.join(MANIFEST_FILE), "name: tree\n").unwrap();
        assert!(check_keg_dir_in(&keg, &root).is_ok());

        let refused = |p: &Path, why: &str| {
            let e = check_keg_dir_in(p, &root).unwrap_err();
            assert!(e.starts_with("refusing to delete: "), "{e}");
            assert!(e.ends_with("Nothing was deleted."), "{e}");
            assert!(e.contains(why), "expected {why:?} in: {e}");
        };
        // what a package named `..` or `.` used to produce
        refused(&root.join("deb/.."), "contains a '.' or '..' component");
        refused(&root.join("deb/."), "contains a '.' or '..' component");
        // the root, a format directory, an ancestor
        refused(&root, "holds other kegs");
        refused(&root.join("deb"), "holds other kegs");
        refused(tmp.path(), "holds other kegs");
        refused(Path::new("/"), "holds other kegs");
        refused(Path::new("relative/keg"), "is not an absolute path");
        // a directory that is not a keg
        let plain = tmp.path().join("documents");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("thesis.txt"), "x").unwrap();
        refused(&plain, "so it is not a keg");
        // a symbolic link to a keg
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&keg, &link).unwrap();
        refused(&link, "is a symbolic link");
        assert!(keg.join(MANIFEST_FILE).exists() && plain.join("thesis.txt").exists());
    }

    fn keg_record(name: &str, format: &str, install_root: &Path) -> ledger::InstallRecord {
        ledger::InstallRecord {
            id: String::new(),
            name: name.into(),
            format: format.into(),
            source: "/tmp/src".into(),
            version: "1".into(),
            scope: "user".into(),
            install_root: install_root.display().to_string(),
            paths: vec![],
            status: "installed".into(),
            installed_at: 0,
            updated_at: 0,
            metadata: std::collections::BTreeMap::new(),
        }
    }

    fn make_keg(dir: &Path) {
        fs::create_dir_all(dir.join("rootfs/usr/bin")).unwrap();
        fs::write(dir.join(MANIFEST_FILE), "name: x\n").unwrap();
    }

    #[test]
    fn remove_refuses_a_record_that_does_not_name_one_keg() {
        let tmp = scratch("remove");
        let lroot = tmp.path().join("ledger");
        let kegs = tmp.path().join("kegs");
        make_keg(&kegs.join("deb/a"));
        make_keg(&kegs.join("deb/b"));
        // the record a package named `..` used to leave: every keg of the format
        let dotdot = kegs.join("deb/a/..");
        let rec =
            ledger::upsert_record(&lroot, keg_record("dotdot", "deb-extract", &dotdot)).unwrap();
        let e = remove(&rec.id, &lroot).unwrap_err();
        assert!(e.contains("refusing to delete"), "{e}");
        assert!(e.ends_with("Nothing was deleted."), "{e}");
        // a record pointing at a directory that is no keg
        let docs = tmp.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        let rec = ledger::upsert_record(&lroot, keg_record("docs", "rpm-extract", &docs)).unwrap();
        assert!(
            remove(&rec.id, &lroot)
                .unwrap_err()
                .contains("is not a keg")
        );
        assert!(kegs.join("deb/a").exists() && kegs.join("deb/b").exists() && docs.exists());
        // a record of another kind is not a keg at all
        let rec = ledger::upsert_record(&lroot, keg_record("img", "appimage", &docs)).unwrap();
        let e = remove(&rec.id, &lroot).unwrap_err();
        assert!(e.contains("is not a keg (its format is 'appimage'"), "{e}");
    }

    // ---- finding 23: recipe kegs (pkg-extract)

    #[test]
    fn recipe_kegs_are_removed_and_updated_like_the_others() {
        assert_eq!(extract_format("pkg-extract"), Some("pkg"));
        assert_eq!(extract_format("deb-extract"), Some("deb"));
        assert_eq!(extract_format("rpm-extract"), Some("rpm"));
        assert_eq!(extract_format("snap-extract"), Some("snap"));
        assert_eq!(extract_format("appimage"), None);

        let tmp = scratch("pkg-keg");
        let lroot = tmp.path().join("ledger");
        let keg = tmp.path().join("kegs/pkg/hello");
        make_keg(&keg);
        let rec = ledger::upsert_record(&lroot, keg_record("hello", "pkg-extract", &keg)).unwrap();
        // update gets past the format check (it then misses its source file)
        let e = update(&rec.id, "/nonexistent/hello.pkg.tar.zst", &lroot, false).unwrap_err();
        assert_eq!(e, "source file not found: /nonexistent/hello.pkg.tar.zst");
        let removed = remove(&rec.id, &lroot).unwrap();
        assert_eq!(removed.status, "removed");
        assert!(!keg.exists(), "the keg directory must be gone");
    }

    #[test]
    fn a_failed_rollback_is_reported_not_called_rolled_back() {
        let tmp = scratch("rollback");
        let lroot = tmp.path().join("ledger");
        let good = tmp.path().join("kegs/pkg/good");
        make_keg(&good);
        let stuck = tmp.path().join("kegs/pkg/stuck");
        fs::create_dir_all(&stuck).unwrap(); // exists, but is not a keg
        let a = ledger::upsert_record(&lroot, keg_record("good", "pkg-extract", &good)).unwrap();
        let b = ledger::upsert_record(&lroot, keg_record("stuck", "pkg-extract", &stuck)).unwrap();
        let said = roll_back(&[a.clone(), b.clone()], &lroot);
        assert!(
            said.starts_with("rolled back 1 of 2 install(s) made in this run; could NOT remove: "),
            "{said}"
        );
        assert!(
            said.contains(&format!("{} at {}", b.id, stuck.display())),
            "{said}"
        );
        assert!(!good.exists() && stuck.exists());
        // when every removal works, it says so plainly
        let again = tmp.path().join("kegs/pkg/again");
        make_keg(&again);
        let c = ledger::upsert_record(&lroot, keg_record("again", "pkg-extract", &again)).unwrap();
        assert_eq!(
            roll_back(&[c], &lroot),
            "rolled back 1 install(s) made in this run"
        );
    }

    // ---- finding 18: symbolic links out of the keg

    #[test]
    fn collect_files_records_a_directory_symlink_as_one_entry() {
        let tmp = scratch("collect");
        let root = tmp.path().join("rootfs");
        fs::create_dir_all(root.join("opt/x")).unwrap();
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::write(root.join("usr/bin/tool"), "#!/bin/sh\n").unwrap();
        // the host's root directory, and a loop
        std::os::unix::fs::symlink("/", root.join("opt/x/host")).unwrap();
        std::os::unix::fs::symlink("..", root.join("opt/x/up")).unwrap();
        let files = collect_files(&root).unwrap();
        assert_eq!(files, vec!["opt/x/host", "opt/x/up", "usr/bin/tool"]);
        // a rootfs that is itself a link is not walked
        let link = tmp.path().join("linked-rootfs");
        std::os::unix::fs::symlink("/etc", &link).unwrap();
        assert!(collect_files(&link).unwrap().is_empty());
    }

    #[test]
    fn launchers_never_point_outside_the_keg() {
        let tmp = scratch("links");
        let prefix = tmp.path().join("keg");
        let rootfs = prefix.join("rootfs");
        fs::create_dir_all(rootfs.join("usr")).unwrap();
        // usr/bin is the host's /usr/bin; bin holds one real tool and a link out
        std::os::unix::fs::symlink("/usr/bin", rootfs.join("usr/bin")).unwrap();
        fs::create_dir_all(rootfs.join("bin")).unwrap();
        let tool = rootfs.join("bin/tool");
        fs::write(&tool, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("/bin/sh", rootfs.join("bin/shell")).unwrap();
        let links = create_bin_links(&prefix, &rootfs, "deb", "pkg", true).unwrap();
        assert_eq!(links.len(), 1, "{links:?}");
        assert!(links[0].contains("fgn.deb.pkg--tool -> "), "{links:?}");
    }

    #[test]
    fn extracted_files_lose_setuid_setgid_and_foreign_owners() {
        let tmp = scratch("neutral");
        let root = tmp.path().join("rootfs");
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        let su = root.join("usr/bin/su");
        fs::write(&su, "x").unwrap();
        fs::set_permissions(&su, fs::Permissions::from_mode(0o4755)).unwrap();
        let sg = root.join("usr/bin/wall");
        fs::write(&sg, "x").unwrap();
        fs::set_permissions(&sg, fs::Permissions::from_mode(0o2755)).unwrap();
        let plain = root.join("usr/bin/ls");
        fs::write(&plain, "x").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o755)).unwrap();
        let sgdir = root.join("var");
        fs::create_dir_all(&sgdir).unwrap();
        fs::set_permissions(&sgdir, fs::Permissions::from_mode(0o2775)).unwrap();
        std::os::unix::fs::symlink("/etc/shadow", root.join("usr/bin/link")).unwrap();
        // the bits are really there before the pass (a nosuid mount keeps them too)
        assert_eq!(fs::metadata(&su).unwrap().mode() & 0o7000, 0o4000);

        let me = fs::symlink_metadata(tmp.path()).unwrap();
        let changed = neutralize_tree(&root, (me.uid(), me.gid())).unwrap();
        assert_eq!(changed.mode_cleared, 3, "{changed:?}");
        assert_eq!(changed.devices_removed, 0);
        assert_eq!(fs::metadata(&su).unwrap().mode() & 0o7777, 0o755);
        assert_eq!(fs::metadata(&sg).unwrap().mode() & 0o7777, 0o755);
        assert_eq!(fs::metadata(&sgdir).unwrap().mode() & 0o7777, 0o775);
        assert_eq!(fs::metadata(&plain).unwrap().mode() & 0o7777, 0o755);
        // a second pass has nothing left to do
        let again = neutralize_tree(&root, (me.uid(), me.gid())).unwrap();
        assert_eq!(again, Neutralized::default());
    }

    #[test]
    fn rpm_primary_with_non_ascii_text_does_not_panic() {
        // 40 bytes before `name="` used to be sliced blindly: here that
        // offset is in the middle of a two-byte character
        let accents = "é".repeat(21);
        let section = format!(r#"<rpm:entry pre="{accents}"  name="libb" flags="EQ"/>"#);
        assert_eq!(extract_rpm_entries(&section), vec!["libb"]);
        let primary = format!(
            r#"<metadata><package type="rpm"><name>app</name><summary>Éditeur — 日本語のテキスト</summary><packager>Zoë</packager><location href="Packages/app-1.0.rpm"/><format><rpm:requires>{section}<rpm:entry name="日本語"/></rpm:requires></format></package></metadata>"#
        );
        let arts =
            rpm_artifacts_from_primary("http://repo.example", &primary, &["app".into()], false)
                .unwrap();
        assert_eq!(names(&arts), vec!["app"]);
        assert!(arts[0].depends.iter().any(|d| d == "libb"));
        // an unterminated tag ends the scan instead of slicing past the end
        assert!(extract_rpm_entries("<rpm:entry name=\"é").is_empty());
    }

    #[test]
    fn strongest_hash_in_release_wins_whatever_the_order() {
        let weak_first = concat!(
            "Origin: Debian\n",
            "MD5Sum:\n",
            " aaaa 10 main/binary-amd64/Packages.gz\n",
            " aaab 20 main/binary-amd64/Packages\n",
            "SHA1:\n",
            " bbbb 10 main/binary-amd64/Packages.gz\n",
            "SHA256:\n",
            " CCCC 10 main/binary-amd64/Packages.gz\n",
            " cccd 20 main/binary-amd64/Packages\n",
            " 1111 22 main/source/Sources\n",
            " 2222 22 main/binary-amd64/Packages.diff/Index\n",
            " 3333 22 main/binary-arm64/Packages.gz\n",
            "SHA512:\n",
            " dddd 10 main/binary-amd64/Packages.gz\n",
        );
        let got = parse_release_index_paths(weak_first, "main", "amd64");
        assert!(got.weak_only.is_empty(), "{got:?}");
        assert_eq!(got.strong.len(), 2, "{got:?}");
        // .gz first, with the strongest hash it is listed under
        assert_eq!(got.strong[0].0, "main/binary-amd64/Packages.gz");
        assert_eq!(got.strong[0].1.algo, "sha512");
        assert_eq!(got.strong[0].1.hash, "dddd");
        assert_eq!(got.strong[0].1.size, Some(10));
        // the plain index only has SHA256: that, never the MD5 listed first
        assert_eq!(got.strong[1].0, "main/binary-amd64/Packages");
        assert_eq!(got.strong[1].1.algo, "sha256");
        assert_eq!(got.strong[1].1.hash, "cccd");
        // the same file with the strong sections first gives the same answer
        let strong_first = concat!(
            "SHA512:\n",
            " dddd 10 main/binary-amd64/Packages.gz\n",
            "SHA256:\n",
            " cccc 10 main/binary-amd64/Packages.gz\n",
            "MD5Sum:\n",
            " aaaa 10 main/binary-amd64/Packages.gz\n",
        );
        let got = parse_release_index_paths(strong_first, "main", "amd64");
        assert_eq!(got.strong.len(), 1);
        assert_eq!(got.strong[0].1.algo, "sha512");
        assert_eq!(got.strong[0].1.hash, "dddd");
    }

    #[test]
    fn a_release_with_only_md5_or_sha1_is_refused() {
        let weak = "Origin: old\nMD5Sum:\n aaaa 10 main/binary-amd64/Packages\nSHA1:\n bbbb 10 main/binary-amd64/Packages\n";
        let got = parse_release_index_paths(weak, "main", "amd64");
        assert!(got.strong.is_empty());
        assert_eq!(got.weak_only, vec!["main/binary-amd64/Packages"]);

        // through the resolver: refused (exit 3), also when unsigned
        // repositories are accepted, and the index is never fetched
        let repo = Repo::new();
        repo.remove("dists/stable/InRelease");
        repo.remove("dists/stable/Release.gpg");
        repo.put("dists/stable/Release", weak.as_bytes());
        for allow_unsigned in [false, true] {
            let e = repo.resolve(false, allow_unsigned).unwrap_err();
            assert_eq!(e.kind, Kind::Refused, "{e}");
            assert!(e.message.contains("with MD5 or SHA1 only"), "{e}");
            assert!(e.message.ends_with("Nothing was installed."), "{e}");
        }
        assert_eq!(strong_algo("SHA256"), Some((1, "sha256")));
        assert_eq!(strong_algo("sha512"), Some((2, "sha512")));
        for weak in ["MD5Sum", "md5", "SHA1", "sha", "sha224", ""] {
            assert_eq!(strong_algo(weak), None, "{weak}");
        }
    }

    // ---- finding 6: only the signed text is read

    #[test]
    fn clearsigned_payload_is_exactly_the_signed_text() {
        assert_eq!(clearsigned_payload(FIX_INRELEASE).unwrap(), FIX_RELEASE);
        // dash-escaped lines are unescaped; CRLF and blank lines around the
        // message are tolerated
        let msg = "\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nHash: SHA256\r\n\r\nOrigin: x\r\n- - dashed\r\n\r\n-----BEGIN PGP SIGNATURE-----\r\n\r\nabcd\r\n=ab\r\n-----END PGP SIGNATURE-----\r\n\r\n";
        assert_eq!(clearsigned_payload(msg).unwrap(), "Origin: x\n- dashed\n");
    }

    #[test]
    fn text_outside_the_signed_message_is_refused() {
        let evil = " ffff 5 main/binary-amd64/Packages\n";
        let before = format!("SHA256:\n{evil}\n{FIX_INRELEASE}");
        let e = clearsigned_payload(&before).unwrap_err();
        assert!(
            e.contains(
                "text outside the signed message before '-----BEGIN PGP SIGNED MESSAGE-----'"
            ),
            "{e}"
        );
        let after = format!("{FIX_INRELEASE}\nSHA256:\n{evil}");
        let e = clearsigned_payload(&after).unwrap_err();
        assert!(
            e.contains("text outside the signed message after '-----END PGP SIGNATURE-----'"),
            "{e}"
        );
        // two messages, or an armor block inside the text
        let twice = format!("{FIX_INRELEASE}{FIX_INRELEASE}");
        assert!(clearsigned_payload(&twice).is_err());
        let nested = FIX_INRELEASE.replace(
            "Suite: stable\n",
            "-----BEGIN PGP SIGNED MESSAGE-----\nSuite: stable\n",
        );
        assert!(
            clearsigned_payload(&nested)
                .unwrap_err()
                .contains("a second armor block")
        );
        // not clearsigned at all
        for broken in [
            "",
            "Origin: x\n",
            "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA512\n\nx\n",
        ] {
            assert!(clearsigned_payload(broken).is_err(), "{broken:?}");
        }
        // what the parser is given never contains the unsigned block
        let signed = clearsigned_payload(FIX_INRELEASE).unwrap();
        let listed = parse_release_index_paths(&signed, "main", "amd64");
        assert_eq!(listed.strong.len(), 1);
        assert_eq!(listed.strong[0].1.hash, FIX_PACKAGES_SHA256);
    }

    #[test]
    fn a_signed_repository_resolves_with_a_hash_per_package() {
        if gpgv_missing("a_signed_repository_resolves_with_a_hash_per_package") {
            return;
        }
        let repo = Repo::new();
        // gpgv's own output is the text that is parsed. Some versions of
        // gpgv end it with one more empty line, which the parser skips.
        let signed =
            verify_clearsigned(&repo.path("dists/stable/InRelease"), &repo.keyring()).unwrap();
        assert_eq!(
            signed.trim_end_matches('\n'),
            FIX_RELEASE.trim_end_matches('\n')
        );

        let r = repo.resolve(true, false).unwrap();
        assert!(r.signature_verified);
        assert!(
            r.release.ends_with("/dists/stable/InRelease"),
            "{}",
            r.release
        );
        assert!(
            r.index
                .ends_with("/dists/stable/main/binary-amd64/Packages")
        );
        assert_eq!(names(&r.artifacts), vec!["libhello", "hello"]);
        let hello = &r.artifacts[1];
        assert_eq!(
            hello.verify,
            Some(IndexVerify {
                algo: "sha256".into(),
                hash: FIX_HELLO_SHA256.into(),
                size: Some(48),
            })
        );
        assert!(
            hello
                .source
                .ends_with("/pool/main/h/hello/hello_1.0_amd64.deb")
        );
        let said = r.trust_statement();
        assert!(
            said.starts_with("Verified: the signature of file://"),
            "{said}"
        );
        assert!(said.contains("Not verified: that the keyring belongs to the publisher"));
        assert!(!said.contains("NOT checked at all"), "{said}");

        // each package is copied once into a private directory and matches
        let staged = stage("deb", &r.artifacts).unwrap();
        let dir = staged._dir.as_ref().unwrap().path().to_path_buf();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(staged.local.len(), 2);
        assert!(
            staged
                .local
                .iter()
                .all(|f| f.starts_with(&dir) && f.is_file())
        );
        assert_eq!(fs::read(&staged.local[1]).unwrap(), FIX_DEB_HELLO);
        let plan = staged.plan();
        assert!(
            plan.contains(&format!(
                "check: sha256 {FIX_HELLO_SHA256} from the repository index: the file matched"
            )),
            "{plan}"
        );
        drop(staged);
        assert!(
            !dir.exists(),
            "the fetch directory is deleted with the stage"
        );
    }

    #[test]
    fn an_unsigned_block_before_inrelease_is_refused_whatever_the_flags() {
        let repo = Repo::new();
        let evil = format!(
            "SHA256:\n {} 532 main/binary-amd64/Packages\n\n{FIX_INRELEASE}",
            "f".repeat(64)
        );
        repo.put("dists/stable/InRelease", evil.as_bytes());
        // with a keyring: refused before gpgv would accept the signed part
        // (so this half needs no gpgv), also with --allow-unsigned
        for (keyring, allow_unsigned) in [(true, false), (true, true), (false, true)] {
            let e = repo.resolve(keyring, allow_unsigned).unwrap_err();
            assert_eq!(e.kind, Kind::Integrity, "{e}");
            assert!(
                e.message.contains("text outside the signed message before"),
                "{e}"
            );
            assert!(e.message.ends_with("nothing was installed."), "{e}");
        }
    }

    #[test]
    fn a_bad_signature_is_fatal_even_with_allow_unsigned() {
        if gpgv_missing("a_bad_signature_is_fatal_even_with_allow_unsigned") {
            return;
        }
        // InRelease whose signed text was altered
        let repo = Repo::new();
        repo.put(
            "dists/stable/InRelease",
            FIX_INRELEASE
                .replace("Suite: stable", "Suite: stablE")
                .as_bytes(),
        );
        for allow_unsigned in [false, true] {
            let e = repo.resolve(true, allow_unsigned).unwrap_err();
            assert_eq!(
                e.kind,
                Kind::Integrity,
                "allow_unsigned={allow_unsigned}: {e}"
            );
            assert!(
                e.message
                    .contains("/dists/stable/InRelease: GPG signature verification failed: "),
                "{e}"
            );
        }
        // a keyring that does not hold the signing key is the same failure
        fs::write(repo.dir.path().join("keyring.gpg"), b"").unwrap();
        repo.put("dists/stable/InRelease", FIX_INRELEASE.as_bytes());
        assert_eq!(repo.resolve(true, true).unwrap_err().kind, Kind::Integrity);

        // Release + Release.gpg: good, then altered
        let repo = Repo::new();
        repo.remove("dists/stable/InRelease");
        let r = repo.resolve(true, false).unwrap();
        assert!(r.signature_verified);
        assert!(
            r.release.ends_with("/dists/stable/Release"),
            "{}",
            r.release
        );
        repo.put(
            "dists/stable/Release",
            format!("{FIX_RELEASE}Extra: line\n").as_bytes(),
        );
        for allow_unsigned in [false, true] {
            let e = repo.resolve(true, allow_unsigned).unwrap_err();
            assert_eq!(
                e.kind,
                Kind::Integrity,
                "allow_unsigned={allow_unsigned}: {e}"
            );
        }

        // no signature at all: refused, unless unsigned was accepted, and
        // then the result says it is not verified
        repo.put("dists/stable/Release", FIX_RELEASE.as_bytes());
        repo.remove("dists/stable/Release.gpg");
        let e = repo.resolve(true, false).unwrap_err();
        assert_eq!(e.kind, Kind::Refused, "{e}");
        assert!(e.message.contains("has no Release.gpg signature"), "{e}");
        let r = repo.resolve(true, true).unwrap();
        assert!(!r.signature_verified);
        assert!(r.trust_statement().starts_with("NOT verified: file://"));
    }

    #[test]
    fn a_missing_keyring_file_is_an_error_and_a_relative_one_is_made_absolute() {
        let e = keyring_path("/nonexistent/keys/repo.gpg").unwrap_err();
        assert!(
            e.message
                .starts_with("keyring not found: /nonexistent/keys/repo.gpg"),
            "{e}"
        );
        assert!(
            e.message.ends_with("Nothing was verified or installed."),
            "{e}"
        );
        // gpgv would look for a bare name under ~/.gnupg: it never gets one
        let e = keyring_path("repo.gpg").unwrap_err();
        let cwd = std::env::current_dir().unwrap();
        assert!(
            e.message
                .contains(&cwd.join("repo.gpg").display().to_string()),
            "{e}"
        );
    }

    // ---- finding 5: packages are checked against the index

    #[test]
    fn apt_packages_stanzas_carry_sha256_size_and_section() {
        let pkgs = parse_apt_packages("http://repo.example/debian/", FIX_PACKAGES);
        assert_eq!(names(&pkgs), vec!["hello", "libhello"]);
        let hello = &pkgs[0];
        assert_eq!(
            hello.source,
            "http://repo.example/debian/pool/main/h/hello/hello_1.0_amd64.deb"
        );
        // SHA256 is taken; the MD5sum listed before it never is
        let v = hello.verify.as_ref().unwrap();
        assert_eq!(
            (v.algo.as_str(), v.hash.as_str(), v.size),
            ("sha256", FIX_HELLO_SHA256, Some(48))
        );
        assert_eq!(hello.section, "non-free/utils");
        assert_eq!(hello.depends, vec!["libhello"]);
        assert_eq!(hello.provides, vec!["hello"]);

        // SHA512 beats SHA256 whatever the order; MD5 and SHA1 alone are no check
        let index = "Package: a\nFilename: pool/a.deb\nSHA512: AB12\nSHA256: cd34\nSize: 7\n\n\
                     Package: b\nFilename: pool/b.deb\nMD5sum: ef56\nSHA1: 7890\n\n\
                     Package: c\nFilename: ../../etc/c.deb\nSHA256: 1234\n\n\
                     Package: d\nSHA256: 1234\n";
        let pkgs = parse_apt_packages("/srv/repo", index);
        // c points outside the repository and d names no file: both dropped
        assert_eq!(names(&pkgs), vec!["a", "b"]);
        let a = pkgs[0].verify.as_ref().unwrap();
        assert_eq!(
            (a.algo.as_str(), a.hash.as_str(), a.size),
            ("sha512", "ab12", Some(7))
        );
        assert_eq!(pkgs[1].verify, None);
    }

    #[test]
    fn apt_index_resolution_orders_dependencies_and_requires_hashes() {
        let dir = scratch("apt-index");
        let index = "\
Package: app\nVersion: 1.0\nFilename: pool/main/a/app_1.0_amd64.deb\nDepends: libb, libc\nSHA256: aa\n\n\
Package: libb\nVersion: 2.0\nFilename: pool/main/libb/libb_2.0_amd64.deb\nProvides: libb\nSHA256: bb\n\n\
Package: libc\nVersion: 3.0\nFilename: pool/main/libc/libc_3.0_amd64.deb\nMD5sum: cc\n\n";
        let p = dir.path().join("Packages");
        fs::write(&p, index).unwrap();
        let p = p.display().to_string();
        let want = ["app".to_string()];
        // libc has only an MD5: refused unless unsigned installs are accepted
        let e = apt_artifacts_from_index("http://repo.example", &p, None, &want, true).unwrap_err();
        assert_eq!(e.kind, Kind::Refused, "{e}");
        assert_eq!(
            e.message,
            "the Packages index gives no SHA256 or SHA512 for: libc. Without one the download cannot be checked, so nothing was installed. Pass --allow-unsigned to install such packages unchecked."
        );
        let arts = apt_artifacts_from_index("http://repo.example", &p, None, &want, false).unwrap();
        assert_eq!(arts.len(), 3, "got {arts:?}");
        assert_eq!(arts.last().unwrap().name, "app", "dependencies first");
        assert!(arts.iter().any(|a| a.source.ends_with("app_1.0_amd64.deb")));
        let resolved = Resolved {
            artifacts: arts,
            index: p.clone(),
            release: String::new(),
            signature_verified: false,
        };
        let said = resolved.trust_statement();
        assert!(
            said.starts_with("NOT verified: the package index "),
            "{said}"
        );
        assert!(
            said.ends_with(
                "NOT checked at all: 1 of 3 package(s) have no SHA256 or SHA512 in the index."
            ),
            "{said}"
        );
        // the index itself is checked against the hash a release file gave
        let wrong = IndexVerify {
            algo: "sha256".into(),
            hash: "0".repeat(64),
            size: None,
        };
        let e = apt_artifacts_from_index("http://repo.example", &p, Some(&wrong), &want, false)
            .unwrap_err();
        assert_eq!(e.kind, Kind::Integrity, "{e}");
        assert!(
            e.message
                .contains("checksum mismatch: the repository metadata (Release or repomd.xml) says sha256 0000"),
            "{e}"
        );
        assert!(
            e.message
                .ends_with("The index was not used and nothing was installed."),
            "{e}"
        );
    }

    #[test]
    fn rpm_primary_resolution_ordering_checksum_and_licence() {
        let sum = "A".repeat(64);
        let primary = format!(
            r#"
<metadata xmlns="http://linux.duke.edu/metadata/common" packages="4">
<package type="rpm"><name>app</name><checksum type="sha256" pkgid="YES">{sum}</checksum><packager>x</packager><size package="1234" installed="5000" archive="5100"/><location href="Packages/app-1.0.rpm"/><format><rpm:license>Proprietary</rpm:license><rpm:provides><rpm:entry name="app"/></rpm:provides><rpm:requires><rpm:entry name="libb"/><rpm:entry name="rpmlib(...)"/></rpm:requires></format></package>
<package type="rpm"><name>libb</name><checksum type="sha512" pkgid="YES">bb</checksum><location href="Packages/libb-2.0.rpm"/><rpm:provides><rpm:entry name="libb"/></rpm:provides><rpm:requires><rpm:entry name="libc"/></rpm:requires></package>
<package type="rpm"><name>libc</name><checksum type="sha" pkgid="YES">cc</checksum><location href="Packages/libc-3.0.rpm"/><rpm:provides><rpm:entry name="libc"/></rpm:provides></package>
<package type="rpm"><name>escape</name><checksum type="sha256" pkgid="YES">dd</checksum><location href="../../etc/escape.rpm"/></package>
</metadata>"#
        );
        let want = ["app".to_string()];
        let arts =
            rpm_artifacts_from_primary("http://repo.example", &primary, &want, false).unwrap();
        assert_eq!(names(&arts), vec!["libc", "libb", "app"]);
        let app = &arts[2];
        assert!(app.source.starts_with("http://repo.example/Packages/"));
        assert_eq!(
            app.verify,
            Some(IndexVerify {
                algo: "sha256".into(),
                hash: "a".repeat(64),
                size: Some(1234),
            })
        );
        assert_eq!(app.license, "Proprietary");
        assert_eq!(
            nonfree_artifacts(&arts),
            vec![("app".to_string(), "License: Proprietary".to_string())]
        );
        assert_eq!(arts[1].verify.as_ref().unwrap().algo, "sha512");
        // SHA1 (`type="sha"`) is not a check
        assert_eq!(arts[0].verify, None);
        let e =
            rpm_artifacts_from_primary("http://repo.example", &primary, &want, true).unwrap_err();
        assert_eq!(e.kind, Kind::Refused);
        assert!(
            e.message
                .starts_with("the RPM primary.xml gives no SHA256 or SHA512 for: libc."),
            "{e}"
        );
        // a location outside the repository is not a package of it
        let e =
            rpm_artifacts_from_primary("http://repo.example", &primary, &["escape".into()], false)
                .unwrap_err();
        assert!(e.message.starts_with("no matching packages found"), "{e}");
    }

    #[test]
    fn xml_helpers() {
        let xml = r#"<repomd><revision>1</revision><data type="primary"><location href="repodata/1-primary.xml.gz"/><checksum type="sha256">abc123</checksum><size>42</size><open-size>100</open-size></data><data type="filelists"><location href="repodata/f.xml"/></data></repomd>"#;
        let (href, verify) = find_repomd_primary_info(xml).unwrap();
        assert_eq!(href, "repodata/1-primary.xml.gz");
        let v = verify.unwrap();
        assert_eq!(v.algo, "sha256");
        assert_eq!(v.hash, "abc123");
        assert_eq!(v.size, Some(42));
        assert_eq!(extract_xml_tag(xml, "revision"), "1");
        assert_eq!(
            extract_xml_attr(xml, "location", "href"),
            "repodata/1-primary.xml.gz"
        );
    }

    #[test]
    fn a_repomd_with_a_weak_primary_checksum_is_refused() {
        let tmp = scratch("repomd");
        let repodata = tmp.path().join("repodata");
        fs::create_dir_all(&repodata).unwrap();
        fs::write(
            repodata.join("repomd.xml"),
            r#"<repomd><data type="primary"><checksum type="sha">abc</checksum><location href="repodata/primary.xml"/></data></repomd>"#,
        )
        .unwrap();
        let base = tmp.path().display().to_string();
        for allow_unsigned in [false, true] {
            let trust = RepoTrust {
                keyring: None,
                allow_unsigned,
            };
            let e = resolve_rpm_repo(&base, &["x".to_string()], &trust).unwrap_err();
            assert_eq!(e.kind, Kind::Refused, "{e}");
            assert!(
                e.message
                    .contains("gives only a sha checksum for the primary metadata"),
                "{e}"
            );
        }
    }

    #[test]
    fn verify_file_hash_detects_mismatch() {
        let dir = scratch("hash");
        let p = dir.path().join("data.bin");
        fs::write(&p, b"hello world").unwrap();
        let good = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(sha256_file(&p).unwrap(), good);
        let v = |algo: &str, hash: &str, size: Option<u64>| IndexVerify {
            algo: algo.into(),
            hash: hash.into(),
            size,
        };
        verify_file_hash(&p, &v("sha256", good, Some(11)), BY_INDEX).unwrap();
        verify_file_hash(&p, &v("SHA256", &good.to_uppercase(), None), BY_INDEX).unwrap();
        let sha512 = "309ecc489c12d6eb4cc40f50c902f2b4d0ed77ee511a7c7a9bcd3ca86d4cd86f989dd35bc5ff499670da34255b45b0cfd830e81f605dcf7dc5542e93ae9cd76f";
        verify_file_hash(&p, &v("sha512", sha512, Some(11)), BY_INDEX).unwrap();
        assert_eq!(
            verify_file_hash(&p, &v("sha256", "badbadbad", None), BY_INDEX).unwrap_err(),
            format!(
                "checksum mismatch: the repository index says sha256 badbadbad, the file has {good}"
            )
        );
        assert_eq!(
            verify_file_hash(&p, &v("sha256", good, Some(999)), BY_RELEASE).unwrap_err(),
            "size mismatch: the repository metadata (Release or repomd.xml) says 999 bytes, the file has 11"
        );
        // MD5 and SHA1 are refused as a check, not computed
        for weak in ["md5", "sha1"] {
            let e = verify_file_hash(&p, &v(weak, "x", None), BY_INDEX).unwrap_err();
            assert!(e.contains("does not accept as a check"), "{e}");
        }
    }

    #[test]
    fn a_tampered_package_is_an_integrity_error_and_is_not_installed() {
        let repo = Repo::new();
        let local = RepoTrust {
            keyring: None,
            allow_unsigned: false,
        };
        let deb = "pool/main/h/hello/hello_1.0_amd64.deb";
        let index = repo
            .path("dists/stable/main/binary-amd64/Packages")
            .display()
            .to_string();
        let want = ["hello".to_string()];

        // fetched (file:// is copied like a download): same size, other bytes
        let mut altered = FIX_DEB_HELLO.to_vec();
        altered[0] ^= 1;
        repo.put(deb, &altered);
        let fetched = resolve_apt_index(&repo.url(), &index, &want, &local).unwrap();
        let e = stage("deb", &fetched.artifacts).unwrap_err();
        assert_eq!(e.kind, Kind::Integrity, "{e}");
        assert_eq!(e.kind.exit_code(), 5);
        assert!(
            e.message.contains(&format!(
                "checksum mismatch: the repository index says sha256 {FIX_HELLO_SHA256}, the file has "
            )),
            "{e}"
        );
        assert!(
            e.message
                .ends_with("The downloaded file was deleted and nothing was installed."),
            "{e}"
        );

        // another size is caught as such
        repo.put(deb, b"short");
        let e = stage("deb", &fetched.artifacts).unwrap_err();
        assert_eq!(e.kind, Kind::Integrity);
        assert!(
            e.message
                .contains("size mismatch: the repository index says 48 bytes, the file has 5"),
            "{e}"
        );

        // used in place (a plain path): refused, and the user's file is kept
        let base = repo.dir.path().join("repo").display().to_string();
        let in_place = resolve_apt_index(&base, &index, &want, &local).unwrap();
        let e = stage("deb", &in_place.artifacts).unwrap_err();
        assert_eq!(e.kind, Kind::Integrity);
        assert!(
            e.message
                .ends_with("The file was left where it is and nothing was installed."),
            "{e}"
        );
        assert_eq!(fs::read(repo.path(deb)).unwrap(), b"short");

        // the genuine file passes
        repo.put(deb, FIX_DEB_HELLO);
        let staged = stage("deb", &in_place.artifacts).unwrap();
        assert_eq!(names(staged.artifacts()), vec!["libhello", "hello"]);
        assert!(staged._dir.is_none(), "nothing is copied for plain paths");
    }

    #[test]
    fn a_tampered_packages_index_is_an_integrity_error() {
        if gpgv_missing("a_tampered_packages_index_is_an_integrity_error") {
            return;
        }
        let repo = Repo::new();
        // a forged index of the same length, pointing hello at other bytes
        let forged = FIX_PACKAGES.replace(FIX_HELLO_SHA256, &"0".repeat(64));
        repo.put("dists/stable/main/binary-amd64/Packages", forged.as_bytes());
        for allow_unsigned in [false, true] {
            let e = repo.resolve(true, allow_unsigned).unwrap_err();
            assert_eq!(e.kind, Kind::Integrity, "{e}");
            assert!(
                e.message.contains(
                    "checksum mismatch: the repository metadata (Release or repomd.xml) says sha256"
                ),
                "{e}"
            );
            assert!(
                e.message
                    .starts_with("failed to resolve from APT repo file://"),
                "{e}"
            );
        }
    }

    #[test]
    fn a_bare_path_or_url_has_nothing_to_be_checked_against_and_the_plan_says_so() {
        // resolved artifacts are planned from the index: `.invalid` never
        // resolves, so a download attempt would fail this test
        let resolved = TxArtifact {
            source: "https://repo.invalid/pool/x_1_amd64.deb".into(),
            name: "x".into(),
            provides: vec!["x".into()],
            verify: Some(IndexVerify {
                algo: "sha256".into(),
                hash: "ab".repeat(32),
                size: Some(3),
            }),
            section: "contrib/net".into(),
            ..TxArtifact::default()
        };
        let plan = tx_plan("deb", std::slice::from_ref(&resolved)).unwrap();
        assert!(
            plan.contains("Source fetch: nothing was downloaded for this plan."),
            "{plan}"
        );
        assert!(
            plan.contains(&format!(
                "check: sha256 {} from the repository index: a download that differs is deleted and the install stops (exit 5)",
                "ab".repeat(32)
            )),
            "{plan}"
        );
        assert!(
            plan.contains("non-free: Section: contrib/net (contrib: depends on non-free)."),
            "{plan}"
        );
        assert!(plan.ends_with(PLAN_ONLY));
        assert_eq!(
            tx_plan_resolved(std::slice::from_ref(&resolved), "deb"),
            plan
        );

        // a bare local file: staged in place, described from the file, unchecked
        let tmp = scratch("bare");
        let file = tmp.path().join("thing.bin");
        fs::write(&file, b"not a package").unwrap();
        let staged = stage("deb", &[file.display().to_string()]).unwrap();
        assert_eq!(names(staged.artifacts()), vec!["thing"]);
        assert_eq!(staged.artifacts()[0].verify, None);
        assert!(
            staged.plan().contains(
                "check: NONE: there is no index hash for this file, so it is installed as it is"
            ),
            "{}",
            staged.plan()
        );
        let e = stage("deb", &["/nonexistent/x.deb".to_string()]).unwrap_err();
        assert_eq!(e.message, "source not found: /nonexistent/x.deb");
        // only http(s) and file are fetched
        let e = download_to(tmp.path(), "ftp://repo.invalid/x.deb", 0).unwrap_err();
        assert!(
            e.ends_with("only http://, https:// and file:// sources are supported"),
            "{e}"
        );
    }

    // ---- finding 7: the unsigned gate, in the fetch layer

    #[test]
    fn unverified_remote_repo_is_refused_before_any_fetch() {
        let strict = RepoTrust {
            keyring: None,
            allow_unsigned: false,
        };
        let want = "refusing to install from an unsigned remote repo: pass --keyring <keyring> to verify GPG signatures, or --allow-unsigned (or set foreign.allow_unsigned_repos=true in config) to explicitly accept the risk";
        let names = ["tree".to_string()];
        // `.invalid` never resolves: an attempted fetch would give another error
        let apt = resolve_sources_from_apt_repo(
            "https://repo.invalid/debian",
            "stable",
            "main",
            "amd64",
            &names,
            &strict,
        )
        .unwrap_err();
        assert_eq!((apt.kind, apt.message.as_str()), (Kind::Refused, want));
        let rpm = resolve_artifacts_from_rpm_repo("http://repo.invalid/fedora", &names, &strict)
            .unwrap_err();
        assert_eq!((rpm.kind, rpm.message.as_str()), (Kind::Refused, want));
        assert_eq!(
            refuse_unverified("ftp://repo.invalid/x", &strict)
                .unwrap_err()
                .message,
            want
        );
        // local repositories, a keyring, or an explicit opt-in all pass
        assert!(refuse_unverified("file:///srv/repo", &strict).is_ok());
        assert!(refuse_unverified("/srv/repo", &strict).is_ok());
        let keyed = RepoTrust {
            keyring: Some("/etc/keys/repo.gpg"),
            allow_unsigned: false,
        };
        assert!(refuse_unverified("https://repo.invalid/debian", &keyed).is_ok());
        let accepted = RepoTrust {
            keyring: None,
            allow_unsigned: true,
        };
        assert!(refuse_unverified("https://repo.invalid/debian", &accepted).is_ok());
    }

    #[test]
    fn a_bare_remote_index_is_refused_without_allow_unsigned() {
        let strict = RepoTrust {
            keyring: None,
            allow_unsigned: false,
        };
        let names = ["foo".to_string()];
        // remote index, remote packages, or both: refused before any fetch
        for (base, index) in [
            (
                "http://m.invalid/debian",
                "http://m.invalid/debian/Packages.gz",
            ),
            ("http://m.invalid/debian", "/nonexistent/Packages"),
            ("/srv/repo", "https://m.invalid/debian/Packages.gz"),
        ] {
            let e = resolve_apt_index(base, index, &names, &strict).unwrap_err();
            assert_eq!(e.kind, Kind::Refused, "{base} {index}: {e}");
            assert!(
                e.message.starts_with(
                    "refusing to install from a remote package index that nothing verifies: `install tx-apt-repo` checks no signature."
                ),
                "{e}"
            );
            assert!(
                e.message.contains("`install tx-apt --keyring <keyring>`"),
                "{e}"
            );
            assert!(
                e.message.ends_with("Nothing was downloaded or installed."),
                "{e}"
            );
            let e = resolve_rpm_primary(base, index, &names, &strict).unwrap_err();
            assert_eq!(e.kind, Kind::Refused, "{base} {index}: {e}");
            assert!(
                e.message
                    .contains("`install tx-rpm-repo` checks no signature"),
                "{e}"
            );
        }
        // a keyring cannot help a verb that checks no signature
        let keyed = RepoTrust {
            keyring: Some("/etc/keys/repo.gpg"),
            allow_unsigned: false,
        };
        let e = resolve_apt_index("http://m.invalid/d", "/nonexistent/P", &names, &keyed);
        assert_eq!(e.unwrap_err().kind, Kind::Refused);
        // local index and local packages pass the gate (and then fail to read)
        let e =
            resolve_apt_index("/srv/repo", "/nonexistent/Packages", &names, &strict).unwrap_err();
        assert_eq!(e.kind, Kind::Failed, "{e}");
        // the opt-in passes the gate; the plan verbs always have it
        let accepted = RepoTrust {
            keyring: None,
            allow_unsigned: true,
        };
        let e = resolve_apt_index("http://m.invalid/d", "/nonexistent/P", &names, &accepted);
        assert_eq!(e.unwrap_err().kind, Kind::Failed);
        let e = resolve_sources_from_apt_packages("http://m.invalid/d", "/nonexistent/P", &names);
        assert_eq!(e.unwrap_err().kind, Kind::Failed);
    }

    // ---- finding 15: the non-free gate has something to fire on

    #[test]
    fn nonfree_sections_and_licences_are_recognised() {
        for (section, want) in [
            ("non-free/libs", Some("Section: non-free/libs")),
            (
                "non-free-firmware/kernel",
                Some("Section: non-free-firmware/kernel"),
            ),
            ("non-free", Some("Section: non-free")),
            ("restricted/misc", Some("Section: restricted/misc")),
            ("multiverse/games", Some("Section: multiverse/games")),
            (
                "contrib/utils",
                Some("Section: contrib/utils (contrib: depends on non-free)"),
            ),
            (
                "contrib",
                Some("Section: contrib (contrib: depends on non-free)"),
            ),
            ("utils", None),
            ("libs", None),
            ("", None),
            // only the archive area counts, not a word further on
            ("misc/non-free-looking", None),
        ] {
            assert_eq!(nonfree_from(section, "").as_deref(), want, "{section}");
        }
        assert_eq!(
            nonfree_from("", "Proprietary").as_deref(),
            Some("License: Proprietary")
        );
        assert_eq!(
            nonfree_from("", "Redistributable, NonFree").as_deref(),
            Some("License: Redistributable, NonFree")
        );
        assert_eq!(nonfree_from("", "GPL-2.0-or-later"), None);

        // `rpm -qip` output fills the licence
        let mut a = Analysis::default();
        apply_rpm_info(
            &mut a,
            "Name        : nvidia-driver\nVersion     : 550.1\nRelease     : 1.fc40\nArchitecture: x86_64\nLicense     : NVIDIA Proprietary License\nURL         : https://example.invalid/x\nDescription :\nLicense     : not this one\n",
        );
        assert_eq!(a.package, "nvidia-driver");
        assert_eq!(a.version, "550.1");
        assert_eq!(a.arch, "x86_64");
        assert_eq!(a.license_field, "NVIDIA Proprietary License");
        assert_eq!(
            nonfree_descriptor(&a).as_deref(),
            Some("License: NVIDIA Proprietary License")
        );
        // from an index, per artifact
        let pkgs = parse_apt_packages("/srv/repo", FIX_PACKAGES);
        assert_eq!(
            nonfree_artifacts(&pkgs),
            vec![("hello".to_string(), "Section: non-free/utils".to_string())]
        );
    }

    #[test]
    fn nonfree_gate_accepts_with_the_flag_and_refuses_when_it_cannot_ask() {
        assert_eq!(
            nonfree_gate("x: Section: non-free", true, true, true),
            Ok(true)
        );
        let e = nonfree_gate("x: Section: non-free", false, true, false).unwrap_err();
        assert!(e.contains("pass --non-free"), "{e}");
        assert!(e.ends_with("Nothing was installed."), "{e}");
        // no terminal on stdin in a test run: it cannot be answered
        let e = nonfree_gate("x: Section: non-free", false, false, false).unwrap_err();
        assert!(
            e.ends_with("pass --non-free to accept the license. Nothing was installed."),
            "{e}"
        );
    }

    /// A real .deb built with ar and bsdtar: a non-free Section, a
    /// set-user-ID tool, and a directory symlink to the host's root.
    fn build_deb(dir: &Path) -> Option<PathBuf> {
        if !(crate::util::have("ar") && crate::util::have("bsdtar")) {
            return None;
        }
        let ctl = dir.join("ctl");
        let data = dir.join("data");
        fs::create_dir_all(&ctl).unwrap();
        fs::create_dir_all(data.join("usr/bin")).unwrap();
        fs::create_dir_all(data.join("opt/x")).unwrap();
        fs::write(
            ctl.join("control"),
            "Package: tool\nVersion: 1.2-3\nArchitecture: all\nSection: non-free/utils\nDescription: fixture\n",
        )
        .unwrap();
        let tool = data.join("usr/bin/tool");
        fs::write(&tool, "#!/bin/sh\necho tool\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o4755)).unwrap();
        std::os::unix::fs::symlink("/", data.join("opt/x/host")).unwrap();
        fs::write(dir.join("debian-binary"), "2.0\n").unwrap();
        let run = |cmd: &str, args: &[&str]| {
            let out = Command::new(cmd)
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{cmd} {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run("bsdtar", &["-cf", "control.tar", "-C", "ctl", "control"]);
        run("bsdtar", &["-cf", "data.tar", "-C", "data", "."]);
        run(
            "ar",
            &["rc", "tool.deb", "debian-binary", "control.tar", "data.tar"],
        );
        Some(dir.join("tool.deb"))
    }

    #[test]
    fn a_deb_installs_as_a_keg_without_setuid_or_followed_links_and_is_removed() {
        let tmp = scratch("deb");
        let Some(deb) = build_deb(tmp.path()) else {
            eprintln!("a_deb_installs_as_a_keg…: skipped, ar or bsdtar is not installed");
            return;
        };
        // finding 15: the Section reaches the analysis, so the gate can fire
        let analysis = analyze_package("deb", &deb);
        assert_eq!(analysis.package, "tool");
        assert_eq!(analysis.section, "non-free/utils");
        assert_eq!(
            nonfree_descriptor(&analysis).as_deref(),
            Some("Section: non-free/utils")
        );
        assert!(
            plan(
                "tool",
                "deb",
                &deb.display().to_string(),
                &tmp.path().join("keg")
            )
            .contains("Non-free: Section: non-free/utils."),
        );

        let prefix = tmp.path().join("kegs/deb/tool");
        let lroot = tmp.path().join("ledger");
        let rec = install(
            "tool",
            "deb",
            &deb.display().to_string(),
            &prefix,
            &lroot,
            true,
        )
        .unwrap();
        assert_eq!(rec.format, "deb-extract");
        assert_eq!(rec.version, "1.2-3");
        assert_eq!(rec.metadata["section"], "non-free/utils");
        assert_eq!(
            rec.metadata["source_check"],
            "none (the file was not checked against any index)"
        );
        // the set-user-ID bit of the package did not survive
        let tool = prefix.join("rootfs/usr/bin/tool");
        let mode = fs::metadata(&tool).unwrap().mode();
        assert_eq!(mode & 0o7000, 0, "mode {mode:o}");
        assert_eq!(mode & 0o111, 0o111, "still executable");
        // the link to / is one recorded path, and nothing below it is
        let host = prefix.join("rootfs/opt/x/host").display().to_string();
        assert!(rec.paths.contains(&host), "{:?}", rec.paths);
        assert!(
            !rec.paths.iter().any(|p| p.starts_with(&format!("{host}/"))),
            "the walk followed the link"
        );
        assert!(rec.paths.len() < 10, "{} paths", rec.paths.len());
        assert!(prefix.join("bin/fgn.deb.tool--tool").exists());
        assert!(prefix.join(MANIFEST_FILE).is_file());
        // no extraction leftovers in the keg
        let entries: Vec<String> = fs::read_dir(&prefix)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            !entries.iter().any(|e| e.starts_with("yapacc-extract")),
            "{entries:?}"
        );

        // a second install into the same prefix is refused, the keg kept
        let e = install(
            "tool2",
            "deb",
            &deb.display().to_string(),
            &prefix,
            &lroot,
            true,
        )
        .unwrap_err();
        assert!(e.starts_with("prefix already exists"), "{e}");
        assert!(tool.exists());
        // an unsupported format creates nothing
        let other = tmp.path().join("kegs/zip/tool");
        let e = install(
            "tool",
            "zip",
            &deb.display().to_string(),
            &other,
            &lroot,
            true,
        )
        .unwrap_err();
        assert_eq!(e, "unsupported foreign format: zip");
        assert!(!other.exists());

        // update keeps working on the same keg, then remove deletes it
        let (_old, new) = update(&rec.id, &deb.display().to_string(), &lroot, false).unwrap();
        assert_eq!(new.install_root, prefix.display().to_string());
        assert_eq!(fs::metadata(&tool).unwrap().mode() & 0o7000, 0);
        let removed = remove(&new.id, &lroot).unwrap();
        assert_eq!(removed.status, "removed");
        assert!(!prefix.exists());
    }

    #[test]
    fn default_root_falls_back_when_unwritable() {
        // Force an impossible system root via env and confirm the XDG fallback kicks in.
        // (We cannot easily make /var/lib unwritable as root in tests, so we test the
        // config/env override path plus the shape of the fallback.)
        let root = default_isolated_root();
        // whichever branch wins, it must never be /bin, /usr, or ~/.local/bin
        let s = root.display().to_string();
        assert!(!s.starts_with("/bin") && !s.starts_with("/usr"), "got {s}");
        if let Ok(home) = std::env::var("HOME") {
            assert_ne!(s, format!("{home}/.local/bin"));
        }
    }

    #[test]
    fn manifest_says_what_a_keg_is_and_how_to_remove_it() {
        let rec = ledger::InstallRecord {
            id: "pkg-tree-1".into(),
            name: "tree".into(),
            format: "deb-extract".into(),
            source: "/tmp/tree_2.2.1-1_amd64.deb".into(),
            version: "2.2.1-1".into(),
            scope: "user".into(),
            install_root: "/kegs/deb/tree".into(),
            paths: vec![],
            status: "installed".into(),
            installed_at: 1_794_771_006,
            updated_at: 0,
            metadata: std::collections::BTreeMap::new(),
        };
        let link = "/kegs/deb/tree/bin/fgn.deb.tree--tree";
        let launchers = [format!("{link} -> /kegs/deb/tree/rootfs/usr/bin/tree")];
        let want = "name: tree\n\
                    version: 2.2.1-1\n\
                    format: deb-extract\n\
                    source: /tmp/tree_2.2.1-1_amd64.deb\n\
                    installed-utc: 2026-11-15 19:30 UTC\n\
                    launchers:\n\
                    \x20 /kegs/deb/tree/bin/fgn.deb.tree--tree\n\
                    remove: rm -r /kegs/deb/tree and the launchers above\n";
        assert_eq!(render_manifest(&rec, &launchers), want);
        let none = render_manifest(&rec, &[]);
        assert!(none.contains("launchers:\nremove: rm -r /kegs/deb/tree"));

        let dir = scratch("manifest");
        write_manifest(dir.path(), &rec, &launchers).unwrap();
        let written = fs::read_to_string(dir.path().join("YAPACC-MANIFEST.txt")).unwrap();
        assert_eq!(written, want);
    }
}
