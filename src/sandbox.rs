// yapacc — build sandbox (bubblewrap)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Confinement for anything that executes third-party build code
//! (`makepkg` sources the PKGBUILD as bash, so even "just printing
//! .SRCINFO" runs code).
//!
//! What a sandboxed process gets:
//!
//! * the host filesystem **read-only**, with these replaced by an empty
//!   tmpfs: `/home`, `/root`, the invoking user's `$HOME`, `/mnt`, `/media`,
//!   `/tmp`, `/var/tmp` and the whole of `/run` (session and system D-Bus
//!   sockets, ssh-agent and gpg-agent sockets, removable disks under
//!   `/run/media`, docker/podman sockets);
//! * while the network is shared, the one file under `/run` that
//!   `/etc/resolv.conf` links to (systemd-resolved, NetworkManager,
//!   resolvconf), read-only, so that names still resolve;
//! * exactly one writable directory of the host — the build directory;
//! * a throwaway `$HOME` ([`SANDBOX_HOME_PATH`]) that lives OUTSIDE the build
//!   directory, in a fresh private directory that is removed afterwards;
//! * a cleared environment (no `SSH_AUTH_SOCK`, no tokens in variables);
//! * its own pid/ipc/uts/cgroup/user namespaces, dies with the parent, no
//!   controlling terminal;
//! * network only when the caller asks for it (`Net::Shared`).
//!
//! What it does NOT protect against: kernel or bubblewrap escapes, reading
//! world-readable files anywhere else (`/etc`, `/usr`, `/var`, `/opt`,
//! `/srv`, other mounted filesystems), services listening on localhost or
//! on abstract unix sockets while the network is shared, exfiltrating the
//! build directory's own contents while the network is shared, or resource
//! exhaustion. See docs/SECURITY.md.

use crate::error::{Error, Result};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

/// What to do when the sandbox cannot be set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Refuse to run the build (default).
    Require,
    /// Warn loudly and run unconfined.
    Prefer,
    /// Never sandbox.
    Off,
}

impl Policy {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "require" => Ok(Policy::Require),
            "prefer" => Ok(Policy::Prefer),
            "off" => Ok(Policy::Off),
            other => Err(format!(
                "invalid sandbox policy '{other}' (expected: require | prefer | off)"
            )),
        }
    }

    /// Policy from `[build] sandbox`; an unparsable value falls back to the
    /// strict default rather than to "off".
    pub fn from_config() -> Self {
        Self::parse(&crate::config::get().build.sandbox).unwrap_or(Policy::Require)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    Shared,
    Isolated,
}

/// Name of the throwaway home (the last component of [`SANDBOX_HOME_PATH`]).
pub const SANDBOX_HOME: &str = ".yapacc-home";

/// Where the throwaway home is mounted inside the sandbox. `/tmp` is a
/// tmpfs there, so the mount point is created in the sandbox only: nothing
/// in the build directory can stand in for it.
pub const SANDBOX_HOME_PATH: &str = "/tmp/.yapacc-home";

const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/bin/site_perl:/usr/bin/vendor_perl:/usr/bin/core_perl";

/// Directories that are replaced by an empty tmpfs: personal data, and the
/// whole of `/run` (sockets of the session, of the system bus and of
/// container engines; removable disks under `/run/media`).
fn mask_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/home", "/root", "/run", "/mnt", "/media"]
        .iter()
        .map(PathBuf::from)
        .collect();
    // `/var/run` is a link to `/run` almost everywhere; where it is a
    // directory of its own it holds the same kind of sockets
    if std::fs::symlink_metadata("/var/run").is_ok_and(|m| m.is_dir()) {
        v.push(PathBuf::from("/var/run"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let home = PathBuf::from(home);
        if !v.iter().any(|m| home.starts_with(m)) {
            v.push(home);
        }
    }
    v
}

/// Lexical clean-up of an absolute path (`.` and `..` removed without
/// touching the filesystem).
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(x) => out.push(x),
            _ => {}
        }
    }
    out
}

/// What name resolution needs from under `run` once that directory is
/// masked: `(source on the host, path inside the sandbox)` pairs.
///
/// `resolv_conf` is followed link by link. On hosts where it ends in a file
/// under `run` (`/run/systemd/resolve/stub-resolv.conf`,
/// `/run/NetworkManager/resolv.conf`, `/run/resolvconf/resolv.conf`) that
/// one file is bound back at every path of the chain that lies under `run`.
/// A plain `/etc/resolv.conf` needs nothing. Sockets (nscd, the
/// systemd-resolved varlink socket) are deliberately not bound: glibc falls
/// back to the `dns` source, which reads the file.
fn resolver_binds(resolv_conf: &Path, run: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Ok(real) = std::fs::canonicalize(resolv_conf) else {
        return Vec::new();
    };
    if !real.is_file() {
        return Vec::new();
    }
    let mut dests: Vec<PathBuf> = Vec::new();
    let mut at = resolv_conf.to_path_buf();
    for _ in 0..8 {
        let Ok(target) = std::fs::read_link(&at) else {
            break;
        };
        let next = match at.parent() {
            Some(parent) if target.is_relative() => parent.join(target),
            _ => target,
        };
        at = lexical(&next);
        if at.starts_with(run) && at != run && !dests.contains(&at) {
            dests.push(at.clone());
        }
    }
    dests.into_iter().map(|d| (real.clone(), d)).collect()
}

