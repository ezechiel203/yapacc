//! The real `yapacc-mcp` binary, driven over standard input and output.
//!
//! The core it runs is a stand-in that writes its arguments to a log, so a
//! test can say exactly which commands a tool call produced. The server's
//! home and XDG directories are a temporary directory: nothing here reads
//! or writes the developer's own configuration, ledger or cache.

use serde_json::{Value, json};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// One test at a time: each writes a script and then has it executed, and a
/// script that another thread's fork still holds open cannot be executed.
static SERIAL: Mutex<()> = Mutex::new(());

struct Bench {
    dir: PathBuf,
}

impl Bench {
    fn new(tag: &str) -> Bench {
        let dir = std::env::temp_dir().join(format!(
            "yapacc-mcp-stdio-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for sub in ["home", "config", "data", "state", "cache"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let b = Bench { dir };
        let log = b.log_path();
        let core = b.core_path();
        std::fs::write(
            &core,
            format!(
                "#!/bin/sh\n\
                 {{ printf 'VIA=%s W=%s' \"${{YAPACC_VIA-}}\" \"${{YAPACC_CONFIG_WARNED-}}\"; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; echo; }} >> '{}'\n\
                 case \" $* \" in *' search '*) echo '[]';; *) echo \"fake core ran: $*\";; esac\n\
                 exit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o755)).unwrap();
        b
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("argv.log")
    }

    fn core_path(&self) -> PathBuf {
        self.dir.join("fakecore")
    }

    /// One line per run of the stand-in core.
    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// Every file under the server's home and XDG directories.
    fn files_in_home(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        for sub in ["home", "config", "data", "state", "cache"] {
            walk(&self.dir.join(sub), &mut out);
        }
        out
    }

    /// The server command, with an isolated environment. `wrapper` is put
    /// in front (for `unshare -r`).
    fn server(&self, wrapper: &[&str], args: &[&str]) -> Command {
        let exe = env!("CARGO_BIN_EXE_yapacc-mcp");
        let mut cmd = match wrapper.split_first() {
            Some((first, rest)) => {
                let mut c = Command::new(first);
                c.args(rest).arg(exe);
                c
            }
            None => Command::new(exe),
        };
        cmd.args(args)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_DATA_HOME", self.dir.join("data"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env("YAPACC_BIN", self.core_path())
            .env_remove("YAPACC_VIA")
            .env_remove("YAPACC_CONFIG_WARNED")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Session {
    /// every line the server wrote to standard output, parsed
    answers: Vec<Value>,
    stderr: String,
    code: Option<i32>,
}

/// Send `input` to the server, close its input, and collect what it said.
fn run(mut cmd: Command, input: &[u8]) -> Session {
    let mut child = cmd.spawn().expect("the server starts");
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    // written from a thread: the server answers while it is still being fed
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    let stdout = String::from_utf8(out.stdout).expect("the server writes UTF-8");
    let answers = stdout
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap_or_else(|e| {
                panic!("standard output carries a line that is not JSON ({e}): {l}")
            })
        })
        .collect();
    Session {
        answers,
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        code: out.status.code(),
    }
}

fn call(id: usize, tool: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } })
}

fn lines(requests: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for r in requests {
        bytes.extend_from_slice(r.to_string().as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn text(answer: &Value) -> &str {
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("not a tool result: {answer}"))
}

fn is_error(answer: &Value) -> bool {
    answer["result"]["isError"] == json!(true)
}

/// Review finding 23.
#[test]
fn bad_input_is_answered_and_never_ends_the_server() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let b = Bench::new("protocol");
    let mut input: Vec<u8> = Vec::new();
    // 1: not UTF-8 (this used to end the server)
    input.extend_from_slice(b"\xff\xfe{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
    // 2: not JSON (this used to be dropped without a word)
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\n");
    // 3: a batch (dropped without a word, so the client waited for ever)
    input.extend_from_slice(
        b"[{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"},{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\"}]\n",
    );
    // 4: an empty batch
    input.extend_from_slice(b"[]\n");
    // a response and two notifications: nothing to answer
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":16,\"result\":{}}\n");
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
    input.extend_from_slice(
        b"{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"params\":{\"name\":\"yapacc_upgrade\",\"arguments\":{\"confirm\":true}}}\n",
    );
    // 5: no method
    input.extend_from_slice(b"{\"id\":8}\n");
    // 6, 7, 8: unknown tool, missing params, arguments that are not an object
    input.extend_from_slice(&lines(&[
        call(9, "no_such_tool", json!({})),
        json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/call" }),
        call(11, "yapacc_doctor", json!(["x"])),
    ]));
    // blank lines, CRLF, and a last request with no final newline
    input.extend_from_slice(b"\n   \n{\"jsonrpc\":\"2.0\",\"id\":12,\"method\":\"ping\"}\r\n");
    input.extend_from_slice(
        b"{\"jsonrpc\":\"2.0\",\"id\":13,\"method\":\"initialize\",\"params\":{}}",
    );

    let s = run(b.server(&[], &[]), &input);
    assert_eq!(s.code, Some(0), "stderr: {}", s.stderr);
    let a = &s.answers;
    assert_eq!(a.len(), 10, "{a:#?}");
    assert_eq!(
        (&a[0]["error"]["code"], &a[0]["id"]),
        (&json!(-32700), &Value::Null)
    );
    assert_eq!(
        (&a[1]["error"]["code"], &a[1]["id"]),
        (&json!(-32700), &Value::Null)
    );
    let batch = a[2].as_array().expect("a batch is answered with an array");
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0]["id"], 3);
    assert_eq!(batch[1]["result"]["tools"].as_array().unwrap().len(), 21);
    assert_eq!(
        (&a[3]["error"]["code"], &a[3]["id"]),
        (&json!(-32600), &Value::Null)
    );
    assert_eq!(
        (&a[4]["error"]["code"], &a[4]["id"]),
        (&json!(-32600), &json!(8))
    );
    for (i, id) in [(5, 9), (6, 10), (7, 11)] {
        assert_eq!(a[i]["error"]["code"], -32602, "{}", a[i]);
        assert_eq!(a[i]["id"], id);
    }
    assert_eq!((&a[8]["id"], &a[8]["result"]), (&json!(12), &json!({})));
    assert_eq!(a[9]["result"]["protocolVersion"], "2024-11-05");
    // the tool call sent as a notification was not run
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!s.stderr.contains("panicked"), "{}", s.stderr);
}

