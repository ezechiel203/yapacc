//! Plugin host — external adapter protocol (git-style, no dylib ABI).
//!
//! Plugins are SEPARATE PACKAGES (community- or vendor-shipped) providing
//! executable adapters discovered by name:
//!   yapacc-source-<name>   resolution-lane source adapters (probe protocol)
//!   yapacc-runtime-<name>  runtime adapters (availability protocol)
//!
//! Discovery: config [plugins] dirs + ~/.local/share/yapacc/plugins + PATH.
//! Protocol (versioned, JSON over stdout, one line per answer):
//!   `yapacc-source-X --protocol`        -> "1"
//!   `yapacc-source-X --probe NAME`      -> {"found":bool,"source":str,"version":str,"note":str}
//!   `yapacc-runtime-X --protocol`       -> "1"
//!   `yapacc-runtime-X --available`      -> "true" | "false"
//! The core keeps its OSS-pure lanes; Wine/games/commercial sources ship as
//! plugin packages users opt into — never inside official yapacc.

use crate::resolve::Candidate;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Source,
    Runtime,
}

#[derive(Clone, Debug)]
pub struct PluginAdapter {
    pub kind: PluginKind,
    /// lane/runtime name (suffix after yapacc-source- / yapacc-runtime-)
    pub name: String,
    pub path: PathBuf,
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// Plugin search dirs: config extras + user dir (PATH is scanned separately).
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = config_dirs();
    // no absolute data directory: no user plugin directory at all, never
    // one under the directory the command happens to run in
    if let Some(xdg) = crate::config::xdg_base("XDG_DATA_HOME", ".local/share") {
        dirs.push(xdg.join("yapacc/plugins"));
    }
    dirs
}

fn config_dirs() -> Vec<PathBuf> {
    crate::config::get()
        .plugins
        .dirs
        .iter()
        .map(PathBuf::from)
        .collect()
}

fn disabled() -> Vec<String> {
    crate::config::get().plugins.disabled.clone()
}