/// Full `bwrap` argument vector (without the program to run). `home` is the
/// host directory that becomes `$HOME` ([`SANDBOX_HOME_PATH`]) inside.
pub fn bwrap_args(dir: &Path, home: &Path, net: Net) -> Vec<String> {
    bwrap_args_with(dir, home, net, Path::new("/etc/resolv.conf"))
}

fn bwrap_args_with(dir: &Path, home: &Path, net: Net, resolv_conf: &Path) -> Vec<String> {
    let d = dir.display().to_string();
    let mut a: Vec<String> = vec!["--unshare-all".into()];
    if net == Net::Shared {
        a.push("--share-net".into());
    }
    for flag in ["--die-with-parent", "--new-session", "--clearenv"] {
        a.push(flag.into());
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "builder".into());
    let term = std::env::var("TERM").unwrap_or_else(|_| "dumb".into());
    for (k, v) in [
        ("PATH", SANDBOX_PATH),
        ("HOME", SANDBOX_HOME_PATH),
        ("USER", user.as_str()),
        ("LOGNAME", user.as_str()),
        ("TERM", term.as_str()),
        // stable, parseable tool output (the failure classifier matches English)
        ("LC_ALL", "C.UTF-8"),
        // keep every makepkg output inside the one writable directory, even
        // when /etc/makepkg.conf points these elsewhere
        ("BUILDDIR", d.as_str()),
        ("PKGDEST", d.as_str()),
        ("SRCDEST", d.as_str()),
        ("SRCPKGDEST", d.as_str()),
        ("LOGDEST", d.as_str()),
    ] {
        a.extend(["--setenv".into(), k.into(), v.into()]);
    }
    a.extend(["--ro-bind".into(), "/".into(), "/".into()]);
    a.extend(["--dev".into(), "/dev".into()]);
    a.extend(["--proc".into(), "/proc".into()]);
    a.extend(["--tmpfs".into(), "/tmp".into()]);
    if Path::new("/var/tmp").is_dir() {
        a.extend(["--tmpfs".into(), "/var/tmp".into()]);
    }
    for m in mask_candidates() {
        if m.is_dir() {
            a.extend(["--tmpfs".into(), m.display().to_string()]);
        }
    }
    // after the masks: the resolver file reappears in the emptied /run, and
    // only while there is a network to resolve names on
    if net == Net::Shared {
        for (src, dest) in resolver_binds(resolv_conf, Path::new("/run")) {
            a.extend([
                "--ro-bind".into(),
                src.display().to_string(),
                dest.display().to_string(),
            ]);
        }
    }
    // the throwaway home: a host directory outside the build directory,
    // mounted on a path that exists only inside the sandbox
    a.extend([
        "--bind".into(),
        home.display().to_string(),
        SANDBOX_HOME_PATH.into(),
    ]);
    // last, so the build directory reappears even under $HOME, /run or /tmp
    a.extend(["--bind".into(), d.clone(), d.clone()]);
    a.extend(["--chdir".into(), d]);
    a
}

fn probe_uncached(run_as: Option<&str>) -> std::result::Result<(), String> {
    if !crate::util::have("bwrap") {
        return Err("bubblewrap (bwrap) is not installed".into());
    }
    let probe = [
        "bwrap",
        "--unshare-all",
        "--die-with-parent",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "true",
    ];
    let mut c = match run_as {
        Some(user) => {
            let mut c = Command::new("runuser");
            c.args(["-u", user, "--"]).args(probe);
            c
        }
        None => {
            let mut c = Command::new(probe[0]);
            c.args(&probe[1..]);
            c
        }
    };
    match c.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "bwrap cannot create a sandbox here ({}); unprivileged user namespaces may be disabled",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("bwrap failed to start: {e}")),
    }
}

/// Can a sandbox actually be created (not merely "is bwrap on PATH")?
pub fn probe(run_as: Option<&str>) -> std::result::Result<(), String> {
    static CACHE: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match run_as {
        None => CACHE.get_or_init(|| probe_uncached(None)).clone(),
        Some(u) => probe_uncached(Some(u)),
    }
}

/// 128 random bits as hex, straight from the kernel.
fn random_hex() -> io::Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// `mkdtemp`: a new directory `base/<prefix><128 random bits>` with mode
/// 0700. The create is not recursive and an existing name is an error, so
/// the result is always a directory this process made, never one (or a
/// link) somebody else put there first.
pub(crate) fn fresh_private_dir(base: &Path, prefix: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let path = base.join(format!("{prefix}{}", random_hex()?));
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    Ok(path)
}

/// Delete a directory tree without following links. Build tools leave
/// read-only directories behind (the Go module cache), so on failure the
/// directories are made writable and the removal is tried once more.
pub(crate) fn remove_tree(path: &Path) {
    fn make_writable(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
            return;
        }
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                make_writable(&e.path());
            }
        }
    }
    if std::fs::remove_dir_all(path).is_err() {
        make_writable(path);
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Is `dir` a real directory (not a link) owned by `uid` that nobody else
/// can write to?
fn private_to(dir: &Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o022 == 0)
}

const HOME_PREFIX: &str = "home-";

