use crate::Error;
use crate::foreign_extract::{PrivateDir, sha256_file};
use crate::ledger;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn default_dest_dir() -> PathBuf {
    home_dir()
        .join("Applications")
        .join("yapacc")
        .join("appimages")
}

/// How an AppImage source may be fetched and what it is checked against.
///
/// An AppImage has no repository index and yapacc checks no publisher
/// signature, so the only check there is, is a SHA-256 the user supplies.
#[derive(Clone, Copy, Debug, Default)]
pub struct FetchOptions<'a> {
    /// the SHA-256 the user expects (64 hex digits). None: nothing to
    /// check the file against, and the output says so.
    pub sha256: Option<&'a str>,
    /// accept a plain `http://` source, and redirects to one
    pub allow_http: bool,
}

/// `--sha256` as lowercase hex, or why it is not a SHA-256.
fn expected_sha256(opts: &FetchOptions) -> crate::error::Result<Option<String>> {
    let Some(given) = opts.sha256 else {
        return Ok(None);
    };
    let hex = given.trim().to_ascii_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::usage(format!(
            "--sha256 must be the 64 hexadecimal digits of a SHA-256, got '{given}'"
        )));
    }
    Ok(Some(hex))
}

fn is_download(source: &str) -> bool {
    !Path::new(source).exists() && (source.starts_with("http://") || source.starts_with("https://"))
}

/// What can be refused before anything is fetched: a malformed `--sha256`
/// (usage), and a plain `http://` download without `--allow-http` (refusal).
pub fn check_source(source: &str, opts: &FetchOptions) -> crate::error::Result<()> {
    expected_sha256(opts)?;
    if is_download(source) && source.starts_with("http://") && !opts.allow_http {
        return Err(Error::refused(format!(
            "refusing to download an AppImage over plain http:// ({source}): anyone on the network path can replace the file, and it is made executable. Use an https:// URL, or pass --allow-http to accept that (with --sha256 the file is still checked). Nothing was downloaded."
        )));
    }
    Ok(())
}

/// What the plan says about verification, before anything is fetched.
fn verification_plan(source: &str, opts: &FetchOptions) -> String {
    let transport = if is_download(source) {
        if source.starts_with("http://") {
            if opts.allow_http {
                "download over plain http:// (--allow-http): the transfer is not protected"
            } else {
                "download over plain http://: REFUSED unless --allow-http is given"
            }
        } else {
            "download over https:// (a redirect to http:// is refused)"
        }
    } else {
        "local file"
    };
    match opts.sha256 {
        Some(h) => format!(
            "Verification: {transport}; the file must have the SHA-256 given with --sha256 ({}), or it is deleted and the install stops (exit 5). Not verified: who published it.",
            h.trim().to_ascii_lowercase()
        ),
        None => format!(
            "Verification: {transport}; NONE of the content. No --sha256 was given, so the file is installed and made executable as it arrives. Pass --sha256 <hex> to check it."
        ),
    }
}

/// What was and was not verified about an installed AppImage, from its
/// ledger record.
pub fn verification_note(rec: &ledger::InstallRecord) -> String {
    let hash = rec
        .metadata
        .get("sha256")
        .map(String::as_str)
        .unwrap_or("?");
    if rec.metadata.get("sha256_checked").map(String::as_str) == Some("yes") {
        format!(
            "Verified: the file has the SHA-256 given with --sha256 ({hash}). Not verified: who published it, and what it does when run."
        )
    } else if rec.source.starts_with("http://") {
        format!(
            "NOT verified: no --sha256 was given and the download was over plain http://, so neither the content nor the transfer was protected. SHA-256 of the installed file: {hash}"
        )
    } else if is_download_url(&rec.source) {
        format!(
            "NOT verified: no --sha256 was given, so nothing ties this download to what its publisher released (https protects the transfer only). SHA-256 of the installed file: {hash}"
        )
    } else {
        format!(
            "NOT verified: no --sha256 was given, so the file was installed as it is. SHA-256 of the installed file: {hash}"
        )
    }
}

