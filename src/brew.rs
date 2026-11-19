//! Homebrew formula source (homebrew-core catalogue).
//!
//! The cascade:
//!   1. parse the formula; if it carries Linux bottles/tags or no mac-only
//!      markers → build from the formula's source via the recipe lane
//!      (factories + review gate + autorepair).
//!   2. metadata inconclusive → try the Linux build; the autorepair engine's
//!      MacOsOnly diagnosis (framework-not-found linker errors) signals
//!      mac-only software.
//!   3. mac-only → ask the user; if Darling is installed and accepted,
//!      fetch the macOS bottle, extract into a keg, validate one binary
//!      under `darling exec`, and install a `fgn.darling.<pkg>--<bin>`
//!      launcher alias.
//!
//! Formulas are fetched from the homebrew-core git raw endpoint; parsing
//! covers the mechanical Ruby subset (url/sha256/depends_on/bottle/macos
//! markers) — we never evaluate Ruby.

use serde::Serialize;

const RAW_BASE: &str = "https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula";

#[derive(Clone, Debug, Default, Serialize)]
pub struct Formula {
    pub name: String,
    pub description: String,
    pub homepage: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub depends_on: Vec<String>,
    /// bottle tags present (e.g. arm64_sonoma, x86_64_linux)
    pub bottle_tags: Vec<String>,
    /// explicit `depends_on macos:` style markers
    pub macos_only_marker: bool,
}

impl Formula {
    pub fn linux_bottled(&self) -> bool {
        self.bottle_tags.iter().any(|t| t.ends_with("_linux"))
    }
    /// classification used by the cascade before trial-and-error
    pub fn classification(&self) -> &'static str {
        if self.linux_bottled() {
            "linux-bottled"
        } else if self.macos_only_marker {
            "mac-only"
        } else {
            "inconclusive"
        }
    }
}

/// Start of the error for a formula yapacc declines to use — a name or a
/// source URL that is not accepted — as opposed to one that could not be
/// fetched or parsed. Callers turn the former into a refusal (exit 3).
const DECLINED: &str = "formula declined: ";

/// Is this error of [`fetch_formula`] / [`parse_formula`] a refusal?
pub fn declined(error: &str) -> bool {
    error.starts_with(DECLINED)
}