/// The throwaway `$HOME` of one sandboxed build.
///
/// It is a fresh mode-0700 directory with a random name, created outside
/// the build directory: the build directory is untrusted (a cloned
/// repository can carry symbolic links), and nothing yapacc writes on the
/// host before bubblewrap starts may go through it. The directory is held
/// under an exclusive lock while in use and removed on drop.
#[derive(Debug)]
pub struct Home {
    path: PathBuf,
    /// open handle on the directory, exclusively locked: tells a later run
    /// that this home is in use (see [`sweep_stale`])
    _lock: Option<std::fs::File>,
}

impl Home {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A new home for a build that runs as the invoking user (`run_as` is
    /// `None`) or, started by root, as the build user `run_as`.
    ///
    /// * invoking user: under the yapacc cache directory
    ///   (`$XDG_CACHE_HOME/yapacc` or `~/.cache/yapacc`), else under
    ///   `$XDG_RUNTIME_DIR/yapacc`; seeded with that user's PUBLIC gpg
    ///   keyring so `validpgpkeys` source checks still work. No secret
    ///   material is copied.
    /// * root with a build user: under `/var/cache/yapacc`, handed to the
    ///   build user, and NOT seeded (root's keyring is not the build
    ///   user's, and root writes nothing into a directory another user
    ///   controls).
    pub fn create(run_as: Option<&str>) -> Result<Home> {
        let refuse = |why: String| {
            Error::unavailable(format!(
                "cannot create the build sandbox's home directory: {why}. The build was not started."
            ))
        };
        match run_as {
            Some(user) => {
                let base = system_base().map_err(|e| refuse(e.to_string()))?;
                let home = Home::create_in(&base, None)
                    .map_err(|e| refuse(format!("{}: {e}", base.display())))?;
                let handed = Command::new("chown")
                    .args(["-h", "--", user])
                    .arg(&home.path)
                    .status();
                if !handed.is_ok_and(|s| s.success()) {
                    return Err(refuse(format!(
                        "{} could not be handed to the build user '{user}'",
                        home.path.display()
                    )));
                }
                Ok(home)
            }
            None => {
                let keyring = std::env::var_os("GNUPGHOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .filter(|v| !v.is_empty())
                            .map(|h| PathBuf::from(h).join(".gnupg"))
                    });
                let mut last = String::from(
                    "neither a cache directory (XDG_CACHE_HOME, HOME) nor XDG_RUNTIME_DIR is set",
                );
                for base in user_bases() {
                    match Home::create_in(&base, keyring.as_deref()) {
                        Ok(home) => return Ok(home),
                        Err(e) => last = format!("{}: {e}", base.display()),
                    }
                }
                Err(refuse(last))
            }
        }
    }

    /// Create a home under `base` and seed it from `keyring` (a GnuPG home
    /// directory), when one is given.
    fn create_in(base: &Path, keyring: Option<&Path>) -> io::Result<Home> {
        use std::os::unix::fs::MetadataExt;
        sweep_stale(base);
        let path = fresh_private_dir(base, HOME_PREFIX)?;
        // The new directory is ours. It is only safe to use if nobody else
        // can rename it away and put something else in its place: the
        // directory it is in, and the one above, must be ours alone too.
        let me = std::fs::symlink_metadata(&path)?.uid();
        let safe = private_to(base, me) && base.parent().is_some_and(|p| private_to(p, me));
        if !safe {
            let _ = std::fs::remove_dir(&path);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "not inside directories that only this user can write to",
            ));
        }
        let lock = std::fs::File::open(&path).ok().filter(|f| f.lock().is_ok());
        let home = Home { path, _lock: lock };
        if let Some(keyring) = keyring
            && let Err(e) = seed_public_keyring(&home.path, keyring)
        {
            eprintln!(
                "note: the public gpg keyring was not copied into the sandbox home ({e}); signature checks of sources may fail."
            );
        }
        Ok(home)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        remove_tree(&self.path);
    }
}

/// Copy the PUBLIC keyring files of `keyring` into `<home>/.gnupg`.
///
/// `home` was created by this process a moment ago inside directories only
/// this user can write to. Even so, nothing here follows a link or reuses
/// an existing entry: the directory is created non-recursively and every
/// file with `O_CREAT|O_EXCL` (`create_new`), which fails on anything that
/// already exists, a symbolic link included.
fn seed_public_keyring(home: &Path, keyring: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let gnupg = home.join(".gnupg");
    std::fs::DirBuilder::new().mode(0o700).create(&gnupg)?;
    for public in ["pubring.kbx", "pubring.gpg", "trustdb.gpg"] {
        let Ok(mut from) = std::fs::File::open(keyring.join(public)) else {
            continue;
        };
        if !from.metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        let mut to = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(gnupg.join(public))?;
        io::copy(&mut from, &mut to)?;
    }
    Ok(())
}

/// Remove homes left behind by a run that was killed before it could clean
/// up (Ctrl-C, power loss). A home in use is locked by its owner; one that
/// can be locked here, and is older than a minute, has no owner any more.
fn sweep_stale(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with(HOME_PREFIX)
            || !e.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let path = e.path();
        let old = std::fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 60);
        if !old {
            continue;
        }
        let unowned = std::fs::File::open(&path).is_ok_and(|f| f.try_lock().is_ok());
        if unowned {
            remove_tree(&path);
        }
    }
}