fn is_download_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// The plan of the plan verb: no `--sha256` or `--allow-http` is known
/// there, so it describes the default (nothing verified, http refused).
pub fn install_plan(name: &str, source: &str, dest_dir: &Path) -> String {
    let mut s = install_plan_with(name, source, dest_dir, &FetchOptions::default());
    s.push('\n');
    s.push_str(crate::prompt::PLAN_ONLY_FOOTER);
    s
}

/// What the install will do and what it will verify. It says nothing about
/// whether this run is a dry run: the caller knows.
pub fn install_plan_with(name: &str, source: &str, dest_dir: &Path, opts: &FetchOptions) -> String {
    let file_name = format!("{}.AppImage", sanitize(name));
    let dest = dest_dir.join(file_name);
    format!(
        "== AppImage install plan ==\nName: {name}\nSource: {source}\nScope: user (home-only)\nDestination: {}\nPlanned actions:\n  1) Ensure destination directory exists\n  2) Fetch/copy AppImage to destination\n  3) chmod +x\n  4) Record install in user ledger (format=appimage)\n{}",
        dest.display(),
        verification_plan(source, opts)
    )
}

fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "app".into();
    }
    out
}

/// A fetched AppImage, in a private directory next to its destination.
struct Fetched {
    file: PathBuf,
    sha256: String,
    /// the hash was compared with a user-supplied `--sha256`
    checked: bool,
    _dir: PrivateDir,
}

/// Copy or download `source` into a fresh private directory inside
/// `dest_dir` (same filesystem, so the final rename is atomic), compare it
/// with `--sha256` when one was given, and make it executable.
///
/// A mismatch deletes the file and is an integrity error. Without
/// `--sha256` nothing about the content is verified.
fn fetch(source: &str, dest_dir: &Path, opts: &FetchOptions) -> crate::error::Result<Fetched> {
    check_source(source, opts)?;
    let expected = expected_sha256(opts)?;
    let dir = PrivateDir::new_in(dest_dir, "appimage")?;
    let file = dir.path().join("download");
    if let Some(path_src) = source.strip_prefix("file://") {
        fs::copy(path_src, &file)
            .map_err(|e| format!("failed to copy {} -> {}: {e}", path_src, file.display()))?;
    } else if Path::new(source).exists() {
        fs::copy(source, &file)
            .map_err(|e| format!("failed to copy {} -> {}: {e}", source, file.display()))?;
    } else if is_download_url(source) {
        // a redirect may not leave the accepted schemes
        let schemes = if opts.allow_http {
            "=http,https"
        } else {
            "=https"
        };
        let status = Command::new("curl")
            .args(["-fL", "--retry", "2", "--proto", schemes, "--proto-redir"])
            .arg(schemes)
            .arg("-o")
            .arg(&file)
            .arg(source)
            .status()
            .map_err(|e| format!("failed to execute curl: {e}"))?;
        if !status.success() {
            return Err(Error::from(format!(
                "download failed (curl exit {status}) for {source}"
            )));
        }
    } else {
        return Err(Error::from(format!(
            "source must be an existing local path or http(s) URL: {source}"
        )));
    }
    let sha256 = sha256_file(&file)?;
    if let Some(want) = &expected
        && *want != sha256
    {
        let _ = fs::remove_file(&file);
        return Err(Error::integrity(format!(
            "checksum mismatch for {source}: --sha256 says {want}, the file has {sha256}. The file was deleted and nothing was installed."
        )));
    }
    let mut perms = fs::metadata(&file)
        .map_err(|e| format!("failed to stat {}: {e}", file.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&file, perms)
        .map_err(|e| format!("failed to chmod +x {}: {e}", file.display()))?;
    Ok(Fetched {
        file,
        sha256,
        checked: expected.is_some(),
        _dir: dir,
    })
}

fn record_check(meta: &mut std::collections::BTreeMap<String, String>, fetched: &Fetched) {
    meta.insert("sha256".into(), fetched.sha256.clone());
    meta.insert(
        "sha256_checked".into(),
        if fetched.checked { "yes" } else { "no" }.into(),
    );
}

pub fn install(
    name: &str,
    source: &str,
    dest_dir: &Path,
    ledger_root: &Path,
    opts: &FetchOptions,
) -> crate::error::Result<ledger::InstallRecord> {
    check_source(source, opts)?;
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("failed to create {}: {e}", dest_dir.display()))?;
    let file_name = format!("{}.AppImage", sanitize(name));
    let dest = dest_dir.join(&file_name);
    let conflicts = ledger::check_path_conflicts(ledger_root, &[dest.display().to_string()])?;
    if !conflicts.is_empty() {
        let msg = conflicts
            .iter()
            .map(|c| format!("{}: {} ({})", c.kind, c.path, c.detail))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::from(format!(
            "refusing to install due to path conflict(s): {msg}"
        )));
    }

    let fetched = fetch(source, dest_dir, opts)?;

    let mut metadata = std::collections::BTreeMap::new();
    record_check(&mut metadata, &fetched);
    let rec = ledger::InstallRecord {
        id: String::new(),
        name: name.to_string(),
        format: "appimage".into(),
        source: source.to_string(),
        version: "unknown".into(),
        scope: "user".into(),
        install_root: dest.display().to_string(),
        paths: vec![dest.display().to_string()],
        status: "installed".into(),
        installed_at: 0,
        updated_at: 0,
        metadata,
    };
    // the file must be at its path before it is recorded; a failed record
    // removes it again
    if let Err(e) = fs::rename(&fetched.file, &dest) {
        return Err(Error::from(format!(
            "failed to finalize install {}: {e}",
            dest.display()
        )));
    }
    match ledger::upsert_record(ledger_root, rec) {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = fs::remove_file(&dest);
            Err(Error::from(format!(
                "ledger record failed, rolled back installed file: {e}"
            )))
        }
    }
}