const HOSTILE: [&str; 20] = [
    "-y",
    "-yy",
    "--yes",
    "--noconfirm",
    "--config=/x",
    "--root=/",
    "--dbpath=/tmp/x",
    "--installation=x",
    "ripgrep --yes",
    "a\nb",
    "a\u{0}b",
    "\u{1b}[2J",
    "x; rm -rf /",
    "$(id)",
    "`id`",
    "../../etc/passwd",
    "",
    " ",
    "--",
    "a b",
];

/// Every argument of every tool that takes a string, a list of strings or
/// a flag: (tool, key).
const STRING_ARGS: [(&str, &str); 12] = [
    ("yapacc_search", "query"),
    ("yapacc_search", "kind"),
    ("yapacc_resolve", "name"),
    ("yapacc_plan", "name"),
    ("yapacc_plan", "from"),
    ("yapacc_install", "name"),
    ("yapacc_install", "from"),
    ("yapacc_why", "name"),
    ("yapacc_names", "name"),
    ("yapacc_timeline", "since"),
    ("yapacc_remove", "names"),
    ("yapacc_search", "categories"),
];

fn base_args(tool: &str) -> Value {
    match tool {
        "yapacc_search" => json!({ "query": "ripgrep" }),
        "yapacc_remove" => json!({ "names": ["ripgrep"], "confirm": true }),
        "yapacc_install" => json!({ "name": "ripgrep", "confirm": true }),
        "yapacc_timeline" => json!({}),
        _ => json!({ "name": "ripgrep" }),
    }
}

