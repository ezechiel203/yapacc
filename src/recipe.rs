//! Recipe fallback — the last lane of the resolution engine.
//!
//! When a package exists nowhere (repos, AUR, flathub, taps), yapacc
//! GENERATES a build recipe (PKGBUILD scaffold) from an upstream release
//! URL, computes the artifact checksum, and stops at a REVIEW GATE:
//! the recipe is printed in full and must be accepted (interactively, or
//! explicitly with --no-review; --script refuses without it). Only then is
//! the source built — non-root, via makepkg — and the built package is
//! installed into the lane-2 keg (extract, attributed bins, ledger).
//!
//! Safety contract: yapacc never silently builds unreviewed upstream code.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the recipe that will be reviewed and built comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeState {
    /// No PKGBUILD existed; a new one was generated.
    New,
    /// A PKGBUILD that yapacc generated, untouched since, was generated
    /// again from the current name, URL and factory.
    Regenerated,
    /// A PKGBUILD that was edited after yapacc generated it (or that yapacc
    /// has no record of generating) exists. It is used as it is and is
    /// never overwritten.
    Kept,
}

pub struct GeneratedRecipe {
    pub dir: PathBuf,
    pub pkgbuild_path: PathBuf,
    /// the text that is shown at the review gate and then built
    pub pkgbuild: String,
    pub checksum_filled: bool,
    pub state: RecipeState,
    /// whether `pkgbuild` is on disk at `pkgbuild_path` (false in a dry run
    /// that would have generated it)
    pub on_disk: bool,
}

/// Build factory kinds: ecosystem-aware build/package step generation.
/// Managed languages (rust/go/...) are the easy cases; legacy C giants get
/// per-project custom handlers via the CUSTOM_FACTORIES registry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactoryKind {
    Cargo,
    Go,
    Meson,
    Cmake,
    Autotools,
    Make,
    Python,
    Node,
    Custom,
}

/// Registry for projects whose build process is famously special (legacy C
/// giants). Entries emit bespoke build()/package() steps. Extend as real
/// cases are met — the hook is the point, not the initial list.
const CUSTOM_FACTORIES: &[(&str, &str, &str)] = &[
    // (project-name-hint, build steps, package steps)
    (
        "openssl",
        "./config --prefix=/usr --openssldir=/etc/ssl --libdir=lib shared\n  make",
        "make DESTDIR=\"$pkgdir\" MANSUFFIX=ssl install_sw install_ssldirs",
    ),
    (
        "llvm",
        "cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release\n  ninja -C build",
        "DESTDIR=\"$pkgdir\" ninja -C build install",
    ),
    (
        "emacs",
        "./configure --prefix=/usr --with-x-toolkit=gtk3\n  make",
        "make DESTDIR=\"$pkgdir\" install",
    ),
];

/// Parse a user-supplied --factory value.
pub fn parse_factory_kind(v: &str) -> Result<FactoryKind, String> {
    match v.to_lowercase().as_str() {
        "cargo" | "rust" => Ok(FactoryKind::Cargo),
        "go" | "golang" => Ok(FactoryKind::Go),
        "meson" => Ok(FactoryKind::Meson),
        "cmake" => Ok(FactoryKind::Cmake),
        "autotools" | "configure" | "autoconf" => Ok(FactoryKind::Autotools),
        "make" | "makefile" => Ok(FactoryKind::Make),
        "python" => Ok(FactoryKind::Python),
        "node" | "npm" | "js" => Ok(FactoryKind::Node),
        "custom" => Ok(FactoryKind::Custom),
        other => Err(format!(
            "unknown factory '{other}' (cargo|go|meson|cmake|autotools|make|python|node|custom)"
        )),
    }
}

/// Detect the factory from a name/URL hint and (optionally) repo contents.
pub fn detect_factory(name: &str, url: Option<&str>) -> FactoryKind {
    let hay = format!("{} {}", name, url.unwrap_or("")).to_lowercase();
    if let Some((_, build, package)) = CUSTOM_FACTORIES.iter().find(|(k, _, _)| hay.contains(k)) {
        let _ = (build, package);
        return FactoryKind::Custom;
    }
    if hay.ends_with(".crate") || hay.contains("cargo") || hay.contains("/rust-") {
        return FactoryKind::Cargo;
    }
    if hay.contains("golang") || hay.ends_with(".go") || hay.contains("go.mod") {
        return FactoryKind::Go;
    }
    FactoryKind::Make // generic fallback; refine when the source is unpacked
}

