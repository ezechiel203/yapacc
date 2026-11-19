//! Typed AUR RPC v5 client (JSON via serde_json; no jq dependency) plus
//! AUR dependency-closure planning and version-based update checks.

use crate::vercmp::vercmp_full;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::process::Command;

const RPC_BASE: &str = "https://aur.archlinux.org/rpc/v5";

/// A member the AUR sends as `null` reads as its empty value: an orphaned
/// package has `"Maintainer": null`, and `URL` and `Description` can be
/// null too. Without this the whole row failed to deserialize and the
/// package was dropped from every answer.
fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")] // AUR RPC returns PascalCase keys (Name, Version, ...)
pub struct AurPkgInfo {
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub version: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub description: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub url: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub license: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub keywords: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub maintainer: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub popularity: f64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_votes: u64,
    #[serde(default)]
    pub out_of_date: Option<u64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub depends: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub make_depends: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub check_depends: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub opt_depends: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub package_base: String,
}

/// `text` as it may be shown on a terminal: what the AUR holds is written
/// by whoever submitted the package, so a control character (an escape
/// sequence that recolours the terminal or rewrites a line) is replaced by
/// U+FFFD, and a tab or a line break by a space. Pure.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// [`printable`] applied to every string of a JSON value.
fn printable_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) if s.chars().any(char::is_control) => *s = printable(s),
        serde_json::Value::Array(items) => items.iter_mut().for_each(printable_json),
        serde_json::Value::Object(members) => members.values_mut().for_each(printable_json),
        _ => {}
    }
}

fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .map_err(|e| format!("failed to query AUR (curl): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "AUR RPC request failed (curl exit {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("AUR RPC response not valid JSON: {e}"))
}

fn parse_results(mut v: serde_json::Value) -> Vec<AurPkgInfo> {
    printable_json(&mut v);
    // accept both v5 object form and raw arrays
    let items: Vec<serde_json::Value> = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => {
            // try .results array
            match v.get("results") {
                Some(serde_json::Value::Array(a)) => a.clone(),
                _ => vec![],
            }
        }
        _ => vec![],
    };
    items
        .into_iter()
        .filter_map(|i| serde_json::from_value::<AurPkgInfo>(i).ok())
        .collect()
}

/// Percent-encode a URI path/query component (AUR queries with raw `& # ?` break otherwise).
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fetch info for exact package names (batched, chunks of 100 per AUR docs).
pub fn fetch_info(names: &[String]) -> Result<Vec<AurPkgInfo>, String> {
    let mut out = Vec::new();
    for chunk in names.chunks(100) {
        if chunk.is_empty() {
            continue;
        }
        let args: Vec<String> = chunk
            .iter()
            .map(|n| format!("arg[]={}", url_encode(n)))
            .collect();
        let url = format!("{RPC_BASE}/info?{}", args.join("&"));
        let json = http_get_json(&url)?;
        out.extend(results_or_error(json)?);
    }
    Ok(out)
}

/// The packages of an RPC answer, or the error it carries. The AUR answers
/// a query it refuses with status 200 and an `error` member (`Query arg
/// too small.`, `Too many package results.`): that is not "nothing found".
/// Pure.
fn results_or_error(json: serde_json::Value) -> Result<Vec<AurPkgInfo>, String> {
    match json.get("error") {
        Some(error) => Err(format!(
            "the AUR refused the query: {}",
            printable(error.as_str().unwrap_or(&error.to_string()))
        )),
        None => Ok(parse_results(json)),
    }
}

/// Search by name/description (v5 search endpoint, returns objects).
pub fn fetch_search(query: &str) -> Result<Vec<AurPkgInfo>, String> {
    let url = format!("{RPC_BASE}/search/{}", url_encode(query));
    results_or_error(http_get_json(&url)?)
}

/// Strip version constraints and alternatives: "sh>=4" / "foo | bar" -> "sh"/"foo".
pub fn dep_base(dep: &str) -> String {
    dep.split('|')
        .next()
        .unwrap_or("")
        .split(['<', '>', '=', ':', '('])
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

#[derive(Clone, Debug, Serialize)]
pub struct ClosureReport {
    pub requested: Vec<String>,
    pub build_order: Vec<String>,
    pub from_repo: Vec<String>,
    pub satisfied_installed: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

fn pacman_has(name: &str) -> bool {
    Command::new("pacman")
        .args(["-Q", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pacman_repo_has(name: &str) -> bool {
    // provider-aware: -T prints unsatisfiable deps; exit 0 => satisfiable from repos/installed
    Command::new("pacman")
        .args(["-T", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compute the AUR build closure for the requested packages: build order
/// (dependencies first), repo-satisfiable deps, already-installed deps, and
/// anything unresolvable. Best-effort: uses local pacman db + AUR RPC metadata.
pub fn aur_closure(requested: &[String]) -> Result<ClosureReport, String> {
    let mut report = ClosureReport {
        requested: requested.to_vec(),
        build_order: vec![],
        from_repo: vec![],
        satisfied_installed: vec![],
        missing: vec![],
        notes: vec![],
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // gather AUR metadata for the transitive set
    let mut aur: Vec<AurPkgInfo> = Vec::new();
    let mut to_fetch: Vec<String> = Vec::new();
    let mut aur_names: BTreeSet<String> = BTreeSet::new();
    for r in requested {
        if !seen.contains(r) {
            seen.insert(r.clone());
            to_fetch.push(r.clone());
        }
    }
    let mut iterations = 0usize;
    while !to_fetch.is_empty() {
        iterations += 1;
        if iterations > 20 {
            report
                .notes
                .push("closure iteration cap hit (cycle?)".into());
            break;
        }
        let infos = fetch_info(&to_fetch)?;
        to_fetch.clear();
        for info in infos {
            let name = info.name.clone();
            aur_names.insert(name.clone());
            for dep in info.depends.iter().chain(info.make_depends.iter()) {
                let base = dep_base(dep);
                if base.is_empty() || seen.contains(&base) {
                    continue;
                }
                seen.insert(base.clone());
                if pacman_has(&base) {
                    report.satisfied_installed.push(base);
                } else if pacman_repo_has(&base) {
                    report.from_repo.push(base);
                } else {
                    // unknown: assume AUR candidate, fetch its info next round
                    to_fetch.push(base.clone());
                    aur_names.insert(base);
                }
            }
            aur.push(info);
        }
    }
    if aur.is_empty() {
        report
            .missing
            .extend(requested.iter().map(|r| format!("{r} (not found in AUR)")));
        return Ok(report);
    }

    // topological order over AUR set (deps first), DFS with visiting marks
    let by_name: std::collections::BTreeMap<String, &AurPkgInfo> =
        aur.iter().map(|a| (a.name.clone(), a)).collect();
    let mut order: Vec<String> = Vec::new();
    let mut state: std::collections::BTreeMap<String, u8> = std::collections::BTreeMap::new(); // 0=new,1=visiting,2=done
    fn visit(
        name: &str,
        by_name: &std::collections::BTreeMap<String, &AurPkgInfo>,
        state: &mut std::collections::BTreeMap<String, u8>,
        order: &mut Vec<String>,
        notes: &mut Vec<String>,
    ) {
        match state.get(name).copied().unwrap_or(0) {
            2 => return,
            1 => {
                notes.push(format!(
                    "dependency cycle detected at {name}; order is best-effort"
                ));
                return;
            }
            _ => {}
        }
        state.insert(name.to_string(), 1);
        if let Some(info) = by_name.get(name) {
            for dep in info.depends.iter().chain(info.make_depends.iter()) {
                let b = dep_base(dep);
                if by_name.contains_key(&b) {
                    visit(&b, by_name, state, order, notes);
                }
            }
        }
        state.insert(name.to_string(), 2);
        order.push(name.to_string());
    }
    for r in requested {
        if by_name.contains_key(r) {
            visit(r, &by_name, &mut state, &mut order, &mut report.notes);
        } else {
            report.missing.push(format!("{r} (not found in AUR)"));
        }
    }
    report.build_order = order;
    Ok(report)
}

/// Compare installed foreign packages against AUR versions.
/// Returns (name, installed, available) triples for outdated packages.
pub fn aur_update_check() -> Result<Vec<(String, String, String)>, String> {
    let out = Command::new("pacman")
        .arg("-Qmq")
        .output()
        .map_err(|e| format!("pacman -Qmq failed: {e}"))?;
    if !out.status.success() {
        return Ok(vec![]); // no foreign packages (or pacman unavailable)
    }
    let foreign: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if foreign.is_empty() {
        return Ok(vec![]);
    }
    let infos = fetch_info(&foreign)?;
    let mut outdated = Vec::new();
    for info in infos {
        if let Some(installed) = foreign.iter().find(|f| *f == &info.name) {
            let cur = Command::new("pacman")
                .args(["-Q", &info.name])
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).to_string();
                    s.split_whitespace().nth(1).map(|x| x.to_string())
                });
            let Some(cur) = cur else { continue };
            let _ = installed;
            if vercmp_full(&cur, &info.version) == std::cmp::Ordering::Less {
                outdated.push((info.name.clone(), cur, info.version.clone()));
            }
        }
    }
    Ok(outdated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_specials() {
        assert_eq!(url_encode("ripgrep"), "ripgrep");
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("a&b"), "a%26b");
        assert_eq!(url_encode("a?b#c"), "a%3Fb%23c");
    }

    #[test]
    fn dep_base_strips_constraints() {
        assert_eq!(dep_base("sh>=4"), "sh");
        assert_eq!(dep_base("python-pyqt5>=5.15"), "python-pyqt5");
        assert_eq!(dep_base("foo | bar"), "foo");
        assert_eq!(dep_base("glibc=2.38"), "glibc");
    }

    /// Two rows of the real answer to `search/tetris` (2026-11-19; the
    /// members this struct does not read are left out): an orphaned
    /// package and one without a URL. Both were dropped.
    #[test]
    fn a_null_maintainer_or_url_does_not_drop_the_package() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"resultcount":2,"results":[
            {"Description": "Retro style 2D Tetris game using the crustygame engine.", "Maintainer": null, "Name": "crustris", "NumVotes": 1, "OutOfDate": null, "PackageBase": "crustris", "Popularity": 0, "URL": "https://github.com/paulguy/crustygame", "Version": "0.3.2-1"},
            {"Description": "A Tetris copy with randomly generated blocks. Made with C++.", "Maintainer": "VisaJE", "Name": "doomsday-tetris-2-git", "NumVotes": 0, "OutOfDate": null, "PackageBase": "doomsday-tetris-2-git", "Popularity": 0, "URL": null, "Version": "1.3.r3.g540d02c-1"}
            ],"type":"search","version":5}"#,
        )
        .unwrap();
        let r = results_or_error(v).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].name.as_str(), r[0].maintainer.as_str()),
            ("crustris", "")
        );
        assert_eq!(r[0].version, "0.3.2-1");
        assert_eq!(
            (r[1].name.as_str(), r[1].url.as_str()),
            ("doomsday-tetris-2-git", "")
        );
    }

    /// The real answers to `search/a` and `search/lib` (2026-11-19):
    /// `search --aur a` printed "0 found".
    #[test]
    fn a_refused_query_is_an_error_not_an_empty_answer() {
        for (answer, said) in [
            (
                r#"{"error":"Query arg too small.","resultcount":0,"results":[],"type":"error","version":5}"#,
                "the AUR refused the query: Query arg too small.",
            ),
            (
                r#"{"error":"Too many package results.","resultcount":0,"results":[],"type":"error","version":5}"#,
                "the AUR refused the query: Too many package results.",
            ),
        ] {
            let v: serde_json::Value = serde_json::from_str(answer).unwrap();
            assert_eq!(results_or_error(v).unwrap_err(), said);
        }
    }

    /// A description is text of whoever submitted the package: an escape
    /// sequence in it must not reach the terminal.
    #[test]
    fn control_characters_of_an_answer_are_not_kept() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[{"Name":"x","Version":"1-1","Description":"nice\u001b[2J\u001b[31m tool\nline two","Keywords":["a\u009bb"],"Maintainer":"m\u0007"}]}"#,
        )
        .unwrap();
        let r = results_or_error(v).unwrap();
        assert_eq!(
            r[0].description,
            "nice\u{fffd}[2J\u{fffd}[31m tool line two"
        );
        assert_eq!(r[0].keywords, ["a\u{fffd}b"]);
        assert_eq!(r[0].maintainer, "m\u{fffd}");
        assert_eq!(
            printable("plain text, accents é, 日本"),
            "plain text, accents é, 日本"
        );
    }

    #[test]
    fn parse_results_handles_object_form() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"resultcount":1,"results":[{"Name":"foo","Version":"1.0-1","Depends":["bar>=2"]}]}"#)
                .unwrap();
        let r = parse_results(v);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "foo");
        assert_eq!(r[0].depends, vec!["bar>=2".to_string()]);
    }
}