/// The promise (review: "the central MCP promise holds"), and review
/// finding 1: with the default `[mcp] apply = "plan"` no tool call can
/// change the system, whatever arguments it sends.
#[test]
fn in_plan_mode_nothing_a_client_sends_produces_a_changing_command() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let b = Bench::new("promise");
    let long = "A".repeat(200_000);
    let mut hostile: Vec<Value> = HOSTILE.iter().map(|h| json!(h)).collect();
    hostile.push(json!(long));
    // not strings at all
    hostile.extend([
        json!(7),
        json!(-1),
        json!(1.5),
        json!(true),
        json!(["--yes"]),
        json!([["ripgrep"]]),
        json!({ "name": "ripgrep" }),
    ]);

    let mut requests = Vec::new();
    let mut expect_refused = Vec::new();
    for (tool, key) in STRING_ARGS {
        for value in &hostile {
            let mut args = base_args(tool);
            // a list argument is tried with the value itself and as an item
            let shapes: Vec<Value> = if key == "names" || key == "categories" {
                vec![
                    value.clone(),
                    json!([value.clone()]),
                    json!(["ripgrep", value.clone()]),
                ]
            } else {
                vec![value.clone()]
            };
            for shape in shapes {
                args[key] = shape.clone();
                let id = requests.len();
                requests.push(call(id, tool, args.clone()));
                // a package name, a lane, a date, a list of names: none of
                // the hostile values is one
                let must_refuse = matches!(key, "name" | "names" | "from" | "since");
                expect_refused.push((must_refuse, tool, key, shape));
            }
        }
    }
    // every staged tool, with a valid request the model "confirms"
    let confirmed_from = requests.len();
    for (tool, mut args) in [
        ("yapacc_install", json!({ "name": "ripgrep" })),
        (
            "yapacc_install",
            json!({ "name": "ripgrep", "from": "aur" }),
        ),
        ("yapacc_undo_last", json!({})),
        ("yapacc_upgrade", json!({})),
        ("yapacc_remove", json!({ "names": ["ripgrep", "fd"] })),
    ] {
        args["confirm"] = json!(true);
        let id = requests.len();
        requests.push(call(id, tool, args));
    }
    // a non-boolean confirm is refused, not read as true or false
    let bad_confirm_from = requests.len();
    for tool in [
        "yapacc_install",
        "yapacc_undo_last",
        "yapacc_upgrade",
        "yapacc_remove",
    ] {
        for bad in [json!("true"), json!("yes"), json!(1), json!(["true"])] {
            let mut args = base_args(tool);
            args["confirm"] = bad;
            let id = requests.len();
            requests.push(call(id, tool, args));
        }
    }
    // the readers answered in the server's own process
    let in_process_from = requests.len();
    for (tool, args) in [
        ("yapacc_platform", json!({})),
        ("yapacc_capabilities", json!({})),
        ("yapacc_ledger", json!({})),
        ("yapacc_why", json!({ "name": "zzz-no-such-package" })),
    ] {
        let id = requests.len();
        requests.push(call(id, tool, args));
    }

    let s = run(b.server(&[], &[]), &lines(&requests));
    assert_eq!(s.code, Some(0), "stderr: {}", s.stderr);
    assert_eq!(s.answers.len(), requests.len(), "every request is answered");
    for (i, answer) in s.answers.iter().enumerate() {
        assert_eq!(answer["id"], i, "answers come in order");
        let t = text(answer);
        assert!(!t.starts_with("APPLIED"), "request {i}: {t}");
        assert!(!t.contains("panicked"), "request {i}: {t}");
    }
    for (i, (must_refuse, tool, key, value)) in expect_refused.iter().enumerate() {
        if *must_refuse {
            assert!(
                is_error(&s.answers[i]),
                "{tool} {key}={value} was accepted: {}",
                text(&s.answers[i])
            );
        }
    }
    let confirmed = &s.answers[confirmed_from..bad_confirm_from];
    for answer in confirmed {
        let t = text(answer);
        assert!(t.starts_with("PLAN ONLY — nothing was changed."), "{t}");
        assert!(t.contains("[mcp] apply = \"plan\""), "{t}");
        assert!(!is_error(answer));
    }
    for answer in &s.answers[bad_confirm_from..in_process_from] {
        assert!(is_error(answer), "{}", text(answer));
        assert!(
            text(answer).contains("'confirm' must be true or false"),
            "{}",
            text(answer)
        );
    }
    let own = &s.answers[in_process_from..];
    assert!(
        serde_json::from_str::<Value>(text(&own[0]))
            .unwrap()
            .is_object()
    );
    assert!(serde_json::from_str::<Value>(text(&own[1])).unwrap()["capabilities"].is_array());
    assert!(
        serde_json::from_str::<Value>(text(&own[2]))
            .unwrap()
            .is_array()
    );
    assert!(
        text(&own[3]).contains("is not installed"),
        "{}",
        text(&own[3])
    );

    // What reached the core. Entries are split on the log's own prefix, so
    // that nothing a client sent can pass for a separate entry.
    let raw = std::fs::read_to_string(b.log_path()).unwrap();
    let entries: Vec<&str> = raw.split("VIA=").filter(|e| !e.is_empty()).collect();
    assert!(entries.len() >= 5 + 30, "{}", entries.len());
    for entry in &entries {
        let entry = entry.trim_end();
        assert!(
            entry.starts_with(" W=1 ["),
            "run as an applied change, or without the warnings mark: {entry}"
        );
        let args = &entry[" W=1 ".len()..];
        for consent in ["[--yes]", "[--noconfirm]", "[-y]", "[-yy]"] {
            assert!(
                !args.contains(consent),
                "a consent flag reached the core: {entry}"
            );
        }
        let read = args.starts_with("[--json] ");
        let plan = (args.starts_with("[get] ") && args.contains("[--explain] [--] "))
            || args.contains(" [--dryrun]");
        assert!(read || plan, "neither a read nor a plan: {entry}");
        // only a search query may carry a client's free text, and it comes
        // last, after `--`
        match args.split_once(" [--] ") {
            Some((before, _)) => {
                for h in HOSTILE
                    .iter()
                    .filter(|h| !h.trim().is_empty() && **h != "--")
                {
                    assert!(
                        !before.contains(&format!("[{h}]")),
                        "{h:?} reached the options of: {entry}"
                    );
                }
            }
            None => {
                for h in HOSTILE
                    .iter()
                    .filter(|h| !h.trim().is_empty() && **h != "--")
                {
                    assert!(!args.contains(&format!("[{h}]")), "{h:?} in: {entry}");
                }
            }
        }
    }
    // no search query with a word that a package manager would read as an
    // option, and none with a control character, reached the core at all
    for entry in &entries {
        if let Some((_, query)) = entry.trim_end().split_once(" [search] ") {
            let query = query.rsplit_once(" [--] [").map(|(_, q)| q).unwrap_or("");
            let query = query.strip_suffix(']').unwrap_or(query);
            assert!(
                !query.split_whitespace().any(|w| w.starts_with('-')),
                "search ran with {query:?}"
            );
            assert!(
                !query.chars().any(char::is_control),
                "search ran with {query:?}"
            );
            assert!(
                query.len() <= 200,
                "search ran with a {}-byte query",
                query.len()
            );
        }
    }
    // the five confirmed calls ran exactly their plans
    for plan in [
        " W=1 [get] [--explain] [--] [ripgrep]",
        " W=1 [get] [--from] [aur] [--explain] [--] [ripgrep]",
        " W=1 [undo] [--dryrun]",
        " W=1 [upgrade] [--dryrun]",
        " W=1 [remove] [repo] [--dryrun] [--] [ripgrep] [fd]",
    ] {
        assert!(
            entries.iter().any(|e| e.trim_end() == plan),
            "missing: {plan}"
        );
    }
    // And the server itself wrote nothing under the home directory. (One
    // thing does appear where flatpak is installed: `flatpak list`, which
    // `yapacc_why` runs, creates flatpak's own empty user installation the
    // first time it is asked. That is flatpak's doing, in flatpak's
    // directory.)
    let flatpak_own = b.dir.join("data/flatpak");
    let written: Vec<PathBuf> = b
        .files_in_home()
        .into_iter()
        .filter(|p| !p.starts_with(&flatpak_own))
        .collect();
    assert!(written.is_empty(), "{written:?}");
}

