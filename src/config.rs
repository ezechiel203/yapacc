//! Configuration loading.
//! Precedence: defaults < user config file < file passed via --config.
//!
//! Three rules, checked in `parse`:
//!   * a file that does not parse, or a known key with an invalid value, is
//!     an error (exit 2) and never a silent fallback to the defaults;
//!   * a section or key this version does not know is ignored, with one
//!     warning line each on standard error naming the file, and is listed
//!     by `yapacc config` (`unknown_keys`). A newer configuration must not
//!     make an older yapacc unusable, and a typo must not go unnoticed;
//!   * paths are absolute. A leading `~/` is expanded; any other relative
//!     path is an error, because it would mean a different place in every
//!     working directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// What the loaded file contains that this version does not know
    /// (`[section]`, or `[section] key`). Never read from the file; shown
    /// by `yapacc config` so that an ignored setting is visible.
    #[serde(skip_deserializing, skip_serializing_if = "Vec::is_empty")]
    pub unknown_keys: Vec<String>,
    pub general: GeneralConfig,
    pub install: InstallConfig,
    pub build: BuildConfig,
    pub repair: RepairConfig,
    pub ledger: LedgerConfig,
    pub mcp: McpConfig,
    pub fixes: FixesConfig,
    pub plugins: PluginsConfig,
    pub paths: PathsConfig,
    pub foreign: ForeignConfig,
    pub safety: SafetyConfig,
}

/// `[general]` has no key at present. The section stays known so that a
/// file that still carries the header loads without a warning.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GeneralConfig {}

/// Resolution policy for `yapacc get` (the smart installer).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallConfig {
    /// Lane probe order for auto-negotiation.
    /// Valid lanes: repo, aur, flatpak, brew, recipe (recipe is always tried last).
    pub order: Vec<String>,
    /// Copied into the JSON form of `get --explain` as `runtime`; nothing else reads it.
    pub runtime_default: String,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            order: vec![
                "repo".into(),
                "aur".into(),
                "flatpak".into(),
                "recipe".into(),
            ],
            runtime_default: "container".into(),
        }
    }
}

/// Confinement of anything that executes third-party build code.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    /// require (default: refuse to build without a working sandbox) |
    /// prefer (warn and build unconfined) | off
    pub sandbox: String,
    /// fetch-only (default: network while downloading sources and running
    /// prepare(), none during build()/package()) | always
    pub network: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            sandbox: "require".into(),
            network: "fetch-only".into(),
        }
    }
}

/// Ledger hardening.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LedgerConfig {
    /// Files that receive a copy of the ledger head after every change.
    /// Point one at storage the ledger's owner cannot rewrite (another
    /// machine's mount, a root-owned append-only file) to make tampering
    /// detectable; see `yapacc ledger verify`. Each entry is `journald`, an
    /// absolute path, or a path starting with `~/`.
    pub witnesses: Vec<String>,
}

/// What an AI client connected through `yapacc-mcp` may do.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// plan (default: a tool can only return the plan and the command the
    /// human runs in their own terminal) | allow (a tool may apply when the
    /// client asserts the human approved; `yapacc-mcp` honours it only from
    /// a file owned by root, see `mcp::apply_policy`)
    pub apply: String,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            apply: "plan".into(),
        }
    }
}

/// Autorepair engine policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RepairConfig {
    pub enabled: bool,
    /// total build attempts (initial + retries)
    pub max_attempts: u32,
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
        }
    }
}

/// External plugin adapters (yapacc-source-* / yapacc-runtime-*).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginsConfig {
    /// extra discovery directories (scanned before the user plugin dir and
    /// PATH); each absolute, or starting with `~/`
    pub dirs: Vec<String>,
    /// adapter names to ignore
    pub disabled: Vec<String>,
}