/// Build/package steps per factory (embedded into the generated PKGBUILD).
pub fn factory_steps(kind: FactoryKind, name: &str, url: Option<&str>) -> (String, String) {
    // custom registry first
    let hay = format!("{name} {}", url.unwrap_or("")).to_lowercase();
    if kind == FactoryKind::Custom
        && let Some((_, build, package)) = CUSTOM_FACTORIES.iter().find(|(k, _, _)| hay.contains(k))
    {
        return (build.to_string(), package.to_string());
    }
    match kind {
        FactoryKind::Cargo => (
            "cargo build --release".into(),
            "install -Dm755 target/release/$pkgname $pkgdir/usr/bin/$pkgname".into(),
        ),
        FactoryKind::Go => (
            "go build -trimpath -o $pkgname ./...".into(),
            "install -Dm755 $pkgname $pkgdir/usr/bin/$pkgname".into(),
        ),
        FactoryKind::Meson => (
            "meson setup build\n  ninja -C build".into(),
            "DESTDIR=$pkgdir ninja -C build install".into(),
        ),
        FactoryKind::Cmake => (
            "cmake -B build -DCMAKE_BUILD_TYPE=Release\n  cmake --build build".into(),
            "DESTDIR=$pkgdir cmake --install build".into(),
        ),
        FactoryKind::Autotools => (
            "./configure --prefix=/usr\n  make".into(),
            "make DESTDIR=$pkgdir install".into(),
        ),
        FactoryKind::Python => (
            "python -m build --wheel --no-isolation".into(),
            "pip install --root=$pkgdir dist/*.whl".into(),
        ),
        FactoryKind::Node => (
            "npm ci || npm install".into(),
            "npm pack\n  install -Dm644 *.tgz $pkgdir/usr/src/".into(),
        ),
        FactoryKind::Make | FactoryKind::Custom => (
            "make".into(),
            "make DESTDIR=$pkgdir PREFIX=/usr install".into(),
        ),
    }
}

/// Rotten-recipe lint: dangerous patterns printed BEFORE the review gate.
/// Returns human-readable warnings (empty when clean).
pub fn lint_recipe(pkgbuild: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let lower = pkgbuild.to_lowercase();
    if lower.contains("curl")
        && (lower.contains("| sh") || lower.contains("| bash") || lower.contains("|sh"))
    {
        warnings.push(
            "pipes remote content into a shell (curl | sh) — classic malicious pattern".into(),
        );
    }
    if lower.contains("| sudo") || lower.contains("sudo ") {
        warnings.push("uses sudo inside the recipe — builds must never need root".into());
    }
    if lower.contains("chmod 777") {
        warnings.push("chmod 777 (world-writable) — sloppy at best".into());
    }
    if lower.contains("rm -rf /")
        && !lower.contains("rm -rf \"$srcdir")
        && !lower.contains("rm -rf \"$pkgdir")
    {
        warnings.push("rm -rf on an absolute path outside $srcdir/$pkgdir".into());
    }
    if lower.contains("eval $(curl") || lower.contains("eval \"$(curl") {
        warnings.push("eval of remote content".into());
    }
    // writes outside $pkgdir inside package()
    if let Some(pkg_start) = pkgbuild.find("package()") {
        let pkg_body = &pkgbuild[pkg_start..];
        for line in pkg_body.lines() {
            let t = line.trim();
            if (t.starts_with("install ") || t.contains(" >> /") || t.contains(" > /"))
                && t.contains('/')
                && !t.contains("$pkgdir")
                && !t.starts_with('#')
            {
                warnings.push(format!("package() writes outside $pkgdir: {t}"));
                break;
            }
        }
    }
    warnings
}

/// Recipes root: XDG data dir (never a system path; builds are user-scoped).
pub fn recipes_root() -> PathBuf {
    let xdg = crate::config::xdg_base("XDG_DATA_HOME", ".local/share")
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    xdg.join("yapacc/recipes")
}