fn allow_config(b: &Bench) -> PathBuf {
    let config = b.dir.join("allow.toml");
    std::fs::write(&config, "[mcp]\napply = \"allow\"\n").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
    config
}

fn staged_requests() -> Vec<Value> {
    vec![
        call(
            0,
            "yapacc_install",
            json!({ "name": "ripgrep", "confirm": true }),
        ),
        call(1, "yapacc_undo_last", json!({ "confirm": true })),
        call(2, "yapacc_upgrade", json!({ "confirm": true })),
        call(
            3,
            "yapacc_remove",
            json!({ "names": ["ripgrep"], "confirm": true }),
        ),
        call(4, "yapacc_upgrade", json!({})),
    ]
}

/// Review finding 30: a coding agent with file access can write the user's
/// own configuration. `allow` from such a file is not honoured.
#[test]
fn allow_in_a_file_the_user_can_write_is_not_honoured() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let b = Bench::new("untrusted");
    let config = allow_config(&b);
    if std::fs::metadata(&config).unwrap().uid() == 0 {
        // run as root: every file this test can make is root's, which is
        // the other test's case
        return;
    }
    let config = config.display().to_string();
    let s = run(
        b.server(&[], &["--config", &config]),
        &lines(&staged_requests()),
    );
    assert_eq!(s.code, Some(0), "stderr: {}", s.stderr);
    assert!(
        s.stderr.contains(&format!(
            "warning: {config}: [mcp] apply = \"allow\" is not honoured"
        )),
        "the server must say so when it starts: {}",
        s.stderr
    );
    assert_eq!(s.answers.len(), 5);
    for answer in &s.answers {
        let t = text(answer);
        assert!(t.starts_with("PLAN ONLY — nothing was changed."), "{t}");
        assert!(t.contains(&config) && t.contains("not honoured"), "{t}");
    }
    let log = b.log();
    assert_eq!(log.len(), 5, "{log:?}");
    for entry in &log {
        assert!(entry.starts_with("VIA= W=1 [--config] "), "{entry}");
        assert!(
            !entry.contains("[--yes]") && !entry.contains("[--noconfirm]"),
            "{entry}"
        );
    }
}