/// Where the invoking user's sandbox homes go, best first. Disk-backed
/// before `$XDG_RUNTIME_DIR`: builds download whole dependency trees into
/// `$HOME` (cargo, go, npm) and the runtime directory is a small tmpfs.
fn user_bases() -> Vec<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let absolute = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let mut bases = Vec::new();
    if let Some(cache) =
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|h| h.join(".cache")))
    {
        bases.push(cache.join("yapacc").join("sandbox-homes"));
    }
    if let Some(runtime) = absolute("XDG_RUNTIME_DIR") {
        bases.push(runtime.join("yapacc").join("sandbox-homes"));
    }
    for base in &bases {
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(base);
    }
    bases
}

/// Where root keeps the homes of build users: root-owned directories the
/// build user can pass through but not change.
fn system_base() -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = PathBuf::from("/var/cache/yapacc/sandbox-homes");
    for dir in [base.parent().unwrap_or(&base), base.as_path()] {
        match std::fs::DirBuilder::new().mode(0o755).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", dir.display()))),
        }
    }
    Ok(base)
}

/// Outcome of [`command`]: the command to run and whether it is confined.
pub struct Prepared {
    pub command: Command,
    pub sandboxed: bool,
    /// The throwaway home of a confined command. It is deleted when the
    /// last holder drops it; clone the `Arc` to give a later phase of the
    /// same build the same home ([`command_with_home`]).
    pub home: Option<Arc<Home>>,
}