/// Broken-package fixer repo (git-backed; yapacc ships NO default server).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FixesConfig {
    /// git URL of the fix repo you control (opt-in; enables push/pull)
    pub remote: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PathsConfig {
    // All three: an absolute path, or one starting with `~/` (expanded at load).
    /// Override managed root for foreign (deb/rpm/snap) isolated installs.
    pub foreign_root: Option<String>,
    /// Override user-scope ledger root.
    pub ledger_user: Option<String>,
    /// Override system-scope ledger root.
    pub ledger_system: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ForeignConfig {
    /// Default bin exposure mode for foreign installs: attributed | none
    pub expose: String,
    /// Allow installing from unsigned remote APT/RPM repos (no GPG verification).
    pub allow_unsigned_repos: bool,
}

impl Default for ForeignConfig {
    fn default() -> Self {
        Self {
            expose: "attributed".into(),
            allow_unsigned_repos: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub pretest_enabled: bool,
    /// auto | btrfs | snapper | timeshift | none
    pub snapshot_provider: String,
    pub require_snapshot_before_upgrade: bool,
    pub rollback_on_failure: bool,
    /// off (default) | auto | auto-best-effort: snapshot before a change
    /// through yapacc (see STATUS.md for what ran against real tools).
    pub snapshot: String,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            pretest_enabled: true,
            snapshot_provider: "auto".into(),
            require_snapshot_before_upgrade: false,
            rollback_on_failure: false,
            snapshot: "off".into(),
        }
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    config_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The user's configuration file, from the two variables that place it. A
/// variable that is empty or not an absolute path is ignored, as the XDG
/// specification says: `XDG_CONFIG_HOME=` would otherwise name
/// `yapacc/config.toml` under whatever directory the command is run in.
/// Pure.
pub fn config_path_from(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let absolute = |v: Option<std::ffi::OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg_config_home).or_else(|| absolute(home).map(|h| h.join(".config")))?;
    Some(base.join("yapacc/config.toml"))
}

/// An XDG base directory for this user: the variable when it holds an
/// absolute path, otherwise `suffix` under HOME when HOME is absolute. An
/// empty or relative value is ignored, as the XDG specification says: it
/// would otherwise place yapacc's files (plugins among them) under whatever
/// directory the command is run in.
pub fn xdg_base(var: &str, home_suffix: &str) -> Option<PathBuf> {
    xdg_base_from(std::env::var_os(var), std::env::var_os("HOME"), home_suffix)
}

/// [`xdg_base`] on given values. Pure.
pub fn xdg_base_from(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    home_suffix: &str,
) -> Option<PathBuf> {
    let absolute = |v: Option<std::ffi::OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    absolute(xdg).or_else(|| absolute(home).map(|h| h.join(home_suffix)))
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static EXPLICIT: OnceLock<Option<String>> = OnceLock::new();
static LOADED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The `--config` path this run was started with, if any.
pub fn explicit_path() -> Option<&'static str> {
    EXPLICIT.get().and_then(|p| p.as_deref())
}

/// The file the active configuration was read from; `None` while the
/// defaults are active (no file, or `init` has not run).
pub fn loaded_path() -> Option<&'static std::path::Path> {
    LOADED.get().and_then(|p| p.as_deref())
}

/// Set to `1` by a component (`yapacc-tui`, `yapacc-mcp`) on the core
/// processes it starts: it has already printed the warnings for this file,
/// so the core does not repeat them in every captured output.
pub const WARNED_ENV: &str = "YAPACC_CONFIG_WARNED";

/// Keys that existed in an earlier version and were removed, with the
/// reason shown instead of the generic "unknown key".
const REMOVED_KEYS: &[(&str, &str, &str)] = &[(
    "general",
    "confirm_actions",
    "nothing ever read it; a change always asks unless --yes is given",
)];

/// The witness name that is not a path (`ledger::WITNESS_JOURNALD`).
const WITNESS_JOURNALD: &str = "journald";

/// What `table` contains that `Config` has no field for: `[section]` for an
/// unknown section (`key` for a value outside any section) and
/// `[section] key` for an unknown key of a known section, sorted. The known
/// names are taken from `Config` itself, so a new field is known without a
/// second list to keep in step.
fn unknown_items(table: &toml::Table) -> Vec<String> {
    let known = serde_json::to_value(Config::default()).unwrap_or_default();
    let mut out = Vec::new();
    for (section, value) in table {
        let Some(known_keys) = known.get(section).and_then(|k| k.as_object()) else {
            out.push(if value.is_table() {
                format!("[{section}]")
            } else {
                section.clone()
            });
            continue;
        };
        if let Some(keys) = value.as_table() {
            for key in keys.keys() {
                if !known_keys.contains_key(key) {
                    out.push(format!("[{section}] {key}"));
                }
            }
        }
    }
    out.sort();
    out
}

/// The warning line for one entry of `unknown_keys`.
pub fn unknown_warning(file: &str, item: &str) -> String {
    for (section, key, why) in REMOVED_KEYS {
        if item == format!("[{section}] {key}") {
            return format!(
                "warning: {file}: [{section}] {key} is no longer a setting ({why}); remove the line"
            );
        }
    }
    match item.split_once(' ') {
        Some((section, key)) => {
            format!("warning: {file}: unknown key {key} in {section} (ignored)")
        }
        None if item.starts_with('[') => {
            format!("warning: {file}: unknown section {item} (ignored)")
        }
        None => format!("warning: {file}: unknown key {item} outside any section (ignored)"),
    }
}

/// An absolute path from a configured one: `~` and a leading `~/` become
/// the home directory, an absolute path is kept, anything else is refused.
pub fn absolute_path(what: &str, value: &str, home: Option<&str>) -> Result<String, String> {
    let rest = if value == "~" {
        Some("")
    } else {
        value.strip_prefix("~/")
    };
    if let Some(rest) = rest {
        let home = home.filter(|h| h.starts_with('/')).ok_or_else(|| {
            format!("{what}: '{value}' starts with ~ but HOME is not set to an absolute path")
        })?;
        let home = home.trim_end_matches('/');
        return Ok(match (home.is_empty(), rest.is_empty()) {
            (true, true) => "/".to_string(),
            (false, true) => home.to_string(),
            (_, false) => format!("{home}/{rest}"),
        });
    }
    if value.starts_with('/') {
        return Ok(value.to_string());
    }
    Err(format!(
        "{what}: '{value}' is not an absolute path (write the full path, or start it with ~/)"
    ))
}

/// Parse and check a configuration. `file` is used in messages only;
/// `home` is what a leading `~/` expands to.
pub fn parse(text: &str, file: &str, home: Option<&str>) -> Result<Config, String> {
    let invalid = |e: String| format!("invalid config {file}: {e}");
    let mut cfg: Config = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    cfg.unknown_keys = unknown_items(&table);
    check(&mut cfg, home).map_err(invalid)?;
    Ok(cfg)
}

/// Value checks, and path expansion.
fn check(cfg: &mut Config, home: Option<&str>) -> Result<(), String> {
    crate::sandbox::Policy::parse(&cfg.build.sandbox)
        .map_err(|e| format!("[build] sandbox: {e}"))?;
    if !matches!(cfg.build.network.as_str(), "fetch-only" | "always") {
        return Err(format!(
            "[build] network: invalid value '{}' (expected: fetch-only | always)",
            cfg.build.network
        ));
    }
    if !matches!(cfg.mcp.apply.as_str(), "plan" | "allow") {
        return Err(format!(
            "[mcp] apply: invalid value '{}' (expected: plan | allow)",
            cfg.mcp.apply
        ));
    }
    if !crate::snapshot::provider_pref_valid(&cfg.safety.snapshot_provider) {
        return Err(format!(
            "[safety] snapshot_provider: invalid value '{}' (expected: {})",
            cfg.safety.snapshot_provider,
            crate::snapshot::PROVIDER_CHOICES
        ));
    }
    crate::snapshot::Mode::parse(&cfg.safety.snapshot)
        .map_err(|e| format!("[safety] snapshot: {e}"))?;
    for (what, slot) in [
        ("[paths] foreign_root", &mut cfg.paths.foreign_root),
        ("[paths] ledger_user", &mut cfg.paths.ledger_user),
        ("[paths] ledger_system", &mut cfg.paths.ledger_system),
    ] {
        if let Some(value) = slot {
            *value = absolute_path(what, value, home)?;
        }
    }
    for w in &mut cfg.ledger.witnesses {
        if w != WITNESS_JOURNALD {
            *w = absolute_path("[ledger] witnesses", w, home)?;
        }
    }
    for d in &mut cfg.plugins.dirs {
        *d = absolute_path("[plugins] dirs", d, home)?;
    }
    Ok(())
}

/// Read the configuration (explicit path or user default) without
/// installing it: the configuration, and the file it came from (`None`
/// when the default file does not exist and the defaults apply).
/// A missing default file is fine. A file that exists but does not parse is
/// an error: silently falling back to defaults would drop the protective
/// settings the user wrote (a required snapshot, a strict sandbox).
pub fn load(explicit_path: Option<&str>) -> Result<(Config, Option<PathBuf>), String> {
    let home = std::env::var("HOME").ok();
    let path = explicit_path
        .map(PathBuf::from)
        .or_else(default_config_path);
    let Some(path) = path else {
        return Ok((Config::default(), None));
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let cfg = parse(&text, &path.display().to_string(), home.as_deref())?;
            Ok((cfg, Some(path)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && explicit_path.is_none() => {
            Ok((Config::default(), None))
        }
        Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
    }
}

/// Load config (explicit path or user default) and install it as the
/// global. Prints one warning line per unknown section or key (see the
/// module header), unless the component that started this process already
/// did (`WARNED_ENV`).
pub fn init(explicit_path: Option<&str>) -> Result<&'static Config, String> {
    if let Some(cfg) = CONFIG.get() {
        return Ok(cfg);
    }
    let _ = EXPLICIT.set(explicit_path.map(str::to_string));
    let (cfg, path) = load(explicit_path)?;
    if let Some(path) = &path
        && std::env::var_os(WARNED_ENV).is_none_or(|v| v != "1")
    {
        let file = path.display().to_string();
        for item in &cfg.unknown_keys {
            eprintln!("{}", unknown_warning(&file, item));
        }
    }
    let _ = LOADED.set(path);
    Ok(CONFIG.get_or_init(|| cfg))
}

pub fn get() -> &'static Config {
    static DEFAULT: OnceLock<Config> = OnceLock::new();
    CONFIG
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Config::default))
}