/// The other half: from a file only root can write, a confirmed call
/// applies, an unconfirmed one does not. A user namespace (`unshare -r`)
/// makes this user's files root's without any privilege; where that is not
/// available the test says so and stops.
#[test]
fn allow_in_a_root_owned_file_applies_only_what_is_confirmed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let unshare_works = Command::new("unshare")
        .args(["-r", "true"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !unshare_works {
        eprintln!("skipped: `unshare -r` is not available here, so no root-owned file can be made");
        return;
    }
    let b = Bench::new("allowed");
    let config = allow_config(&b).display().to_string();
    let s = run(
        b.server(&["unshare", "-r"], &["--config", &config]),
        &lines(&staged_requests()),
    );
    assert_eq!(s.code, Some(0), "stderr: {}", s.stderr);
    assert!(!s.stderr.contains("not honoured"), "{}", s.stderr);
    assert_eq!(s.answers.len(), 5);
    for (answer, human) in s.answers[..4].iter().zip([
        "yapacc get ripgrep",
        "yapacc undo",
        "yapacc upgrade",
        "yapacc remove repo ripgrep",
    ]) {
        let t = text(answer);
        assert!(t.starts_with(&format!("APPLIED: {human}\n")), "{t}");
        assert!(!is_error(answer));
    }
    assert!(
        text(&s.answers[4]).starts_with("PLAN ONLY (confirm=false)"),
        "{}",
        text(&s.answers[4])
    );
    let c = format!("[--config] [{config}]");
    assert_eq!(
        b.log(),
        [
            format!("VIA= W=1 {c} [get] [--explain] [--] [ripgrep]"),
            format!("VIA=mcp W=1 {c} [get] [--noconfirm] [--] [ripgrep]"),
            format!("VIA= W=1 {c} [undo] [--dryrun]"),
            format!("VIA=mcp W=1 {c} [undo] [--yes]"),
            format!("VIA= W=1 {c} [upgrade] [--dryrun]"),
            format!("VIA=mcp W=1 {c} [upgrade] [--yes]"),
            format!("VIA= W=1 {c} [remove] [repo] [--dryrun] [--] [ripgrep]"),
            format!("VIA=mcp W=1 {c} [remove] [repo] [--yes] [--] [ripgrep]"),
            format!("VIA= W=1 {c} [upgrade] [--dryrun]"),
        ]
    );
}

/// Review finding 2, at the server: unknown settings are warned about once,
/// by the server, and the core runs it starts do not repeat them.
#[test]
fn the_server_warns_about_unknown_settings_and_refuses_a_bad_configuration() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let b = Bench::new("config");
    let config = b.dir.join("c.toml");
    std::fs::write(
        &config,
        "[general]\ndry_run_default = true\nconfirm_actions = true\n\n[safty]\nsnapshot = \"auto\"\n",
    )
    .unwrap();
    let path = config.display().to_string();
    let s = run(
        b.server(&[], &["--config", &path]),
        &lines(&[call(0, "yapacc_doctor", json!({}))]),
    );
    assert_eq!(s.code, Some(0), "stderr: {}", s.stderr);
    let warnings: Vec<&str> = s
        .stderr
        .lines()
        .filter(|l| l.starts_with("warning: "))
        .collect();
    assert_eq!(
        warnings,
        [
            format!(
                "warning: {path}: [general] confirm_actions is no longer a setting (nothing ever read it; a change always asks unless --yes is given); remove the line"
            ),
            format!("warning: {path}: unknown key dry_run_default in [general] (ignored)"),
            format!("warning: {path}: unknown section [safty] (ignored)"),
        ]
    );
    assert_eq!(
        b.log(),
        [format!("VIA= W=1 [--config] [{path}] [--json] [doctor]")]
    );

    // a known key with an invalid value, a relative path, a file that is
    // not TOML: exit 2, the file named, nothing served
    for bad in [
        "[mcp]\napply = \"yes\"\n",
        "[paths]\nledger_user = \"ledger\"\n",
        "[mcp\n",
    ] {
        std::fs::write(&config, bad).unwrap();
        let s = run(
            b.server(&[], &["--config", &path]),
            &lines(&[call(0, "yapacc_doctor", json!({}))]),
        );
        assert_eq!(s.code, Some(2), "{bad:?}: {}", s.stderr);
        assert!(s.stderr.contains(&path), "{bad:?}: {}", s.stderr);
        assert!(s.answers.is_empty(), "{bad:?}");
    }
    assert_eq!(b.log().len(), 1);
    // a bare --config, a missing file, a directory: exit 2 as well
    for args in [
        vec!["--config"],
        vec!["--config", "/nonexistent/yapacc.toml"],
        vec!["--config", "/tmp"],
    ] {
        let s = run(b.server(&[], &args), b"");
        assert_eq!(s.code, Some(2), "{args:?}: {}", s.stderr);
    }
}