fn fetch(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "fetch {url} failed ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Fetch a formula from homebrew-core (name may need the letter-subdir path;
/// modern layout is flat Formula/<name>.rb).
pub fn fetch_formula(name: &str) -> Result<Formula, String> {
    // the name becomes part of the URL fetched here and, further on, of a
    // generated PKGBUILD: package-name characters only (no `/`, no `..`)
    crate::recipe::valid_recipe_name(name)
        .map_err(|e| format!("{DECLINED}{e} (no formula was fetched)"))?;
    let first = name.chars().next().unwrap_or('0').to_ascii_lowercase();
    let shard = if first.is_ascii_digit() {
        "0".to_string()
    } else {
        first.to_string()
    };
    let mut last_err = String::new();
    for url in [
        format!("{RAW_BASE}/{shard}/{name}.rb"),
        format!("{RAW_BASE}/{name}.rb"),
    ] {
        match fetch(&url) {
            Ok(text) => return parse_formula(name, &text),
            Err(e) => last_err = e,
        }
    }
    Err(format!(
        "formula '{name}' not found in homebrew-core: {last_err}"
    ))
}

/// Parse the mechanical Ruby subset of a formula. Pure; unit-tested.
pub fn parse_formula(name: &str, ruby: &str) -> Result<Formula, String> {
    let mut f = Formula {
        name: name.to_string(),
        ..Default::default()
    };
    let mut stable_block = false;
    for line in ruby.lines() {
        let t = line.trim();
        if t.starts_with("desc ") {
            f.description = printable_rb_string(t.trim_start_matches("desc "));
        } else if t.starts_with("homepage ") {
            f.homepage = printable_rb_string(t.trim_start_matches("homepage "));
        } else if t == "stable do" || t == "stable {" {
            stable_block = true;
        } else if stable_block && t.starts_with("url ") {
            f.url = strip_rb_string(t.trim_start_matches("url "));
        } else if stable_block && t.starts_with("sha256 ") {
            f.sha256 = strip_rb_string(t.trim_start_matches("sha256 "));
            if let Some(v) = version_from_url(&f.url) {
                f.version = v;
            }
            stable_block = false;
        } else if !stable_block && t.starts_with("url ") && f.url.is_empty() {
            // top-level url (old-style formula)
            f.url = strip_rb_string(t.trim_start_matches("url "));
            if let Some(v) = version_from_url(&f.url) {
                f.version = v;
            }
        } else if t.starts_with("depends_on ") {
            if t.contains("macos:") {
                f.macos_only_marker = true;
            }
            // "depends_on \"name\" => :build" -> record the name
            let rest = t.trim_start_matches("depends_on ").trim_end_matches(',');
            if rest.starts_with('"') || rest.starts_with('\'') {
                let dep = printable_rb_string(rest);
                if !dep.is_empty() {
                    f.depends_on.push(dep);
                }
            }
        } else if t.starts_with("bottle do") {
            // following "sha256 ... => tag" lines are bottle tags
        }
        if let Some(tag) = t.strip_prefix("sha256 ")
            && let Some((_, tail)) = tag.split_once("=>")
        {
            let tag_name = tail.trim().trim_end_matches(',');
            if tag_name.starts_with(':') {
                f.bottle_tags
                    .push(tag_name.trim_start_matches(':').to_string());
            }
        }
        if t == "end" {
            stable_block = false;
        }
    }
    if f.url.is_empty() {
        return Err(format!(
            "formula '{name}' has no parsable source URL (skipping — never evaluate Ruby)"
        ));
    }
    // The formula is remote text. Its URL goes to curl as an argument and
    // into a generated PKGBUILD that makepkg sources as bash: accept only a
    // plain http(s) URL (no `$(...)`, backquotes, quotes, spaces). The
    // recipe generator checks again; refusing here names the formula.
    crate::recipe::valid_source_url(&f.url)
        .map_err(|e| format!("{DECLINED}'{name}': {e} (yapacc never rewrites a formula's URL)"))?;
    Ok(f)
}

fn strip_rb_string(s: &str) -> String {
    // extract the FIRST quoted segment: `"name" => :build` -> `name`
    let s = s.trim();
    let bytes: Vec<char> = s.chars().collect();
    if bytes.first() == Some(&'"') || bytes.first() == Some(&'\'') {
        let q = bytes[0];
        let inner: String = bytes[1..].iter().collect();
        return inner.split(q).next().unwrap_or("").to_string();
    }
    // bare token (symbol/formula refs)
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// A formula string that is only ever printed (description, homepage,
/// dependency names): without the control characters a remote file could
/// use to send escape sequences to the terminal.
fn printable_rb_string(s: &str) -> String {
    strip_rb_string(s)
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// "https://foo.io/bar-1.2.3.tar.gz" → "1.2.3" (v-prefix aware)
pub fn version_from_url(url: &str) -> Option<String> {
    let base = url.rsplit('/').next()?;
    let stem = base.split(".tar").next()?;
    let mut stem = stem;
    if let Some(rest) = stem.strip_prefix('v')
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        stem = rest;
    }
    let seg = stem
        .rsplit('-')
        .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let cleaned: String = seg
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Whether Darling (the macOS translation layer) is installed.
pub fn darling_available() -> bool {
    crate::util::have("darling")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_LINUX: &str = r#"
class Ripgrep < Formula
  desc "Fast, greplike tool"
  homepage "https://github.com/BurntSushi/ripgrep"
  stable do
    url "https://github.com/BurntSushi/ripgrep/archive/14.1.0.tar.gz"
    sha256 "abc123deadbeef"
  end
  depends_on "rust" => :build
  bottle do
    sha256 cellar: :any, "111" => :arm64_sonoma
    sha256 "222" => :x86_64_linux
  end
end
"#;

    const FIXTURE_MAC_ONLY: &str = r#"
class SomeMacApp < Formula
  desc "A mac-only thing"
  homepage "https://example.com"
  url "https://example.com/somemacapp-2.0.tar.gz"
  sha256 "feed"
  depends_on "cocoa"
  depends_on macos: :ventura
end
"#;

    #[test]
    fn parses_linux_bottled_formula() {
        let f = parse_formula("ripgrep", FIXTURE_LINUX).unwrap();
        assert_eq!(f.description, "Fast, greplike tool");
        assert_eq!(
            f.url,
            "https://github.com/BurntSushi/ripgrep/archive/14.1.0.tar.gz"
        );
        assert_eq!(f.sha256, "abc123deadbeef");
        assert_eq!(f.version, "14.1.0");
        assert!(f.linux_bottled(), "tags: {:?}", f.bottle_tags);
        assert!(!f.macos_only_marker);
        assert_eq!(f.classification(), "linux-bottled");
        // "=> :build" deps are recorded as names
        assert!(f.depends_on.iter().any(|i| i == "rust"));
    }

    #[test]
    fn parses_mac_only_formula() {
        let f = parse_formula("somemacapp", FIXTURE_MAC_ONLY).unwrap();
        assert!(f.macos_only_marker);
        assert!(!f.linux_bottled());
        assert_eq!(f.classification(), "mac-only");
        assert_eq!(f.version, "2.0");
    }

    #[test]
    fn refuses_formulas_without_parsable_url() {
        assert!(parse_formula("x", "class X < Formula\n  desc \"no url\"\nend").is_err());
    }

    fn formula_with_url(url_line: &str) -> String {
        format!(
            "class X < Formula\n  desc \"x\"\n  homepage \"https://example.com\"\n  {url_line}\n  sha256 \"feed\"\nend\n"
        )
    }

    /// A formula is remote text; its URL ends up in a PKGBUILD that makepkg
    /// sources as bash, and on a curl command line.
    #[test]
    fn refuses_formula_urls_that_carry_shell_syntax() {
        for url_line in [
            r#"url "https://example.com/x-$(touch /tmp/pwned).tar.gz""#,
            r#"url "https://example.com/x-`id`.tar.gz""#,
            r#"url "https://example.com/x-1.0'.tar.gz""#,
            r#"url 'https://example.com/x-1.0".tar.gz'"#,
            r#"url "https://example.com/x 1.0.tar.gz""#,
            r#"url "https://example.com/x-${HOME}.tar.gz""#,
            r#"url "https://example.com/x-1.0.tar.gz|id""#,
            "url \"https://example.com/x-1.0\u{1b}[2J.tar.gz\"",
            r#"url "file:///etc/passwd""#,
            r#"url "-o/tmp/pwned""#,
            r#"url "ftp://example.com/x-1.0.tar.gz""#,
        ] {
            let r = parse_formula("x", &formula_with_url(url_line));
            assert!(r.is_err(), "accepted: {url_line}");
            let e = r.unwrap_err();
            assert!(declined(&e), "{e}");
            assert!(e.contains("nothing was generated"), "{e}");
        }
        // an ordinary one still parses
        let ok = parse_formula(
            "x",
            &formula_with_url(r#"url "https://example.com/x-1.0.tar.gz""#),
        )
        .unwrap();
        assert_eq!(ok.url, "https://example.com/x-1.0.tar.gz");
    }

    #[test]
    fn printed_formula_fields_carry_no_control_characters() {
        let f = parse_formula(
            "x",
            "class X < Formula\n  desc \"nice\u{1b}[2Jtool\u{7}\"\n  homepage \"https://example.com/\u{1b}]0;t\u{7}\"\n  url \"https://example.com/x-1.0.tar.gz\"\nend\n",
        )
        .unwrap();
        assert_eq!(f.description, "nice[2Jtool");
        assert!(!f.homepage.chars().any(|c| c.is_control()));
    }

    #[test]
    fn formula_names_that_are_not_package_names_are_never_fetched() {
        // refused before any network request: these would otherwise become
        // part of the raw.githubusercontent.com URL
        for name in [
            "../../../../attacker/repo/main/x",
            "a/b",
            "$(id)",
            "-o",
            "a b",
            "",
        ] {
            let e = fetch_formula(name).unwrap_err();
            assert!(e.contains("no formula was fetched"), "{name:?}: {e}");
            assert!(declined(&e), "{name:?}: {e}");
        }
    }

    #[test]
    fn version_from_url_variants() {
        assert_eq!(
            version_from_url("https://e.io/a-1.2.3.tar.gz").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            version_from_url("https://e.io/v3.4.tar.bz2").as_deref(),
            Some("3.4")
        );
        assert_eq!(version_from_url("https://e.io/nope.tar.gz"), None);
    }
}