/// List ledger-tracked AppImages (library API; external list covers the CLI surface).
#[allow(dead_code)]
pub fn list(ledger_root: &Path) -> Result<Vec<ledger::InstallRecord>, String> {
    let mut recs = ledger::list_records(ledger_root)?;
    recs.retain(|r| r.format.eq_ignore_ascii_case("appimage") && r.status != "removed");
    Ok(recs)
}

pub fn update_plan(record: &ledger::InstallRecord, new_source: &str) -> String {
    format!(
        "== AppImage update plan ==\nID: {}\nName: {}\nCurrent: {}\nNew source: {}\nPlanned actions:\n  1) Backup current AppImage\n  2) Fetch/copy new AppImage to temp, chmod +x\n  3) Replace file atomically and update ledger (old record marked removed, new record installed)\n  4) On failure, restore backup and keep previous state\n{}",
        record.id,
        record.name,
        record.install_root,
        new_source,
        verification_plan(new_source, &FetchOptions::default())
    )
}

/// Replace a tracked AppImage with a new file. `update` takes no
/// `--sha256` and no `--allow-http`: a plain `http://` source is refused,
/// and the new file is NOT verified, which this function prints.
pub fn update(
    record_id: &str,
    new_source: &str,
    ledger_root: &Path,
) -> Result<(ledger::InstallRecord, ledger::InstallRecord), String> {
    let recs = ledger::list_records(ledger_root)?;
    let old = recs
        .into_iter()
        .find(|r| r.id == record_id)
        .ok_or_else(|| format!("ledger record not found: {record_id}"))?;
    if !old.format.eq_ignore_ascii_case("appimage") {
        return Err(format!("record {record_id} is not an appimage install"));
    }
    if old.status == "removed" {
        return Err(format!("record {record_id} is already removed"));
    }
    let dest = PathBuf::from(&old.install_root);
    let dest_dir = dest
        .parent()
        .ok_or_else(|| format!("invalid install path: {}", old.install_root))?;
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("failed to ensure dest dir {}: {e}", dest_dir.display()))?;
    let file_name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid appimage file name")?
        .to_string();
    let backup = dest_dir.join(format!(".{}.bak-{}", file_name, std::process::id()));

    let fetched = fetch(new_source, dest_dir, &FetchOptions::default())?;

    // backup existing (if present)
    let had_existing = dest.exists();
    if had_existing {
        fs::rename(&dest, &backup)
            .map_err(|e| format!("failed to backup existing appimage: {e}"))?;
    }
    // place new
    if let Err(e) = fs::rename(&fetched.file, &dest) {
        if had_existing {
            let _ = fs::rename(&backup, &dest);
        }
        return Err(format!(
            "failed to replace appimage {}: {e}",
            dest.display()
        ));
    }

    // ledger updates: mark old removed, add new installed record
    let mut old_removed = old.clone();
    old_removed.status = "removed".into();
    let mut metadata = old.metadata.clone();
    record_check(&mut metadata, &fetched);
    let new_rec = ledger::InstallRecord {
        id: String::new(),
        name: old.name.clone(),
        format: "appimage".into(),
        source: new_source.to_string(),
        version: "unknown".into(),
        scope: old.scope.clone(),
        install_root: old.install_root.clone(),
        paths: vec![old.install_root.clone()],
        status: "installed".into(),
        installed_at: 0,
        updated_at: 0,
        metadata,
    };
    let ledger_res = (|| {
        ledger::upsert_record(ledger_root, old_removed.clone())?;
        ledger::upsert_record(ledger_root, new_rec.clone())
    })();
    match ledger_res {
        Ok(nr) => {
            let _ = fs::remove_file(&backup);
            println!("{}", verification_note(&nr));
            Ok((old_removed, nr))
        }
        Err(e) => {
            // rollback file state
            let _ = fs::remove_file(&dest);
            if had_existing {
                let _ = fs::rename(&backup, &dest);
            } else {
                let _ = fs::remove_file(&backup);
            }
            Err(format!(
                "ledger update failed, rolled back appimage update: {e}"
            ))
        }
    }
}