/// Best-effort version extraction from a release filename:
/// `foo-1.2.3.tar.gz` → `1.2.3`, `foo-1.2.3-2.tar.gz` → `1.2.3`.
pub fn parse_ver_from_url(url: &str) -> String {
    let base = url.rsplit('/').next().unwrap_or("");
    let mut stem = base.split(".tar").next().unwrap_or(base);
    // strip a leading 'v' when followed by a digit (v1.2.3 release tags)
    if let Some(rest) = stem.strip_prefix('v')
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        stem = rest;
    }
    // last '-'-separated segment starting with a digit
    if let Some(seg) = stem
        .rsplit('-')
        .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
    {
        let cleaned: String = seg
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    "0.0.0".into()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `url` once (data only, never executed) and return its sha256
/// and size. The download goes into a fresh private directory (random
/// name, mode 0700, never a directory someone else prepared in /tmp) that
/// is removed again whatever happens.
fn sha256_of_url(url: &str) -> Option<(String, u64)> {
    valid_source_url(url).ok()?;
    let dir = crate::sandbox::fresh_private_dir(&std::env::temp_dir(), "yapacc-recipe-").ok()?;
    let path = dir.join("artifact");
    let fetched = Command::new("curl")
        .args(["--proto", "=http,https", "-fsSL", "--max-time", "60", "-o"])
        .arg(&path)
        .arg(url)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|_| fs::read(&path).ok());
    crate::sandbox::remove_tree(&dir);
    let bytes = fetched?;
    Some((sha256_hex(&bytes), bytes.len() as u64))
}

/// A recipe name that can stand unquoted in a PKGBUILD (`pkgname=`), in a
/// comment and in a URL path: the Arch `pkgname` characters only.
pub fn valid_recipe_name(name: &str) -> Result<(), String> {
    if crate::util::valid_pkg_name(name) {
        Ok(())
    } else {
        Err(format!(
            "'{}' cannot be a recipe name: only ASCII letters, digits and @ . _ + - are allowed, and it cannot start with - or . ; nothing was generated",
            name.escape_debug()
        ))
    }
}

/// A `pkgver` that can stand unquoted in a PKGBUILD: ASCII letters, digits
/// and `. _ +` (makepkg forbids `-`, `:`, `/` and whitespace itself).
pub fn valid_recipe_version(version: &str) -> Result<(), String> {
    let ok = !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "'{}' cannot be a recipe version: only ASCII letters, digits and . _ + are allowed; nothing was generated",
            version.escape_debug()
        ))
    }
}

/// A source URL that can stand in single quotes in a PKGBUILD and be given
/// to curl as one argument: `http://` or `https://`, then only URL
/// characters that mean nothing to a shell. Refused outright: `'`, `"`,
/// backquote, `$`, `\`, `(`, `)`, whitespace, control characters, and `::`
/// (which makepkg reads as `filename::url`). A URL that needs one of them
/// can carry it percent-encoded.
pub fn valid_source_url(url: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "the source URL '{}' is not accepted ({why}); nothing was generated",
            url.escape_debug()
        ))
    };
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return refuse("it must start with https:// or http://");
    };
    if rest.is_empty() || rest.starts_with('/') {
        return refuse("it names no host");
    }
    if url.len() > 2048 {
        return refuse("it is longer than 2048 characters");
    }
    if let Some(bad) = url.chars().find(|c| {
        !(c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '.'
                    | '_'
                    | '~'
                    | ':'
                    | '/'
                    | '?'
                    | '#'
                    | '['
                    | ']'
                    | '@'
                    | '&'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '%'
            ))
    }) {
        return refuse(&format!(
            "the character {:?} is not allowed; percent-encode it",
            bad
        ));
    }
    if url.contains("::") {
        return refuse("makepkg reads '::' as a rename");
    }
    Ok(())
}