/// Path of the config file that would be loaded (for `config` subcommand display).
pub fn loaded_from(explicit_path: Option<&str>) -> String {
    let p = explicit_path
        .map(PathBuf::from)
        .or_else(default_config_path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no home/config path)".into());
    if std::path::Path::new(&p).exists() {
        p
    } else {
        format!("{p} (not present, defaults active)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/etc/x/config.toml";

    fn ok(text: &str) -> Config {
        parse(text, FILE, Some("/home/u")).unwrap()
    }

    fn err(text: &str) -> String {
        parse(text, FILE, Some("/home/u")).unwrap_err()
    }

    /// `XDG_CONFIG_HOME=` (set, empty) made yapacc read
    /// `yapacc/config.toml` of the current directory.
    #[test]
    fn an_empty_or_relative_config_home_is_ignored() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            config_path_from(os(""), os("/home/u")),
            Some(PathBuf::from("/home/u/.config/yapacc/config.toml"))
        );
        assert_eq!(
            config_path_from(os("conf"), os("/home/u")),
            Some(PathBuf::from("/home/u/.config/yapacc/config.toml"))
        );
        assert_eq!(
            config_path_from(os("/etc/xdg-u"), os("/home/u")),
            Some(PathBuf::from("/etc/xdg-u/yapacc/config.toml"))
        );
        assert_eq!(config_path_from(os(""), os("")), None);
        assert_eq!(config_path_from(None, None), None);
    }

    /// The same rule for every directory yapacc derives from an XDG
    /// variable: `XDG_DATA_HOME=` once put the plugin directory under the
    /// current directory.
    #[test]
    fn an_empty_or_relative_xdg_directory_is_ignored() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        let share = ".local/share";
        assert_eq!(
            xdg_base_from(os(""), os("/home/u"), share),
            Some(PathBuf::from("/home/u/.local/share"))
        );
        assert_eq!(
            xdg_base_from(os("data"), os("/home/u"), share),
            Some(PathBuf::from("/home/u/.local/share"))
        );
        assert_eq!(
            xdg_base_from(os("/srv/data"), os("/home/u"), share),
            Some(PathBuf::from("/srv/data"))
        );
        assert_eq!(xdg_base_from(os(""), os("relative"), share), None);
        assert_eq!(xdg_base_from(None, None, share), None);
    }

    #[test]
    fn defaults_applied_for_missing_sections() {
        let cfg = ok("");
        assert!(cfg.unknown_keys.is_empty());
        assert_eq!(cfg.foreign.expose, "attributed");
        assert!(!cfg.foreign.allow_unsigned_repos);
        assert!(cfg.safety.pretest_enabled);
        assert_eq!(
            cfg.install.order,
            vec!["repo", "aur", "flatpak", "recipe"],
            "brew and tap are opt-in lanes"
        );
        assert_eq!(cfg.install.runtime_default, "container");
        assert!(cfg.repair.enabled);
        assert_eq!(cfg.repair.max_attempts, 3);
        assert_eq!(
            cfg.build.sandbox, "require",
            "builds are confined by default"
        );
        assert_eq!(cfg.build.network, "fetch-only");
        assert_eq!(
            cfg.mcp.apply, "plan",
            "an AI client cannot apply by default"
        );
        assert!(cfg.ledger.witnesses.is_empty());
        assert!(cfg.fixes.remote.is_none(), "no default fix server, ever");
        assert!(cfg.plugins.dirs.is_empty());
        assert!(cfg.plugins.disabled.is_empty());
    }

    #[test]
    fn repair_and_fixes_overrides() {
        let cfg = ok(r#"
[repair]
enabled = false
max_attempts = 5

[fixes]
remote = "git@github.com:me/yapacc-fixes.git"
"#);
        assert!(!cfg.repair.enabled);
        assert_eq!(cfg.repair.max_attempts, 5);
        assert_eq!(
            cfg.fixes.remote.as_deref(),
            Some("git@github.com:me/yapacc-fixes.git")
        );
        assert!(cfg.unknown_keys.is_empty());
    }

    #[test]
    fn install_order_override() {
        let cfg = ok(r#"
[install]
order = ["flatpak", "repo"]
runtime_default = "extract"
"#);
        assert_eq!(cfg.install.order, vec!["flatpak", "repo"]);
        assert_eq!(cfg.install.runtime_default, "extract");
    }

    #[test]
    fn parses_example_surface() {
        let cfg = ok(r#"
[general]

[paths]
foreign_root = "/opt/pc-foreign"

[foreign]
expose = "none"
allow_unsigned_repos = true

[safety]
pretest_enabled = false
require_snapshot_before_upgrade = true
"#);
        assert_eq!(cfg.paths.foreign_root.as_deref(), Some("/opt/pc-foreign"));
        assert_eq!(cfg.foreign.expose, "none");
        assert!(cfg.foreign.allow_unsigned_repos);
        assert!(!cfg.safety.pretest_enabled);
        assert!(cfg.safety.require_snapshot_before_upgrade);
        assert!(cfg.unknown_keys.is_empty(), "{:?}", cfg.unknown_keys);
    }

    /// `# key = value` lines of the example with the comment mark removed.
    fn with_commented_settings_switched_on(text: &str) -> String {
        let is_setting = |rest: &str| {
            rest.split_once(" = ").is_some_and(|(key, _)| {
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
        };
        text.lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest) if is_setting(rest) => rest,
                _ => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The shipped example must load without a single warning: every key it
    /// shows exists, including the ones it shows commented out.
    #[test]
    fn the_example_file_has_no_unknown_key() {
        let text = include_str!("../yapacc.example.toml");
        let cfg = ok(text);
        assert!(cfg.unknown_keys.is_empty(), "{:?}", cfg.unknown_keys);
        assert!(!text.contains("confirm_actions"));
        assert!(!text.contains("dry_run_default"));

        let all = with_commented_settings_switched_on(text);
        assert!(all.contains("\nledger_user = \"~/"), "{all}");
        assert!(all.contains("\nforeign_root = "), "{all}");
        let cfg = ok(&all);
        assert!(cfg.unknown_keys.is_empty(), "{:?}", cfg.unknown_keys);
        assert_eq!(
            cfg.paths.ledger_user.as_deref(),
            Some("/home/u/.local/state/yapacc/ledger"),
            "the path the example suggests must be usable as written"
        );
    }

    #[test]
    fn snapshot_option_defaults_off_and_parses() {
        assert_eq!(Config::default().safety.snapshot, "off");
        let cfg = ok("[safety]\nsnapshot = \"auto\"\n");
        assert_eq!(cfg.safety.snapshot, "auto");
    }

    #[test]
    fn unknown_sections_and_keys_are_reported_not_swallowed() {
        let cfg = ok(r#"
bogus_top = 1

[general]
dry_run_default = true
bogus = 1

[nosuchsection]
x = 1

[safty]
snapshot = "auto"

[safety]
snapshot = "auto"
snapshto = "auto"
"#);
        assert_eq!(
            cfg.unknown_keys,
            vec![
                "[general] bogus",
                "[general] dry_run_default",
                "[nosuchsection]",
                "[safety] snapshto",
                "[safty]",
                "bogus_top",
            ]
        );
        // the settings that are known still apply
        assert_eq!(cfg.safety.snapshot, "auto");
        assert_eq!(
            unknown_warning(FILE, "[safty]"),
            "warning: /etc/x/config.toml: unknown section [safty] (ignored)"
        );
        assert_eq!(
            unknown_warning(FILE, "[general] dry_run_default"),
            "warning: /etc/x/config.toml: unknown key dry_run_default in [general] (ignored)"
        );
        assert_eq!(
            unknown_warning(FILE, "bogus_top"),
            "warning: /etc/x/config.toml: unknown key bogus_top outside any section (ignored)"
        );
    }

    #[test]
    fn the_removed_key_is_named_as_removed() {
        let cfg = ok("[general]\nconfirm_actions = true\n");
        assert_eq!(cfg.unknown_keys, vec!["[general] confirm_actions"]);
        let w = unknown_warning(FILE, &cfg.unknown_keys[0]);
        assert!(
            w.starts_with(
                "warning: /etc/x/config.toml: [general] confirm_actions is no longer a setting"
            ),
            "{w}"
        );
        // it is gone from what `yapacc config` prints as settings
        let shown = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!shown.contains("confirm_actions"), "{shown}");
        assert!(!shown.contains("unknown_keys"), "{shown}");
    }

    #[test]
    fn unknown_keys_are_in_what_the_config_command_prints() {
        let cfg = ok("[nosuchsection]\n[general]\nbogus = 1\n");
        // `yapacc config` prints the TOML form, `--json config` the JSON one
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("unknown_keys = ["), "{text}");
        assert!(text.contains("\"[nosuchsection]\""), "{text}");
        assert!(text.contains("\"[general] bogus\""), "{text}");
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(
            json["unknown_keys"],
            serde_json::json!(["[general] bogus", "[nosuchsection]"])
        );
        // a file cannot set the list itself: the key is reported like any other
        let cfg = ok("unknown_keys = [\"x\"]\n");
        assert_eq!(cfg.unknown_keys, vec!["unknown_keys"]);
    }

    #[test]
    fn a_known_key_with_an_invalid_value_is_an_error() {
        for (text, needle) in [
            ("[mcp]\napply = \"yes\"\n", "[mcp] apply"),
            ("[mcp]\napply = true\n", "invalid config"),
            ("[build]\nsandbox = \"maybe\"\n", "[build] sandbox"),
            ("[build]\nnetwork = \"never\"\n", "[build] network"),
            ("[safety]\nsnapshot = \"always\"\n", "[safety] snapshot"),
            (
                "[safety]\nsnapshot_provider = \"lvm\"\n",
                "snapshot_provider",
            ),
            ("[repair]\nmax_attempts = \"three\"\n", "invalid config"),
            ("general = 3\n", "invalid config"),
            ("[general\n", "invalid config"),
        ] {
            let e = err(text);
            assert!(e.contains(needle), "{text:?}: {e}");
            assert!(e.contains(FILE), "the file must be named: {e}");
        }
    }

    #[test]
    fn a_leading_tilde_is_expanded_and_other_relative_paths_are_refused() {
        let cfg = ok(r#"
[paths]
foreign_root = "~/foreign"
ledger_user = "~/.local/state/yapacc/ledger"
ledger_system = "/var/lib/yapacc/ledger"

[ledger]
witnesses = ["journald", "~/w.log", "/mnt/other/w.log"]

[plugins]
dirs = ["~/plugins", "/opt/plugins"]
"#);
        assert_eq!(cfg.paths.foreign_root.as_deref(), Some("/home/u/foreign"));
        assert_eq!(
            cfg.paths.ledger_user.as_deref(),
            Some("/home/u/.local/state/yapacc/ledger")
        );
        assert_eq!(
            cfg.paths.ledger_system.as_deref(),
            Some("/var/lib/yapacc/ledger")
        );
        assert_eq!(
            cfg.ledger.witnesses,
            vec!["journald", "/home/u/w.log", "/mnt/other/w.log"]
        );
        assert_eq!(cfg.plugins.dirs, vec!["/home/u/plugins", "/opt/plugins"]);

        for text in [
            "[paths]\nledger_user = \"ledger\"\n",
            "[paths]\nledger_user = \"./ledger\"\n",
            "[paths]\nledger_user = \"~bob/ledger\"\n",
            "[paths]\nledger_user = \"$HOME/ledger\"\n",
            "[paths]\nledger_user = \"\"\n",
            "[paths]\nledger_system = \"var/lib\"\n",
            "[paths]\nforeign_root = \"opt\"\n",
            "[ledger]\nwitnesses = [\"w.log\"]\n",
            "[plugins]\ndirs = [\"plugins\"]\n",
        ] {
            let e = err(text);
            assert!(e.contains("is not an absolute path"), "{text:?}: {e}");
            assert!(e.contains(FILE), "{e}");
        }
        // ~ needs a home directory to stand for
        for home in [None, Some(""), Some("relative")] {
            let e = parse("[paths]\nledger_user = \"~/l\"\n", FILE, home).unwrap_err();
            assert!(e.contains("HOME is not set"), "{home:?}: {e}");
        }
        assert_eq!(
            absolute_path("x", "~", Some("/home/u/")).unwrap(),
            "/home/u"
        );
        assert_eq!(absolute_path("x", "~/a", Some("/")).unwrap(), "/a");
        assert_eq!(absolute_path("x", "~", Some("/")).unwrap(), "/");
    }

    #[test]
    fn a_missing_explicit_file_is_an_error() {
        let e = load(Some("/nonexistent/yapacc.toml")).unwrap_err();
        assert!(
            e.contains("cannot read config /nonexistent/yapacc.toml"),
            "{e}"
        );
    }
}