pub fn remove_plan(record: &ledger::InstallRecord) -> String {
    format!(
        "== AppImage remove plan ==\nID: {}\nName: {}\nTracked paths: {}\nPlanned actions:\n  1) Remove tracked AppImage file(s)\n  2) Mark ledger record as removed",
        record.id,
        record.name,
        if record.paths.is_empty() {
            "(none)".into()
        } else {
            record.paths.join(", ")
        }
    )
}

pub fn remove(record_id: &str, ledger_root: &Path) -> Result<ledger::InstallRecord, String> {
    let recs = ledger::list_records(ledger_root)?;
    let mut rec = recs
        .into_iter()
        .find(|r| r.id == record_id)
        .ok_or_else(|| format!("ledger record not found: {record_id}"))?;
    if !rec.format.eq_ignore_ascii_case("appimage") {
        return Err(format!("record {record_id} is not an appimage install"));
    }
    for p in &rec.paths {
        let path = Path::new(p);
        if path.exists() {
            fs::remove_file(path).map_err(|e| format!("failed to remove {}: {e}", p))?;
        }
    }
    rec.status = "removed".into();
    ledger::upsert_record(ledger_root, rec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PrivateDir {
        PrivateDir::new(&format!("appimage-test-{tag}")).unwrap()
    }

    const HELLO_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    #[test]
    fn plain_http_is_refused_before_any_download_unless_allowed() {
        let strict = FetchOptions::default();
        let e = check_source("http://example.invalid/app.AppImage", &strict).unwrap_err();
        assert_eq!(e.kind, crate::Kind::Refused);
        assert!(e.message.contains("--allow-http"), "{e}");
        assert!(e.message.ends_with("Nothing was downloaded."), "{e}");
        // `.invalid` never resolves: reaching curl would give another error
        let dir = scratch("http");
        let e = install(
            "app",
            "http://example.invalid/app.AppImage",
            &dir.path().join("dest"),
            &dir.path().join("ledger"),
            &strict,
        )
        .unwrap_err();
        assert_eq!(e.kind, crate::Kind::Refused);
        assert!(!dir.path().join("dest").exists(), "nothing may be created");
        assert!(!dir.path().join("ledger").exists());

        let allowed = FetchOptions {
            sha256: None,
            allow_http: true,
        };
        assert!(check_source("http://example.invalid/app.AppImage", &allowed).is_ok());
        assert!(check_source("https://example.invalid/app.AppImage", &strict).is_ok());
    }

    #[test]
    fn a_given_sha256_is_checked_and_a_mismatch_deletes_the_file() {
        let dir = scratch("sha");
        let src = dir.path().join("src.AppImage");
        fs::write(&src, b"hello world").unwrap();
        let dest = dir.path().join("dest");
        let lroot = dir.path().join("ledger");
        let source = src.display().to_string();

        let wrong = "0".repeat(64);
        let e = install(
            "app",
            &source,
            &dest,
            &lroot,
            &FetchOptions {
                sha256: Some(&wrong),
                allow_http: false,
            },
        )
        .unwrap_err();
        assert_eq!(e.kind, crate::Kind::Integrity, "{e}");
        assert!(e.message.contains(HELLO_SHA256), "{e}");
        assert!(!dest.join("app.AppImage").exists());
        let left: Vec<_> = fs::read_dir(&dest).unwrap().flatten().collect();
        assert!(
            left.is_empty(),
            "the fetched file was not deleted: {left:?}"
        );

        // the right hash, in upper case: accepted and recorded as checked
        let upper = HELLO_SHA256.to_uppercase();
        let rec = install(
            "app",
            &source,
            &dest,
            &lroot,
            &FetchOptions {
                sha256: Some(&upper),
                allow_http: false,
            },
        )
        .unwrap();
        assert_eq!(rec.metadata["sha256"], HELLO_SHA256);
        assert_eq!(rec.metadata["sha256_checked"], "yes");
        let note = verification_note(&rec);
        assert!(
            note.starts_with("Verified: the file has the SHA-256"),
            "{note}"
        );
        assert!(note.contains("Not verified: who published it"), "{note}");
        let mode = fs::metadata(dest.join("app.AppImage"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn without_sha256_the_output_says_the_file_was_not_verified() {
        let dir = scratch("nosha");
        let src = dir.path().join("src.AppImage");
        fs::write(&src, b"hello world").unwrap();
        let rec = install(
            "app",
            &src.display().to_string(),
            &dir.path().join("dest"),
            &dir.path().join("ledger"),
            &FetchOptions::default(),
        )
        .unwrap();
        assert_eq!(rec.metadata["sha256_checked"], "no");
        let note = verification_note(&rec);
        assert_eq!(
            note,
            format!(
                "NOT verified: no --sha256 was given, so the file was installed as it is. SHA-256 of the installed file: {HELLO_SHA256}"
            )
        );
        // a download says what https does not cover
        let mut remote = rec.clone();
        remote.source = "https://example.invalid/app.AppImage".into();
        let note = verification_note(&remote);
        assert!(
            note.starts_with(
                "NOT verified: no --sha256 was given, so nothing ties this download to what its publisher released"
            ),
            "{note}"
        );
        remote.source = "http://example.invalid/app.AppImage".into();
        assert!(
            verification_note(&remote).starts_with(
                "NOT verified: no --sha256 was given and the download was over plain http://, so neither the content nor the transfer was protected."
            ),
            "{}",
            verification_note(&remote)
        );
        // and the plan says it before anything is fetched
        let plan = install_plan("app", "https://example.invalid/app.AppImage", dir.path());
        assert!(
            plan.contains("NONE of the content. No --sha256 was given"),
            "{plan}"
        );
        let plan = install_plan("app", "http://example.invalid/app.AppImage", dir.path());
        assert!(
            plan.contains("REFUSED unless --allow-http is given"),
            "{plan}"
        );
    }

    #[test]
    fn a_malformed_sha256_is_a_usage_error() {
        for bad in ["abc", &"g".repeat(64), &"a".repeat(63)] {
            let e = check_source(
                "/tmp/x.AppImage",
                &FetchOptions {
                    sha256: Some(bad),
                    allow_http: false,
                },
            )
            .unwrap_err();
            assert_eq!(e.kind, crate::Kind::Usage, "{bad}");
        }
    }
}