/// The PKGBUILD text for these values. Pure. Every value is validated
/// before it is interpolated: the name and version against strict
/// character sets (they stand unquoted), the URL against
/// [`valid_source_url`] (it stands in single quotes, where the shell
/// expands nothing), the checksum as hex or `SKIP`.
pub fn render_pkgbuild(
    name: &str,
    url: Option<&str>,
    pkgver: &str,
    sha: &str,
    kind: FactoryKind,
) -> Result<String, String> {
    valid_recipe_name(name)?;
    valid_recipe_version(pkgver)?;
    if let Some(u) = url {
        valid_source_url(u)?;
    }
    if sha != "SKIP" && !(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(format!(
            "'{}' is not a sha256 checksum; nothing was generated",
            sha.escape_debug()
        ));
    }
    let source_line = match url {
        Some(u) => format!("source=('{u}')"),
        None => "source=('TODO: upstream release tarball URL')".to_string(),
    };
    let (build_steps, package_steps) = factory_steps(kind, name, url);
    Ok(format!(
        r#"# Generated by `yapacc get {name} --from recipe` — REVIEW BEFORE BUILDING
# This recipe is a scaffold. Edit pkgver/source/depends/build() as needed.
# yapacc will NOT build it until you accept the review gate.
# An edited recipe is kept: yapacc does not overwrite it on the next run.
pkgname={name}
pkgver={pkgver}
pkgrel=1
pkgdesc="TODO: description"
arch=('x86_64')
url="https://example.invalid/{name}"
license=('TODO')
depends=()
makedepends=()
{source_line}
sha256sums=('{sha}')

# Build factory: {kind:?} (ecosystem-aware steps; edit freely)
build() {{
  cd "$srcdir" || cd . || return 0
  {build_steps}
}}

package() {{
  cd "$srcdir" || cd . || return 0
  {package_steps}
}}
"#
    ))
}

/// Records the sha256 of the PKGBUILD as yapacc last generated it, so the
/// next run can tell an untouched scaffold from a recipe the user edited.
const GENERATED_STAMP: &str = ".yapacc-generated.sha256";

/// Write `content` to `path` without ever writing through a link: the
/// content goes to a new file (`O_CREAT|O_EXCL`) next to it, which is then
/// renamed over `path`. A symbolic link at `path` is replaced, not followed.
pub(crate) fn replace_file(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.yapacc-new-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)
        .and_then(|mut f| f.write_all(content.as_bytes()))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Prepare the recipe for `name`.
///
/// * A PKGBUILD that exists and was edited (it no longer matches what
///   yapacc recorded when it generated it, or there is no such record) is
///   returned as it is ([`RecipeState::Kept`]). Nothing is downloaded and
///   nothing is written: the user was told to edit that file and re-run.
/// * Otherwise a recipe is generated. With `url`, the tarball is downloaded
///   once (data only, never executed) to compute sha256 and pkgver; without
///   it, the scaffold carries a SKIP checksum and must be edited by hand
///   before build.
///
/// With `write` = false (`--dryrun`) nothing is created or changed on disk.
pub fn prepare(
    name: &str,
    url: Option<&str>,
    factory: Option<FactoryKind>,
    write: bool,
) -> Result<GeneratedRecipe, String> {
    prepare_in(&recipes_root(), name, url, factory, write, &|u| {
        let fetched = sha256_of_url(u);
        if let Some((_, size)) = &fetched {
            println!(
                "recipe: fetched upstream artifact {u} ({} bytes, sha256 verified at build time)",
                human(*size)
            );
        }
        fetched
    })
}

fn prepare_in(
    root: &Path,
    name: &str,
    url: Option<&str>,
    factory: Option<FactoryKind>,
    write: bool,
    fetch: &dyn Fn(&str) -> Option<(String, u64)>,
) -> Result<GeneratedRecipe, String> {
    valid_recipe_name(name)?;
    if let Some(u) = url {
        valid_source_url(u)?;
    }
    let dir = root.join(sanitize(name));
    let pkgbuild_path = dir.join("PKGBUILD");
    let stamp_path = dir.join(GENERATED_STAMP);

    let existing = match fs::symlink_metadata(&pkgbuild_path) {
        Ok(m) if m.is_file() => Some(
            fs::read_to_string(&pkgbuild_path)
                .map_err(|e| format!("cannot read {}: {e}", pkgbuild_path.display()))?,
        ),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a regular file; nothing was generated",
                pkgbuild_path.display()
            ));
        }
        Err(_) => None,
    };
    if let Some(text) = &existing {
        let untouched = fs::symlink_metadata(&stamp_path).is_ok_and(|m| m.is_file())
            && fs::read_to_string(&stamp_path)
                .is_ok_and(|stamp| stamp.trim() == sha256_hex(text.as_bytes()));
        if !untouched {
            let unfinished =
                text.contains("TODO: upstream release tarball URL") || text.contains("'SKIP'");
            return Ok(GeneratedRecipe {
                dir,
                pkgbuild_path,
                pkgbuild: text.clone(),
                checksum_filled: !unfinished,
                state: RecipeState::Kept,
                on_disk: true,
            });
        }
    }

    let (pkgver, sha, checksum_filled) = match url {
        Some(u) => {
            let (digest, _) =
                fetch(u).ok_or_else(|| format!("failed to download/hash release artifact: {u}"))?;
            (parse_ver_from_url(u), digest, true)
        }
        None => ("0.0.0".to_string(), "SKIP".to_string(), false),
    };
    let kind = factory.unwrap_or_else(|| detect_factory(name, url));
    let pkgbuild = render_pkgbuild(name, url, &pkgver, &sha, kind)?;
    let unchanged = existing.as_deref() == Some(pkgbuild.as_str());
    if write && !unchanged {
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        replace_file(&pkgbuild_path, &pkgbuild)
            .map_err(|e| format!("write {}: {e}", pkgbuild_path.display()))?;
        replace_file(
            &stamp_path,
            &format!("{}\n", sha256_hex(pkgbuild.as_bytes())),
        )
        .map_err(|e| format!("write {}: {e}", stamp_path.display()))?;
    }
    Ok(GeneratedRecipe {
        dir,
        pkgbuild_path,
        pkgbuild,
        checksum_filled,
        state: if existing.is_some() {
            RecipeState::Regenerated
        } else {
            RecipeState::New
        },
        on_disk: write || unchanged,
    })
}