/// `dir` made absolute against the current directory (no symlink resolution,
/// the directory need not exist yet).
fn absolute_dir(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// Build the command that runs `argv` in `dir` under `policy`.
///
/// `run_as` drops from root to an unprivileged build user first (`runuser`);
/// the sandbox then applies to that user as well.
///
/// Keep the returned [`Prepared`] alive until the command has finished: its
/// home is removed when it is dropped.
pub fn command(
    dir: &Path,
    net: Net,
    policy: Policy,
    run_as: Option<&str>,
    argv: &[&str],
) -> Result<Prepared> {
    command_with_home(dir, net, policy, run_as, None, argv)
}

/// [`command`], reusing the home of an earlier phase of the same build
/// (what `prepare()` downloaded into `$HOME` must still be there for
/// `build()`). With `home` = `None` a new home is created.
pub fn command_with_home(
    dir: &Path,
    net: Net,
    policy: Policy,
    run_as: Option<&str>,
    home: Option<Arc<Home>>,
    argv: &[&str],
) -> Result<Prepared> {
    // The child starts with `dir` as its working directory, so a relative
    // `dir` (the AUR flow's `./NAME-aur-build`) must not reach bwrap as is:
    // `--bind ./x ./x` is resolved from inside `x` and fails.
    let dir = absolute_dir(dir);
    let dir = dir.as_path();
    let confined = match policy {
        Policy::Off => false,
        Policy::Require | Policy::Prefer => match probe(run_as) {
            Ok(()) => true,
            Err(why) if policy == Policy::Require => {
                return Err(Error::unavailable(format!(
                    "build sandbox unavailable: {why}.\nBuilds execute third-party code, so yapacc refuses to run them unconfined by default.\nInstall bubblewrap, or accept the risk explicitly with --no-sandbox (or [build] sandbox = \"prefer\" | \"off\")."
                )));
            }
            Err(why) => {
                eprintln!(
                    "WARNING: build sandbox unavailable ({why}); running UNCONFINED ([build] sandbox = \"prefer\")."
                );
                false
            }
        },
    };
    let mut full: Vec<String> = Vec::new();
    if let Some(user) = run_as {
        full.extend(["runuser".into(), "-u".into(), user.into(), "--".into()]);
    }
    let home = match (confined, home) {
        (false, _) => None,
        (true, Some(shared)) => Some(shared),
        (true, None) => Some(Arc::new(Home::create(run_as)?)),
    };
    if let Some(home) = &home {
        full.push("bwrap".into());
        full.extend(bwrap_args(dir, home.path(), net));
        full.push("--".into());
    }
    full.extend(argv.iter().map(|s| s.to_string()));
    let mut c = Command::new(&full[0]);
    c.args(&full[1..]).current_dir(dir);
    Ok(Prepared {
        command: c,
        sandboxed: confined,
        home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "yapacc-sandbox-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A base for homes whose two last levels belong to this user alone.
    fn home_base(root: &Path) -> PathBuf {
        let base = root.join("cache").join("sandbox-homes");
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().mode() & 0o7777
    }

    fn chmod(p: &Path, m: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    fn fake_keyring(root: &Path) -> PathBuf {
        let k = root.join("gnupg");
        std::fs::create_dir_all(k.join("private-keys-v1.d")).unwrap();
        std::fs::write(k.join("pubring.kbx"), "PUBLIC KEYRING").unwrap();
        std::fs::write(k.join("trustdb.gpg"), "TRUSTDB").unwrap();
        std::fs::write(k.join("secring.gpg"), "SECRET").unwrap();
        std::fs::write(k.join("private-keys-v1.d/abc.key"), "SECRET").unwrap();
        k
    }

    #[test]
    fn policy_parses_and_rejects_unknown() {
        assert_eq!(Policy::parse("require").unwrap(), Policy::Require);
        assert_eq!(Policy::parse("prefer").unwrap(), Policy::Prefer);
        assert_eq!(Policy::parse("off").unwrap(), Policy::Off);
        assert!(Policy::parse("true").is_err());
    }

    #[test]
    fn args_isolate_network_unless_shared() {
        let d = PathBuf::from("/tmp/x");
        let h = PathBuf::from("/var/empty/h");
        let iso = bwrap_args(&d, &h, Net::Isolated);
        let shared = bwrap_args(&d, &h, Net::Shared);
        assert!(iso.iter().any(|i| i == "--unshare-all"));
        assert!(!iso.iter().any(|i| i == "--share-net"));
        assert!(shared.iter().any(|i| i == "--share-net"));
        for a in [&iso, &shared] {
            assert!(a.iter().any(|i: &String| i == "--clearenv"));
            assert!(a.iter().any(|i: &String| i == "--die-with-parent"));
            assert!(a.iter().any(|i: &String| i == "--new-session"));
        }
    }

    #[test]
    fn build_dir_is_bound_after_every_mask_and_after_the_home() {
        let d = PathBuf::from("/home/someone/factory");
        let h = PathBuf::from("/var/empty/h");
        let a = bwrap_args(&d, &h, Net::Isolated);
        let last_tmpfs = a.iter().rposition(|x| x == "--tmpfs").unwrap();
        let binds: Vec<usize> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "--bind")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(binds.len(), 2, "the home and the build directory: {a:?}");
        assert!(binds[0] > last_tmpfs, "binds must come after the masks");
        assert_eq!(a[binds[0] + 1], "/var/empty/h");
        assert_eq!(a[binds[0] + 2], SANDBOX_HOME_PATH);
        assert_eq!(a[binds[1] + 1], "/home/someone/factory");
        assert_eq!(a[binds[1] + 2], "/home/someone/factory");
    }

    #[test]
    fn home_is_never_a_path_inside_the_build_directory() {
        let d = PathBuf::from("/home/someone/factory");
        let h = PathBuf::from("/var/empty/h");
        let a = bwrap_args(&d, &h, Net::Shared);
        let home_env = a.iter().position(|x| x == "HOME").unwrap();
        assert_eq!(a[home_env + 1], SANDBOX_HOME_PATH);
        assert!(SANDBOX_HOME_PATH.ends_with(SANDBOX_HOME));
        assert!(
            a.iter().all(|x| !x.starts_with("/home/someone/factory/")),
            "nothing may point below the build directory: {a:?}"
        );
    }

    #[test]
    fn run_is_masked_as_a_whole() {
        if !Path::new("/run").is_dir() {
            return;
        }
        let a = bwrap_args(
            Path::new("/tmp/x"),
            Path::new("/var/empty/h"),
            Net::Isolated,
        );
        let masked: Vec<&String> = a
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 && a[*i - 1] == "--tmpfs")
            .map(|(_, x)| x)
            .collect();
        assert!(masked.iter().any(|m| *m == "/run"), "{masked:?}");
        // without network nothing from /run comes back
        assert!(
            !a.iter()
                .enumerate()
                .any(|(i, x)| x == "--ro-bind" && a[i + 2].starts_with("/run")),
            "{a:?}"
        );
    }

    #[test]
    fn resolver_binds_follow_resolv_conf_into_run() {
        let root = scratch("resolv");
        let run = root.join("run");
        let etc = root.join("etc");
        std::fs::create_dir_all(run.join("systemd/resolve")).unwrap();
        std::fs::create_dir_all(run.join("NetworkManager")).unwrap();
        std::fs::create_dir_all(&etc).unwrap();
        let stub = run.join("systemd/resolve/stub-resolv.conf");
        std::fs::write(&stub, "nameserver 127.0.0.53\n").unwrap();
        let nm = run.join("NetworkManager/resolv.conf");
        std::fs::write(&nm, "nameserver 192.0.2.1\n").unwrap();
        let real_stub = std::fs::canonicalize(&stub).unwrap();
        let real_nm = std::fs::canonicalize(&nm).unwrap();

        // a plain file needs nothing
        let plain = etc.join("plain.conf");
        std::fs::write(&plain, "nameserver 192.0.2.1\n").unwrap();
        assert!(resolver_binds(&plain, &run).is_empty());

        // systemd-resolved: a relative link, as shipped
        let resolved = etc.join("resolved.conf");
        symlink("../run/systemd/resolve/stub-resolv.conf", &resolved).unwrap();
        assert_eq!(
            resolver_binds(&resolved, &run),
            vec![(real_stub.clone(), stub.clone())]
        );

        // NetworkManager (rc-manager=symlink): an absolute link
        let nmlink = etc.join("nm.conf");
        symlink(&nm, &nmlink).unwrap();
        assert_eq!(
            resolver_binds(&nmlink, &run),
            vec![(real_nm.clone(), nm.clone())]
        );

        // a chain inside /run: the file is bound at every hop under /run
        let hop = run.join("resolvconf.link");
        symlink(&nm, &hop).unwrap();
        let chained = etc.join("chained.conf");
        symlink(&hop, &chained).unwrap();
        assert_eq!(
            resolver_binds(&chained, &run),
            vec![
                (real_nm.clone(), hop.clone()),
                (real_nm.clone(), nm.clone())
            ]
        );

        // a dangling link and a link to a directory bind nothing
        let dangling = etc.join("dangling.conf");
        symlink(run.join("nowhere"), &dangling).unwrap();
        assert!(resolver_binds(&dangling, &run).is_empty());
        let to_dir = etc.join("dir.conf");
        symlink(run.join("systemd"), &to_dir).unwrap();
        assert!(resolver_binds(&to_dir, &run).is_empty());

        // a link that stays outside /run binds nothing
        let outside = etc.join("outside.conf");
        symlink(&plain, &outside).unwrap();
        assert!(resolver_binds(&outside, &run).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_build_dir_reaches_bwrap_as_absolute_path() {
        // regression: `yapacc install aur` builds in `./NAME-aur-build`; bwrap
        // runs with that directory as cwd and could not find `./NAME-aur-build`
        let rel = Path::new("./some-aur-build");
        let abs = absolute_dir(rel);
        assert!(abs.is_absolute());
        assert!(abs.ends_with("some-aur-build"));
        let args = bwrap_args(&abs, Path::new("/var/empty/h"), Net::Isolated);
        assert!(
            args.iter().all(|a| !a.starts_with("./")),
            "relative path in {args:?}"
        );
        assert_eq!(absolute_dir(Path::new("/x/y")), PathBuf::from("/x/y"));
    }

    #[test]
    fn off_policy_never_wraps_and_creates_no_home() {
        let d = scratch("off");
        let p = command(&d, Net::Isolated, Policy::Off, None, &["true"]).unwrap();
        assert!(!p.sandboxed);
        assert!(p.home.is_none());
        assert_eq!(p.command.get_program(), "true");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fresh_private_dir_is_new_private_and_not_recursive() {
        let root = scratch("mkdtemp");
        let a = fresh_private_dir(&root, "x-").unwrap();
        let b = fresh_private_dir(&root, "x-").unwrap();
        assert_ne!(a, b);
        assert_eq!(mode(&a), 0o700);
        assert!(a.file_name().unwrap().to_string_lossy().len() >= 2 + 32);
        // the parent is never created on the way
        assert!(fresh_private_dir(&root.join("missing"), "x-").is_err());
        assert!(!root.join("missing").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_is_private_seeded_with_public_files_only_and_removed_on_drop() {
        let root = scratch("home");
        let base = home_base(&root);
        let keyring = fake_keyring(&root);
        let home = Home::create_in(&base, Some(&keyring)).unwrap();
        let path = home.path().to_path_buf();
        assert_eq!(path.parent().unwrap(), base);
        assert_eq!(mode(&path), 0o700);
        assert_eq!(mode(&path.join(".gnupg")), 0o700);
        assert_eq!(mode(&path.join(".gnupg/pubring.kbx")), 0o600);
        assert_eq!(
            std::fs::read_to_string(path.join(".gnupg/pubring.kbx")).unwrap(),
            "PUBLIC KEYRING"
        );
        assert_eq!(
            std::fs::read_to_string(path.join(".gnupg/trustdb.gpg")).unwrap(),
            "TRUSTDB"
        );
        assert!(!path.join(".gnupg/secring.gpg").exists());
        assert!(!path.join(".gnupg/private-keys-v1.d").exists());
        drop(home);
        assert!(!path.exists(), "the home must be removed after use");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_without_a_keyring_is_empty() {
        let root = scratch("unseeded");
        let base = home_base(&root);
        let home = Home::create_in(&base, None).unwrap();
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_is_refused_where_others_can_write() {
        let root = scratch("shared-base");
        // the base itself is writable by others
        let base = home_base(&root);
        chmod(&base, 0o777);
        assert!(Home::create_in(&base, None).is_err());
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0, "no leftovers");
        // the directory above the base is writable by others
        chmod(&base, 0o700);
        chmod(base.parent().unwrap(), 0o777);
        assert!(Home::create_in(&base, None).is_err());
        // a base that is a link to a private directory is not accepted either
        chmod(base.parent().unwrap(), 0o700);
        let link = base.parent().unwrap().join("link");
        symlink(&base, &link).unwrap();
        assert!(Home::create_in(&link, None).is_err());
        assert!(Home::create_in(&base, None).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Finding 4 of the 2026-11 review. The build directory is untrusted;
    /// the old code created `<build dir>/.yapacc-home/.gnupg` with
    /// `create_dir_all`, chmod'ed it and copied the keyring into it, all
    /// through whatever links the directory carried.
    #[test]
    fn links_planted_in_the_build_directory_are_never_followed() {
        let root = scratch("planted");
        let keyring = fake_keyring(&root);
        let base = home_base(&root);

        let victim_file = root.join("victim-id_ed25519");
        std::fs::write(&victim_file, "PRIVATE KEY").unwrap();
        chmod(&victim_file, 0o640);
        let victim_dir = root.join("victim-dir");
        std::fs::create_dir(&victim_dir).unwrap();
        std::fs::write(victim_dir.join("keep.txt"), "keep").unwrap();
        chmod(&victim_dir, 0o755);

        // .yapacc-home/.gnupg/pubring.kbx -> <victim file>
        let file_case = root.join("build-file-link");
        std::fs::create_dir_all(file_case.join(SANDBOX_HOME).join(".gnupg")).unwrap();
        symlink(
            &victim_file,
            file_case.join(SANDBOX_HOME).join(".gnupg/pubring.kbx"),
        )
        .unwrap();
        // .yapacc-home/.gnupg -> <victim dir>
        let dir_case = root.join("build-dir-link");
        std::fs::create_dir_all(dir_case.join(SANDBOX_HOME)).unwrap();
        symlink(&victim_dir, dir_case.join(SANDBOX_HOME).join(".gnupg")).unwrap();
        // .yapacc-home -> <victim dir>
        let home_case = root.join("build-home-link");
        std::fs::create_dir_all(&home_case).unwrap();
        symlink(&victim_dir, home_case.join(SANDBOX_HOME)).unwrap();

        for dir in [&file_case, &dir_case, &home_case] {
            let home = Home::create_in(&base, Some(&keyring)).unwrap();
            assert!(!home.path().starts_with(dir));
            assert_eq!(
                std::fs::read_to_string(home.path().join(".gnupg/pubring.kbx")).unwrap(),
                "PUBLIC KEYRING",
                "the keyring is seeded in the home outside the build directory"
            );
            // and through a real sandbox, where one can be made
            if probe(None).is_ok() {
                let out = Command::new("bwrap")
                    .args(bwrap_args(dir, home.path(), Net::Isolated))
                    .args([
                        "--",
                        "bash",
                        "-c",
                        "cat \"$HOME/.gnupg/pubring.kbx\" && echo inside > \"$HOME/.gnupg/written\"",
                    ])
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                assert_eq!(String::from_utf8_lossy(&out.stdout), "PUBLIC KEYRING");
                assert!(home.path().join(".gnupg/written").is_file());
            }
            drop(home);

            assert_eq!(
                std::fs::read_to_string(&victim_file).unwrap(),
                "PRIVATE KEY",
                "victim file content changed ({})",
                dir.display()
            );
            assert_eq!(mode(&victim_file), 0o640, "victim file mode changed");
            assert_eq!(mode(&victim_dir), 0o755, "victim directory mode changed");
            let names: Vec<String> = std::fs::read_dir(&victim_dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            assert_eq!(names, vec!["keep.txt"], "victim directory gained entries");
        }
        // the planted links are left exactly as they were
        assert!(
            std::fs::symlink_metadata(file_case.join(SANDBOX_HOME).join(".gnupg/pubring.kbx"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(dir_case.join(SANDBOX_HOME).join(".gnupg"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The seeding itself must not follow a link either, wherever it runs.
    #[test]
    fn seeding_fails_on_existing_entries_instead_of_following_them() {
        let root = scratch("seed");
        let keyring = fake_keyring(&root);
        let victim_file = root.join("victim");
        std::fs::write(&victim_file, "PRIVATE KEY").unwrap();
        chmod(&victim_file, 0o640);
        let victim_dir = root.join("victim-dir");
        std::fs::create_dir(&victim_dir).unwrap();
        chmod(&victim_dir, 0o755);

        // <home>/.gnupg is a link to a directory
        let home_a = root.join("home-a");
        std::fs::create_dir(&home_a).unwrap();
        symlink(&victim_dir, home_a.join(".gnupg")).unwrap();
        assert!(seed_public_keyring(&home_a, &keyring).is_err());
        assert_eq!(mode(&victim_dir), 0o755);
        assert_eq!(std::fs::read_dir(&victim_dir).unwrap().count(), 0);

        // <home>/.gnupg exists as a directory
        let home_b = root.join("home-b");
        std::fs::create_dir_all(home_b.join(".gnupg")).unwrap();
        assert!(seed_public_keyring(&home_b, &keyring).is_err());

        // <home>/.gnupg/pubring.kbx is a link to a file
        let home_d = root.join("home-d");
        std::fs::create_dir(&home_d).unwrap();
        let planted = home_d.join(".gnupg");
        // (the directory cannot pre-exist for the real call, so the file
        // step is exercised directly: create_new must refuse the link)
        std::fs::create_dir(&planted).unwrap();
        symlink(&victim_file, planted.join("pubring.kbx")).unwrap();
        let refused = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(planted.join("pubring.kbx"))
        };
        assert_eq!(
            refused.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "O_CREAT|O_EXCL must not follow a symbolic link"
        );

        // an empty home is seeded with new 0600 files
        let keyring_c = root.join("gnupg-c");
        std::fs::create_dir(&keyring_c).unwrap();
        std::fs::write(keyring_c.join("pubring.kbx"), "PUBLIC").unwrap();
        let home_c = root.join("home-c");
        std::fs::create_dir(&home_c).unwrap();
        seed_public_keyring(&home_c, &keyring_c).unwrap();
        assert_eq!(mode(&home_c.join(".gnupg/pubring.kbx")), 0o600);

        assert_eq!(
            std::fs::read_to_string(&victim_file).unwrap(),
            "PRIVATE KEY"
        );
        assert_eq!(mode(&victim_file), 0o640);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_homes_are_swept_but_never_one_in_use_or_a_new_one() {
        let root = scratch("sweep");
        let base = home_base(&root);
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let age = |p: &Path| {
            std::fs::File::open(p)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        };
        // left behind by a killed run: old, nobody holds it; it contains a
        // read-only directory like the Go module cache
        let stale = base.join("home-stale");
        std::fs::create_dir_all(stale.join("go/pkg/mod")).unwrap();
        std::fs::write(stale.join("go/pkg/mod/file"), "x").unwrap();
        chmod(&stale.join("go/pkg/mod"), 0o555);
        chmod(&stale.join("go/pkg"), 0o555);
        age(&stale);
        // old but in use
        let busy = Home::create_in(&base, None).unwrap();
        age(busy.path());
        // not a home at all
        let other = base.join("something-else");
        std::fs::create_dir(&other).unwrap();
        age(&other);
        // a link named like a home is not touched, nor is its target
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        age(&target);
        symlink(&target, base.join("home-link")).unwrap();

        let fresh = Home::create_in(&base, None).unwrap();
        assert!(!stale.exists(), "the stale home must be removed");
        assert!(busy.path().is_dir(), "a locked home must be kept");
        assert!(fresh.path().is_dir());
        assert!(other.is_dir());
        assert!(target.is_dir());
        drop(busy);
        drop(fresh);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real confinement check. Self-skips where a sandbox cannot be created
    /// (containers without user namespaces, hosts without bubblewrap).
    #[test]
    fn sandbox_hides_home_run_and_network_but_allows_the_build_dir() {
        if probe(None).is_err() {
            eprintln!("skipped: no working bubblewrap on this host");
            return;
        }
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        // a canary in the real home that the sandbox must not be able to read
        let canary = home.join(format!(".yapacc-sandbox-canary-{}", std::process::id()));
        std::fs::write(&canary, "secret").unwrap();
        let d = scratch("real");
        let script = format!(
            "set -u\n\
             if cat '{canary}' >/dev/null 2>&1; then echo LEAK-HOME; fi\n\
             if [ -n \"${{SSH_AUTH_SOCK:-}}\" ]; then echo LEAK-ENV; fi\n\
             if touch /usr/yapacc-canary 2>/dev/null; then echo LEAK-WRITE; fi\n\
             if [ \"$(tail -n +3 /proc/net/dev | grep -vc '^ *lo:')\" != 0 ]; then echo LEAK-NET; fi\n\
             for p in /run/user /run/media /run/dbus /run/docker.sock /run/podman /run/containerd /var/run/docker.sock /var/run/dbus; do\n\
               if [ -e \"$p\" ]; then echo \"LEAK-RUN $p\"; fi\n\
             done\n\
             echo \"run-entries=$(ls -A /run | wc -l)\"\n\
             echo ok > result.txt && echo \"home=$HOME\" && echo kept > \"$HOME/in-home\"\n",
            canary = canary.display()
        );
        let mut p = command(
            &d,
            Net::Isolated,
            Policy::Require,
            None,
            &["bash", "-c", &script],
        )
        .unwrap();
        assert!(p.sandboxed);
        let host_home = p.home.as_ref().unwrap().path().to_path_buf();
        assert!(
            !host_home.starts_with(&d),
            "the home must not be in the build dir"
        );
        let out = p
            .command
            .env("SSH_AUTH_SOCK", "/run/user/0/fake-agent")
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&canary);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "sandboxed script failed: {stderr}");
        assert!(!stdout.contains("LEAK"), "sandbox leaked: {stdout}");
        assert!(
            stdout.contains("run-entries=0"),
            "/run must be empty without network: {stdout}"
        );
        assert!(
            stdout.contains(&format!("home={SANDBOX_HOME_PATH}")),
            "HOME not redirected: {stdout}"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("result.txt"))
                .unwrap()
                .trim(),
            "ok",
            "the build directory must stay writable"
        );
        assert!(
            !d.join(SANDBOX_HOME).exists(),
            "nothing is created in the build directory for the home"
        );
        assert!(host_home.join("in-home").is_file(), "$HOME is writable");
        drop(p);
        assert!(!host_home.exists(), "the home is removed after the command");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The mechanism that keeps name resolution working on hosts whose
    /// `/etc/resolv.conf` is a link into `/run`, exercised for real: a file
    /// under the real `/run` is reachable through a link outside it, and
    /// nothing else of `/run` is.
    #[test]
    fn resolver_file_under_run_is_bound_back_read_only_with_network_only() {
        if probe(None).is_err() {
            return;
        }
        let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.starts_with("/run") && p.is_dir())
        else {
            eprintln!("skipped: no writable directory under /run");
            return;
        };
        let root = scratch("resolv-real");
        let base = home_base(&root);
        let under_run = fresh_private_dir(&runtime, "yapacc-test-").unwrap();
        let file = under_run.join("stub-resolv.conf");
        std::fs::write(&file, "nameserver 127.0.0.53\n").unwrap();
        std::fs::write(under_run.join("other-secret"), "x").unwrap();
        let d = root.join("build");
        std::fs::create_dir(&d).unwrap();
        let link = d.join("resolv.conf");
        symlink(&file, &link).unwrap();
        let script = format!(
            "cat resolv.conf 2>/dev/null || echo UNREADABLE\n\
             echo x >> '{f}' 2>/dev/null && echo LEAK-WRITABLE\n\
             echo \"entries=$(find /run -mindepth 1 | wc -l)\"\n\
             find /run -mindepth 1 -not -type d\n",
            f = file.display()
        );
        let run = |net: Net| {
            let home = Home::create_in(&base, None).unwrap();
            let out = Command::new("bwrap")
                .args(bwrap_args_with(&d, home.path(), net, &link))
                .args(["--", "bash", "-c", &script])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let shared = run(Net::Shared);
        assert!(shared.contains("nameserver 127.0.0.53"), "{shared}");
        assert!(!shared.contains("LEAK"), "{shared}");
        // the only non-directory under /run is that one file
        assert!(
            shared.trim_end().ends_with(&file.display().to_string()),
            "{shared}"
        );
        assert!(!shared.contains("other-secret"), "{shared}");
        let isolated = run(Net::Isolated);
        assert!(isolated.contains("UNREADABLE"), "{isolated}");
        assert!(isolated.contains("entries=0"), "{isolated}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "nameserver 127.0.0.53\n"
        );
        let _ = std::fs::remove_dir_all(&under_run);
        let _ = std::fs::remove_dir_all(&root);
    }
}
