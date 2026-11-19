//! The real `yapacc` binary: what the configuration and the ledger commands
//! print and which exit code they give. Every run has a temporary home and
//! XDG directories, so the developer's own files are neither read nor
//! written.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use yapacc::ledger;

struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Home {
        let dir = std::env::temp_dir().join(format!(
            "yapacc-cli-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for sub in ["home", "config", "data", "state", "cache", "work"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Home { dir }
    }

    fn yapacc(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_yapacc"))
            .args(args)
            .current_dir(self.dir.join("work"))
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_DATA_HOME", self.dir.join("data"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env_remove("YAPACC_CONFIG_WARNED")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    fn config(&self, text: &str) -> String {
        let p = self.dir.join("config.toml");
        std::fs::write(&p, text).unwrap();
        p.display().to_string()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// Review finding 2: `dry_run_default = true`, `bogus = 1` and
/// `[nosuchsection]` loaded with exit 0 and not a word.
#[test]
fn unknown_settings_are_warned_about_and_listed_by_config() {
    let h = Home::new("unknown");
    let path = h.config(
        "[general]\ndry_run_default = true\nbogus = 1\n\n[nosuchsection]\nx = 1\n\n[safty]\nsnapshot = \"auto\"\n\n[safety]\nsnapshot = \"auto-best-effort\"\n",
    );
    let o = h.yapacc(&["--config", &path, "config"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let stderr = err(&o);
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("warning: "))
        .collect();
    assert_eq!(
        warnings,
        [
            format!("warning: {path}: unknown key bogus in [general] (ignored)"),
            format!("warning: {path}: unknown key dry_run_default in [general] (ignored)"),
            format!("warning: {path}: unknown section [nosuchsection] (ignored)"),
            format!("warning: {path}: unknown section [safty] (ignored)"),
        ],
        "one line per unknown item, naming the file"
    );
    let shown = out(&o);
    assert!(shown.contains(&format!("Loaded from: {path}")), "{shown}");
    assert!(shown.contains("unknown_keys = ["), "{shown}");
    for item in [
        "\"[general] bogus\"",
        "\"[general] dry_run_default\"",
        "\"[nosuchsection]\"",
        "\"[safty]\"",
    ] {
        assert!(
            shown.contains(item),
            "`yapacc config` must list {item}: {shown}"
        );
    }
    // the known setting beside the typo still applies
    assert!(shown.contains("snapshot = \"auto-best-effort\""), "{shown}");
    assert!(!shown.contains("confirm_actions"), "{shown}");

    let o = h.yapacc(&["--json", "--config", &path, "config"]);
    assert_eq!(o.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(json["unknown_keys"].as_array().unwrap().len(), 4);

    // a component that already printed them tells the core not to repeat
    let o = Command::new(env!("CARGO_BIN_EXE_yapacc"))
        .args(["--config", &path, "config"])
        .env("HOME", h.dir.join("home"))
        .env("XDG_CONFIG_HOME", h.dir.join("config"))
        .env("YAPACC_CONFIG_WARNED", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(!err(&o).contains("warning: "), "{}", err(&o));
    assert!(out(&o).contains("unknown_keys = ["), "still listed");

    // a configuration without anything unknown: no warning, no list
    let clean = h.config("[safety]\nsnapshot = \"auto\"\n");
    let o = h.yapacc(&["--config", &clean, "config"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(!err(&o).contains("warning: "), "{}", err(&o));
    assert!(!out(&o).contains("unknown_keys"), "{}", out(&o));
}

#[test]
fn an_invalid_value_or_a_relative_path_is_exit_2() {
    let h = Home::new("invalid");
    for (text, needle) in [
        (
            "[mcp]\napply = \"yes\"\n",
            "[mcp] apply: invalid value 'yes'",
        ),
        ("[safety]\nsnapshot = \"always\"\n", "[safety] snapshot"),
        ("[build]\nsandbox = 3\n", "invalid config"),
        (
            "[paths]\nledger_user = \"ledger\"\n",
            "[paths] ledger_user: 'ledger' is not an absolute path",
        ),
        (
            "[paths]\nledger_system = \"./l\"\n",
            "is not an absolute path",
        ),
        (
            "[paths]\nforeign_root = \"$HOME/x\"\n",
            "is not an absolute path",
        ),
        (
            "[ledger]\nwitnesses = [\"w.log\"]\n",
            "[ledger] witnesses: 'w.log' is not an absolute path",
        ),
        ("[general\n", "invalid config"),
    ] {
        let path = h.config(text);
        let o = h.yapacc(&["--config", &path, "config"]);
        assert_eq!(o.status.code(), Some(2), "{text:?}: {}", err(&o));
        assert!(err(&o).contains(needle), "{text:?}: {}", err(&o));
        assert!(
            err(&o).contains(&path),
            "the file must be named: {}",
            err(&o)
        );
        assert!(out(&o).is_empty(), "{text:?}: {}", out(&o));
    }
}

/// Review finding 20: the path the example file suggests
/// (`ledger_user = "~/.local/state/yapacc/ledger"`) made a directory
/// literally named `~` under the current directory.
#[test]
fn a_tilde_path_is_the_home_directory_wherever_the_command_runs() {
    let h = Home::new("tilde");
    let path = h.config("[paths]\nledger_user = \"~/.local/state/yapacc/ledger\"\n\n[ledger]\nwitnesses = [\"~/w.log\"]\n");
    let o = h.yapacc(&["--config", &path, "ledger", "status"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let home = h.dir.join("home");
    let shown = out(&o);
    assert!(
        shown.contains(&format!(
            "ledger root: {}/.local/state/yapacc/ledger",
            home.display()
        )),
        "{shown}"
    );
    assert!(
        shown.contains(&format!("witnesses: {}/w.log", home.display())),
        "{shown}"
    );
    assert!(
        !h.dir.join("work/~").exists(),
        "no directory named ~ in the working directory"
    );
}

fn record(name: &str) -> ledger::InstallRecord {
    ledger::InstallRecord {
        id: String::new(),
        name: name.into(),
        format: "appimage".into(),
        source: "test".into(),
        version: "1.0".into(),
        scope: "user".into(),
        install_root: format!("/tmp/x/{name}"),
        paths: vec![format!("/tmp/x/{name}/bin")],
        status: "installed".into(),
        installed_at: 0,
        updated_at: 0,
        metadata: Default::default(),
    }
}

fn ledger_cmd(h: &Home, verb: &str, root: &Path, more: &[&str]) -> Output {
    let root = root.display().to_string();
    let mut args = vec!["ledger", verb, "--path", &root];
    args.extend_from_slice(more);
    h.yapacc(&args)
}

/// Review finding 19: after editing only `state.json`, `ledger verify`
/// printed "chain OK" with exit 0 and `ledger list` served the edited
/// record, whose paths `remove appimage` would delete.
#[test]
fn an_edited_state_cache_is_exit_5_and_is_never_what_commands_act_on() {
    let h = Home::new("ledger");
    let root = h.dir.join("ledger");
    ledger::upsert_record(&root, record("honest")).unwrap();

    let o = ledger_cmd(&h, "verify", &root, &[]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        out(&o).contains("verify: chain OK (2 events"),
        "{}",
        out(&o)
    );
    let o = ledger_cmd(&h, "status", &root, &[]);
    assert!(out(&o).contains("state cache: current"), "{}", out(&o));

    let state = root.join("state.json");
    let edited = std::fs::read_to_string(&state)
        .unwrap()
        .replace("\"honest\"", "\"evil\"")
        .replace("/tmp/x/honest", "/home/victim/Documents");
    std::fs::write(&state, edited).unwrap();

    let o = ledger_cmd(&h, "verify", &root, &[]);
    assert_eq!(o.status.code(), Some(5), "{}{}", out(&o), err(&o));
    assert!(
        err(&o).contains("verify failed: state.json does not match the journal"),
        "{}",
        err(&o)
    );
    assert!(!out(&o).contains("chain OK"), "{}", out(&o));
    let o = ledger_cmd(&h, "status", &root, &[]);
    assert_eq!(o.status.code(), Some(5), "{}", out(&o));
    // what a command would act on is the journal's record
    let o = h.yapacc(&[
        "--json",
        "ledger",
        "list",
        "--path",
        &root.display().to_string(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let listed = out(&o);
    assert!(
        listed.contains("\"honest\"") && listed.contains("/tmp/x/honest"),
        "{listed}"
    );
    assert!(
        !listed.contains("evil") && !listed.contains("victim"),
        "{listed}"
    );

    // "state.json present, journal missing" is an integrity error everywhere
    std::fs::remove_file(root.join("journal.jsonl")).unwrap();
    for verb in ["verify", "status", "list", "head", "audit"] {
        let o = ledger_cmd(&h, verb, &root, &[]);
        assert_eq!(
            o.status.code(),
            Some(5),
            "ledger {verb}: {}{}",
            out(&o),
            err(&o)
        );
        assert!(
            err(&o).contains("state.json is present but journal.jsonl is missing"),
            "ledger {verb}: {}",
            err(&o)
        );
    }
}

/// Review finding 28: `ledger head` on an empty ledger prints `0:GENESIS`,
/// which `--anchor` rejected.
#[test]
fn the_head_of_an_empty_ledger_is_accepted_as_an_anchor() {
    let h = Home::new("anchor");
    let root = h.dir.join("empty");
    let o = ledger_cmd(&h, "head", &root, &[]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(out(&o).trim(), "0:GENESIS");
    let o = ledger_cmd(&h, "verify", &root, &["--anchor", "0:GENESIS"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(!root.exists(), "reading creates nothing");
    // a malformed anchor is still a usage error
    let o = ledger_cmd(&h, "verify", &root, &["--anchor", "0:nonsense"]);
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
}