fn human(size: u64) -> String {
    let b = size as f64;
    if b < 1024.0 {
        format!("{b:.0} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KiB", b / 1024.0)
    } else {
        format!("{:.1} MiB", b / (1024.0 * 1024.0))
    }
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() { "pkg".into() } else { out }
}

/// What is said above the recipe text, at the gate and in a dry run: which
/// file it is, and whether it was generated or is the user's edited one.
pub fn describe(recipe: &GeneratedRecipe) -> String {
    let path = recipe.pkgbuild_path.display();
    match (recipe.state, recipe.on_disk) {
        (RecipeState::Kept, _) => format!(
            "== Existing recipe: {path} ==\nThis recipe was edited after yapacc generated it (or yapacc has no record of generating it), so it was NOT regenerated and is used as it is. Delete that file and re-run to have yapacc generate a new one."
        ),
        (RecipeState::New, true) => format!("== Generated recipe: {path} =="),
        (RecipeState::Regenerated, true) => format!(
            "== Generated recipe: {path} ==\nThe earlier generated recipe there had not been edited; it was generated again."
        ),
        (RecipeState::New, false) => format!(
            "== Dry run: recipe generated for display only. It would be written to {path}; nothing was written. =="
        ),
        (RecipeState::Regenerated, false) => format!(
            "== Dry run: recipe generated for display only. It would replace the unedited generated recipe at {path}; nothing was written. =="
        ),
    }
}