/// Discover adapters named yapacc-source-* / yapacc-runtime-*.
pub fn discover() -> Vec<PluginAdapter> {
    let mut out: Vec<PluginAdapter> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let off = disabled();
    let mut scan = |dir: &Path| {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for ent in entries.flatten() {
            let p = ent.path();
            let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !is_executable(&p) {
                continue;
            }
            let (kind, name) = if let Some(n) = fname.strip_prefix("yapacc-source-") {
                (PluginKind::Source, n.to_string())
            } else if let Some(n) = fname.strip_prefix("yapacc-runtime-") {
                (PluginKind::Runtime, n.to_string())
            } else {
                continue;
            };
            if name.is_empty() || off.iter().any(|d| d == &name) {
                continue;
            }
            // first one wins (config dirs take precedence over later dirs)
            let key = format!("{kind:?}/{name}");
            if seen.insert(key) {
                out.push(PluginAdapter {
                    kind,
                    name,
                    path: p,
                });
            }
        }
    };
    for d in plugin_dirs() {
        scan(&d);
    }
    // PATH scan (lowest precedence)
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            scan(Path::new(dir));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// How long an adapter may take to answer one question.
const ADAPTER_TIMEOUT: Duration = Duration::from_secs(5);

/// What an adapter did with one question.
enum Answer {
    /// it exited with success; its standard output, trimmed
    Text(String),
    /// it could not be started, or exited with a failure
    Failed,
    /// it was still running after `ADAPTER_TIMEOUT` and was killed
    TimedOut,
}

fn timeout_note() -> String {
    format!("adapter timed out after {}s", ADAPTER_TIMEOUT.as_secs())
}

/// Run an adapter with a time limit: poll every 50 ms, kill it when the
/// limit is reached. Its output is drained by a thread so that a full pipe
/// cannot stall it.
fn ask(path: &Path, args: &[&str]) -> Answer {
    // A file that is still open for writing somewhere cannot be executed
    // ("text file busy"). That lasts milliseconds: try again for one second.
    let mut busy = 0;
    let mut child = loop {
        let spawned = Command::new(path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => break child,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && busy < 40 => {
                busy += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Answer::Failed,
        }
    };
    let mut pipe = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(p) = pipe.as_mut() {
            let _ = p.read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = Instant::now() + ADAPTER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let bytes = reader.join().unwrap_or_default();
                return Answer::Text(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            Ok(Some(_)) | Err(_) => return Answer::Failed,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Answer::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn run_capture(path: &Path, args: &[&str]) -> Option<String> {
    match ask(path, args) {
        Answer::Text(text) => Some(text),
        Answer::Failed | Answer::TimedOut => None,
    }
}

/// Verify an adapter speaks our protocol version.
pub fn protocol_ok(a: &PluginAdapter) -> bool {
    run_capture(&a.path, &["--protocol"]).as_deref() == Some(PROTOCOL_VERSION.to_string().as_str())
}

#[derive(Deserialize)]
struct ProbeReply {
    found: bool,
    #[serde(default)]
    source: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    note: String,
}

/// Probe a source plugin for an exact package-name match.
pub fn probe_source(a: &PluginAdapter, name: &str) -> Candidate {
    let mut c = Candidate {
        lane: a.name.clone(),
        source: String::new(),
        version: String::new(),
        found: false,
        note: String::new(),
    };
    if a.kind != PluginKind::Source {
        c.note = "not a source adapter".into();
        return c;
    }
    // The name is the value of `--probe` on the adapter's command line. The
    // protocol has no `--` (an adapter reads its second argument), so a
    // name that is not a package name is not passed at all: it could be
    // read as an option by the adapter or by what the adapter runs.
    if !crate::util::valid_pkg_name(name) {
        c.note = crate::resolve::INVALID_NAME_NOTE.into();
        return c;
    }
    match ask(&a.path, &["--protocol"]) {
        Answer::Text(version) if version == PROTOCOL_VERSION.to_string() => {}
        Answer::TimedOut => {
            c.note = timeout_note();
            return c;
        }
        Answer::Text(_) | Answer::Failed => {
            c.note = format!(
                "plugin '{}' does not speak protocol v{PROTOCOL_VERSION}",
                a.name
            );
            return c;
        }
    }
    match ask(&a.path, &["--probe", name]) {
        Answer::Text(stdout) => match serde_json::from_str::<ProbeReply>(&stdout) {
            Ok(r) => {
                c.found = r.found;
                c.source = r.source;
                c.version = r.version;
                c.note = r.note;
            }
            Err(e) => c.note = format!("plugin probe reply not valid JSON: {e}"),
        },
        Answer::Failed => c.note = "plugin probe failed".into(),
        Answer::TimedOut => c.note = timeout_note(),
    }
    c
}

/// Runtime availability (used by consent rules, e.g. wine/darling plugins).
pub fn runtime_available(a: &PluginAdapter) -> bool {
    a.kind == PluginKind::Runtime
        && protocol_ok(a)
        && run_capture(&a.path, &["--available"]).as_deref() == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(dir: &Path, fname: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(fname);
        std::fs::write(&p, body).unwrap();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm).unwrap();
        p
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "yapacc-plugin-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn discover_and_probe_mock_source_plugin() {
        let dir = tmp("mock");
        write_plugin(
            &dir,
            "yapacc-source-mock",
            "#!/bin/sh\ncase \"$1\" in --protocol) echo 1; exit 0;; --probe) echo '{\"found\":true,\"source\":\"mock-store\",\"version\":\"9.9\",\"note\":\"mock hit\"}'; exit 0;; esac\nexit 1\n",
        );
        write_plugin(&dir, "notaplugin.txt", "ignore me");
        let found = discover_in(&dir);
        assert!(found.len() == 1, "found: {found:?}");
        assert_eq!(found[0].name, "mock");
        assert_eq!(found[0].kind, PluginKind::Source);
        assert!(protocol_ok(&found[0]));
        let c = probe_source(&found[0], "anything");
        assert!(c.found);
        assert_eq!(c.lane, "mock");
        assert_eq!(c.source, "mock-store");
        assert_eq!(c.version, "9.9");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_adapter_is_never_asked_about_an_invalid_name() {
        let dir = tmp("hostile");
        let log = dir.join("asked.log");
        write_plugin(
            &dir,
            "yapacc-source-logging",
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1\" in --protocol) echo 1;; --probe) echo '{{\"found\":true}}';; esac\n",
                log.display()
            ),
        );
        let found = discover_in(&dir);
        for bad in ["--exec=/bin/sh", "-y", "a b", "a\nb", "x;id", ""] {
            let c = probe_source(&found[0], bad);
            assert!(!c.found, "{bad:?}");
            assert_eq!(c.note, crate::resolve::INVALID_NAME_NOTE, "{bad:?}");
        }
        assert!(!log.exists(), "the adapter was run for an invalid name");
        // a valid name is asked about, as the second argument
        assert!(probe_source(&found[0], "ripgrep").found);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--protocol\n--probe ripgrep\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_plugin_availability() {
        let dir = tmp("rt");
        write_plugin(
            &dir,
            "yapacc-runtime-wine",
            "#!/bin/sh\ncase \"$1\" in --protocol) echo 1;; --available) echo true;; *) exit 1;; esac\n",
        );
        let found = discover_in(&dir);
        assert_eq!(found[0].name, "wine");
        assert_eq!(found[0].kind, PluginKind::Runtime);
        assert!(runtime_available(&found[0]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_protocol_is_rejected() {
        let dir = tmp("bad");
        write_plugin(
            &dir,
            "yapacc-source-bad",
            "#!/bin/sh\ncase \"$1\" in --protocol) echo 99;; --probe) echo '{}'; exit 0;; esac\n",
        );
        let found = discover_in(&dir);
        assert!(!protocol_ok(&found[0]));
        let c = probe_source(&found[0], "x");
        assert!(!c.found);
        assert!(c.note.contains("protocol"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn silent_adapter_is_killed_at_the_time_limit() {
        let dir = tmp("slow");
        write_plugin(&dir, "yapacc-source-slow", "#!/bin/sh\nexec sleep 30\n");
        let found = discover_in(&dir);
        let started = Instant::now();
        let c = probe_source(&found[0], "anything");
        let took = started.elapsed();
        assert!(took >= ADAPTER_TIMEOUT, "returned too early: {took:?}");
        assert!(took < Duration::from_secs(10), "took {took:?}");
        assert!(!c.found);
        assert_eq!(c.note, "adapter timed out after 5s");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slow_probe_after_a_good_protocol_answer_times_out() {
        let dir = tmp("halfslow");
        write_plugin(
            &dir,
            "yapacc-source-halfslow",
            "#!/bin/sh\ncase \"$1\" in --protocol) echo 1;; *) exec sleep 30;; esac\n",
        );
        let found = discover_in(&dir);
        let started = Instant::now();
        let c = probe_source(&found[0], "anything");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(c.note, "adapter timed out after 5s");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn discover_in(dir: &Path) -> Vec<PluginAdapter> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !is_executable(&p) {
                    continue;
                }
                if let Some(n) = fname.strip_prefix("yapacc-source-") {
                    out.push(PluginAdapter {
                        kind: PluginKind::Source,
                        name: n.to_string(),
                        path: p,
                    });
                } else if let Some(n) = fname.strip_prefix("yapacc-runtime-") {
                    out.push(PluginAdapter {
                        kind: PluginKind::Runtime,
                        name: n.to_string(),
                        path: p,
                    });
                }
            }
        }
        out
    }
}
