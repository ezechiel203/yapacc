// yapacc — release archives, built locally.
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! `cargo xtask dist` builds what a release publishes, into `target/dist/`.
//! `.github/workflows/release.yml` runs this same command, so there is one
//! implementation: the workflow only adds the checkout, the toolchain, the
//! attestation and the upload.
//!
//! Written to `target/dist/` (`<v>` is the version in `Cargo.toml`):
//!
//! ```text
//! yapacc-<v>-x86_64-linux-musl.tar.gz    static binaries, see the layout below
//! yapacc-<v>-aarch64-linux-musl.tar.gz   the same for aarch64 (when the target is installed)
//! yapacc-<v>.tar.gz                      the source: `git archive` of HEAD
//! yapacc-<v>-packaging.tar.gz            contrib/packaging, filled for this release
//! SHA256SUMS                             of the four files above
//! ```
//!
//! Layout of a binary archive (contrib/packaging/README.md, "layout contract"):
//!
//! ```text
//! yapacc-<v>-<arch>-linux-musl/
//!   bin/yapacc
//!   lib/yapacc/yapacc-tui
//!   lib/yapacc/yapacc-mcp
//!   share/man/man1/yapacc.1
//!   share/bash-completion/completions/yapacc
//!   share/zsh/site-functions/_yapacc
//!   share/fish/vendor_completions.d/yapacc.fish
//!   LICENSE  README.md
//! ```
//!
//! What makes two runs on the same commit give the same bytes:
//!
//! - `--locked`, and `rust-lld` (shipped with the toolchain) as the linker
//!   of both targets, so the host's C compiler and linker play no part;
//! - `--remap-path-prefix` for the home directory, cargo's home, the
//!   toolchain and the checkout, so no path of the build machine is
//!   compiled in; the binaries are then searched for those paths and the
//!   run fails when one is found;
//! - `tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@<commit
//!   time>` on a tree whose file modes were set here, and `gzip -n`.
//!
//! The toolchain is the one `cargo` resolves to; a different compiler gives
//! different binaries. The release workflow pins its version; to build with
//! the same one locally: `cargo +<version> xtask dist`.
//!
//! Needs: GNU tar, gzip, sha256sum, git, a rustup toolchain (for `rust-lld`)
//! with the `x86_64-unknown-linux-musl` target. The aarch64 archive is
//! built when `aarch64-unknown-linux-musl` is installed too, and its
//! binaries are run under `qemu-aarch64` when that exists.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The core binary must stay under this size (docs/STRATEGY.md).
pub const CORE_SIZE_LIMIT: u64 = 6 * 1024 * 1024;

const USAGE: &str = "usage: cargo xtask dist [--require-aarch64] [--allow-dirty] [--out DIR]

  --require-aarch64  fail, instead of skipping, when the aarch64 archive
                     cannot be built or its binaries cannot be run
  --allow-dirty      go on when tracked files differ from HEAD (the source
                     tarball is always HEAD; the binaries would not match it)
  --out DIR          write there instead of <target directory>/dist";

struct Target {
    /// Name used in the archive: `x86_64`, `aarch64`.
    arch: &'static str,
    triple: &'static str,
    /// `e_machine` of an ELF file for this architecture.
    elf_machine: u16,
}

const TARGETS: &[Target] = &[
    Target {
        arch: "x86_64",
        triple: "x86_64-unknown-linux-musl",
        elf_machine: 62,
    },
    Target {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-musl",
        elf_machine: 183,
    },
];

const BINARIES: &[&str] = &["yapacc", "yapacc-mcp", "yapacc-tui"];