/// Review gate: print the recipe, require interactive acceptance unless
/// `no_review` (script mode refuses without it — same rule as AUR installs).
/// The recipe shown is the one that is built: the generated one, or the
/// user's edited file when there is one.
pub fn review_gate(
    recipe: &GeneratedRecipe,
    no_review: bool,
    noninteractive: bool,
    script_mode: bool,
) -> Result<bool, String> {
    println!("{}", describe(recipe));
    if !recipe.checksum_filled {
        println!(
            "WARNING: the checksum is SKIP or the source URL is still a TODO — edit {} before building.",
            recipe.pkgbuild_path.display()
        );
    }
    println!("{}", recipe.pkgbuild);
    let warnings = lint_recipe(&recipe.pkgbuild);
    if !warnings.is_empty() {
        println!(
            "== Recipe lint: {} warning(s) — ROTTERN-RECIPE CHECK ==",
            warnings.len()
        );
        for w in &warnings {
            println!("  ⚠ {w}");
        }
    }
    if no_review {
        println!("(recipe review gate skipped by --no-review)");
        return Ok(true);
    }
    if noninteractive || script_mode {
        return Err(format!(
            "Nothing was built or installed: the recipe above has not been reviewed, and a --script/--noconfirm run cannot ask. Read it, edit {} if needed (an edited recipe is kept and used by the next run), then re-run with --no-review to accept it.",
            recipe.pkgbuild_path.display()
        ));
    }
    print!("Recipe review: build and install into the yapacc prefix? [y/N]: ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok();
    Ok(matches!(s.trim(), "y" | "Y" | "yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_from_urls() {
        assert_eq!(
            parse_ver_from_url("https://example.com/foo-1.2.3.tar.gz"),
            "1.2.3"
        );
        assert_eq!(
            parse_ver_from_url("https://example.com/foo-1.2.3.tar.xz"),
            "1.2.3"
        );
        assert_eq!(
            parse_ver_from_url("https://example.org/downloads/hello-2.10.tar.gz"),
            "2.10"
        );
        assert_eq!(
            parse_ver_from_url("https://example.com/v1.2.3.tar.gz"),
            "1.2.3"
        );
        assert_eq!(
            parse_ver_from_url("https://example.com/no-version.tar.gz"),
            "0.0.0"
        );
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "yapacc-recipe-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SHA2: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn no_fetch(_: &str) -> Option<(String, u64)> {
        panic!("nothing may be downloaded here")
    }

    /// Every file under `dir`, with its content: to assert "nothing changed".
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.clone(), fs::read(&p).unwrap_or_default()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn scaffold_contains_essentials() {
        let root = scratch("scaffold");
        let r = prepare_in(&root, "testpkg", None, None, true, &no_fetch).unwrap();
        assert!(r.pkgbuild.contains("pkgname=testpkg"));
        assert!(r.pkgbuild.contains("pkgver=0.0.0"));
        assert!(r.pkgbuild.contains("sha256sums=('SKIP')"));
        assert!(r.pkgbuild.contains("TODO: upstream release tarball URL"));
        assert!(r.pkgbuild.contains("REVIEW BEFORE BUILDING"));
        assert!(!r.checksum_filled);
        assert_eq!(r.state, RecipeState::New);
        assert!(r.on_disk);
        assert_eq!(r.pkgbuild_path, root.join("testpkg/PKGBUILD"));
        assert_eq!(fs::read_to_string(&r.pkgbuild_path).unwrap(), r.pkgbuild);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn url_is_single_quoted_and_values_stand_where_expected() {
        let url = "https://example.org/dl/hello-2.10.tar.gz?a=1&b=%24%28x%29";
        let pb = render_pkgbuild("hello", Some(url), "2.10", SHA, FactoryKind::Make).unwrap();
        assert!(pb.contains(&format!("source=('{url}')")), "{pb}");
        assert!(pb.contains("pkgname=hello\n"));
        assert!(pb.contains("pkgver=2.10\n"));
        assert!(pb.contains(&format!("sha256sums=('{SHA}')")));
        // the only double-quoted interpolation is the validated name
        assert!(pb.contains("url=\"https://example.invalid/hello\"\n"));
    }

    /// `$(...)`, backquotes, quotes and newlines never reach a PKGBUILD:
    /// makepkg sources that file as bash.
    #[test]
    fn hostile_values_are_refused_before_any_text_is_generated() {
        let hostile_urls = [
            "https://example.org/$(touch /tmp/pwned).tar.gz",
            "https://example.org/`touch /tmp/pwned`.tar.gz",
            "https://example.org/a'b.tar.gz",
            "https://example.org/a\"b.tar.gz",
            "https://example.org/a\nbuild() { evil; }\n.tar.gz",
            "https://example.org/a b.tar.gz",
            "https://example.org/a\tb.tar.gz",
            "https://example.org/a\\b.tar.gz",
            "https://example.org/a;b|c.tar.gz",
            "https://example.org/a(b).tar.gz",
            "https://example.org/${IFS}x.tar.gz",
            "https://example.org/x.tar.gz\u{0}",
            "https://example.org/é.tar.gz",
            "https://example.org/local::https://evil.example/x.tar.gz",
            "file:///etc/passwd",
            "ftp://example.org/x.tar.gz",
            "-o/tmp/pwned",
            "--config=/tmp/evil",
            "https://",
            "https:///nohost",
            "",
        ];
        for u in hostile_urls {
            assert!(valid_source_url(u).is_err(), "accepted URL {u:?}");
            let r = render_pkgbuild("hello", Some(u), "1.0", SHA, FactoryKind::Make);
            assert!(r.is_err(), "rendered a PKGBUILD from URL {u:?}");
            assert!(r.unwrap_err().contains("nothing was generated"));
        }
        let hostile_names = [
            "$(touch /tmp/pwned)",
            "`id`",
            "a'b",
            "a\"b",
            "a\nb",
            "a b",
            "a;b",
            "a/b",
            "../evil",
            "-rf",
            ".hidden",
            "a$b",
            "a\\b",
            "",
            "é",
        ];
        for n in hostile_names {
            assert!(valid_recipe_name(n).is_err(), "accepted name {n:?}");
            assert!(
                render_pkgbuild(n, None, "1.0", "SKIP", FactoryKind::Make).is_err(),
                "rendered a PKGBUILD for name {n:?}"
            );
        }
        let hostile_versions = [
            "$(id)", "1.0`id`", "1.0'", "1.0\"", "1.0\nx=1", "1 0", "1-0", "1:0", "1/0", "",
        ];
        for v in hostile_versions {
            assert!(valid_recipe_version(v).is_err(), "accepted version {v:?}");
            assert!(
                render_pkgbuild("hello", None, v, "SKIP", FactoryKind::Make).is_err(),
                "rendered a PKGBUILD for version {v:?}"
            );
        }
        for sha in ["", "skip", "abc", "$(id)", "'; evil; '"] {
            assert!(
                render_pkgbuild("hello", None, "1.0", sha, FactoryKind::Make).is_err(),
                "rendered a PKGBUILD for checksum {sha:?}"
            );
        }
        // ordinary values pass
        for u in [
            "https://github.com/BurntSushi/ripgrep/archive/14.1.0.tar.gz",
            "http://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz",
            "https://example.org/a_b-c~d/x+y@1.0.tar.xz?token=ab%20cd&x=1#frag",
            "https://[2001:db8:0:0:0:0:0:1]:8443/x.tar.gz",
        ] {
            assert!(valid_source_url(u).is_ok(), "refused URL {u:?}");
        }
        for n in ["hello", "python@3.12", "gtk+", "lib32-foo_bar", "7zip"] {
            assert!(valid_recipe_name(n).is_ok(), "refused name {n:?}");
        }
    }

    /// The same check end to end: a hostile URL writes nothing at all.
    #[test]
    fn hostile_url_generates_and_downloads_nothing() {
        let root = scratch("hostile");
        let r = prepare_in(
            &root,
            "hello",
            Some("https://example.org/$(touch pwned).tar.gz"),
            None,
            true,
            &no_fetch,
        );
        assert!(r.is_err());
        assert!(snapshot(&root).is_empty(), "nothing may be written");
        assert!(!root.join("hello").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_writes_nothing_for_a_new_recipe() {
        let root = scratch("dry-new");
        let r = prepare_in(
            &root,
            "hello",
            Some("https://example.org/hello-2.10.tar.gz"),
            None,
            false,
            &|_| Some((SHA.to_string(), 10)),
        )
        .unwrap();
        assert_eq!(r.state, RecipeState::New);
        assert!(!r.on_disk);
        assert!(r.pkgbuild.contains("pkgver=2.10"));
        assert!(snapshot(&root).is_empty(), "a dry run must write nothing");
        assert!(!root.join("hello").exists(), "not even the directory");
        assert!(describe(&r).contains("nothing was written"));
        let _ = fs::remove_dir_all(&root);
    }

    /// Finding 17 of the 2026-11 review: the gate tells the user to edit the
    /// PKGBUILD and re-run; the re-run must build that file, not overwrite it.
    #[test]
    fn an_edited_recipe_is_kept_and_gated_never_regenerated() {
        let root = scratch("edited");
        let url = "https://example.org/hello-2.10.tar.gz";
        let first = prepare_in(&root, "hello", Some(url), None, true, &|_| {
            Some((SHA.to_string(), 10))
        })
        .unwrap();
        assert_eq!(first.state, RecipeState::New);
        // the user fixes build() as the gate told them to
        let edited = first.pkgbuild.replace("  make\n", "  make -j1 V=1\n");
        assert_ne!(edited, first.pkgbuild);
        fs::write(&first.pkgbuild_path, &edited).unwrap();
        let before = snapshot(&root);

        // the re-run, also with a different URL and in a dry run: the file
        // on disk is what is shown and built; nothing is downloaded
        for (u, write) in [
            (Some(url), true),
            (Some("https://example.org/hello-3.0.tar.gz"), true),
            (None, true),
            (Some(url), false),
        ] {
            let again = prepare_in(&root, "hello", u, None, write, &no_fetch).unwrap();
            assert_eq!(again.state, RecipeState::Kept);
            assert_eq!(again.pkgbuild, edited, "the gate must show the edited file");
            assert!(again.on_disk);
            assert!(again.checksum_filled);
            assert_eq!(snapshot(&root), before, "the edit must not be overwritten");
            let said = describe(&again);
            assert!(said.contains("Existing recipe"), "{said}");
            assert!(said.contains("NOT regenerated"), "{said}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recipe_yapacc_has_no_record_of_is_kept() {
        // written by hand, or generated by a version that kept no record
        let root = scratch("foreign");
        fs::create_dir_all(root.join("hello")).unwrap();
        let own = "pkgname=hello\npkgver=1\nsource=('TODO: upstream release tarball URL')\n";
        fs::write(root.join("hello/PKGBUILD"), own).unwrap();
        let r = prepare_in(&root, "hello", None, None, true, &no_fetch).unwrap();
        assert_eq!(r.state, RecipeState::Kept);
        assert_eq!(r.pkgbuild, own);
        assert!(
            !r.checksum_filled,
            "an unfinished recipe is still warned about"
        );
        assert_eq!(
            fs::read_to_string(root.join("hello/PKGBUILD")).unwrap(),
            own
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_untouched_generated_recipe_is_generated_again() {
        let root = scratch("untouched");
        let first = prepare_in(
            &root,
            "hello",
            Some("https://example.org/hello-2.10.tar.gz"),
            None,
            true,
            &|_| Some((SHA.to_string(), 10)),
        )
        .unwrap();
        let before = snapshot(&root);
        let newer = "https://example.org/hello-3.0.tar.gz";

        // dry run: the new text is computed and shown, the disk is untouched
        let dry = prepare_in(&root, "hello", Some(newer), None, false, &|_| {
            Some((SHA2.to_string(), 10))
        })
        .unwrap();
        assert_eq!(dry.state, RecipeState::Regenerated);
        assert!(!dry.on_disk);
        assert!(dry.pkgbuild.contains("pkgver=3.0"));
        assert_eq!(snapshot(&root), before, "a dry run must write nothing");

        // real run: nothing of the user's is lost, so the scaffold follows
        // the new URL
        let real = prepare_in(&root, "hello", Some(newer), None, true, &|_| {
            Some((SHA2.to_string(), 10))
        })
        .unwrap();
        assert_eq!(real.state, RecipeState::Regenerated);
        assert!(real.on_disk);
        assert_ne!(real.pkgbuild, first.pkgbuild);
        assert_eq!(
            fs::read_to_string(&real.pkgbuild_path).unwrap(),
            real.pkgbuild
        );
        // and that one is again recognised as untouched
        let same = prepare_in(&root, "hello", Some(newer), None, true, &|_| {
            Some((SHA2.to_string(), 10))
        })
        .unwrap();
        assert_eq!(same.state, RecipeState::Regenerated);
        assert_eq!(same.pkgbuild, real.pkgbuild);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pkgbuild_that_is_a_link_is_refused_and_never_written_through() {
        use std::os::unix::fs::symlink;
        let root = scratch("link");
        let victim = root.join("victim");
        fs::write(&victim, "victim").unwrap();
        fs::create_dir_all(root.join("hello")).unwrap();
        symlink(&victim, root.join("hello/PKGBUILD")).unwrap();
        let r = prepare_in(&root, "hello", None, None, true, &no_fetch);
        assert!(r.is_err_and(|e| e.contains("not a regular file")));
        assert_eq!(fs::read_to_string(&victim).unwrap(), "victim");

        // replace_file swaps the link for a file; the target is untouched
        replace_file(&root.join("hello/PKGBUILD"), "new").unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "victim");
        assert!(
            fs::symlink_metadata(root.join("hello/PKGBUILD"))
                .unwrap()
                .is_file()
        );
        assert_eq!(
            fs::read_to_string(root.join("hello/PKGBUILD")).unwrap(),
            "new"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recipes_root_is_user_scoped() {
        let root = recipes_root().to_string_lossy().to_string();
        assert!(
            !root.starts_with("/usr"),
            "recipes must never live in system paths: {root}"
        );
        assert!(root.contains("yapacc"));
    }

    #[test]
    fn sanitize_rejects_traversal() {
        // path separators and dots can never smuggle a traversal into the keg dir
        assert!(!sanitize("../evil").contains('/'));
        assert!(!sanitize("..").starts_with('/'));
        assert_eq!(sanitize(""), "pkg");
    }
}