fn output(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{program} {}` failed: {}\n{}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The value of `key = "value"` in the section `[section]` of a manifest.
/// Enough for the two version lines this file needs; not a TOML parser.
fn manifest_value(manifest: &str, section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            let v = v.trim();
            return v
                .strip_prefix('"')
                .and_then(|v| v.split_once('"'))
                .map(|(v, _)| v.to_string());
        }
    }
    None
}

/// The version of the root package. It is written twice in `Cargo.toml`
/// (`[package]` and `[workspace.package]`, which the member crates inherit);
/// the two must agree.
pub fn version_of(manifest: &str) -> Result<String, String> {
    let package = manifest_value(manifest, "package", "version")
        .ok_or("Cargo.toml: no literal version in [package]")?;
    let workspace = manifest_value(manifest, "workspace.package", "version")
        .ok_or("Cargo.toml: no version in [workspace.package]")?;
    if package != workspace {
        return Err(format!(
            "Cargo.toml: [package] says version {package}, [workspace.package] says {workspace}"
        ));
    }
    Ok(package)
}

/// Facts about the toolchain `cargo` resolves to in `root`.
struct Toolchain {
    /// `rustc --version`, for the report.
    version: String,
    host: String,
    commit_hash: String,
    sysroot: PathBuf,
}

impl Toolchain {
    fn detect(root: &Path) -> Result<Self, String> {
        let verbose = output(root, "rustc", &["-vV"])?;
        let field = |name: &str| {
            verbose
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .map(|v| v.trim().to_string())
                .ok_or_else(|| format!("rustc -vV prints no `{name}` line"))
        };
        Ok(Self {
            version: verbose.lines().next().unwrap_or_default().to_string(),
            host: field("host:")?,
            commit_hash: field("commit-hash:")?,
            sysroot: PathBuf::from(output(root, "rustc", &["--print", "sysroot"])?.trim()),
        })
    }

    fn has_target(&self, triple: &str) -> bool {
        self.sysroot
            .join("lib/rustlib")
            .join(triple)
            .join("lib")
            .is_dir()
    }

    fn has_rust_lld(&self) -> bool {
        self.sysroot
            .join("lib/rustlib")
            .join(&self.host)
            .join("bin/rust-lld")
            .is_file()
    }
}

/// Directories of the build machine that must not appear in a binary, with
/// the name each is replaced by. Order matters: rustc applies the last
/// prefix that matches, so the more specific ones come later.
fn private_paths(root: &Path, toolchain: &Toolchain) -> Vec<(PathBuf, String)> {
    let mut paths: Vec<(PathBuf, String)> = Vec::new();
    // A home directory of "/" (some containers) is no prefix worth hiding,
    // and replacing it would rewrite every path.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.components().count() > 1);
    if let Some(home) = &home {
        paths.push((home.clone(), "/builder".into()));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    if let Some(cargo_home) = cargo_home {
        paths.push((cargo_home, "/cargo".into()));
    }
    let rustc = format!("/rustc/{}", toolchain.commit_hash);
    paths.push((toolchain.sysroot.clone(), format!("{rustc}/sysroot")));
    // Where the `rust-src` component puts the standard library's sources;
    // without the component the compiler already names them /rustc/<hash>.
    paths.push((toolchain.sysroot.join("lib/rustlib/src/rust"), rustc));
    paths.push((root.to_path_buf(), "/yapacc".into()));
    paths
}

/// The environment of every `cargo build` of the release binaries.
fn build_env(
    target: &Target,
    private: &[(PathBuf, String)],
    commit_time: u64,
) -> Vec<(String, String)> {
    // CARGO_ENCODED_RUSTFLAGS separates flags with 0x1f, so a path with a
    // space in it stays one flag. It replaces RUSTFLAGS and the rustflags
    // of any cargo configuration file for these builds.
    let flags: Vec<String> = private
        .iter()
        .map(|(from, to)| format!("--remap-path-prefix={}={to}", from.display()))
        .collect();
    let linker = format!(
        "CARGO_TARGET_{}_LINKER",
        target.triple.to_uppercase().replace('-', "_")
    );
    vec![
        ("CARGO_ENCODED_RUSTFLAGS".into(), flags.join("\u{1f}")),
        (linker, "rust-lld".into()),
        ("CARGO_INCREMENTAL".into(), "0".into()),
        ("SOURCE_DATE_EPOCH".into(), commit_time.to_string()),
    ]
}

fn cargo_build(
    root: &Path,
    env: &[(String, String)],
    triple: &str,
    features: &[&str],
    bins: &[&str],
) -> Result<(), String> {
    let mut args = vec!["build", "--release", "--locked", "--no-default-features"];
    args.extend(features);
    args.extend(["--target", triple]);
    for bin in bins {
        args.extend(["--bin", bin]);
    }
    println!("\n$ cargo {}", args.join(" "));
    let status = Command::new("cargo")
        .args(&args)
        .current_dir(root)
        .env_remove("RUSTFLAGS")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed: {status}", args.join(" ")))
    }
}

/// What the header of an ELF file says, as far as a release check needs it.
#[derive(Debug, PartialEq)]
pub struct Elf {
    pub machine: u16,
    /// A program interpreter is requested (`PT_INTERP`): the file needs a
    /// dynamic loader, so it is not a static executable.
    pub has_interpreter: bool,
}

/// Read the ELF header and the program headers of a 64-bit little-endian
/// file (both targets are).
pub fn parse_elf(bytes: &[u8]) -> Result<Elf, String> {
    let u16_at = |at: usize| -> Result<u16, String> {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "truncated ELF file".to_string())
    };
    if bytes.get(..4) != Some(b"\x7fELF") {
        return Err("not an ELF file".into());
    }
    if bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    let machine = u16_at(0x12)?;
    let phoff = bytes
        .get(0x20..0x28)
        .map(|b| u64::from_le_bytes(b.try_into().expect("eight bytes")))
        .ok_or("truncated ELF file")? as usize;
    let phentsize = u16_at(0x36)? as usize;
    let phnum = u16_at(0x38)? as usize;
    let mut has_interpreter = false;
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        let p_type = bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
            .ok_or("truncated ELF program headers")?;
        if p_type == 3 {
            has_interpreter = true;
        }
    }
    Ok(Elf {
        machine,
        has_interpreter,
    })
}

/// The printable text around the first occurrence of `needle`, if any.
pub fn find_path(bytes: &[u8], needle: &[u8]) -> Option<String> {
    if needle.is_empty() || bytes.len() < needle.len() {
        return None;
    }
    let at = bytes.windows(needle.len()).position(|w| w == needle)?;
    let printable = |b: &u8| (0x20..0x7f).contains(b);
    let start = at
        - bytes[..at]
            .iter()
            .rev()
            .take(40)
            .take_while(|b| printable(b))
            .count();
    let end = at
        + bytes[at..]
            .iter()
            .take(needle.len() + 80)
            .take_while(|b| printable(b))
            .count();
    Some(String::from_utf8_lossy(&bytes[start..end]).to_string())
}

/// The checks every release binary must pass before it is packed.
fn check_binary(path: &Path, target: &Target, private: &[(PathBuf, String)]) -> Result<(), String> {
    let name = path.display();
    let bytes = fs::read(path).map_err(|e| format!("cannot read {name}: {e}"))?;
    let elf = parse_elf(&bytes).map_err(|e| format!("{name}: {e}"))?;
    if elf.machine != target.elf_machine {
        return Err(format!(
            "{name}: ELF machine {} is not {}",
            elf.machine, target.arch
        ));
    }
    if elf.has_interpreter {
        return Err(format!(
            "{name} asks for a dynamic loader: it is not statically linked"
        ));
    }
    for (dir, _) in private {
        let needle = dir.to_string_lossy();
        if let Some(found) = find_path(&bytes, needle.as_bytes()) {
            return Err(format!(
                "{name} contains a path of this machine ({needle}):\n  {found}\n\
                 --remap-path-prefix does not reach a path compiled in with env!(); \
                 remove it from the source"
            ));
        }
    }
    Ok(())
}

/// How to start a binary of `target` on this machine: directly, through
/// qemu's user-mode emulator, or not at all.
fn runner(target: &Target) -> Option<Vec<String>> {
    if target.arch == std::env::consts::ARCH {
        return Some(vec![]);
    }
    let qemu = format!("qemu-{}", target.arch);
    Command::new(&qemu)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        .then(|| vec![qemu])
}

fn run_binary(runner: &[String], bin: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = match runner.first() {
        Some(emulator) => {
            let mut c = Command::new(emulator);
            c.arg(bin);
            c
        }
        None => Command::new(bin),
    };
    let out = command
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "`{} {}` failed: {}\n{}",
            bin.display(),
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn install(from: &Path, to: &Path, mode: u32) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::copy(from, to).map_err(|e| format!("copy {} to {}: {e}", from.display(), to.display()))?;
    fs::set_permissions(to, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", to.display()))
}

fn write_file(to: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(to, content).map_err(|e| format!("write {}: {e}", to.display()))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// Give a tree the modes an archive should record whatever the umask of
/// the machine was: 755 for directories and executable files, 644 for the
/// rest.
fn normalize_modes(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let executable = meta.is_dir() || meta.permissions().mode() & 0o111 != 0;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    if meta.is_dir() {
        let entries = fs::read_dir(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        for entry in entries.flatten() {
            normalize_modes(&entry.path())?;
        }
    }
    Ok(())
}

/// Finish `producer | gzip -n -9 > out`.
fn gzip_to(mut producer: std::process::Child, what: &str, out: &Path) -> Result<(), String> {
    let file = fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let stdout = producer
        .stdout
        .take()
        .expect("the producer's stdout is piped");
    let gzip = Command::new("gzip")
        .args(["-n", "-9"])
        .stdin(stdout)
        .stdout(file)
        .status()
        .map_err(|e| format!("cannot run gzip: {e}"))?;
    let produced = producer
        .wait()
        .map_err(|e| format!("cannot wait for {what}: {e}"))?;
    if !produced.success() {
        return Err(format!("{what} failed: {produced}"));
    }
    if !gzip.success() {
        return Err(format!("gzip failed: {gzip}"));
    }
    Ok(())
}

/// Pack `parent/name` as `out`, so that the same tree gives the same bytes.
fn tar_gz(parent: &Path, name: &str, mtime: u64, out: &Path) -> Result<(), String> {
    normalize_modes(&parent.join(name))?;
    let mtime = format!("--mtime=@{mtime}");
    let tar = Command::new("tar")
        .current_dir(parent)
        .env("LC_ALL", "C")
        .args(["--sort=name", "--format=gnu", "--owner=0", "--group=0"])
        .args(["--numeric-owner", &mtime, "-cf", "-", name])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    gzip_to(tar, "tar", out)
}

/// The placeholders a filled recipe must no longer contain.
const PLACEHOLDERS: &[&str] = &[
    "sha256sums=('SKIP')",
    "sha256sums = SKIP",
    "sha512sums=\"\"",
    "checksum=0000000000000000",
    "example.invalid",
    "UNRELEASED",
];

/// Every `file:line` below `dir` that still holds a placeholder. The
/// directory's own documentation and the script that fills the recipes
/// name the placeholders on purpose and are not recipes.
pub fn placeholders_in(dir: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = fs::read_dir(&d).map_err(|e| format!("read {}: {e}", d.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let top_level = path.parent() == Some(dir);
            let name = entry.file_name();
            if top_level && (name == "README.md" || name == "fill-release.sh") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if PLACEHOLDERS.iter().any(|p| line.contains(p)) {
                    found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The section of a "Keep a Changelog" file for `version`: what follows the
/// heading `## [version] - date` (or `## version`) up to the next heading of
/// that level, without the link definitions that end such a file. `None`
/// when there is no such heading or nothing under it.
pub fn changelog_section(changelog: &str, version: &str) -> Option<String> {
    let mut lines = changelog.lines().skip_while(|line| {
        let Some(title) = line.strip_prefix("## ") else {
            return true;
        };
        let name = title.split(" - ").next().unwrap_or_default().trim();
        name.trim_start_matches('[').trim_end_matches(']') != version
    });
    lines.next()?;
    let is_link_definition = |line: &str| {
        line.strip_prefix('[')
            .and_then(|rest| rest.split_once("]:"))
            .is_some_and(|(_, target)| target.trim_start().starts_with("http"))
    };
    let body: Vec<&str> = lines
        .take_while(|line| !line.starts_with("## "))
        .filter(|line| !is_link_definition(line))
        .collect();
    let text = body.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// `cargo xtask release-notes [VERSION]`: print the section of CHANGELOG.md
/// for the version (default: the one in Cargo.toml). The release workflow
/// gives it to `gh release create --notes-file`.
pub fn release_notes(root: &Path, args: &[String]) -> Result<(), String> {
    let version = match args {
        [] => {
            let manifest = fs::read_to_string(root.join("Cargo.toml"))
                .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
            version_of(&manifest)?
        }
        [version] if !version.starts_with('-') => version.trim_start_matches('v').to_string(),
        _ => return Err("usage: cargo xtask release-notes [VERSION]".into()),
    };
    let changelog = fs::read_to_string(root.join("CHANGELOG.md"))
        .map_err(|e| format!("cannot read CHANGELOG.md: {e}"))?;
    let section = changelog_section(&changelog, &version).ok_or_else(|| {
        format!("CHANGELOG.md has no section `## [{version}] - <date>` with text under it")
    })?;
    println!("{section}");
    Ok(())
}

/// Remove what an earlier run left in the output directory, and nothing
/// else: `--out` may name a directory that holds other files.
fn clean(out: &Path) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(out) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let removed = if name == ".work" {
            fs::remove_dir_all(&path)
        } else if name == "SHA256SUMS" || (name.starts_with("yapacc-") && name.ends_with(".tar.gz"))
        {
            fs::remove_file(&path)
        } else {
            Ok(())
        };
        removed.map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(())
}

struct Options {
    require_aarch64: bool,
    allow_dirty: bool,
    out: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        require_aarch64: false,
        allow_dirty: false,
        out: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--require-aarch64" => options.require_aarch64 = true,
            "--allow-dirty" => options.allow_dirty = true,
            "--out" => {
                let dir = it.next().ok_or("--out needs a directory")?;
                options.out = Some(PathBuf::from(dir));
            }
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    Ok(options)
}

/// Build one architecture and leave its three checked binaries in `stage`.
fn build_target(
    root: &Path,
    target_dir: &Path,
    target: &Target,
    private: &[(PathBuf, String)],
    commit_time: u64,
    stage: &Path,
) -> Result<(), String> {
    let env = build_env(target, private, commit_time);
    let built = target_dir.join(target.triple).join("release");
    // The core and the MCP server carry no optional feature. They are built
    // first and copied out, so that the core in the archive is the one the
    // size budget is about, not one compiled with the tui feature.
    cargo_build(root, &env, target.triple, &[], &["yapacc", "yapacc-mcp"])?;
    install(&built.join("yapacc"), &stage.join("yapacc"), 0o755)?;
    install(&built.join("yapacc-mcp"), &stage.join("yapacc-mcp"), 0o755)?;
    cargo_build(
        root,
        &env,
        target.triple,
        &["--features", "tui"],
        &["yapacc-tui"],
    )?;
    install(&built.join("yapacc-tui"), &stage.join("yapacc-tui"), 0o755)?;
    for bin in BINARIES {
        check_binary(&stage.join(bin), target, private)?;
    }
    let size = fs::metadata(stage.join("yapacc"))
        .map_err(|e| format!("stat the core: {e}"))?
        .len();
    println!(
        "{}: static, no path of this machine; core {size} bytes (limit {CORE_SIZE_LIMIT})",
        target.arch
    );
    if size > CORE_SIZE_LIMIT {
        return Err(format!(
            "the {} core is {size} bytes, over the {CORE_SIZE_LIMIT} budget",
            target.arch
        ));
    }
    Ok(())
}

/// Start each binary once; `version` is what all three must report.
fn smoke(runner: &[String], stage: &Path, version: &str) -> Result<(), String> {
    for bin in BINARIES {
        let said = run_binary(runner, &stage.join(bin), &["--version"])?;
        let expected = format!("{bin} {version}");
        if said.trim() != expected {
            return Err(format!(
                "{} --version printed `{}`, expected `{expected}`",
                stage.join(bin).display(),
                said.trim()
            ));
        }
    }
    Ok(())
}

pub fn run(root: &Path, target_dir: &Path, args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let options = parse_args(args)?;

    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    let version = version_of(&manifest)?;

    let tar_version = output(root, "tar", &["--version"])?;
    if !tar_version.contains("GNU tar") {
        return Err("GNU tar is needed (--sort=name, --mtime)".into());
    }
    let dirty = output(
        root,
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !dirty.trim().is_empty() {
        let note = "tracked files differ from HEAD";
        if !options.allow_dirty {
            return Err(format!(
                "{note}; commit first, or pass --allow-dirty:\n{dirty}"
            ));
        }
        println!("warning: {note}; the binaries will not match the source tarball");
    }
    let commit = output(root, "git", &["rev-parse", "HEAD"])?
        .trim()
        .to_string();
    let commit_time: u64 = output(root, "git", &["log", "-1", "--format=%ct"])?
        .trim()
        .parse()
        .map_err(|e| format!("git log printed no commit time: {e}"))?;

    let toolchain = Toolchain::detect(root)?;
    if !toolchain.has_rust_lld() {
        return Err(format!(
            "no rust-lld in {}: the release binaries are linked with the linker \
             a rustup toolchain ships",
            toolchain.sysroot.display()
        ));
    }
    let private = private_paths(root, &toolchain);

    let out = options
        .out
        .clone()
        .unwrap_or_else(|| target_dir.join("dist"));
    let work = out.join(".work");
    clean(&out)?;
    fs::create_dir_all(&work).map_err(|e| format!("create {}: {e}", work.display()))?;
    let out = out
        .canonicalize()
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let work = out.join(".work");
    println!(
        "yapacc {version}, commit {commit}\n{}\noutput: {}",
        toolchain.version,
        out.display()
    );

    // 1. Binaries.
    let mut built: Vec<&Target> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for target in TARGETS {
        let required = target.arch == "x86_64" || options.require_aarch64;
        if !toolchain.has_target(target.triple) {
            let missing = format!(
                "the rust target {t} is not installed (rustup target add {t})",
                t = target.triple
            );
            if required {
                return Err(missing);
            }
            println!("\nSKIPPED {}: {missing}", target.arch);
            notes.push(format!("no {} archive: {missing}", target.arch));
            continue;
        }
        let stage = work.join("bin").join(target.arch);
        build_target(root, target_dir, target, &private, commit_time, &stage)?;
        match runner(target) {
            Some(runner) => {
                smoke(&runner, &stage, &version)?;
                let how = runner.first().map_or("directly", String::as_str);
                println!("{}: the three binaries start ({how})", target.arch);
            }
            None if required => {
                return Err(format!(
                    "cannot run the {a} binaries here: qemu-{a} was not found",
                    a = target.arch
                ));
            }
            None => {
                println!("{a}: NOT executed, qemu-{a} was not found", a = target.arch);
                notes.push(format!("the {} binaries were built, not run", target.arch));
            }
        }
        built.push(target);
    }

    // 2. Shell completions: text, the same for every architecture, printed
    //    by the first binary this machine can start.
    let completions = work.join("completions");
    let (printer, printer_runner) = built
        .iter()
        .find_map(|t| runner(t).map(|r| (*t, r)))
        .ok_or("no built binary can be started here to print the shell completions")?;
    let core = work.join("bin").join(printer.arch).join("yapacc");
    for (shell, file) in [
        ("bash", "yapacc"),
        ("zsh", "_yapacc"),
        ("fish", "yapacc.fish"),
    ] {
        let text = run_binary(&printer_runner, &core, &["completions", shell])?;
        if text.trim().is_empty() {
            return Err(format!("empty {shell} completions"));
        }
        write_file(&completions.join(file), &text)?;
    }

    // 3. One archive per architecture.
    let mut artifacts: Vec<String> = Vec::new();
    for target in &built {
        let name = format!("yapacc-{version}-{}-linux-musl", target.arch);
        let tree = work.join("pack").join(&name);
        let stage = work.join("bin").join(target.arch);
        install(&stage.join("yapacc"), &tree.join("bin/yapacc"), 0o755)?;
        for component in ["yapacc-tui", "yapacc-mcp"] {
            let to = tree.join("lib/yapacc").join(component);
            install(&stage.join(component), &to, 0o755)?;
        }
        let share = tree.join("share");
        for (from, to) in [
            (
                root.join("contrib/man/yapacc.1"),
                share.join("man/man1/yapacc.1"),
            ),
            (
                completions.join("yapacc"),
                share.join("bash-completion/completions/yapacc"),
            ),
            (
                completions.join("_yapacc"),
                share.join("zsh/site-functions/_yapacc"),
            ),
            (
                completions.join("yapacc.fish"),
                share.join("fish/vendor_completions.d/yapacc.fish"),
            ),
            (root.join("LICENSE"), tree.join("LICENSE")),
            (root.join("README.md"), tree.join("README.md")),
        ] {
            install(&from, &to, 0o644)?;
        }
        let archive = format!("{name}.tar.gz");
        tar_gz(&work.join("pack"), &name, commit_time, &out.join(&archive))?;
        artifacts.push(archive);
    }

    // 4. The source tarball: what the distribution recipes build from. Its
    //    bytes are a function of the commit (git archive, gzip -n).
    let source = format!("yapacc-{version}.tar.gz");
    let prefix = format!("--prefix=yapacc-{version}/");
    let archive = Command::new("git")
        .current_dir(root)
        .args(["archive", "--format=tar", &prefix, "HEAD"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    gzip_to(archive, "git archive", &out.join(&source))?;
    let member = format!("yapacc-{version}/Cargo.toml");
    let source_path = out.join(&source).display().to_string();
    let packed_manifest = output(root, "tar", &["-xzOf", &source_path, &member])?;
    let packed_version = version_of(&packed_manifest)?;
    if packed_version != version {
        return Err(format!(
            "HEAD is version {packed_version}, the working tree is {version}"
        ));
    }
    artifacts.push(source.clone());

    // 5. contrib/packaging with the version and the checksum of that
    //    tarball filled in.
    let packaging = format!("yapacc-{version}-packaging");
    let recipes = work.join(&packaging);
    copy_tree(&root.join("contrib/packaging"), &recipes)?;
    let fill = root.join("contrib/packaging/fill-release.sh");
    let status = Command::new("sh")
        .arg(&fill)
        .arg("-d")
        .arg(&recipes)
        .args([&version, &source_path])
        .env("SOURCE_DATE_EPOCH", commit_time.to_string())
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run sh: {e}"))?;
    if !status.success() {
        return Err(format!("fill-release.sh failed: {status}"));
    }
    let left = placeholders_in(&recipes)?;
    if !left.is_empty() {
        return Err(format!(
            "a placeholder survived in the filled recipes:\n{}",
            left.join("\n")
        ));
    }
    let archive = format!("{packaging}.tar.gz");
    tar_gz(&work, &packaging, commit_time, &out.join(&archive))?;
    artifacts.push(archive);

    // 6. Checksums.
    fs::remove_dir_all(&work).map_err(|e| format!("remove {}: {e}", work.display()))?;
    artifacts.sort();
    let names: Vec<&str> = artifacts.iter().map(String::as_str).collect();
    let sums = output(&out, "sha256sum", &names)?;
    write_file(&out.join("SHA256SUMS"), &sums)?;
    output(&out, "sha256sum", &["-c", "SHA256SUMS"])?;

    println!("\n{}:", out.display());
    for line in sums.lines() {
        let name = line.rsplit(' ').next().unwrap_or_default();
        let size = fs::metadata(out.join(name)).map(|m| m.len()).unwrap_or(0);
        println!("  {line}  ({size} bytes)");
    }
    for note in &notes {
        println!("note: {note}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_versions_of_the_manifest_agree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let version = version_of(&manifest).expect("one version, written twice");
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        // site.rs reads the version after the first occurrence of the
        // section header, wherever it is: it must be the header itself.
        assert_eq!(manifest.matches("[package]").count(), 1);
        let after = manifest.split("[package]").nth(1).unwrap();
        let first = after
            .lines()
            .find(|line| line.trim().starts_with("version"))
            .unwrap();
        assert_eq!(first, format!("version = \"{version}\""));
    }

    #[test]
    fn version_is_read_from_its_section_only() {
        let manifest = "[workspace.package]\nversion = \"1.2.3\" # the one\n\
            [workspace.dependencies]\nversion = \"9\"\n[package]\nname = \"x\"\n\
            version = \"1.2.3\"\n[dependencies]\nversion = \"8\"\n";
        assert_eq!(version_of(manifest).as_deref(), Ok("1.2.3"));
        let differs = manifest.replacen("1.2.3", "1.2.4", 1);
        assert!(version_of(&differs).unwrap_err().contains("1.2.4"));
        let inherited =
            "[workspace.package]\nversion = \"1.0.0\"\n[package]\nversion.workspace = true\n";
        assert!(version_of(inherited).is_err());
    }

    /// A minimal ELF64 header followed by `types.len()` program headers.
    fn elf(machine: u16, types: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x40 + types.len() * 0x38];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
        bytes[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        bytes[0x36..0x38].copy_from_slice(&0x38u16.to_le_bytes());
        bytes[0x38..0x3a].copy_from_slice(&(types.len() as u16).to_le_bytes());
        for (i, t) in types.iter().enumerate() {
            let at = 0x40 + i * 0x38;
            bytes[at..at + 4].copy_from_slice(&t.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_program_interpreter_means_not_static() {
        // PT_LOAD = 1, PT_DYNAMIC = 2 (a static-pie has one), PT_INTERP = 3
        let static_pie = parse_elf(&elf(62, &[1, 2, 1])).unwrap();
        assert_eq!(static_pie.machine, 62);
        assert!(!static_pie.has_interpreter);
        assert!(parse_elf(&elf(183, &[1, 3, 2])).unwrap().has_interpreter);
        assert!(parse_elf(b"#!/bin/sh\n").is_err());
        assert!(parse_elf(&elf(62, &[1])[..0x42]).is_err());
    }

    #[test]
    fn a_compiled_in_path_is_found_with_its_surroundings() {
        let bytes = b"\x00\x01see /home/someone/src/x.rs:4\x00more";
        let found = find_path(bytes, b"/home/someone").unwrap();
        assert_eq!(found, "see /home/someone/src/x.rs:4");
        assert_eq!(find_path(bytes, b"/home/nobody"), None);
        assert_eq!(find_path(b"ab", b"abc"), None);
    }

    #[test]
    fn the_remapping_covers_the_checkout_last() {
        let toolchain = Toolchain {
            version: String::new(),
            host: "x86_64-unknown-linux-gnu".into(),
            commit_hash: "abc".into(),
            sysroot: PathBuf::from("/opt/rust"),
        };
        let private = private_paths(Path::new("/src/yapacc"), &toolchain);
        let (last, to) = private.last().unwrap();
        assert_eq!(
            (last.as_path(), to.as_str()),
            (Path::new("/src/yapacc"), "/yapacc")
        );
        let env = build_env(&TARGETS[1], &private, 7);
        let flags = &env
            .iter()
            .find(|(k, _)| k == "CARGO_ENCODED_RUSTFLAGS")
            .unwrap()
            .1;
        assert!(flags.contains("--remap-path-prefix=/opt/rust/lib/rustlib/src/rust=/rustc/abc"));
        assert!(env.iter().any(
            |(k, v)| k == "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER" && v == "rust-lld"
        ));
    }

    #[test]
    fn the_changelog_section_of_a_version_is_cut_out() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n- soon\n\n\
            ## [0.2.0] - 2027-01-01\n\n### Added\n\n- b, see [the docs][d]\n\n\
            ## [0.1.0] - 2026-11-20\n\n### Added\n\n- a\n\n\
            [Unreleased]: https://example.org/compare\n[0.1.0]: https://example.org/v0.1.0\n";
        assert_eq!(
            changelog_section(changelog, "0.2.0").as_deref(),
            Some("### Added\n\n- b, see [the docs][d]")
        );
        assert_eq!(
            changelog_section(changelog, "0.1.0").as_deref(),
            Some("### Added\n\n- a")
        );
        assert_eq!(changelog_section(changelog, "0.1"), None);
        assert_eq!(changelog_section(changelog, "0.3.0"), None);
        assert_eq!(
            changelog_section("## [0.3.0] - x\n\n## [0.2.0]\n", "0.3.0"),
            None
        );
        assert_eq!(
            changelog_section("## 1.0.0\ntext\n", "1.0.0").as_deref(),
            Some("text")
        );
    }

    #[test]
    fn cleaning_the_output_directory_spares_what_is_not_ours() {
        let dir = std::env::temp_dir().join(format!("yapacc-xtask-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for file in [
            "SHA256SUMS",
            "yapacc-0.1.0.tar.gz",
            "yapacc-0.1.0-x86_64-linux-musl.tar.gz",
            ".work/pack/x",
            "notes.txt",
            "other-1.0.tar.gz",
        ] {
            write_file(&dir.join(file), "x").unwrap();
        }
        clean(&dir).unwrap();
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, ["notes.txt", "other-1.0.tar.gz"]);
        clean(&dir.join("absent")).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn placeholders_are_found_in_recipes_and_not_in_the_documentation() {
        let dir = std::env::temp_dir().join(format!("yapacc-xtask-ph-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir.join("README.md"), "the UNRELEASED placeholder\n").unwrap();
        write_file(
            &dir.join("arch/PKGBUILD"),
            "pkgver=1\nsha256sums=('SKIP')\n",
        )
        .unwrap();
        write_file(
            &dir.join("debian/control"),
            "Homepage: https://example.invalid/x\n",
        )
        .unwrap();
        let found = placeholders_in(&dir).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("PKGBUILD:2"), "{found:?}");
        write_file(&dir.join("arch/PKGBUILD"), "pkgver=1\nsha256sums=('abc')\n").unwrap();
        write_file(
            &dir.join("debian/control"),
            "Homepage: https://example.org/x\n",
        )
        .unwrap();
        assert!(placeholders_in(&dir).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
